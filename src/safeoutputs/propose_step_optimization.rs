//! `propose-step-optimization` safe-output tool — opt-in Flow B
//! surface for runtime self-optimization.
//!
//! When the agent's front matter sets `self-optimization.enabled:
//! true` (see [`crate::compile::types::SelfOptimizationConfig`]), the
//! Stage-1 agent gets access to this tool. The agent uses it to
//! propose lifting **deterministic** bash work it ran successfully —
//! clone, install, cache restore, artifact download — out of its
//! prompt body and into the front-matter `steps:` / `post-steps:`
//! (and, when explicitly opted in, `setup:` / `teardown:`) sections.
//!
//! Stage 2 (threat analysis) cross-checks `source_command_evidence`
//! against the agent's actual command history (recorded via
//! [`crate::audit::analyzers::mcp`]); any bash in the proposed steps
//! block that the agent didn't demonstrably execute is a strong
//! prompt-injection signal.
//!
//! Stage 3 (the executor) IR-validates the proposed steps block via
//! [`crate::compile::ir::validate_step_block`] with
//! [`crate::compile::ir::StepKindAllow::Curated`] — only Bash steps
//! and the typed-factory tasks (see
//! [`crate::compile::ir::tasks::CURATED_TASK_IDS`]) are accepted —
//! and then either renders a `🎭`-marked diff preview to the build
//! summary (`staged: true`, the default) or opens a PR against the
//! source `.md` adding the new step entries (`staged: false`). The
//! Stage 3 staged-preview and live-PR paths land in subsequent
//! commits; this commit ships the Stage-1 surface plus a
//! placeholder Stage-3 executor that records the proposal for audit
//! but does not yet apply it.
//!
//! Tool registration is gated by
//! [`crate::safeoutputs::OPT_IN_GATED_TOOLS`]: the MCP layer strips
//! the route unless the compiler explicitly enables it via
//! `--enabled-tools propose-step-optimization`, which only happens
//! when `self-optimization.enabled: true` is in the front matter.

use anyhow::Context as _;
use log::{debug, info, warn};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::safeoutputs::{ExecutionContext, ExecutionResult, Executor, Validate};
use crate::sanitize::{SanitizeContent, sanitize_config};
use crate::tool_result;
use anyhow::ensure;

// ── Wire-format Section enum ────────────────────────────────────────────

/// Front-matter section a proposal targets.
///
/// Mirrors [`crate::compile::types::StepSection`] (kebab-case wire
/// format). Defined locally so the safe-output's Stage-1 surface
/// doesn't drag schemars into `compile::types`. Stage 3 compares the
/// agent's claimed section against the front-matter's
/// `allowed_sections` directly from the deserialized string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ProposalSection {
    /// `steps:` — runs BEFORE the agent inside the agent job.
    Steps,
    /// `post-steps:` — runs AFTER the agent inside the agent job.
    PostSteps,
    /// `setup:` — separate job that runs before the agent job.
    Setup,
    /// `teardown:` — separate job that runs after safe-outputs.
    Teardown,
}

impl ProposalSection {
    /// Stable, kebab-case wire string for the section. Used by
    /// Stage 3 to look up the matching front-matter
    /// `allowed_sections` entry without depending on
    /// `compile::types::StepSection` Display semantics.
    pub fn as_wire_str(self) -> &'static str {
        match self {
            ProposalSection::Steps => "steps",
            ProposalSection::PostSteps => "post-steps",
            ProposalSection::Setup => "setup",
            ProposalSection::Teardown => "teardown",
        }
    }
}

// ── Stage 1: Params (agent-provided) ──────────────────────────────────────

/// Parameters the agent supplies when calling `propose-step-optimization`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ProposeStepOptimizationParams {
    /// Which front-matter section to propose into. Stage 3 rejects
    /// proposals that target a section not listed under
    /// `self-optimization.allowed-sections` in the front matter
    /// (default: `[steps, post-steps]` — opt-in required for
    /// `setup` / `teardown`).
    pub section: ProposalSection,
    /// Short, plain-prose explanation of why this hoist saves time
    /// or tokens. Sanitised at Stage 3; surfaced in the build
    /// summary preview and the live-mode PR body. Max 2 KB.
    pub rationale: String,
    /// Optional estimated token savings per build. Informational
    /// only; surfaced in the preview/PR body. Stage 3 does not
    /// trust this number — it's a hint the author can use to
    /// prioritise reviewing proposals.
    pub estimated_token_savings: Option<u64>,
    /// The proposed step block as a JSON array of step entries
    /// (one entry per step). Stage 3 IR-validates this with
    /// `StepKindAllow::Curated` — only Bash steps and the curated
    /// task allow-list (`CURATED_TASK_IDS`) are accepted.
    pub steps: serde_json::Value,
    /// Bash commands the agent actually executed during this run.
    /// Used by Stage 2 detection as the cross-check signal: any
    /// bash content in `steps` that does not have a matching entry
    /// in this list indicates the proposal is not grounded in the
    /// agent's observed behaviour and is treated as a
    /// prompt-injection candidate.
    pub source_command_evidence: Vec<String>,
}

/// Cap on the rationale string. Keeps the PR body / build-summary
/// preview compact and bounds the sanitiser's work.
const MAX_RATIONALE_BYTES: usize = 2_048;

/// Cap on the number of evidence entries the agent may submit per
/// proposal. The agent's full command history is already preserved
/// in the audit artefacts; this list is a cross-check on the
/// proposal, not a re-emission of every command.
const MAX_EVIDENCE_ENTRIES: usize = 64;

/// Cap on the size of an individual evidence entry. Mirrors the
/// `MAX_BASH_BODY_BYTES` cap inside the step-block validator so a
/// matching `steps` entry cannot exceed it.
const MAX_EVIDENCE_ENTRY_BYTES: usize = 10_000;

impl Validate for ProposeStepOptimizationParams {
    fn validate(&self) -> anyhow::Result<()> {
        ensure!(!self.rationale.trim().is_empty(), "rationale must not be empty");
        ensure!(
            self.rationale.len() <= MAX_RATIONALE_BYTES,
            "rationale must be at most {MAX_RATIONALE_BYTES} bytes (got {})",
            self.rationale.len()
        );
        // `steps` must be a JSON array; the deeper structural
        // validation runs in Stage 3 via the IR validator.
        ensure!(
            self.steps.is_array(),
            "steps must be a JSON array of ADO step entries"
        );
        ensure!(
            !self
                .steps
                .as_array()
                .expect("checked array above")
                .is_empty(),
            "steps must contain at least one entry"
        );
        ensure!(
            self.source_command_evidence.len() <= MAX_EVIDENCE_ENTRIES,
            "source_command_evidence must contain at most {MAX_EVIDENCE_ENTRIES} entries (got {})",
            self.source_command_evidence.len()
        );
        for (i, entry) in self.source_command_evidence.iter().enumerate() {
            ensure!(
                entry.len() <= MAX_EVIDENCE_ENTRY_BYTES,
                "source_command_evidence[{i}] exceeds {MAX_EVIDENCE_ENTRY_BYTES} bytes"
            );
        }
        Ok(())
    }
}

// ── Stage 1: Result (generated by macro) ──────────────────────────────────

tool_result! {
    name = "propose-step-optimization",
    write = true,
    params = ProposeStepOptimizationParams,
    /// Result of proposing a step-block optimisation. Stage 2
    /// cross-checks; Stage 3 IR-validates the steps block and
    /// either previews (staged) or opens a PR against the source
    /// `.md` (live).
    pub struct ProposeStepOptimizationResult {
        section: ProposalSection,
        rationale: String,
        estimated_token_savings: Option<u64>,
        steps: serde_json::Value,
        source_command_evidence: Vec<String>,
    }
}

// ── Stage 3: Sanitisation ────────────────────────────────────────────────

impl SanitizeContent for ProposeStepOptimizationResult {
    fn sanitize_content_fields(&mut self) {
        self.rationale = sanitize_config(&self.rationale);
        // `steps` and `source_command_evidence` are passed through
        // unchanged: `steps` is validated structurally by
        // `validate_step_block` in Stage 3 (which enforces tight
        // shape and length constraints), and the evidence list is
        // bounded by Validate above. Sanitising bash bodies would
        // mangle their literal content and break the Stage 2
        // command-history cross-check.
    }
}

// ── Stage 3: Execution (placeholder) ─────────────────────────────────────

#[async_trait::async_trait]
impl Executor for ProposeStepOptimizationResult {
    fn dry_run_summary(&self) -> String {
        let entry_count = self.steps.as_array().map(Vec::len).unwrap_or(0);
        format!(
            "propose hoisting {entry_count} step(s) into front-matter `{}` (rationale: {})",
            self.section.as_wire_str(),
            truncate(&self.rationale, 120),
        )
    }

    async fn execute_impl(&self, ctx: &ExecutionContext) -> anyhow::Result<ExecutionResult> {
        info!(
            "propose-step-optimization: agent proposed {} entries for section `{}`",
            self.steps.as_array().map(Vec::len).unwrap_or(0),
            self.section.as_wire_str()
        );
        debug!(
            "propose-step-optimization payload: rationale={:?}, est_savings={:?}, evidence_count={}",
            self.rationale,
            self.estimated_token_savings,
            self.source_command_evidence.len()
        );

        // 1. Read the self-optimization config injected by main.rs into
        //    tool_configs["propose-step-optimization"].
        let config: crate::compile::types::SelfOptimizationConfig =
            ctx.get_tool_config("propose-step-optimization");

        // 2. Validate section against allowed_sections.
        let section_wire = self.section.as_wire_str();
        let section_allowed = config.allowed_sections.iter().any(|s| {
            serde_yaml::to_string(s)
                .unwrap_or_default()
                .trim()
                == section_wire
        });
        if !section_allowed {
            let allowed_names: Vec<String> = config
                .allowed_sections
                .iter()
                .map(|s| {
                    serde_yaml::to_string(s)
                        .unwrap_or_default()
                        .trim()
                        .to_string()
                })
                .collect();
            return Ok(ExecutionResult::failure(format!(
                "Section `{section_wire}` is not in the self-optimization \
                 allowed-sections list. Allowed: {allowed_names:?}. \
                 Add it to `self-optimization.allowed-sections` in the \
                 front matter to enable proposals targeting this section.",
            )));
        }

        // 3. IR-validate the proposed steps via the shared structural validator.
        //    Convert JSON -> YAML (lossless for JSON-shaped inputs).
        let json_text = serde_json::to_string(&self.steps)
            .map_err(|e| anyhow::anyhow!("Failed to re-serialize steps: {e}"))?;
        let yaml_value: serde_yaml::Value = serde_yaml::from_str(&json_text)
            .map_err(|e| anyhow::anyhow!("steps JSON is not valid YAML: {e}"))?;

        let validation = crate::compile::ir::validate_step_block(
            &yaml_value,
            crate::compile::ir::StepKindAllow::Curated,
        );

        if let Err(errors) = validation {
            let error_summary: Vec<String> = errors
                .iter()
                .map(|e| format!("  [{}] {}: {}", e.step_index, e.path, e.message))
                .collect();
            warn!(
                "propose-step-optimization IR validation failed ({} error(s))",
                errors.len()
            );
            return Ok(ExecutionResult::failure(format!(
                "Proposed steps failed IR validation ({} error(s)):\n{}",
                errors.len(),
                error_summary.join("\n")
            )));
        }

        // 4. Render the proposed block as canonical YAML for display.
        let proposed_yaml = serde_yaml::to_string(&yaml_value)
            .unwrap_or_else(|_| "<failed to render YAML>".to_string());

        // 5. Staged preview or live mode.
        if config.staged {
            // Staged mode: print a 🎭-marked preview to the Stage 3 step log.
            // Authors reviewing the build log see exactly what would land
            // in their front matter if they flipped `staged: false`.
            let savings_line = match self.estimated_token_savings {
                Some(n) => format!("Estimated token savings: ~{n} tokens/build\n"),
                None => String::new(),
            };
            let preview = format!(
                "\n\
                 ═══════════════════════════════════════════════════════════════════\n\
                 🎭 Proposed Step Optimization (staged preview — no changes applied)\n\
                 ═══════════════════════════════════════════════════════════════════\n\
                 \n\
                 Section: `{section_wire}`\n\
                 Rationale: {}\n\
                 {savings_line}\
                 \n\
                 Proposed YAML (add to `{section_wire}:` in your agent .md):\n\
                 ```yaml\n\
                 {proposed_yaml}\
                 ```\n\
                 \n\
                 To apply this optimization, set `self-optimization.staged: false`\n\
                 in your front matter and the next build will open a PR.\n\
                 ═══════════════════════════════════════════════════════════════════\n",
                self.rationale
            );
            println!("{preview}");
            info!("propose-step-optimization: staged preview rendered to build log");

            Ok(ExecutionResult::success(
                "Step-optimization proposal staged (preview only). \
                 The proposed YAML is visible in the build log above. \
                 Set `self-optimization.staged: false` to enable live PRs."
                    .to_string(),
            ))
        } else {
            // Live mode: read the source .md, patch the front matter to
            // include the proposed steps, push a commit to a new branch
            // via ADO REST, and open a PR.
            self.execute_live_pr(ctx, section_wire, &proposed_yaml)
                .await
        }
    }
}

impl ProposeStepOptimizationResult {
    /// Live-mode execution: patch the source .md and open a PR.
    async fn execute_live_pr(
        &self,
        ctx: &ExecutionContext,
        section_wire: &str,
        proposed_yaml: &str,
    ) -> anyhow::Result<ExecutionResult> {
        use anyhow::Context;
        use percent_encoding::utf8_percent_encode;

        let org_url = ctx
            .ado_org_url
            .as_ref()
            .context("AZURE_DEVOPS_ORG_URL not set")?;
        let project = ctx
            .ado_project
            .as_ref()
            .context("SYSTEM_TEAMPROJECT not set")?;
        let token = ctx
            .access_token
            .as_ref()
            .context("No access token available")?;
        let repo_id = ctx
            .repository_id
            .as_ref()
            .context("BUILD_REPOSITORY_ID not set")?;
        let source_rel_path = ctx
            .source_file_relative_path
            .as_ref()
            .context(
                "source_file_relative_path not set — cannot determine which \
                 file to edit for the live PR",
            )?;

        // Read the current source .md from the checked-out repo.
        let source_full_path = ctx.source_directory.join(source_rel_path);
        let original_content = tokio::fs::read_to_string(&source_full_path)
            .await
            .with_context(|| {
                format!(
                    "Failed to read source file for live-mode PR: {}",
                    source_full_path.display()
                )
            })?;

        // Patch the front matter: insert the proposed steps into the target section.
        let new_content = match patch_front_matter(&original_content, section_wire, proposed_yaml)
        {
            Ok(c) => c,
            Err(e) => {
                return Ok(ExecutionResult::failure(format!(
                    "Failed to patch front matter for live-mode PR: {e}"
                )));
            }
        };

        if new_content == original_content {
            return Ok(ExecutionResult::success(
                "Proposed steps are already present in the front matter — no PR needed."
                    .to_string(),
            ));
        }

        let client = reqwest::Client::new();

        // ── Idempotency / dedup: check for an existing open PR that
        // already proposes steps for this section. If found, add a
        // comment instead of opening a duplicate.
        let search_prefix = format!("ado-aw/self-opt-{}-", section_wire);
        let pr_search_url = format!(
            "{}{}/_apis/git/repositories/{}/pullrequests?searchCriteria.status=active&searchCriteria.sourceRefName=refs/heads/{}&api-version=7.1",
            org_url.trim_end_matches('/'),
            format!("/{}", utf8_percent_encode(project, super::PATH_SEGMENT)),
            repo_id,
            utf8_percent_encode(&search_prefix, super::PATH_SEGMENT),
        );
        // ADO's sourceRefName filter is prefix-based, so this finds
        // any branch starting with our section-specific prefix.
        if let Ok(search_resp) = client
            .get(&pr_search_url)
            .basic_auth("", Some(token))
            .send()
            .await
        {
            if search_resp.status().is_success() {
                if let Ok(body) = search_resp.json::<serde_json::Value>().await {
                    if let Some(prs) = body["value"].as_array() {
                        if let Some(existing_pr) = prs.first() {
                            let pr_id = existing_pr["pullRequestId"].as_u64().unwrap_or(0);
                            info!(
                                "propose-step-optimization: found existing open PR #{} \
                                 for section `{section_wire}` — skipping duplicate",
                                pr_id
                            );
                            return Ok(ExecutionResult::success(format!(
                                "An open self-optimization PR (#{pr_id}) already targets \
                                 the `{section_wire}` section. No duplicate PR was created. \
                                 Review and merge (or close) the existing PR first."
                            )));
                        }
                    }
                }
            }
        }

        // Resolve the HEAD commit of the default branch (target for the PR).
        let default_branch = ctx
            .source_branch_name
            .as_deref()
            .unwrap_or("main");
        let refs_url = format!(
            "{}{}/_apis/git/repositories/{}/refs?filter=heads/{}&api-version=7.1",
            org_url.trim_end_matches('/'),
            format!("/{}", utf8_percent_encode(project, super::PATH_SEGMENT)),
            repo_id,
            default_branch,
        );
        let refs_resp = client
            .get(&refs_url)
            .basic_auth("", Some(token))
            .send()
            .await
            .context("Failed to resolve HEAD ref")?;
        if !refs_resp.status().is_success() {
            let status = refs_resp.status();
            let body = refs_resp.text().await.unwrap_or_default();
            return Ok(ExecutionResult::failure(format!(
                "Failed to resolve HEAD of {default_branch}: HTTP {status} — {body}"
            )));
        }
        let refs_body: serde_json::Value = refs_resp.json().await?;
        let base_commit = refs_body["value"]
            .as_array()
            .and_then(|a| a.first())
            .and_then(|r| r["objectId"].as_str())
            .context("Could not parse HEAD objectId from refs response")?
            .to_string();

        // Generate a branch name for the PR.
        let short_id = {
            use rand::RngExt;
            let v: u32 = rand::rng().random();
            format!("{:08x}", v)
        };
        let source_branch = format!("ado-aw/self-opt-{}-{}", section_wire, short_id);
        let source_ref = format!("refs/heads/{}", source_branch);

        // Build the single-file edit change.
        let file_path = format!("/{}", source_rel_path);
        let change = serde_json::json!({
            "changeType": "edit",
            "item": { "path": file_path },
            "newContent": {
                "content": new_content,
                "contentType": "rawtext"
            }
        });

        let commit_message = format!(
            "chore(ado-aw): self-optimize `{}` steps\n\n{}",
            section_wire,
            truncate(&self.rationale, 200)
        );

        // Push the commit to the new branch.
        let push_url = format!(
            "{}{}/_apis/git/repositories/{}/pushes?api-version=7.1",
            org_url.trim_end_matches('/'),
            format!("/{}", utf8_percent_encode(project, super::PATH_SEGMENT)),
            repo_id,
        );
        let push_body = serde_json::json!({
            "refUpdates": [{
                "name": source_ref,
                "oldObjectId": "0000000000000000000000000000000000000000"
            }],
            "commits": [{
                "comment": commit_message,
                "changes": [change],
                "parents": [base_commit]
            }]
        });

        let push_resp = client
            .post(&push_url)
            .basic_auth("", Some(token))
            .json(&push_body)
            .send()
            .await
            .context("Failed to push commit")?;

        if !push_resp.status().is_success() {
            let status = push_resp.status();
            let body = push_resp.text().await.unwrap_or_default();
            return Ok(ExecutionResult::failure(format!(
                "Failed to push self-optimization commit: HTTP {status} — {body}"
            )));
        }
        info!("propose-step-optimization: pushed branch {source_branch}");

        // Create the PR.
        let pr_title = format!(
            "chore(ado-aw): self-optimize `{}` steps",
            section_wire
        );
        let savings_note = match self.estimated_token_savings {
            Some(n) => format!("\n\nEstimated token savings: ~{n} tokens/build."),
            None => String::new(),
        };
        let pr_body = format!(
            "## Self-Optimization Proposal\n\n\
             **Section:** `{section_wire}`\n\
             **Rationale:** {}{savings_note}\n\n\
             This PR was automatically opened by the `propose-step-optimization` \
             safe-output (self-optimization live mode). The proposed steps passed \
             IR validation (Curated allow-list: bash + typed-factory tasks only) \
             and the Stage 2 detection cross-check.\n\n\
             Review the YAML changes and merge if they look correct.",
            self.rationale
        );

        let target_ref = format!("refs/heads/{}", default_branch);
        let pr_url = format!(
            "{}{}/_apis/git/repositories/{}/pullrequests?api-version=7.1",
            org_url.trim_end_matches('/'),
            format!("/{}", utf8_percent_encode(project, super::PATH_SEGMENT)),
            repo_id,
        );
        let pr_payload = serde_json::json!({
            "sourceRefName": source_ref,
            "targetRefName": target_ref,
            "title": pr_title,
            "description": pr_body,
        });

        let pr_resp = client
            .post(&pr_url)
            .basic_auth("", Some(token))
            .json(&pr_payload)
            .send()
            .await
            .context("Failed to create PR")?;

        if !pr_resp.status().is_success() {
            let status = pr_resp.status();
            let body = pr_resp.text().await.unwrap_or_default();
            return Ok(ExecutionResult::failure(format!(
                "Pushed branch {source_branch} but failed to create PR: HTTP {status} — {body}"
            )));
        }

        let pr_data: serde_json::Value = pr_resp.json().await.unwrap_or_default();
        let pr_id = pr_data["pullRequestId"].as_u64().unwrap_or(0);
        let pr_url_human = pr_data["url"]
            .as_str()
            .unwrap_or("<url unavailable>");

        info!(
            "propose-step-optimization: opened PR #{} on branch {}",
            pr_id, source_branch
        );

        Ok(ExecutionResult::success_with_data(
            format!(
                "Self-optimization PR #{pr_id} opened on branch `{source_branch}` \
                 targeting `{default_branch}`. Review and merge to apply the \
                 proposed `{section_wire}` steps."
            ),
            serde_json::json!({
                "pull_request_id": pr_id,
                "source_branch": source_branch,
                "target_branch": default_branch,
                "url": pr_url_human,
                "section": section_wire,
            }),
        ))
    }
}

/// Patch the front matter of an agent `.md` file to insert proposed
/// steps into the target section.
///
/// Strategy: parse the YAML, insert/append steps under the target key,
/// re-serialize. This loses comments (acceptable for machine-generated
/// PRs — the author reviews and can format to taste).
fn patch_front_matter(
    original: &str,
    section_key: &str,
    proposed_yaml: &str,
) -> anyhow::Result<String> {
    // Split on front-matter fences: ---\n<yaml>\n---\n<body>
    let trimmed = original.strip_prefix("---\n").or_else(|| original.strip_prefix("---\r\n"));
    let Some(after_first_fence) = trimmed else {
        anyhow::bail!("source file does not start with a `---` front-matter fence");
    };
    let Some(fence_end) = after_first_fence.find("\n---") else {
        anyhow::bail!("source file does not have a closing `---` front-matter fence");
    };
    let yaml_str = &after_first_fence[..fence_end];
    let body_with_fence = &after_first_fence[fence_end..]; // includes "\n---\n<body>"

    // Parse the YAML front matter as a mapping.
    let mut fm: serde_yaml::Value = serde_yaml::from_str(yaml_str)
        .context("failed to parse front matter YAML")?;
    let mapping = fm
        .as_mapping_mut()
        .context("front matter is not a YAML mapping")?;

    // Parse the proposed steps.
    let proposed: serde_yaml::Value = serde_yaml::from_str(proposed_yaml)
        .context("failed to parse proposed YAML")?;
    let proposed_seq = proposed
        .as_sequence()
        .context("proposed YAML is not a sequence")?;

    // Insert or extend the target section.
    let key = serde_yaml::Value::String(section_key.to_string());
    if let Some(existing) = mapping.get_mut(&key) {
        if let Some(seq) = existing.as_sequence_mut() {
            seq.extend(proposed_seq.iter().cloned());
        } else {
            // Section exists but is not a sequence — overwrite with the proposed
            *existing = serde_yaml::Value::Sequence(proposed_seq.clone());
        }
    } else {
        mapping.insert(key, serde_yaml::Value::Sequence(proposed_seq.clone()));
    }

    // Re-serialize.
    let new_yaml = serde_yaml::to_string(&fm).context("failed to re-serialize front matter")?;
    Ok(format!("---\n{new_yaml}{body_with_fence}"))
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let mut out = s.chars().take(max).collect::<String>();
        out.push('…');
        out
    }
}

// No per-tool `Config` struct: this tool is configured exclusively via
// the top-level `self-optimization:` front-matter section (see
// `crate::compile::types::SelfOptimizationConfig`). Stage 3 reads
// `self_optimization.staged` and `allowed_sections` directly from
// the front matter, not via `ctx.get_tool_config(...)`. A stray
// `safe-outputs.propose-step-optimization:` block is independently
// rejected by `compile::common::validate_self_optimization_config`.

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::safeoutputs::ToolResult;

    fn minimal_params() -> ProposeStepOptimizationParams {
        ProposeStepOptimizationParams {
            section: ProposalSection::Steps,
            rationale: "Lift deterministic clone+install out of the agent".into(),
            estimated_token_savings: Some(4200),
            steps: serde_json::json!([
                {"bash": "git fetch --depth=1 origin main", "displayName": "Fetch main"}
            ]),
            source_command_evidence: vec![
                "git fetch --depth=1 origin main".into(),
                "git fetch --depth=1 origin main".into(),
            ],
        }
    }

    #[test]
    fn result_has_correct_name() {
        assert_eq!(
            ProposeStepOptimizationResult::NAME,
            "propose-step-optimization"
        );
    }

    #[test]
    fn requires_write_is_true() {
        assert!(
            ProposeStepOptimizationResult::REQUIRES_WRITE,
            "propose-step-optimization opens PRs in live mode; must require write"
        );
    }

    #[test]
    fn params_round_trip_through_json() {
        let p = minimal_params();
        // Round-trip via JSON to confirm the wire shape (the MCP
        // transport hands us JSON).
        let json = serde_json::to_string(&serde_json::json!({
            "section": "steps",
            "rationale": p.rationale,
            "estimated_token_savings": p.estimated_token_savings,
            "steps": p.steps,
            "source_command_evidence": p.source_command_evidence,
        }))
        .unwrap();
        let parsed: ProposeStepOptimizationParams = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.section, ProposalSection::Steps);
        assert_eq!(parsed.rationale, p.rationale);
        assert_eq!(parsed.estimated_token_savings, Some(4200));
        assert!(parsed.steps.is_array());
        assert_eq!(parsed.source_command_evidence.len(), 2);
    }

    #[test]
    fn section_kebab_case_round_trip() {
        for (variant, wire) in [
            (ProposalSection::Steps, "steps"),
            (ProposalSection::PostSteps, "post-steps"),
            (ProposalSection::Setup, "setup"),
            (ProposalSection::Teardown, "teardown"),
        ] {
            let serialised = serde_json::to_string(&variant).unwrap();
            assert!(
                serialised.contains(wire),
                "{variant:?} should serialise as {wire:?}; got {serialised}"
            );
            let parsed: ProposalSection =
                serde_json::from_str(&format!("\"{wire}\"")).unwrap();
            assert_eq!(parsed, variant);
            assert_eq!(variant.as_wire_str(), wire);
        }
    }

    #[test]
    fn validation_rejects_empty_rationale() {
        let mut p = minimal_params();
        p.rationale = "   ".into();
        let r: Result<ProposeStepOptimizationResult, _> = p.try_into();
        let err = r.expect_err("empty rationale must be rejected");
        assert!(format!("{err}").contains("rationale"));
    }

    #[test]
    fn validation_rejects_oversized_rationale() {
        let mut p = minimal_params();
        p.rationale = "x".repeat(MAX_RATIONALE_BYTES + 1);
        let r: Result<ProposeStepOptimizationResult, _> = p.try_into();
        assert!(r.is_err(), "oversized rationale must be rejected");
    }

    #[test]
    fn validation_rejects_non_array_steps() {
        let mut p = minimal_params();
        p.steps = serde_json::json!({ "bash": "echo hi" });
        let r: Result<ProposeStepOptimizationResult, _> = p.try_into();
        let err = r.expect_err("steps must be a JSON array");
        assert!(format!("{err}").contains("array"));
    }

    #[test]
    fn validation_rejects_empty_steps_array() {
        let mut p = minimal_params();
        p.steps = serde_json::json!([]);
        let r: Result<ProposeStepOptimizationResult, _> = p.try_into();
        assert!(r.is_err(), "empty steps array must be rejected");
    }

    #[test]
    fn validation_rejects_too_many_evidence_entries() {
        let mut p = minimal_params();
        p.source_command_evidence = (0..MAX_EVIDENCE_ENTRIES + 1)
            .map(|i| format!("echo {i}"))
            .collect();
        let r: Result<ProposeStepOptimizationResult, _> = p.try_into();
        assert!(r.is_err(), "over-cap evidence list must be rejected");
    }

    #[test]
    fn validation_rejects_oversized_evidence_entry() {
        let mut p = minimal_params();
        p.source_command_evidence = vec!["x".repeat(MAX_EVIDENCE_ENTRY_BYTES + 1)];
        let r: Result<ProposeStepOptimizationResult, _> = p.try_into();
        assert!(r.is_err(), "oversized evidence entry must be rejected");
    }

    #[test]
    fn dry_run_summary_includes_section_and_truncated_rationale() {
        let p = minimal_params();
        let r: ProposeStepOptimizationResult = p.try_into().unwrap();
        let summary = r.dry_run_summary();
        assert!(summary.contains("steps"));
        assert!(summary.contains("Lift deterministic"));
    }

    #[test]
    fn sanitize_preserves_steps_and_evidence_unchanged() {
        let p = minimal_params();
        let mut r: ProposeStepOptimizationResult = p.try_into().unwrap();
        let steps_before = r.steps.clone();
        let evidence_before = r.source_command_evidence.clone();
        r.sanitize_content_fields();
        assert_eq!(
            r.steps, steps_before,
            "steps must pass through sanitize unchanged — \
             Stage 3 IR validator enforces structure; mangling here \
             would break the Stage 2 command-history cross-check"
        );
        assert_eq!(
            r.source_command_evidence, evidence_before,
            "evidence list must pass through sanitize unchanged"
        );
    }


    // ── patch_front_matter tests ──────────────────────────────────────────

    #[test]
    fn patch_inserts_steps_into_existing_section() {
        let original = "---\nname: test\nsteps:\n  - bash: echo existing\n---\n\nBody\n";
        let proposed = "- bash: echo new\n  displayName: New\n";
        let patched = patch_front_matter(original, "steps", proposed).unwrap();
        assert!(patched.contains("echo existing"), "must keep existing");
        assert!(patched.contains("echo new"), "must add new");
        assert!(patched.contains("\n---\n"), "must preserve body fence");
        assert!(patched.contains("Body"), "must preserve body");
    }

    #[test]
    fn patch_creates_section_when_absent() {
        let original = "---\nname: test\ndescription: x\n---\n\nBody\n";
        let proposed = "- bash: echo hi\n";
        let patched = patch_front_matter(original, "post-steps", proposed).unwrap();
        assert!(patched.contains("post-steps"), "must create section");
        assert!(patched.contains("echo hi"), "must add step");
    }

    #[test]
    fn patch_returns_error_for_missing_fences() {
        let no_fence = "name: test\ndescription: x\n";
        assert!(patch_front_matter(no_fence, "steps", "- bash: echo hi\n").is_err());
    }
}
