//! Stage 3 execution: Parse safe outputs and execute actions
//!
//! After the agent (Stage 1) generates safe outputs as an NDJSON file,
//! Stage 3 parses this file and executes the corresponding actions.

use anyhow::{Context, Result};
use chrono::{SecondsFormat, Utc};
use log::{debug, error, info, warn};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::fs::OpenOptions;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use crate::ndjson::{self, EXECUTED_NDJSON_FILENAME, SAFE_OUTPUT_FILENAME};
use crate::safe_outputs::{
    AddBuildTagResult, AddPrCommentResult, CommentOnWorkItemResult, CreateBranchResult,
    CreateGitTagResult, CreateIssueResult, CreatePrResult, CreateWikiPageResult,
    CreateWorkItemResult, ExecutionContext, ExecutionResult, Executor, LinkWorkItemsResult,
    MissingDataResult, MissingToolResult, NoopResult, QueueBuildResult, ReplyToPrCommentResult,
    ReportIncompleteResult, ResolvePrThreadResult, SubmitPrReviewResult, ToolResult,
    UpdatePrResult, UpdateWikiPageResult, UpdateWorkItemResult, UploadBuildAttachmentResult,
    UploadPipelineArtifactResult, UploadWorkitemAttachmentResult,
};
use crate::sanitize::{SanitizeConfig, neutralize_pipeline_commands, sanitize, sanitize_config};

// Re-export memory types for use by main.rs
pub use crate::tools::cache_memory::{MemoryConfig, process_agent_memory};

/// Selects which safe-output entries Stage 3 executes, by tool name.
///
/// Used to split execution into an automatic path and a manual-review path:
/// the auto execution job `exclude`s the reviewed tools, and the reviewed
/// execution job runs `only` the reviewed tools (after the approval gate).
/// An empty filter (the default) executes every entry.
#[derive(Debug, Default, Clone)]
pub struct ToolFilter {
    /// When non-empty, only entries whose tool name appears here run.
    pub only: Vec<String>,
    /// Entries whose tool name appears here are skipped.
    pub exclude: Vec<String>,
}

impl ToolFilter {
    /// Whether an entry with tool name `tool` is permitted by this filter.
    pub fn allows(&self, tool: &str) -> bool {
        if !self.only.is_empty() && !self.only.iter().any(|t| t == tool) {
            return false;
        }
        if self.exclude.iter().any(|t| t == tool) {
            return false;
        }
        true
    }
}

/// Additional `ado-aw execute` custom safe-output modes.
#[derive(Debug, Default, Clone)]
pub struct CustomExecuteOptions {
    /// Scripts-style native dispatcher config.
    pub custom_config: Option<PathBuf>,
    /// Jobs-style wrapper phase: `pre` or `post`.
    pub custom_phase: Option<String>,
    /// Jobs-style custom tool name.
    pub tool: Option<String>,
    /// Jobs-style pre output path for selected proposals.
    pub proposals_out: Option<PathBuf>,
    /// Jobs-style post input path for component result records.
    pub results_in: Option<PathBuf>,
    /// Compiler-owned component provenance.
    pub component_sha: Option<String>,
    pub component_source: Option<String>,
    pub manifest_digest: Option<String>,
    pub schema_digest: Option<String>,
}

impl CustomExecuteOptions {
    /// Whether any custom-mode flag was supplied.
    pub fn has_any_custom_flag(&self) -> bool {
        self.custom_config.is_some()
            || self.custom_phase.is_some()
            || self.tool.is_some()
            || self.proposals_out.is_some()
            || self.results_in.is_some()
            || self.component_sha.is_some()
            || self.component_source.is_some()
            || self.manifest_digest.is_some()
            || self.schema_digest.is_some()
    }
}

const CUSTOM_SCHEMA_VERSION: u32 = 1;
const DEFAULT_CUSTOM_MAX: usize = 3;

fn default_custom_max() -> usize {
    DEFAULT_CUSTOM_MAX
}

fn default_custom_cwd() -> PathBuf {
    PathBuf::from(".")
}

/// Scripts-style custom safe-output config emitted by the compiler.
#[derive(Debug, Deserialize)]
pub struct CustomScriptsConfig {
    pub tools: HashMap<String, CustomScriptToolConfig>,
}

/// One scripts-style custom safe-output handler.
#[derive(Debug, Deserialize)]
pub struct CustomScriptToolConfig {
    pub entrypoint: String,
    #[serde(default = "default_custom_cwd")]
    pub cwd: PathBuf,
    #[serde(default = "default_custom_max")]
    pub max: usize,
}

/// Compiler-owned custom component provenance attached to each final record.
#[derive(Debug, Clone, Serialize)]
pub struct CustomComponentProvenance {
    pub source: Option<String>,
    pub sha: Option<String>,
    pub manifest_digest: Option<String>,
    pub schema_digest: Option<String>,
}

/// Attempt metadata attached to each custom execution record.
#[derive(Debug, Clone, Serialize)]
pub struct CustomAttemptMetadata {
    pub number: u32,
    pub staged: bool,
    pub started_at: String,
    pub ended_at: String,
}

/// Final custom safe-output execution record.
///
/// The top-level `name`, `status`, and `timestamp` fields intentionally mirror
/// the built-in `ExecutionRecord` so the existing audit reader can continue to
/// key off `name`/`status` while custom records carry richer provenance.
#[derive(Debug, Clone, Serialize)]
pub struct CustomExecutionRecord {
    pub schema_version: u32,
    pub tool: String,
    pub proposal_id: String,
    pub proposal_index: usize,
    pub name: String,
    pub status: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    pub component: CustomComponentProvenance,
    pub attempt: CustomAttemptMetadata,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub timestamp: String,
}

#[derive(Debug)]
struct SelectedCustomProposal {
    proposal_id: String,
    proposal_index: usize,
    entry: Value,
    budget_result: Option<ExecutionResult>,
}

impl SelectedCustomProposal {
    fn attempted(&self) -> bool {
        self.budget_result.is_none()
    }
}

#[derive(Debug, Deserialize)]
struct ScriptResultLine {
    status: String,
    message: String,
    data: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct ComponentResultLine {
    #[serde(rename = "schema_version")]
    _schema_version: u32,
    proposal_id: String,
    status: String,
    message: String,
    data: Option<Value>,
}

struct CustomToolOutcome {
    result: ExecutionResult,
    record_status: &'static str,
}

/// Execute a custom safe-output mode. Built-in execution is untouched unless
/// at least one custom flag is present.
pub async fn execute_custom_safe_outputs(
    source: &Path,
    safe_output_dir: &Path,
    dry_run: bool,
    options: CustomExecuteOptions,
) -> Result<Vec<ExecutionResult>> {
    match (
        options.custom_config.as_ref(),
        options.custom_phase.as_deref(),
    ) {
        (Some(config_path), None) => {
            execute_custom_scripts(config_path, safe_output_dir, dry_run, &options).await
        }
        (None, Some("pre")) => {
            execute_custom_pre(source, safe_output_dir, dry_run, &options).await?;
            Ok(Vec::new())
        }
        (None, Some("post")) => {
            execute_custom_post(source, safe_output_dir, dry_run, &options).await
        }
        (Some(_), Some(_)) => {
            anyhow::bail!("--custom-config and --custom-phase are mutually exclusive")
        }
        (None, Some(other)) => {
            anyhow::bail!("Unsupported --custom-phase '{other}' (expected 'pre' or 'post')")
        }
        (None, None) => {
            anyhow::bail!("Custom execute mode requested without --custom-config or --custom-phase")
        }
    }
}

async fn execute_custom_scripts(
    config_path: &Path,
    safe_output_dir: &Path,
    dry_run: bool,
    options: &CustomExecuteOptions,
) -> Result<Vec<ExecutionResult>> {
    if options.tool.is_some() || options.proposals_out.is_some() || options.results_in.is_some() {
        anyhow::bail!(
            "--custom-config cannot be combined with --tool, --proposals-out, or --results-in"
        );
    }

    let config = load_custom_scripts_config(config_path).await?;
    let safe_output_path = safe_output_dir.join(SAFE_OUTPUT_FILENAME);
    let Some(entries) = load_safe_output_entries(&safe_output_path).await? else {
        return Ok(Vec::new());
    };

    let config_dir = config_path.parent().unwrap_or_else(|| Path::new("."));
    let provenance = provenance_from_options(options);
    let mut budgets: HashMap<String, (usize, usize)> = config
        .tools
        .iter()
        .map(|(name, tool)| (name.clone(), (0, tool.max)))
        .collect();

    let mut results = Vec::new();
    for (i, entry) in entries.iter().enumerate() {
        let Some(tool_name) = entry.get("name").and_then(|name| name.as_str()) else {
            continue;
        };
        let Some(tool_config) = config.tools.get(tool_name) else {
            continue;
        };

        let proposal_context = entry.get("context").and_then(|value| value.as_str());
        let (executed, max) = budgets
            .get_mut(tool_name)
            .expect("budget map is initialized from config tools");
        let context_id = extract_entry_context(entry);
        let proposal_id = proposal_id(tool_name, i);
        if let Some(result) =
            check_budget(entries.len(), i, tool_name, &context_id, *executed, *max)
        {
            append_custom_execution_record_for_result(
                safe_output_dir,
                tool_name,
                &proposal_id,
                i,
                proposal_context,
                &result,
                provenance.clone(),
                0,
                dry_run,
            )
            .await;
            results.push(result);
            continue;
        }
        *executed += 1;

        let started_at = now_timestamp();
        let outcome = if dry_run {
            let message = format!(
                "Staged custom tool '{tool_name}' proposal '{proposal_id}'; would run '{}'",
                sanitize_config(&tool_config.entrypoint)
            );
            custom_status_to_outcome("staged", message, None)
        } else {
            let cwd = resolve_custom_cwd(config_dir, &tool_config.cwd);
            run_custom_entrypoint(
                tool_name,
                &proposal_id,
                &tool_config.entrypoint,
                &cwd,
                entry,
            )
            .await
        };
        let ended_at = now_timestamp();
        log_and_print_entry_result(i, entries.len(), tool_name, &outcome.result);
        append_custom_execution_record_for_result_with_times(
            safe_output_dir,
            tool_name,
            &proposal_id,
            i,
            proposal_context,
            &outcome.result,
            provenance.clone(),
            CustomAttemptMetadata {
                number: 1,
                staged: dry_run,
                started_at,
                ended_at,
            },
            Some(outcome.record_status),
        )
        .await;
        results.push(outcome.result);
    }

    Ok(results)
}

async fn execute_custom_pre(
    source: &Path,
    safe_output_dir: &Path,
    dry_run: bool,
    options: &CustomExecuteOptions,
) -> Result<()> {
    let tool = required_custom_tool(options)?;
    let proposals_out = options
        .proposals_out
        .as_ref()
        .context("--custom-phase pre requires --proposals-out")?;
    if options.custom_config.is_some() || options.results_in.is_some() {
        anyhow::bail!("--custom-phase pre cannot be combined with --custom-config or --results-in");
    }

    let max = load_custom_max(source, tool).await?;
    let entries = load_entries_or_empty(safe_output_dir).await?;
    let selected = select_custom_proposals(&entries, tool, max);
    let attempted: Vec<Value> = selected
        .iter()
        .filter(|proposal| proposal.attempted())
        .map(proposal_with_id)
        .collect();
    write_ndjson_values(proposals_out, &attempted).await?;

    if dry_run {
        // ADO consumes this logging command and exposes the staged contract to
        // downstream component steps as `ADO_AW_SAFE_OUTPUTS_STAGED=true`.
        println!("##vso[task.setvariable variable=ADO_AW_SAFE_OUTPUTS_STAGED]true");
    }
    println!(
        "Wrote {} custom proposal(s) for '{}' to {}",
        attempted.len(),
        sanitize_config(tool),
        proposals_out.display()
    );
    Ok(())
}

async fn execute_custom_post(
    source: &Path,
    safe_output_dir: &Path,
    dry_run: bool,
    options: &CustomExecuteOptions,
) -> Result<Vec<ExecutionResult>> {
    let tool = required_custom_tool(options)?;
    let results_in = options
        .results_in
        .as_ref()
        .context("--custom-phase post requires --results-in")?;
    if options.custom_config.is_some() || options.proposals_out.is_some() {
        anyhow::bail!(
            "--custom-phase post cannot be combined with --custom-config or --proposals-out"
        );
    }

    let max = load_custom_max(source, tool).await?;
    let entries = load_entries_or_empty(safe_output_dir).await?;
    let selected = select_custom_proposals(&entries, tool, max);
    let attempted_ids: HashSet<String> = selected
        .iter()
        .filter(|proposal| proposal.attempted())
        .map(|proposal| proposal.proposal_id.clone())
        .collect();
    let mut component_results = read_component_results(results_in, &attempted_ids).await?;
    let provenance = provenance_from_options(options);
    let mut results = Vec::new();

    for proposal in selected {
        let proposal_context = proposal
            .entry
            .get("context")
            .and_then(|value| value.as_str());
        if let Some(result) = proposal.budget_result {
            append_custom_execution_record_for_result(
                safe_output_dir,
                tool,
                &proposal.proposal_id,
                proposal.proposal_index,
                proposal_context,
                &result,
                provenance.clone(),
                0,
                dry_run,
            )
            .await;
            results.push(result);
            continue;
        }

        let started_at = now_timestamp();
        let outcome = if let Some(line) = component_results.remove(&proposal.proposal_id) {
            custom_status_to_outcome(&line.status, line.message, line.data)
        } else {
            CustomToolOutcome {
                result: ExecutionResult::failure(format!(
                    "Missing custom result for proposal_id '{}'",
                    sanitize(&proposal.proposal_id)
                )),
                record_status: "failed",
            }
        };
        let ended_at = now_timestamp();
        append_custom_execution_record_for_result_with_times(
            safe_output_dir,
            tool,
            &proposal.proposal_id,
            proposal.proposal_index,
            proposal_context,
            &outcome.result,
            provenance.clone(),
            CustomAttemptMetadata {
                number: 1,
                staged: dry_run || outcome.record_status == "staged",
                started_at,
                ended_at,
            },
            Some(outcome.record_status),
        )
        .await;
        results.push(outcome.result);
    }

    Ok(results)
}

fn required_custom_tool(options: &CustomExecuteOptions) -> Result<&str> {
    options
        .tool
        .as_deref()
        .context("--custom-phase requires --tool")
}

async fn load_custom_scripts_config(path: &Path) -> Result<CustomScriptsConfig> {
    let contents = tokio::fs::read_to_string(path)
        .await
        .with_context(|| format!("Failed to read custom config: {}", path.display()))?;
    serde_json::from_str(&contents)
        .with_context(|| format!("Failed to parse custom config: {}", path.display()))
}

async fn load_entries_or_empty(safe_output_dir: &Path) -> Result<Vec<Value>> {
    let safe_output_path = safe_output_dir.join(SAFE_OUTPUT_FILENAME);
    Ok(load_safe_output_entries(&safe_output_path)
        .await?
        .unwrap_or_default())
}

async fn load_custom_max(source: &Path, tool: &str) -> Result<usize> {
    let content = tokio::fs::read_to_string(source)
        .await
        .with_context(|| format!("Failed to read source file: {}", source.display()))?;
    let parsed = crate::compile::parse_markdown_detailed(&content)
        .with_context(|| format!("Failed to parse source file: {}", source.display()))?;
    let mut front_matter = parsed.front_matter;
    front_matter.sanitize_config_fields();
    Ok(custom_max_from_safe_outputs(
        &front_matter.safe_outputs,
        tool,
    ))
}

fn custom_max_from_safe_outputs(safe_outputs: &HashMap<String, Value>, tool: &str) -> usize {
    max_from_value(safe_outputs.get(tool))
        .or_else(|| nested_custom_max(safe_outputs, "scripts", tool))
        .or_else(|| nested_custom_max(safe_outputs, "jobs", tool))
        .unwrap_or(DEFAULT_CUSTOM_MAX)
}

fn nested_custom_max(
    safe_outputs: &HashMap<String, Value>,
    section: &str,
    tool: &str,
) -> Option<usize> {
    safe_outputs
        .get(section)
        .and_then(|section| section.get(tool))
        .and_then(|tool_cfg| max_from_value(Some(tool_cfg)))
}

fn max_from_value(value: Option<&Value>) -> Option<usize> {
    value
        .and_then(|v| v.get("max"))
        .and_then(|v| v.as_u64())
        .map(|v| v as usize)
}

fn select_custom_proposals(
    entries: &[Value],
    tool: &str,
    max: usize,
) -> Vec<SelectedCustomProposal> {
    let mut selected = Vec::new();
    let mut executed = 0usize;
    for (i, entry) in entries.iter().enumerate() {
        if entry.get("name").and_then(|name| name.as_str()) != Some(tool) {
            continue;
        }
        let context_id = extract_entry_context(entry);
        let budget_result = check_budget(entries.len(), i, tool, &context_id, executed, max);
        if budget_result.is_none() {
            executed += 1;
        }
        selected.push(SelectedCustomProposal {
            proposal_id: proposal_id(tool, i),
            proposal_index: i,
            entry: entry.clone(),
            budget_result,
        });
    }
    selected
}

fn proposal_id(tool: &str, index: usize) -> String {
    format!("{}-{}", sanitize_config(tool), index)
}

fn proposal_with_id(proposal: &SelectedCustomProposal) -> Value {
    let mut value = proposal.entry.clone();
    if let Value::Object(ref mut map) = value {
        map.insert(
            "proposal_id".to_string(),
            Value::String(proposal.proposal_id.clone()),
        );
        map.insert(
            "proposal_index".to_string(),
            Value::Number(serde_json::Number::from(proposal.proposal_index)),
        );
    }
    value
}

async fn write_ndjson_values(path: &Path, values: &[Value]) -> Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
    }
    let mut contents = String::new();
    for value in values {
        contents.push_str(&serde_json::to_string(value).context("Failed to serialize proposal")?);
        contents.push('\n');
    }
    tokio::fs::write(path, contents)
        .await
        .with_context(|| format!("Failed to write proposals file: {}", path.display()))
}

fn resolve_custom_cwd(config_dir: &Path, cwd: &Path) -> PathBuf {
    if cwd.is_absolute() {
        cwd.to_path_buf()
    } else {
        config_dir.join(cwd)
    }
}

async fn run_custom_entrypoint(
    tool: &str,
    proposal_id: &str,
    entrypoint: &str,
    cwd: &Path,
    proposal: &Value,
) -> CustomToolOutcome {
    let proposal_json = match serde_json::to_string(proposal) {
        Ok(json) => json,
        Err(err) => {
            return CustomToolOutcome {
                result: ExecutionResult::failure(format!(
                    "Failed to serialize custom proposal '{}': {}",
                    sanitize(proposal_id),
                    sanitize(&err.to_string())
                )),
                record_status: "failed",
            };
        }
    };

    let mut command = if cfg!(windows) {
        let mut command = Command::new("cmd");
        command.arg("/C").arg(entrypoint);
        command
    } else {
        let mut command = Command::new("sh");
        command.arg("-c").arg(entrypoint);
        command
    };
    command
        .current_dir(cwd)
        .env("AW_PROPOSAL", &proposal_json)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(err) => {
            return CustomToolOutcome {
                result: ExecutionResult::failure(format!(
                    "Failed to start custom tool '{}': {}",
                    sanitize_config(tool),
                    sanitize(&err.to_string())
                )),
                record_status: "failed",
            };
        }
    };

    // Write the proposal to the child's stdin CONCURRENTLY with draining its
    // stdout/stderr. Writing the whole payload before reading any output would
    // deadlock if the child emits more than a pipe buffer's worth before
    // consuming stdin. The payload is ALSO available to the child via the
    // `AW_PROPOSAL` env var; dropping the stdin handle when the write finishes
    // signals EOF to a dispatcher that reads stdin. A stdin write error is
    // non-fatal here (broken pipe if the child ignored stdin) — the child's own
    // exit status is authoritative and handled below.
    let stdin_payload = proposal_json.clone();
    if let Some(mut stdin) = child.stdin.take() {
        tokio::spawn(async move {
            let _ = stdin.write_all(stdin_payload.as_bytes()).await;
            // `stdin` is dropped here, closing the pipe (EOF).
        });
    }

    let output = match child.wait_with_output().await {
        Ok(output) => output,
        Err(err) => {
            return CustomToolOutcome {
                result: ExecutionResult::failure(format!(
                    "Failed to wait for custom tool '{}': {}",
                    sanitize_config(tool),
                    sanitize(&err.to_string())
                )),
                record_status: "failed",
            };
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let detail = if stderr.trim().is_empty() {
            stdout.trim()
        } else {
            stderr.trim()
        };
        return CustomToolOutcome {
            result: ExecutionResult::failure(format!(
                "Custom tool '{}' exited with status {}{}",
                sanitize_config(tool),
                output.status,
                if detail.is_empty() {
                    String::new()
                } else {
                    format!(": {}", sanitize(detail))
                }
            )),
            record_status: "failed",
        };
    }

    parse_script_result_stdout(tool, &output.stdout)
}

fn parse_script_result_stdout(tool: &str, stdout: &[u8]) -> CustomToolOutcome {
    let stdout = String::from_utf8_lossy(stdout);
    let lines: Vec<&str> = stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    if lines.len() != 1 {
        // Distinguish "no output" from "extra output" and, when there is stray
        // output, surface the first line so an author can locate a debug
        // `console.log`/`print` that broke the one-JSON-line contract.
        let detail = if lines.is_empty() {
            " (no output — the tool must print exactly one JSON result line to stdout)"
                .to_string()
        } else {
            let first: String = lines[0].chars().take(200).collect();
            format!(
                " — is debug output going to stdout? first line: {}",
                sanitize(&first)
            )
        };
        return CustomToolOutcome {
            result: ExecutionResult::failure(format!(
                "Custom tool '{}' must print exactly one JSON line, got {}{}",
                sanitize_config(tool),
                lines.len(),
                detail
            )),
            record_status: "failed",
        };
    }
    let parsed: ScriptResultLine = match serde_json::from_str(lines[0]) {
        Ok(parsed) => parsed,
        Err(err) => {
            return CustomToolOutcome {
                result: ExecutionResult::failure(format!(
                    "Custom tool '{}' printed malformed result JSON: {}",
                    sanitize_config(tool),
                    sanitize(&err.to_string())
                )),
                record_status: "failed",
            };
        }
    };
    custom_status_to_outcome(&parsed.status, parsed.message, parsed.data)
}

async fn read_component_results(
    path: &Path,
    attempted_ids: &HashSet<String>,
) -> Result<HashMap<String, ComponentResultLine>> {
    let values = if path.exists() {
        ndjson::read_ndjson_file(path).await?
    } else {
        Vec::new()
    };
    let mut results = HashMap::new();
    for value in values {
        let schema_version = value.get("schema_version").and_then(|v| v.as_u64());
        if schema_version != Some(CUSTOM_SCHEMA_VERSION as u64) {
            anyhow::bail!(
                "Custom result record has missing or unsupported schema_version: {}",
                value
                    .get("schema_version")
                    .map(Value::to_string)
                    .unwrap_or_else(|| "<missing>".to_string())
            );
        }
        let line: ComponentResultLine =
            serde_json::from_value(value).context("Malformed custom result record")?;
        if !attempted_ids.contains(&line.proposal_id) {
            anyhow::bail!(
                "Custom result references unknown proposal_id '{}'",
                sanitize(&line.proposal_id)
            );
        }
        if results.insert(line.proposal_id.clone(), line).is_some() {
            anyhow::bail!("Duplicate custom result record for proposal_id");
        }
    }
    Ok(results)
}

fn custom_status_to_outcome(
    status: &str,
    message: String,
    data: Option<Value>,
) -> CustomToolOutcome {
    let message = sanitize(&message);
    let data = data.map(sanitize_json_value);
    match status {
        "success" | "succeeded" => CustomToolOutcome {
            result: match data {
                Some(data) => ExecutionResult::success_with_data(message, data),
                None => ExecutionResult::success(message),
            },
            record_status: "succeeded",
        },
        "failure" | "failed" => CustomToolOutcome {
            result: match data {
                Some(data) => ExecutionResult::failure_with_data(message, data),
                None => ExecutionResult::failure(message),
            },
            record_status: "failed",
        },
        "staged" => CustomToolOutcome {
            result: match data {
                Some(data) => ExecutionResult::success_with_data(message, data),
                None => ExecutionResult::success(message),
            },
            record_status: "staged",
        },
        other => CustomToolOutcome {
            result: ExecutionResult::failure(format!(
                "Custom result has unsupported status '{}'",
                sanitize(other)
            )),
            record_status: "failed",
        },
    }
}

fn sanitize_json_value(value: Value) -> Value {
    match value {
        Value::String(s) => Value::String(sanitize(&s)),
        Value::Array(values) => Value::Array(values.into_iter().map(sanitize_json_value).collect()),
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(key, value)| (sanitize(&key), sanitize_json_value(value)))
                .collect(),
        ),
        other => other,
    }
}

fn provenance_from_options(options: &CustomExecuteOptions) -> CustomComponentProvenance {
    CustomComponentProvenance {
        source: options.component_source.as_deref().map(sanitize_config),
        sha: options.component_sha.as_deref().map(sanitize_config),
        manifest_digest: options.manifest_digest.as_deref().map(sanitize_config),
        schema_digest: options.schema_digest.as_deref().map(sanitize_config),
    }
}

fn now_timestamp() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn custom_record_status(result: &ExecutionResult, staged: bool) -> &'static str {
    if result.is_budget_exhausted() {
        "budget_exhausted"
    } else if staged && result.success {
        "staged"
    } else {
        execution_record_status(result)
    }
}

#[allow(clippy::too_many_arguments)]
async fn append_custom_execution_record_for_result(
    safe_output_dir: &Path,
    tool: &str,
    proposal_id: &str,
    proposal_index: usize,
    proposal_context: Option<&str>,
    result: &ExecutionResult,
    provenance: CustomComponentProvenance,
    attempt_number: u32,
    staged: bool,
) {
    let timestamp = now_timestamp();
    append_custom_execution_record_for_result_with_times(
        safe_output_dir,
        tool,
        proposal_id,
        proposal_index,
        proposal_context,
        result,
        provenance,
        CustomAttemptMetadata {
            number: attempt_number,
            staged,
            started_at: timestamp.clone(),
            ended_at: timestamp,
        },
        None,
    )
    .await;
}

#[allow(clippy::too_many_arguments)]
async fn append_custom_execution_record_for_result_with_times(
    safe_output_dir: &Path,
    tool: &str,
    proposal_id: &str,
    proposal_index: usize,
    proposal_context: Option<&str>,
    result: &ExecutionResult,
    provenance: CustomComponentProvenance,
    attempt: CustomAttemptMetadata,
    record_status_override: Option<&str>,
) {
    let status = record_status_override
        .map(str::to_string)
        .unwrap_or_else(|| custom_record_status(result, attempt.staged).to_string());
    let data = result.data.clone().map(sanitize_json_value);
    let message = sanitize(&result.message);
    let record = CustomExecutionRecord {
        schema_version: CUSTOM_SCHEMA_VERSION,
        tool: sanitize_config(tool),
        proposal_id: sanitize(proposal_id),
        proposal_index,
        name: sanitize_config(tool),
        status: status.clone(),
        message: message.clone(),
        data: data.clone(),
        component: provenance,
        attempt,
        context: proposal_context.map(sanitize),
        result: if matches!(status.as_str(), "succeeded" | "staged") {
            data
        } else {
            None
        },
        error: if matches!(status.as_str(), "succeeded" | "staged") {
            None
        } else {
            Some(message)
        },
        timestamp: now_timestamp(),
    };
    append_custom_execution_record(safe_output_dir, &record).await;
}

async fn append_custom_execution_record(safe_output_dir: &Path, record: &CustomExecutionRecord) {
    if let Err(err) = append_custom_execution_record_impl(safe_output_dir, record).await {
        warn!(
            "Failed to append custom execution record for {}: {}",
            record.tool,
            neutralize_pipeline_commands(&err.to_string())
        );
    }
}

async fn append_custom_execution_record_impl(
    safe_output_dir: &Path,
    record: &CustomExecutionRecord,
) -> Result<()> {
    let line = serde_json::to_string(record)
        .context("Failed to serialize custom execution record")?
        + "\n";
    let path = safe_output_dir.join(EXECUTED_NDJSON_FILENAME);
    let mut file = OpenOptions::new()
        .append(true)
        .create(true)
        .open(&path)
        .await
        .with_context(|| format!("Failed to open executed NDJSON file: {}", path.display()))?;
    file.write_all(line.as_bytes())
        .await
        .with_context(|| format!("Failed to append executed NDJSON file: {}", path.display()))?;
    file.flush()
        .await
        .with_context(|| format!("Failed to flush executed NDJSON file: {}", path.display()))?;
    Ok(())
}

/// Execute all safe outputs from the NDJSON file in the specified directory
pub async fn execute_safe_outputs(
    safe_output_dir: &Path,
    ctx: &ExecutionContext,
    filter: &ToolFilter,
) -> Result<Vec<ExecutionResult>> {
    let safe_output_path = safe_output_dir.join(SAFE_OUTPUT_FILENAME);

    log_execution_context(safe_output_dir, ctx);

    let Some(entries) = load_safe_output_entries(&safe_output_path).await? else {
        return Ok(vec![]);
    };

    log_queued_entries(&entries);

    // Build budget map: tool_name → (executed_count, max_allowed).
    // Each tool declares its DEFAULT_MAX via the ToolResult trait; the operator can
    // override it with `max` in the front-matter config JSON.
    //
    // IMPORTANT: When adding a new ToolResult implementor, also register it here
    // so its budget is enforced. There is no compile-time guard for this.
    let mut budgets: HashMap<&'static str, (usize, usize)> = HashMap::new();
    macro_rules! register_budgets {
        ($($tool:ty),+ $(,)?) => {
            $({
                let name = <$tool>::NAME;
                let default = <$tool>::DEFAULT_MAX;
                let max = resolve_max(ctx, name, default);
                budgets.insert(name, (0, max));
            })+
        };
    }
    register_budgets!(
        CreateWorkItemResult,
        CreatePrResult,
        UpdateWorkItemResult,
        CommentOnWorkItemResult,
        CreateWikiPageResult,
        UpdateWikiPageResult,
        AddPrCommentResult,
        LinkWorkItemsResult,
        QueueBuildResult,
        CreateGitTagResult,
        AddBuildTagResult,
        CreateBranchResult,
        UpdatePrResult,
        UploadBuildAttachmentResult,
        UploadPipelineArtifactResult,
        UploadWorkitemAttachmentResult,
        SubmitPrReviewResult,
        ReplyToPrCommentResult,
        ResolvePrThreadResult,
        CreateIssueResult,
    );

    let mut results = Vec::new();
    for (i, entry) in entries.iter().enumerate() {
        if let Some(result) = process_one_entry(
            i,
            entries.len(),
            entry,
            &mut budgets,
            filter,
            ctx,
            safe_output_dir,
        )
        .await
        {
            results.push(result);
        }
    }

    // Log final summary
    let success_count = results
        .iter()
        .filter(|r| r.success && !r.is_warning())
        .count();
    let warning_count = results.iter().filter(|r| r.is_warning()).count();
    let failure_count = results.iter().filter(|r| !r.success).count();
    info!(
        "Stage 3 execution complete: {} succeeded, {} warnings, {} failed",
        success_count, warning_count, failure_count
    );

    Ok(results)
}

/// Load and validate safe-output entries from the specified NDJSON path.
///
/// Returns `Ok(None)` when there is nothing to execute (file absent or empty),
/// with appropriate log and console output already emitted.
async fn load_safe_output_entries(path: &Path) -> Result<Option<Vec<Value>>> {
    if !path.exists() {
        info!("No safe outputs file found at: {}", path.display());
        println!("No safe outputs file found at: {}", path.display());
        return Ok(None);
    }
    info!("Processing safe outputs: {}", path.display());
    println!("Processing safe outputs: {}", path.display());
    let entries = ndjson::read_ndjson_file(path).await?;
    if entries.is_empty() {
        info!("Safe outputs file is empty");
        println!("Safe outputs file is empty");
        return Ok(None);
    }
    info!("Found {} safe output(s) to execute", entries.len());
    println!("Found {} safe output(s) to execute", entries.len());
    Ok(Some(entries))
}

/// Process a single safe-output entry: apply the tool filter, enforce the budget, and execute.
///
/// Returns `None` when the entry is filtered out (caller skips it, nothing pushed to results).
/// Returns `Some(result)` for all other outcomes: budget-exhausted, successful, and failed.
#[allow(clippy::too_many_arguments)]
async fn process_one_entry(
    i: usize,
    total: usize,
    entry: &Value,
    budgets: &mut HashMap<&'static str, (usize, usize)>,
    filter: &ToolFilter,
    ctx: &ExecutionContext,
    safe_output_dir: &Path,
) -> Option<ExecutionResult> {
    let proposal_context = entry.get("context").and_then(|value| value.as_str());
    let proposal_tool_name = entry
        .get("name")
        .and_then(|name| name.as_str())
        .unwrap_or("unknown");

    // Skip entries the active filter excludes (manual-review split: the
    // auto job excludes reviewed tools; the reviewed job runs only them).
    if !filter.allows(proposal_tool_name) {
        debug!(
            "[{}/{}] Skipping entry for tool '{}' (filtered out)",
            i + 1,
            total,
            proposal_tool_name
        );
        return None;
    }

    let entry_json = serde_json::to_string(entry).unwrap_or_else(|_| "<invalid>".to_string());
    debug!("[{}/{}] Executing entry: {}", i + 1, total, entry_json);

    // Generic budget enforcement: skip excess entries rather than aborting the whole batch.
    // Budget is consumed before execution so that failed attempts (target policy rejection,
    // network errors) still count — this prevents unbounded retries against a failing endpoint.
    if let Some(result) = enforce_budget(entry, budgets, total, i) {
        append_execution_record(
            safe_output_dir,
            proposal_tool_name,
            &result,
            proposal_context,
        )
        .await;
        return Some(result);
    }

    let result = match execute_safe_output(entry, ctx).await {
        Ok((tool_name, result)) => {
            log_and_print_entry_result(i, total, &tool_name, &result);
            append_execution_record(safe_output_dir, &tool_name, &result, proposal_context).await;
            result
        }
        Err(e) => {
            error!("[{}/{}] Execution error: {}", i + 1, total, e);
            let raw_msg = format!("Failed to execute entry: {}", e);
            let safe_msg = neutralize_pipeline_commands(&raw_msg);
            let result = ExecutionResult::failure(safe_msg);
            println!("[{}/{}] ✗ - {}", i + 1, total, result.message);
            append_execution_record(
                safe_output_dir,
                proposal_tool_name,
                &result,
                proposal_context,
            )
            .await;
            result
        }
    };
    Some(result)
}

/// Emit debug-level context about the execution environment at Stage 3 startup.
fn log_execution_context(safe_output_dir: &Path, ctx: &ExecutionContext) {
    info!("Stage 3 execution starting");
    debug!("Safe output directory: {}", safe_output_dir.display());
    debug!("Source directory: {}", ctx.source_directory.display());
    debug!(
        "ADO org: {}",
        ctx.ado_org_url.as_deref().unwrap_or("<not set>")
    );
    debug!(
        "ADO project: {}",
        ctx.ado_project.as_deref().unwrap_or("<not set>")
    );
    debug!(
        "Repository ID: {}",
        ctx.repository_id.as_deref().unwrap_or("<not set>")
    );
    debug!(
        "Repository name: {}",
        ctx.repository_name.as_deref().unwrap_or("<not set>")
    );
    debug!(
        "Build ID: {}",
        ctx.build_id
            .map(|id| id.to_string())
            .unwrap_or_else(|| "<not set>".to_string())
    );
    debug!(
        "Build reason: {}",
        ctx.build_reason.as_deref().unwrap_or("<not set>")
    );
    debug!(
        "Triggered by definition: {}",
        ctx.triggered_by_definition_name
            .as_deref()
            .unwrap_or("<not set>")
    );
    if !ctx.allowed_repositories.is_empty() {
        debug!(
            "Allowed repositories: {}",
            ctx.allowed_repositories
                .keys()
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
}

/// Log each queued entry at debug level before execution begins.
fn log_queued_entries(entries: &[Value]) {
    for (i, entry) in entries.iter().enumerate() {
        if let Some(name) = entry.get("name").and_then(|n| n.as_str()) {
            debug!("[{}/{}] Queued: {}", i + 1, entries.len(), name);
        }
    }
}

/// Check the per-tool budget for an entry.
///
/// Returns `Some(result)` when the budget is exhausted (caller should push the result and
/// skip execution). When a slot is available the counter is incremented and `None` is
/// returned so execution can proceed.
fn enforce_budget(
    entry: &Value,
    budgets: &mut HashMap<&'static str, (usize, usize)>,
    total: usize,
    i: usize,
) -> Option<ExecutionResult> {
    let tool_name = entry.get("name").and_then(|n| n.as_str())?;
    let (executed, max) = budgets.get_mut(tool_name)?;
    let context_id = extract_entry_context(entry);
    if let Some(result) = check_budget(total, i, tool_name, &context_id, *executed, *max) {
        return Some(result);
    }
    *executed += 1;
    None
}

/// Log and print the outcome of a single safe-output execution.
fn log_and_print_entry_result(i: usize, total: usize, tool_name: &str, result: &ExecutionResult) {
    if result.is_warning() {
        warn!(
            "[{}/{}] {} warning: {}",
            i + 1,
            total,
            tool_name,
            result.message
        );
    } else if result.success {
        info!(
            "[{}/{}] {} succeeded: {}",
            i + 1,
            total,
            tool_name,
            result.message
        );
    } else {
        warn!(
            "[{}/{}] {} failed: {}",
            i + 1,
            total,
            tool_name,
            result.message
        );
    }
    let symbol = if result.is_warning() {
        "⚠"
    } else if result.success {
        "✓"
    } else {
        "✗"
    };
    let safe_msg = neutralize_pipeline_commands(&result.message);
    println!(
        "[{}/{}] {} - {} - {}",
        i + 1,
        total,
        tool_name,
        symbol,
        safe_msg
    );
}

#[derive(Serialize)]
struct ExecutionRecord {
    name: String,
    status: &'static str,
    context: Option<String>,
    result: Option<Value>,
    error: Option<String>,
    timestamp: String,
}

fn execution_record_status(result: &ExecutionResult) -> &'static str {
    if result.is_budget_exhausted() {
        "budget_exhausted"
    } else if result.is_warning() {
        // Tools such as `noop` and `missing-tool` succeed with a warning when
        // they have nothing to persist (e.g. missing ADO credentials). They
        // ran successfully — they just produced no externally-visible artifact
        // — so report this as a distinct `warning` status rather than
        // conflating it with the `skipped` rejection bucket.
        "warning"
    } else if result.success {
        "succeeded"
    } else {
        "failed"
    }
}

async fn append_execution_record_impl(
    safe_output_dir: &Path,
    tool_name: &str,
    result: &ExecutionResult,
    proposal_context: Option<&str>,
) -> Result<()> {
    let status = execution_record_status(result);
    let record = ExecutionRecord {
        name: tool_name.replace('-', "_"),
        status,
        context: proposal_context.map(str::to_owned),
        result: if status == "succeeded" {
            result.data.clone()
        } else {
            None
        },
        error: if status == "succeeded" {
            None
        } else {
            Some(result.message.clone())
        },
        timestamp: Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
    };
    let line =
        serde_json::to_string(&record).context("Failed to serialize execution record")? + "\n";
    let path = safe_output_dir.join(EXECUTED_NDJSON_FILENAME);
    let mut file = OpenOptions::new()
        .append(true)
        .create(true)
        .open(&path)
        .await
        .with_context(|| format!("Failed to open executed NDJSON file: {}", path.display()))?;
    file.write_all(line.as_bytes())
        .await
        .with_context(|| format!("Failed to append executed NDJSON file: {}", path.display()))?;
    file.flush()
        .await
        .with_context(|| format!("Failed to flush executed NDJSON file: {}", path.display()))?;
    Ok(())
}

/// Append one execution record to `<safe_output_dir>/safe-outputs-executed.ndjson`,
/// creating the file on first call. Errors are logged at WARN level and swallowed —
/// failing to append to the audit log must never break Stage 3 execution.
pub async fn append_execution_record(
    safe_output_dir: &Path,
    tool_name: &str,
    result: &ExecutionResult,
    proposal_context: Option<&str>,
) {
    if let Err(err) =
        append_execution_record_impl(safe_output_dir, tool_name, result, proposal_context).await
    {
        warn!(
            "Failed to append execution record for {}: {}",
            tool_name,
            neutralize_pipeline_commands(&err.to_string())
        );
    }
}

/// Parse a JSON entry as `T` and run it through `execute_sanitized`.
///
/// This is the common path for all tools that implement `Executor`. The tool name
/// is used only for the error message so callers don't have to repeat it.
async fn dispatch_tool<T>(
    tool_name: &str,
    entry: &Value,
    ctx: &ExecutionContext,
) -> Result<ExecutionResult>
where
    T: DeserializeOwned + Executor,
{
    debug!("Parsing {} payload", tool_name);
    let mut output: T = serde_json::from_value(entry.clone())
        .map_err(|e| anyhow::anyhow!("Failed to parse {}: {}", tool_name, e))?;
    output.execute_sanitized(ctx).await
}

macro_rules! dispatch_executor_tools {
    ($tool_name:expr, $entry:expr, $ctx:expr, { $($name:literal => $ty:ty),+ $(,)? }) => {
        match $tool_name {
            $(
                $name => dispatch_tool::<$ty>($tool_name, $entry, $ctx).await.map(Some),
            )+
            _ => Ok(None),
        }
    };
}

/// Execute a single safe output entry, returning the tool name and result
pub async fn execute_safe_output(
    entry: &Value,
    ctx: &ExecutionContext,
) -> Result<(String, ExecutionResult)> {
    // First check the name field to dispatch correctly
    let tool_name = entry
        .get("name")
        .and_then(|n| n.as_str())
        .ok_or_else(|| anyhow::anyhow!("Safe output missing 'name' field"))?;

    debug!("Dispatching tool: {}", tool_name);

    // Dispatch based on tool name. All registered tools go through `dispatch_tool`,
    // which handles deserialization and sanitized execution uniformly.
    // The dispatch is split across category helpers to keep each function's complexity low.
    let result = find_tool_executor(tool_name, entry, ctx)
        .await?
        .ok_or_else(|| {
            error!("Unknown tool type: {}", tool_name);
            anyhow::anyhow!("Unknown tool type: {}. No executor registered.", tool_name)
        })?;

    Ok((tool_name.to_string(), result))
}

/// Try each dispatch category in order and return the first match.
async fn find_tool_executor(
    tool_name: &str,
    entry: &Value,
    ctx: &ExecutionContext,
) -> Result<Option<ExecutionResult>> {
    if let Some(r) = dispatch_meta_tools(tool_name, entry, ctx).await? {
        return Ok(Some(r));
    }
    if let Some(r) = dispatch_work_item_tools(tool_name, entry, ctx).await? {
        return Ok(Some(r));
    }
    if let Some(r) = dispatch_pr_tools(tool_name, entry, ctx).await? {
        return Ok(Some(r));
    }
    if let Some(r) = dispatch_resource_tools(tool_name, entry, ctx).await? {
        return Ok(Some(r));
    }
    if let Some(r) = dispatch_debug_tools(tool_name, entry, ctx).await? {
        return Ok(Some(r));
    }
    Ok(None)
}

/// Dispatch meta/signal tools: noop, missing-tool, missing-data, report-incomplete.
async fn dispatch_meta_tools(
    tool_name: &str,
    entry: &Value,
    ctx: &ExecutionContext,
) -> Result<Option<ExecutionResult>> {
    dispatch_executor_tools!(tool_name, entry, ctx, {
        "noop" => NoopResult,
        "missing-tool" => MissingToolResult,
        "missing-data" => MissingDataResult,
        "report-incomplete" => ReportIncompleteResult,
    })
}

/// Dispatch work-item tools.
async fn dispatch_work_item_tools(
    tool_name: &str,
    entry: &Value,
    ctx: &ExecutionContext,
) -> Result<Option<ExecutionResult>> {
    dispatch_executor_tools!(tool_name, entry, ctx, {
        "create-work-item" => CreateWorkItemResult,
        "comment-on-work-item" => CommentOnWorkItemResult,
        "update-work-item" => UpdateWorkItemResult,
        "link-work-items" => LinkWorkItemsResult,
        "upload-workitem-attachment" => UploadWorkitemAttachmentResult,
    })
}

/// Dispatch pull-request tools.
async fn dispatch_pr_tools(
    tool_name: &str,
    entry: &Value,
    ctx: &ExecutionContext,
) -> Result<Option<ExecutionResult>> {
    dispatch_executor_tools!(tool_name, entry, ctx, {
        "create-pull-request" => CreatePrResult,
        "add-pr-comment" => AddPrCommentResult,
        "update-pr" => UpdatePrResult,
        "submit-pr-review" => SubmitPrReviewResult,
        "reply-to-pr-comment" => ReplyToPrCommentResult,
        "resolve-pr-thread" => ResolvePrThreadResult,
    })
}

/// Dispatch git, build, and wiki tools.
async fn dispatch_resource_tools(
    tool_name: &str,
    entry: &Value,
    ctx: &ExecutionContext,
) -> Result<Option<ExecutionResult>> {
    dispatch_executor_tools!(tool_name, entry, ctx, {
        "update-wiki-page" => UpdateWikiPageResult,
        "create-wiki-page" => CreateWikiPageResult,
        "queue-build" => QueueBuildResult,
        "create-git-tag" => CreateGitTagResult,
        "add-build-tag" => AddBuildTagResult,
        "create-branch" => CreateBranchResult,
        "upload-build-attachment" => UploadBuildAttachmentResult,
        "upload-pipeline-artifact" => UploadPipelineArtifactResult,
    })
}

/// Dispatch debug-only tools (gated by `ado-aw-debug:` front-matter section
/// at compile time and `DEBUG_ONLY_TOOLS` at the MCP layer at runtime).
async fn dispatch_debug_tools(
    tool_name: &str,
    entry: &Value,
    ctx: &ExecutionContext,
) -> Result<Option<ExecutionResult>> {
    dispatch_executor_tools!(tool_name, entry, ctx, {
        "create-issue" => CreateIssueResult,
    })
}

/// Read the operator's `max` override from the tool's config JSON, falling back to the
/// tool's `DEFAULT_MAX` (declared on the `ToolResult` trait) when not configured.
fn resolve_max(ctx: &ExecutionContext, tool_name: &str, default_max: u32) -> usize {
    ctx.tool_configs
        .get(tool_name)
        .and_then(|v| v.get("max"))
        .and_then(|v| v.as_u64())
        .map(|v| v as usize)
        .unwrap_or(default_max as usize)
}

/// Extract a human-readable context identifier from a safe-output entry for log messages.
/// Called before sanitization, so all string values are stripped of control characters
/// and ADO pipeline commands are neutralized to prevent log injection via stdout.
fn extract_entry_context(entry: &Value) -> String {
    if let Some(id) = entry.get("id").and_then(|v| v.as_u64()) {
        return format!(" (work item #{})", id);
    }
    if let Some(id) = entry.get("work_item_id").and_then(|v| v.as_i64()) {
        return format!(" (work item #{})", id);
    }
    if let Some(title) = entry.get("title").and_then(|v| v.as_str()) {
        let clean: String = title.chars().filter(|c| !c.is_control()).collect();
        let clean = neutralize_pipeline_commands(&clean);
        let truncated: &str = if clean.chars().count() > 40 {
            &clean[..clean
                .char_indices()
                .nth(40)
                .map(|(i, _)| i)
                .unwrap_or(clean.len())]
        } else {
            &clean
        };
        return format!(" (\"{}\")", truncated);
    }
    if let Some(path) = entry.get("path").and_then(|v| v.as_str()) {
        let clean: String = path.chars().filter(|c| !c.is_control()).collect();
        let clean = neutralize_pipeline_commands(&clean);
        return format!(" (path: {})", clean);
    }
    String::new()
}

/// Returns `Some(result)` when the budget for `tool_name` is exhausted so the caller can push the
/// result and `continue` to the next entry. Returns `None` when a budget slot is still available
/// and the caller should proceed with execution.
///
/// `total` is the total number of entries (for the `[i/total]` log prefix), `i` is the
/// zero-based index of the current entry, `wi_id` is a pre-formatted context string like
/// `" (work item #42)"` or `""`.
fn check_budget(
    total: usize,
    i: usize,
    tool_name: &str,
    wi_id: &str,
    executed: usize,
    max: usize,
) -> Option<ExecutionResult> {
    if executed < max {
        return None;
    }
    warn!(
        "[{}/{}] Skipping {}{} entry: max ({}) already reached for this run",
        i + 1,
        total,
        tool_name,
        wi_id,
        max
    );
    let result = ExecutionResult::budget_exhausted(format!(
        "Skipped{}: maximum {} count ({}) already reached. \
         Increase 'max' in safe-outputs.{} to allow more.",
        wi_id, tool_name, max, tool_name
    ));
    println!(
        "[{}/{}] {} - ✗ - {}",
        i + 1,
        total,
        tool_name,
        result.message
    );
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::path::PathBuf;

    // ── extract_entry_context ─────────────────────────────────────────────────

    #[test]
    fn test_extract_entry_context_neutralizes_vso_in_title() {
        let entry = serde_json::json!({
            "title": "##vso[task.complete result=Failed]"
        });
        let ctx = extract_entry_context(&entry);
        assert!(
            !ctx.contains("##vso[task."),
            "VSO command in title should be neutralized; got: {ctx}"
        );
        assert!(
            ctx.contains("`##vso[`"),
            "VSO command should be wrapped in backticks; got: {ctx}"
        );
    }

    #[test]
    fn test_extract_entry_context_neutralizes_vso_in_path() {
        let entry = serde_json::json!({
            "path": "##vso[task.setvariable variable=X]injected"
        });
        let ctx = extract_entry_context(&entry);
        assert!(
            !ctx.contains("##vso[task."),
            "VSO command in path should be neutralized; got: {ctx}"
        );
        assert!(
            ctx.contains("`##vso[`"),
            "VSO command should be wrapped in backticks; got: {ctx}"
        );
    }

    #[test]
    fn test_extract_entry_context_prefers_id_over_title() {
        let entry = serde_json::json!({"id": 42, "title": "should be ignored"});
        assert_eq!(extract_entry_context(&entry), " (work item #42)");
    }

    #[test]
    fn test_tool_filter_allows() {
        // Empty filter allows everything.
        let f = ToolFilter::default();
        assert!(f.allows("create-pull-request"));
        assert!(f.allows("add-pr-comment"));

        // `only` restricts to the listed tools.
        let f = ToolFilter {
            only: vec!["create-pull-request".into()],
            exclude: vec![],
        };
        assert!(f.allows("create-pull-request"));
        assert!(!f.allows("add-pr-comment"));

        // `exclude` removes the listed tools.
        let f = ToolFilter {
            only: vec![],
            exclude: vec!["create-pull-request".into()],
        };
        assert!(!f.allows("create-pull-request"));
        assert!(f.allows("add-pr-comment"));
    }

    async fn write_success_script(dir: &Path) -> String {
        tokio::fs::write(
            dir.join("success.py"),
            "import json\nprint(json.dumps({'status':'success','message':'ok'}))\n",
        )
        .await
        .unwrap();
        "python success.py".to_string()
    }

    fn failing_entrypoint() -> &'static str {
        if cfg!(windows) { "exit /B 1" } else { "exit 1" }
    }

    async fn write_safe_outputs(dir: &Path, contents: &str) {
        tokio::fs::write(dir.join(SAFE_OUTPUT_FILENAME), contents)
            .await
            .unwrap();
    }

    async fn write_custom_config(dir: &Path, entrypoint: &str, max: usize) -> PathBuf {
        let path = dir.join("custom-config.json");
        let config = serde_json::json!({
            "tools": {
                "send-notification": {
                    "entrypoint": entrypoint,
                    "cwd": ".",
                    "max": max
                }
            }
        });
        tokio::fs::write(&path, serde_json::to_string(&config).unwrap())
            .await
            .unwrap();
        path
    }

    async fn write_custom_source(dir: &Path, max: usize) -> PathBuf {
        let path = dir.join("agent.md");
        let content = format!(
            r#"---
name: Custom executor test
description: Test custom executor
safe-outputs:
  jobs:
    send-notification:
      max: {max}
---

Test body.
"#
        );
        tokio::fs::write(&path, content).await.unwrap();
        path
    }

    async fn read_executed_records(dir: &Path) -> Vec<Value> {
        ndjson::read_ndjson_file(&dir.join(EXECUTED_NDJSON_FILENAME))
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn test_custom_execute_scripts_native_dispatch_success_record() {
        let temp_dir = tempfile::tempdir().unwrap();
        write_safe_outputs(
            temp_dir.path(),
            r#"{"name":"send-notification","context":"hello"}"#,
        )
        .await;
        let entrypoint = write_success_script(temp_dir.path()).await;
        let config_path = write_custom_config(temp_dir.path(), &entrypoint, 3).await;

        let results = execute_custom_safe_outputs(
            Path::new("unused.md"),
            temp_dir.path(),
            false,
            CustomExecuteOptions {
                custom_config: Some(config_path),
                ..Default::default()
            },
        )
        .await
        .unwrap();

        assert_eq!(results.len(), 1);
        assert!(results[0].success);
        let records = read_executed_records(temp_dir.path()).await;
        assert_eq!(records[0]["name"], "send-notification");
        assert_eq!(records[0]["status"], "succeeded");
        assert_eq!(records[0]["message"], "ok");
    }

    #[tokio::test]
    async fn test_custom_execute_scripts_native_dispatch_failure_record() {
        let temp_dir = tempfile::tempdir().unwrap();
        write_safe_outputs(temp_dir.path(), r#"{"name":"send-notification"}"#).await;
        let config_path = write_custom_config(temp_dir.path(), failing_entrypoint(), 3).await;

        let results = execute_custom_safe_outputs(
            Path::new("unused.md"),
            temp_dir.path(),
            false,
            CustomExecuteOptions {
                custom_config: Some(config_path),
                ..Default::default()
            },
        )
        .await
        .unwrap();

        assert_eq!(results.len(), 1);
        assert!(!results[0].success);
        let records = read_executed_records(temp_dir.path()).await;
        assert_eq!(records[0]["status"], "failed");
    }

    #[tokio::test]
    async fn test_custom_execute_scripts_budget_exhausted_record() {
        let temp_dir = tempfile::tempdir().unwrap();
        write_safe_outputs(
            temp_dir.path(),
            r#"{"name":"send-notification","context":"first"}
{"name":"send-notification","context":"second"}
"#,
        )
        .await;
        let entrypoint = write_success_script(temp_dir.path()).await;
        let config_path = write_custom_config(temp_dir.path(), &entrypoint, 1).await;

        let results = execute_custom_safe_outputs(
            Path::new("unused.md"),
            temp_dir.path(),
            false,
            CustomExecuteOptions {
                custom_config: Some(config_path),
                ..Default::default()
            },
        )
        .await
        .unwrap();

        assert_eq!(results.len(), 2);
        assert!(results[1].is_budget_exhausted());
        let records = read_executed_records(temp_dir.path()).await;
        assert_eq!(records[1]["status"], "budget_exhausted");
    }

    #[tokio::test]
    async fn test_custom_execute_scripts_dry_run_stages_without_spawn() {
        let temp_dir = tempfile::tempdir().unwrap();
        write_safe_outputs(temp_dir.path(), r#"{"name":"send-notification"}"#).await;
        let config_path =
            write_custom_config(temp_dir.path(), "definitely-not-a-real-command-ado-aw", 3).await;

        let results = execute_custom_safe_outputs(
            Path::new("unused.md"),
            temp_dir.path(),
            true,
            CustomExecuteOptions {
                custom_config: Some(config_path),
                ..Default::default()
            },
        )
        .await
        .unwrap();

        assert_eq!(results.len(), 1);
        assert!(results[0].success);
        let records = read_executed_records(temp_dir.path()).await;
        assert_eq!(records[0]["status"], "staged");
    }

    #[tokio::test]
    async fn test_custom_execute_jobs_pre_writes_filtered_proposals_with_ids() {
        let temp_dir = tempfile::tempdir().unwrap();
        let source = write_custom_source(temp_dir.path(), 2).await;
        let proposals_out = temp_dir.path().join("proposals.ndjson");
        write_safe_outputs(
            temp_dir.path(),
            r#"{"name":"send-notification","message":"first"}
{"name":"noop","context":"ignore"}
{"name":"send-notification","message":"second"}
"#,
        )
        .await;

        execute_custom_safe_outputs(
            &source,
            temp_dir.path(),
            false,
            CustomExecuteOptions {
                custom_phase: Some("pre".to_string()),
                tool: Some("send-notification".to_string()),
                proposals_out: Some(proposals_out.clone()),
                ..Default::default()
            },
        )
        .await
        .unwrap();

        let proposals = ndjson::read_ndjson_file(&proposals_out).await.unwrap();
        assert_eq!(proposals.len(), 2);
        assert_eq!(proposals[0]["proposal_id"], "send-notification-0");
        assert_eq!(proposals[1]["proposal_id"], "send-notification-2");
    }

    #[tokio::test]
    async fn test_custom_execute_jobs_post_enriches_component_results() {
        let temp_dir = tempfile::tempdir().unwrap();
        let source = write_custom_source(temp_dir.path(), 2).await;
        let results_in = temp_dir.path().join("results.ndjson");
        write_safe_outputs(
            temp_dir.path(),
            r#"{"name":"send-notification","context":"first"}
{"name":"send-notification","context":"second"}
"#,
        )
        .await;
        tokio::fs::write(
            &results_in,
            r#"{"schema_version":1,"proposal_id":"send-notification-0","status":"success","message":"ok0","data":{"url":"https://example.com"}}
{"schema_version":1,"proposal_id":"send-notification-1","status":"success","message":"ok1"}
"#,
        )
        .await
        .unwrap();

        let results = execute_custom_safe_outputs(
            &source,
            temp_dir.path(),
            false,
            CustomExecuteOptions {
                custom_phase: Some("post".to_string()),
                tool: Some("send-notification".to_string()),
                results_in: Some(results_in),
                component_source: Some("repo/path".to_string()),
                component_sha: Some("abc123".to_string()),
                manifest_digest: Some("sha256:manifest".to_string()),
                schema_digest: Some("sha256:schema".to_string()),
                ..Default::default()
            },
        )
        .await
        .unwrap();

        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|result| result.success));
        let records = read_executed_records(temp_dir.path()).await;
        assert_eq!(records[0]["component"]["source"], "repo/path");
        assert_eq!(records[0]["component"]["sha"], "abc123");
        assert_eq!(records[0]["status"], "succeeded");
    }

    #[tokio::test]
    async fn test_custom_execute_jobs_post_missing_result_becomes_failure() {
        let temp_dir = tempfile::tempdir().unwrap();
        let source = write_custom_source(temp_dir.path(), 2).await;
        let results_in = temp_dir.path().join("results.ndjson");
        write_safe_outputs(
            temp_dir.path(),
            r#"{"name":"send-notification"}
{"name":"send-notification"}
"#,
        )
        .await;
        tokio::fs::write(
            &results_in,
            r#"{"schema_version":1,"proposal_id":"send-notification-0","status":"success","message":"ok"}"#,
        )
        .await
        .unwrap();

        let results = execute_custom_safe_outputs(
            &source,
            temp_dir.path(),
            false,
            CustomExecuteOptions {
                custom_phase: Some("post".to_string()),
                tool: Some("send-notification".to_string()),
                results_in: Some(results_in),
                ..Default::default()
            },
        )
        .await
        .unwrap();

        assert_eq!(results.len(), 2);
        assert!(!results[1].success);
        assert!(results[1].message.contains("Missing custom result"));
        let records = read_executed_records(temp_dir.path()).await;
        assert_eq!(records[1]["status"], "failed");
    }

    #[tokio::test]
    async fn test_custom_execute_jobs_post_unknown_schema_version_fails_closed() {
        let temp_dir = tempfile::tempdir().unwrap();
        let source = write_custom_source(temp_dir.path(), 1).await;
        let results_in = temp_dir.path().join("results.ndjson");
        write_safe_outputs(temp_dir.path(), r#"{"name":"send-notification"}"#).await;
        tokio::fs::write(
            &results_in,
            r#"{"schema_version":99,"proposal_id":"send-notification-0","status":"success","message":"ok"}"#,
        )
        .await
        .unwrap();

        let result = execute_custom_safe_outputs(
            &source,
            temp_dir.path(),
            false,
            CustomExecuteOptions {
                custom_phase: Some("post".to_string()),
                tool: Some("send-notification".to_string()),
                results_in: Some(results_in),
                ..Default::default()
            },
        )
        .await;

        let err = result.unwrap_err();
        assert!(
            err.to_string()
                .contains("missing or unsupported schema_version: 99"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_custom_execution_record_serializes_name_and_status() {
        let record = CustomExecutionRecord {
            schema_version: 1,
            tool: "send-notification".to_string(),
            proposal_id: "send-notification-0".to_string(),
            proposal_index: 0,
            name: "send-notification".to_string(),
            status: "succeeded".to_string(),
            message: "ok".to_string(),
            data: None,
            component: CustomComponentProvenance {
                source: None,
                sha: None,
                manifest_digest: None,
                schema_digest: None,
            },
            attempt: CustomAttemptMetadata {
                number: 1,
                staged: false,
                started_at: "2026-01-01T00:00:00Z".to_string(),
                ended_at: "2026-01-01T00:00:01Z".to_string(),
            },
            context: None,
            result: None,
            error: None,
            timestamp: "2026-01-01T00:00:01Z".to_string(),
        };

        let value = serde_json::to_value(record).unwrap();
        assert_eq!(value["name"], "send-notification");
        assert_eq!(value["status"], "succeeded");
    }

    #[tokio::test]
    async fn test_execute_no_custom_flags_normal_path_unaffected_smoke() {
        let temp_dir = tempfile::tempdir().unwrap();
        assert!(!CustomExecuteOptions::default().has_any_custom_flag());

        let results = execute_safe_outputs(
            temp_dir.path(),
            &ExecutionContext::default(),
            &ToolFilter::default(),
        )
        .await
        .unwrap();

        assert!(results.is_empty());
    }

    #[test]
    fn test_stdout_print_neutralizes_result_message_pipeline_commands() {
        let message = "Uploaded '##vso[task.setvariable variable=X]y.txt'";
        let safe = neutralize_pipeline_commands(message);
        assert!(!safe.contains("##vso[task"));
        assert!(safe.contains("`##vso[`"));
    }

    #[tokio::test]
    async fn test_execute_unknown_tool_fails() {
        let entry = serde_json::json!({"name": "unknown_tool", "foo": "bar"});
        let ctx = ExecutionContext {
            ado_org_url: None,
            ado_organization: None,
            ado_project: None,
            access_token: None,
            working_directory: PathBuf::from("."),
            source_directory: PathBuf::from("."),
            tool_configs: HashMap::new(),
            repository_id: None,
            repository_name: None,
            allowed_repositories: HashMap::new(),
            repo_refs: HashMap::new(),
            agent_stats: None,
            dry_run: false,
            ..Default::default()
        };

        let result = execute_safe_output(&entry, &ctx).await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Unknown tool type")
        );
    }

    #[tokio::test]
    async fn test_execute_create_work_item_missing_context() {
        let entry = serde_json::json!({
            "name": "create-work-item",
            "title": "Test work item",
            "description": "A description that is definitely longer than thirty characters."
        });

        // Context without required fields
        let ctx = ExecutionContext {
            ado_org_url: None,
            ado_organization: None,
            ado_project: None,
            access_token: None,
            working_directory: PathBuf::from("."),
            source_directory: PathBuf::from("."),
            tool_configs: HashMap::new(),
            repository_id: None,
            repository_name: None,
            allowed_repositories: HashMap::new(),
            repo_refs: HashMap::new(),
            agent_stats: None,
            dry_run: false,
            ..Default::default()
        };

        let result = execute_safe_output(&entry, &ctx).await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("AZURE_DEVOPS_ORG_URL")
        );
    }

    #[tokio::test]
    async fn test_execute_missing_name_fails() {
        let entry = serde_json::json!({"foo": "bar"});
        let ctx = ExecutionContext::default();

        let result = execute_safe_output(&entry, &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("name"));
    }

    #[tokio::test]
    async fn test_execute_noop_succeeds() {
        let entry = serde_json::json!({"name": "noop", "context": "test"});
        let ctx = ExecutionContext::default();

        let result = execute_safe_output(&entry, &ctx).await;
        assert!(result.is_ok());
        let (tool_name, result) = result.unwrap();
        assert_eq!(tool_name, "noop");
        // noop is a pass-through diagnostic signal — work-item filing is now
        // handled by the Conclusion job, so execute_impl returns plain success.
        assert!(result.success);
        assert!(!result.is_warning());
        assert!(
            result.message.contains("No operation needed"),
            "noop should report no-op message, got: {}",
            result.message
        );
    }

    #[tokio::test]
    async fn test_execute_missing_tool_succeeds() {
        let entry = serde_json::json!({"name": "missing-tool", "tool_name": "some_tool"});
        let ctx = ExecutionContext::default();

        let result = execute_safe_output(&entry, &ctx).await;
        assert!(result.is_ok());
        let (tool_name, result) = result.unwrap();
        assert_eq!(tool_name, "missing-tool");
        // missing-tool is a pass-through diagnostic signal — work-item filing
        // is now handled by the Conclusion job, so execute_impl returns plain success.
        assert!(result.success);
        assert!(!result.is_warning());
        assert!(
            result.message.contains("Missing tool reported"),
            "missing-tool should report tool name, got: {}",
            result.message
        );
    }

    #[tokio::test]
    async fn test_execute_safe_outputs_empty_dir() {
        let temp_dir = tempfile::tempdir().unwrap();
        let ctx = ExecutionContext::default();

        let results = execute_safe_outputs(temp_dir.path(), &ctx, &ToolFilter::default())
            .await
            .unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_execute_safe_outputs_from_ndjson() {
        let temp_dir = tempfile::tempdir().unwrap();
        let safe_output_path = temp_dir.path().join(SAFE_OUTPUT_FILENAME);

        // Write test NDJSON
        let ndjson = r#"{"name":"noop","context":"test1"}
{"name":"missing-tool","tool_name":"my_tool"}
"#;
        tokio::fs::write(&safe_output_path, ndjson).await.unwrap();

        let ctx = ExecutionContext::default();
        let results = execute_safe_outputs(temp_dir.path(), &ctx, &ToolFilter::default())
            .await
            .unwrap();

        assert_eq!(results.len(), 2);
        assert!(results[0].success);
        assert!(results[1].success);

        let manifest = read_executed_manifest(&temp_dir).await;
        assert_eq!(manifest.len(), 2);
        assert_eq!(manifest[0]["status"], "succeeded");
        assert_eq!(manifest[0]["context"], "test1");
        assert_eq!(manifest[1]["status"], "succeeded");
    }

    #[tokio::test]
    async fn test_execute_safe_outputs_empty_file_returns_empty() {
        let temp_dir = tempfile::tempdir().unwrap();
        let safe_output_path = temp_dir.path().join(SAFE_OUTPUT_FILENAME);
        tokio::fs::write(&safe_output_path, "").await.unwrap();

        let ctx = ExecutionContext::default();
        let results = execute_safe_outputs(temp_dir.path(), &ctx, &ToolFilter::default())
            .await
            .unwrap();
        assert!(results.is_empty());
    }

    async fn read_executed_manifest(temp_dir: &tempfile::TempDir) -> Vec<Value> {
        ndjson::read_ndjson_file(&temp_dir.path().join(EXECUTED_NDJSON_FILENAME))
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn test_execute_safe_outputs_writes_success_manifest_records() {
        let temp_dir = tempfile::tempdir().unwrap();
        let safe_output_path = temp_dir.path().join(SAFE_OUTPUT_FILENAME);
        let ndjson = r#"{"name":"noop","context":"first noop"}
{"name":"noop","context":"second noop"}
"#;
        tokio::fs::write(&safe_output_path, ndjson).await.unwrap();

        let ctx = ExecutionContext {
            dry_run: true,
            ..Default::default()
        };
        let results = execute_safe_outputs(temp_dir.path(), &ctx, &ToolFilter::default())
            .await
            .unwrap();
        assert_eq!(results.len(), 2);

        let executed_path = temp_dir.path().join(EXECUTED_NDJSON_FILENAME);
        assert!(executed_path.exists(), "executed manifest should exist");

        let manifest = read_executed_manifest(&temp_dir).await;
        assert_eq!(manifest.len(), 2);
        assert_eq!(manifest[0]["name"], "noop");
        assert_eq!(manifest[0]["status"], "succeeded");
        assert_eq!(manifest[0]["context"], "first noop");
        assert!(manifest[0]["error"].is_null());
        assert_eq!(manifest[1]["name"], "noop");
        assert_eq!(manifest[1]["status"], "succeeded");
        assert_eq!(manifest[1]["context"], "second noop");
        assert!(manifest[1]["error"].is_null());
    }

    #[tokio::test]
    async fn test_execute_safe_outputs_writes_mixed_success_failure_manifest_records() {
        let temp_dir = tempfile::tempdir().unwrap();
        let safe_output_path = temp_dir.path().join(SAFE_OUTPUT_FILENAME);
        let ndjson = r#"{"name":"noop","context":"ok"}
{"name":"unknown_tool","context":"bad"}
"#;
        tokio::fs::write(&safe_output_path, ndjson).await.unwrap();

        let ctx = ExecutionContext {
            dry_run: true,
            ..Default::default()
        };
        let results = execute_safe_outputs(temp_dir.path(), &ctx, &ToolFilter::default())
            .await
            .unwrap();
        assert_eq!(results.len(), 2);

        let manifest = read_executed_manifest(&temp_dir).await;
        assert_eq!(manifest.len(), 2);
        assert_eq!(manifest[0]["name"], "noop");
        assert_eq!(manifest[0]["status"], "succeeded");
        assert_eq!(manifest[1]["name"], "unknown_tool");
        assert_eq!(manifest[1]["status"], "failed");
        assert_eq!(manifest[1]["context"], "bad");
        assert!(manifest[1]["result"].is_null());
        assert!(manifest[1]["error"].is_string());
    }

    #[tokio::test]
    async fn test_execute_safe_outputs_empty_input_does_not_create_manifest() {
        let temp_dir = tempfile::tempdir().unwrap();
        let safe_output_path = temp_dir.path().join(SAFE_OUTPUT_FILENAME);
        tokio::fs::write(&safe_output_path, "").await.unwrap();

        let ctx = ExecutionContext::default();
        let results = execute_safe_outputs(temp_dir.path(), &ctx, &ToolFilter::default())
            .await
            .unwrap();
        assert!(results.is_empty());
        assert!(!temp_dir.path().join(EXECUTED_NDJSON_FILENAME).exists());
    }

    #[tokio::test]
    async fn test_execute_missing_data_succeeds() {
        let entry = serde_json::json!({"name": "missing-data", "data_type": "schema", "reason": "not available"});
        let ctx = ExecutionContext::default();

        let result = execute_safe_output(&entry, &ctx).await;
        assert!(result.is_ok());
        let (tool_name, result) = result.unwrap();
        assert_eq!(tool_name, "missing-data");
        assert!(result.success);
    }

    #[tokio::test]
    async fn test_execute_safe_output_malformed_work_item_returns_err() {
        // Missing required fields (title and description)
        let entry = serde_json::json!({"name": "create-work-item"});
        let ctx = ExecutionContext::default();

        let result = execute_safe_output(&entry, &ctx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_execute_unknown_tool_error_contains_tool_name() {
        let entry = serde_json::json!({"name": "evil-backdoor"});
        let ctx = ExecutionContext::default();

        let result = execute_safe_output(&entry, &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("evil-backdoor"));
    }

    #[tokio::test]
    async fn test_execute_malformed_update_wiki_page_returns_err() {
        // Missing required fields (path and content)
        let entry = serde_json::json!({"name": "update-wiki-page"});
        let ctx = ExecutionContext::default();

        let result = execute_safe_output(&entry, &ctx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_execute_update_wiki_page_missing_context() {
        let entry = serde_json::json!({
            "name": "update-wiki-page",
            "path": "/Overview",
            "content": "This is some valid wiki content."
        });

        // Context without required fields (ado_org_url, etc.)
        let ctx = ExecutionContext {
            ado_org_url: None,
            ado_organization: None,
            ado_project: None,
            access_token: None,
            working_directory: PathBuf::from("."),
            source_directory: PathBuf::from("."),
            tool_configs: HashMap::new(),
            repository_id: None,
            repository_name: None,
            allowed_repositories: HashMap::new(),
            repo_refs: HashMap::new(),
            agent_stats: None,
            dry_run: false,
            ..Default::default()
        };

        let result = execute_safe_output(&entry, &ctx).await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("AZURE_DEVOPS_ORG_URL")
        );
    }

    #[tokio::test]
    async fn test_execute_malformed_create_wiki_page_returns_err() {
        // Missing required fields (path and content)
        let entry = serde_json::json!({"name": "create-wiki-page"});
        let ctx = ExecutionContext::default();

        let result = execute_safe_output(&entry, &ctx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_execute_malformed_upload_pipeline_artifact_returns_err() {
        // Missing required fields (artifact_name and file_path)
        let entry = serde_json::json!({"name": "upload-pipeline-artifact"});
        let ctx = ExecutionContext::default();

        let result = execute_safe_output(&entry, &ctx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_execute_create_wiki_page_missing_context() {
        let entry = serde_json::json!({
            "name": "create-wiki-page",
            "path": "/NewPage",
            "content": "This is some valid wiki content."
        });

        // Context without required fields (ado_org_url, etc.)
        let ctx = ExecutionContext {
            ado_org_url: None,
            ado_organization: None,
            ado_project: None,
            access_token: None,
            working_directory: PathBuf::from("."),
            source_directory: PathBuf::from("."),
            tool_configs: HashMap::new(),
            repository_id: None,
            repository_name: None,
            allowed_repositories: HashMap::new(),
            repo_refs: HashMap::new(),
            agent_stats: None,
            dry_run: false,
            ..Default::default()
        };

        let result = execute_safe_output(&entry, &ctx).await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("AZURE_DEVOPS_ORG_URL")
        );
    }

    #[tokio::test]
    async fn test_execute_malformed_comment_on_work_item_returns_err() {
        // Missing required fields (work_item_id and body)
        let entry = serde_json::json!({"name": "comment-on-work-item"});
        let ctx = ExecutionContext::default();

        let result = execute_safe_output(&entry, &ctx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_execute_malformed_upload_workitem_attachment_returns_err() {
        // Missing required fields (work_item_id, file_path)
        let entry = serde_json::json!({"name": "upload-workitem-attachment"});
        let ctx = ExecutionContext::default();

        let result = execute_safe_output(&entry, &ctx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_execute_upload_workitem_attachment_missing_context() {
        let entry = serde_json::json!({
            "name": "upload-workitem-attachment",
            "work_item_id": 12345,
            "file_path": "report.log"
        });

        // Context without required fields (ado_org_url, etc.)
        let ctx = ExecutionContext {
            ado_org_url: None,
            ado_organization: None,
            ado_project: None,
            access_token: None,
            working_directory: PathBuf::from("."),
            source_directory: PathBuf::from("."),
            tool_configs: HashMap::new(),
            repository_id: None,
            repository_name: None,
            allowed_repositories: HashMap::new(),
            repo_refs: HashMap::new(),
            agent_stats: None,
            dry_run: false,
            ..Default::default()
        };

        let result = execute_safe_output(&entry, &ctx).await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("AZURE_DEVOPS_ORG_URL")
        );
    }

    #[tokio::test]
    async fn test_execute_malformed_upload_build_attachment_returns_err() {
        // Missing required fields (artifact_name, file_path, staged_file, etc.)
        let entry = serde_json::json!({"name": "upload-build-attachment"});
        let ctx = ExecutionContext::default();

        let result = execute_safe_output(&entry, &ctx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_execute_upload_build_attachment_missing_context() {
        let entry = serde_json::json!({
            "name": "upload-build-attachment",
            "artifact_name": "my-artifact",
            "file_path": "staged_file.txt",
            "staged_file": "staged_file.txt",
            "file_size": 5_u64,
            "staged_sha256": "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        });

        // Context without required fields (ado_org_url, etc.)
        let ctx = ExecutionContext {
            ado_org_url: None,
            ado_organization: None,
            ado_project: None,
            access_token: None,
            working_directory: PathBuf::from("."),
            source_directory: PathBuf::from("."),
            tool_configs: HashMap::new(),
            repository_id: None,
            repository_name: None,
            allowed_repositories: HashMap::new(),
            repo_refs: HashMap::new(),
            agent_stats: None,
            dry_run: false,
            ..Default::default()
        };

        let result = execute_safe_output(&entry, &ctx).await;
        // Missing BUILD_BUILDID / ADO context must not succeed. Depending on
        // whether the test host has BUILD_BUILDID set, this surfaces either as
        // a hard error (missing staged file / ADO context) or as a clean
        // unsuccessful ExecutionResult (current run undeterminable).
        if let Ok(r) = result {
            assert!(!r.1.success, "expected non-success, got: {:?}", r);
        }
    }

    #[tokio::test]
    async fn test_execute_comment_on_work_item_missing_context() {
        let entry = serde_json::json!({
            "name": "comment-on-work-item",
            "work_item_id": 12345,
            "body": "This is a comment on the work item."
        });

        // Context without required fields (ado_org_url, etc.)
        let ctx = ExecutionContext {
            ado_org_url: None,
            ado_organization: None,
            ado_project: None,
            access_token: None,
            working_directory: PathBuf::from("."),
            source_directory: PathBuf::from("."),
            tool_configs: HashMap::new(),
            repository_id: None,
            repository_name: None,
            allowed_repositories: HashMap::new(),
            repo_refs: HashMap::new(),
            agent_stats: None,
            dry_run: false,
            ..Default::default()
        };

        let result = execute_safe_output(&entry, &ctx).await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("AZURE_DEVOPS_ORG_URL")
        );
    }

    /// Excess update-work-item entries beyond `max` are skipped (failure result added) rather than
    /// aborting the entire batch. Other tool entries must still execute.
    #[tokio::test]
    async fn test_execute_update_work_item_max_skips_excess_not_abort() {
        let temp_dir = tempfile::tempdir().unwrap();
        let safe_output_path = temp_dir.path().join(SAFE_OUTPUT_FILENAME);

        // Write 3 update-work-item entries + 1 noop; max defaults to 1
        let ndjson = r#"{"name":"update-work-item","id":1,"title":"First update"}
{"name":"update-work-item","id":2,"title":"Second update"}
{"name":"update-work-item","id":3,"title":"Third update"}
{"name":"noop","context":"still runs"}
"#;
        tokio::fs::write(&safe_output_path, ndjson).await.unwrap();

        // Config: update-work-item with max=1 (default), title=true so the field check passes
        let update_cfg = serde_json::json!({
            "title": true,
            "max": 1,
            "target": "*"
        });
        let mut tool_configs = HashMap::new();
        tool_configs.insert("update-work-item".to_string(), update_cfg);

        let ctx = ExecutionContext {
            ado_org_url: Some("https://dev.azure.com/org".to_string()),
            ado_organization: Some("org".to_string()),
            ado_project: Some("Proj".to_string()),
            access_token: Some("token".to_string()),
            working_directory: PathBuf::from("."),
            source_directory: PathBuf::from("."),
            tool_configs,
            repository_id: None,
            repository_name: None,
            allowed_repositories: HashMap::new(),
            repo_refs: HashMap::new(),
            agent_stats: None,
            dry_run: false,
            ..Default::default()
        };

        let results = execute_safe_outputs(temp_dir.path(), &ctx, &ToolFilter::default()).await;
        // The batch must NOT abort — execute_safe_outputs should return Ok
        assert!(
            results.is_ok(),
            "Batch should not abort when max is exceeded; got: {:?}",
            results
        );
        let results = results.unwrap();
        // 4 entries total: 3 update-work-item + 1 noop
        assert_eq!(results.len(), 4, "Expected 4 results (3 uwi + 1 noop)");

        // The first update-work-item fails with HTTP error (no real ADO) but was attempted
        // The 2nd and 3rd are skipped due to max
        let skipped: Vec<_> = results
            .iter()
            .filter(|r| r.message.contains("maximum update-work-item count"))
            .collect();
        assert_eq!(
            skipped.len(),
            2,
            "Expected 2 skipped entries, got: {:?}",
            skipped
        );

        // The noop still executes successfully
        let noop_result = &results[3];
        assert!(
            noop_result.success,
            "noop should still succeed even when prior entries are skipped"
        );
    }

    // --- check_budget unit tests ---

    #[test]
    fn test_check_budget_returns_none_when_under_limit() {
        let result = check_budget(5, 0, "update-work-item", "", 0, 3);
        assert!(result.is_none());
    }

    #[test]
    fn test_check_budget_returns_none_at_exactly_one_below_limit() {
        let result = check_budget(5, 1, "update-work-item", "", 2, 3);
        assert!(result.is_none());
    }

    #[test]
    fn test_check_budget_returns_failure_when_at_limit() {
        let result = check_budget(5, 2, "update-work-item", "", 3, 3);
        assert!(result.is_some());
        let r = result.unwrap();
        assert!(!r.success);
        assert!(r.message.contains("maximum update-work-item count (3)"));
        assert!(r.message.contains("safe-outputs.update-work-item"));
    }

    #[test]
    fn test_check_budget_returns_failure_when_over_limit() {
        let result = check_budget(5, 3, "comment-on-work-item", " (work item #99)", 5, 2);
        assert!(result.is_some());
        let r = result.unwrap();
        assert!(!r.success);
        assert!(r.message.contains("(work item #99)"));
        assert!(r.message.contains("maximum comment-on-work-item count (2)"));
    }

    #[test]
    fn test_check_budget_zero_max_always_skips() {
        let result = check_budget(3, 0, "update-work-item", "", 0, 0);
        assert!(result.is_some());
        let r = result.unwrap();
        assert!(!r.success);
        assert!(r.message.contains("maximum update-work-item count (0)"));
    }

    #[test]
    fn test_check_budget_wi_id_included_in_message() {
        let result = check_budget(4, 1, "update-work-item", " (work item #42)", 1, 1);
        assert!(result.is_some());
        let r = result.unwrap();
        assert!(r.message.contains("(work item #42)"));
    }

    // --- extract_entry_context unit tests ---

    #[test]
    fn test_extract_entry_context_with_id() {
        let entry = serde_json::json!({"name": "update-work-item", "id": 42});
        assert_eq!(extract_entry_context(&entry), " (work item #42)");
    }

    #[test]
    fn test_extract_entry_context_with_work_item_id() {
        let entry = serde_json::json!({"name": "comment-on-work-item", "work_item_id": 99});
        assert_eq!(extract_entry_context(&entry), " (work item #99)");
    }

    #[test]
    fn test_extract_entry_context_with_title() {
        let entry = serde_json::json!({"name": "create-work-item", "title": "Fix the bug"});
        assert_eq!(extract_entry_context(&entry), " (\"Fix the bug\")");
    }

    #[test]
    fn test_extract_entry_context_with_path() {
        let entry = serde_json::json!({"name": "create-wiki-page", "path": "/Overview/NewPage"});
        assert_eq!(extract_entry_context(&entry), " (path: /Overview/NewPage)");
    }

    #[test]
    fn test_extract_entry_context_truncates_long_title_utf8_safe() {
        // 41 emoji characters — each is 4 bytes, so naive &title[..40] would panic
        let title = "🔥".repeat(41);
        let entry = serde_json::json!({"name": "create-work-item", "title": title});
        let ctx = extract_entry_context(&entry);
        assert!(ctx.starts_with(" (\""));
        assert!(ctx.ends_with("\")"));
        // Should contain exactly 40 emoji chars (not panic)
        let inner = &ctx[3..ctx.len() - 2];
        assert_eq!(inner.chars().count(), 40);
    }

    #[test]
    fn test_extract_entry_context_empty() {
        let entry = serde_json::json!({"name": "noop"});
        assert_eq!(extract_entry_context(&entry), "");
    }

    #[test]
    fn test_extract_entry_context_strips_control_chars() {
        let entry = serde_json::json!({"name": "create-work-item", "title": "Good\ntitle\r\nhere"});
        assert_eq!(extract_entry_context(&entry), " (\"Goodtitlehere\")");
    }

    #[test]
    fn test_extract_entry_context_strips_control_chars_from_path() {
        let entry = serde_json::json!({"name": "create-wiki-page", "path": "/Page\n/Injected"});
        assert_eq!(extract_entry_context(&entry), " (path: /Page/Injected)");
    }

    #[test]
    fn test_extract_entry_context_neutralizes_shorthand_pipeline_command_in_title() {
        let entry = serde_json::json!({
            "title": "##[error]Build failed – exfiltrate secrets"
        });
        let ctx = extract_entry_context(&entry);
        assert!(
            !ctx.contains("##[error]"),
            "##[ shorthand in title should be neutralized; got: {ctx}"
        );
        assert!(
            ctx.contains("`##[`"),
            "##[ shorthand should be wrapped in backticks; got: {ctx}"
        );
    }

    #[test]
    fn test_extract_entry_context_neutralizes_shorthand_pipeline_command_in_path() {
        let entry = serde_json::json!({
            "path": "##[section]My Section"
        });
        let ctx = extract_entry_context(&entry);
        assert!(
            !ctx.contains("##[section]"),
            "##[ shorthand in path should be neutralized; got: {ctx}"
        );
        assert!(
            ctx.contains("`##[`"),
            "##[ shorthand should be wrapped in backticks; got: {ctx}"
        );
    }

    #[tokio::test]
    async fn test_execute_safe_outputs_unknown_tool_with_vso_in_name_does_not_echo_raw_command() {
        let temp_dir = tempfile::tempdir().unwrap();
        let safe_output_path = temp_dir.path().join(SAFE_OUTPUT_FILENAME);

        // Simulate an adversarial NDJSON entry where the agent injects a VSO pipeline command
        // into the 'name' field, trying to get it echoed to stdout by Stage 3.
        let ndjson = "{\"name\":\"##vso[task.setvariable variable=PAT;issecret=true]stolen\"}\n";
        tokio::fs::write(&safe_output_path, ndjson).await.unwrap();

        let ctx = ExecutionContext::default();
        let results = execute_safe_outputs(temp_dir.path(), &ctx, &ToolFilter::default())
            .await
            .unwrap();

        // One entry processed (as a failure — unknown tool)
        assert_eq!(results.len(), 1);
        assert!(!results[0].success);

        // The raw ##vso[task... pattern must not appear — neutralization breaks it at ##vso[
        // so "##vso[task" cannot appear (it becomes "`##vso[`task").
        assert!(
            !results[0].message.contains("##vso[task"),
            "Raw VSO pipeline command must not appear in Stage 3 output; got: {}",
            results[0].message
        );
        // Confirm the neutralized (backtick-wrapped) form is present.
        assert!(
            results[0].message.contains("`##vso[`"),
            "VSO command should be neutralized (wrapped in backticks); got: {}",
            results[0].message
        );
    }

    // --- resolve_max and DEFAULT_MAX unit tests ---

    #[test]
    fn test_default_max_trait_constant() {
        assert_eq!(CreateWorkItemResult::DEFAULT_MAX, 1);
        assert_eq!(CreatePrResult::DEFAULT_MAX, 1);
        assert_eq!(UpdateWorkItemResult::DEFAULT_MAX, 1);
        assert_eq!(CommentOnWorkItemResult::DEFAULT_MAX, 1);
        assert_eq!(CreateWikiPageResult::DEFAULT_MAX, 1);
        assert_eq!(UpdateWikiPageResult::DEFAULT_MAX, 1);
    }

    #[test]
    fn test_resolve_max_uses_config_override() {
        let mut tool_configs = HashMap::new();
        tool_configs.insert("test-tool".to_string(), serde_json::json!({"max": 5}));
        let ctx = ExecutionContext {
            tool_configs,
            dry_run: false,
            ..ExecutionContext::default()
        };
        assert_eq!(resolve_max(&ctx, "test-tool", 1), 5);
    }

    #[test]
    fn test_resolve_max_falls_back_to_default() {
        let ctx = ExecutionContext::default();
        assert_eq!(resolve_max(&ctx, "nonexistent-tool", 3), 3);
    }

    #[test]
    fn test_resolve_max_uses_default_when_no_max_in_config() {
        let mut tool_configs = HashMap::new();
        tool_configs.insert("test-tool".to_string(), serde_json::json!({"other": true}));
        let ctx = ExecutionContext {
            tool_configs,
            dry_run: false,
            ..ExecutionContext::default()
        };
        assert_eq!(resolve_max(&ctx, "test-tool", 7), 7);
    }

    // --- Generic budget enforcement for all tool types ---

    #[tokio::test]
    async fn test_budget_enforcement_create_work_item_max() {
        let temp_dir = tempfile::tempdir().unwrap();
        let safe_output_path = temp_dir.path().join(SAFE_OUTPUT_FILENAME);

        // Write 3 create-work-item entries + 1 noop; max set to 2
        let ndjson = r#"{"name":"create-work-item","title":"First item","description":"A description that is definitely longer than thirty characters."}
{"name":"create-work-item","title":"Second item","description":"A description that is definitely longer than thirty characters."}
{"name":"create-work-item","title":"Third item","description":"A description that is definitely longer than thirty characters."}
{"name":"noop","context":"still runs"}
"#;
        tokio::fs::write(&safe_output_path, ndjson).await.unwrap();

        let mut tool_configs = HashMap::new();
        tool_configs.insert(
            "create-work-item".to_string(),
            serde_json::json!({"max": 2}),
        );

        let ctx = ExecutionContext {
            ado_org_url: Some("https://dev.azure.com/org".to_string()),
            ado_organization: Some("org".to_string()),
            ado_project: Some("Proj".to_string()),
            access_token: Some("token".to_string()),
            working_directory: PathBuf::from("."),
            source_directory: PathBuf::from("."),
            tool_configs,
            repository_id: None,
            repository_name: None,
            allowed_repositories: HashMap::new(),
            repo_refs: HashMap::new(),
            agent_stats: None,
            dry_run: false,
            ..Default::default()
        };

        let results = execute_safe_outputs(temp_dir.path(), &ctx, &ToolFilter::default()).await;
        assert!(
            results.is_ok(),
            "Batch should not abort when max is exceeded"
        );
        let results = results.unwrap();
        assert_eq!(results.len(), 4, "Expected 4 results");

        // Only 1 should be skipped (max=2 allows first 2, third is skipped)
        let skipped: Vec<_> = results
            .iter()
            .filter(|r| r.message.contains("maximum create-work-item count"))
            .collect();
        assert_eq!(
            skipped.len(),
            1,
            "Expected 1 skipped entry, got: {:?}",
            skipped
        );

        // noop still runs
        assert!(results[3].success, "noop should still succeed");

        let manifest = read_executed_manifest(&temp_dir).await;
        assert_eq!(manifest.len(), 4, "Expected 4 execution records");
        assert_eq!(
            manifest
                .iter()
                .filter(|entry| entry["status"] == "budget_exhausted")
                .count(),
            1,
            "Expected 1 budget_exhausted record"
        );
    }

    #[tokio::test]
    async fn test_budget_enforcement_mixed_tools_independent_budgets() {
        let temp_dir = tempfile::tempdir().unwrap();
        let safe_output_path = temp_dir.path().join(SAFE_OUTPUT_FILENAME);

        // Mix of tools: each has max=1 (default), so only the first of each type should pass budget
        let ndjson = r#"{"name":"create-work-item","title":"WI 1","description":"A description that is definitely longer than thirty characters."}
{"name":"create-work-item","title":"WI 2","description":"A description that is definitely longer than thirty characters."}
{"name":"create-wiki-page","path":"/Page1","content":"Some valid wiki content here."}
{"name":"create-wiki-page","path":"/Page2","content":"Some valid wiki content here."}
{"name":"noop","context":"always runs"}
"#;
        tokio::fs::write(&safe_output_path, ndjson).await.unwrap();

        let ctx = ExecutionContext {
            ado_org_url: Some("https://dev.azure.com/org".to_string()),
            ado_organization: Some("org".to_string()),
            ado_project: Some("Proj".to_string()),
            access_token: Some("token".to_string()),
            working_directory: PathBuf::from("."),
            source_directory: PathBuf::from("."),
            tool_configs: HashMap::new(), // defaults: max=1 for all
            repository_id: None,
            repository_name: None,
            allowed_repositories: HashMap::new(),
            repo_refs: HashMap::new(),
            agent_stats: None,
            dry_run: false,
            ..Default::default()
        };

        let results = execute_safe_outputs(temp_dir.path(), &ctx, &ToolFilter::default())
            .await
            .unwrap();
        assert_eq!(results.len(), 5);

        // Second create-work-item should be skipped
        let cwi_skipped: Vec<_> = results
            .iter()
            .filter(|r| r.message.contains("maximum create-work-item count"))
            .collect();
        assert_eq!(cwi_skipped.len(), 1, "Expected 1 skipped create-work-item");

        // Second create-wiki-page should be skipped
        let cwp_skipped: Vec<_> = results
            .iter()
            .filter(|r| r.message.contains("maximum create-wiki-page count"))
            .collect();
        assert_eq!(cwp_skipped.len(), 1, "Expected 1 skipped create-wiki-page");

        // noop always runs
        assert!(results[4].success, "noop should still succeed");
    }

    // ─── dry-run tests ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_dry_run_create_work_item_succeeds() {
        let temp_dir = tempfile::tempdir().unwrap();
        let safe_output_path = temp_dir.path().join(SAFE_OUTPUT_FILENAME);

        let ndjson = r#"{"name":"create-work-item","title":"Test work item title","description":"This is a test description that is long enough to pass validation checks"}"#;
        tokio::fs::write(&safe_output_path, ndjson).await.unwrap();

        let ctx = ExecutionContext {
            dry_run: true,
            ..Default::default()
        };

        let results = execute_safe_outputs(temp_dir.path(), &ctx, &ToolFilter::default())
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].success, "dry-run should succeed");
        assert!(
            results[0].message.contains("[DRY-RUN]"),
            "message should contain [DRY-RUN], got: {}",
            results[0].message
        );
        assert!(
            results[0].message.contains("create work item"),
            "message should contain tool summary, got: {}",
            results[0].message
        );
    }

    #[tokio::test]
    async fn test_dry_run_multiple_tools() {
        let temp_dir = tempfile::tempdir().unwrap();
        let safe_output_path = temp_dir.path().join(SAFE_OUTPUT_FILENAME);

        let ndjson = [
            r#"{"name":"create-work-item","title":"Test work item title","description":"This is a test description that is long enough to pass validation checks"}"#,
            r#"{"name":"noop","context":"nothing to do"}"#,
        ]
        .join("\n");
        tokio::fs::write(&safe_output_path, ndjson).await.unwrap();

        let ctx = ExecutionContext {
            dry_run: true,
            ..Default::default()
        };

        let results = execute_safe_outputs(temp_dir.path(), &ctx, &ToolFilter::default())
            .await
            .unwrap();
        assert_eq!(results.len(), 2);
        // create-work-item goes through Executor trait → dry-run intercepted
        assert!(results[0].message.contains("[DRY-RUN]"));
        // noop now also goes through Executor trait → dry-run intercepted
        assert!(results[1].success);
        assert!(results[1].message.contains("[DRY-RUN]"));
    }

    #[tokio::test]
    async fn test_dry_run_default_is_false() {
        let ctx = ExecutionContext::default();
        assert!(!ctx.dry_run, "dry_run should default to false");
    }

    #[tokio::test]
    async fn test_non_dry_run_still_fails_without_ado_config() {
        // Verify dry_run=false (default) still behaves normally: missing ADO config causes error
        let entry = serde_json::json!({
            "name": "create-work-item",
            "title": "Test work item",
            "description": "A description that is definitely longer than thirty characters."
        });

        let ctx = ExecutionContext {
            ado_org_url: None,
            ado_organization: None,
            ado_project: None,
            access_token: None,
            working_directory: PathBuf::from("."),
            source_directory: PathBuf::from("."),
            tool_configs: HashMap::new(),
            repository_id: None,
            repository_name: None,
            allowed_repositories: HashMap::new(),
            repo_refs: HashMap::new(),
            agent_stats: None,
            dry_run: false,
            ..Default::default()
        };

        let result = execute_safe_output(&entry, &ctx).await;
        assert!(
            result.is_err(),
            "should fail without ADO config when not in dry-run mode"
        );
    }

    #[tokio::test]
    async fn test_dry_run_succeeds_without_ado_config() {
        // With dry_run=true, missing ADO config should NOT cause failure.
        // Input validation (title length, etc.) still runs — only ADO API calls are skipped.
        let entry = serde_json::json!({
            "name": "create-work-item",
            "title": "Test work item",
            "description": "A description that is definitely longer than thirty characters."
        });

        let ctx = ExecutionContext {
            ado_org_url: None,
            ado_organization: None,
            ado_project: None,
            access_token: None,
            working_directory: PathBuf::from("."),
            source_directory: PathBuf::from("."),
            tool_configs: HashMap::new(),
            repository_id: None,
            repository_name: None,
            allowed_repositories: HashMap::new(),
            repo_refs: HashMap::new(),
            agent_stats: None,
            dry_run: true,
            ..Default::default()
        };

        let result = execute_safe_output(&entry, &ctx).await;
        assert!(result.is_ok(), "dry-run should succeed without ADO config");
        let (tool_name, exec_result) = result.unwrap();
        assert_eq!(tool_name, "create-work-item");
        assert!(exec_result.success);
        assert!(exec_result.message.contains("[DRY-RUN]"));
    }

    #[tokio::test]
    async fn test_dry_run_report_incomplete_still_fails() {
        // report-incomplete uses an Executor override that still returns
        // ExecutionResult::failure even in dry-run mode.
        // This is correct: the agent declared it couldn't complete the task.
        let entry = serde_json::json!({
            "name": "report-incomplete",
            "reason": "Could not find the required data to complete the analysis"
        });

        let ctx = ExecutionContext {
            dry_run: true,
            ..Default::default()
        };

        let result = execute_safe_output(&entry, &ctx).await;
        assert!(result.is_ok(), "dispatch should succeed");
        let (tool_name, exec_result) = result.unwrap();
        assert_eq!(tool_name, "report-incomplete");
        assert!(
            !exec_result.success,
            "report-incomplete should still be a failure in dry-run mode"
        );
        assert!(
            exec_result.message.contains("incomplete"),
            "message should mention incomplete, got: {}",
            exec_result.message
        );
    }
}
