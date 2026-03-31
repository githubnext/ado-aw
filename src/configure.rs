//! The `configure` CLI command.
//!
//! Detects agentic pipelines in a local repository and updates the `GITHUB_TOKEN`
//! pipeline variable on their corresponding Azure DevOps build definitions.
//!
//! Uses the same ADO REST API patterns as the existing tools in `src/tools/`
//! (reqwest + `.basic_auth("", Some(token))` for authentication).

use anyhow::{Context, Result};
use log::{debug, info, warn};
use serde::Deserialize;
use std::path::Path;

use crate::detect;

/// ADO resource ID for minting ADO-scoped tokens via Azure CLI.
const ADO_RESOURCE_ID: &str = "499b84ac-1321-427f-aa17-267ca6975798";

/// Attempt to acquire an ADO-scoped access token via `az account get-access-token`.
/// Returns `Ok(token)` if the Azure CLI is installed and the user is logged in,
/// or an error if the CLI is missing or the command fails.
async fn try_azure_cli_token() -> Result<String> {
    // On Windows, `az` is a .cmd batch script that must be invoked via cmd.exe.
    let output = if cfg!(windows) {
        tokio::process::Command::new("cmd")
            .args([
                "/C", "az", "account", "get-access-token",
                "--resource", ADO_RESOURCE_ID,
                "--query", "accessToken",
                "-o", "tsv",
            ])
            .output()
            .await
    } else {
        tokio::process::Command::new("az")
            .args([
                "account", "get-access-token",
                "--resource", ADO_RESOURCE_ID,
                "--query", "accessToken",
                "-o", "tsv",
            ])
            .output()
            .await
    }
    .context("Failed to run 'az account get-access-token'. Is the Azure CLI installed?")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Azure CLI token acquisition failed: {}", stderr.trim());
    }

    let token = String::from_utf8(output.stdout)
        .context("Azure CLI returned non-UTF-8 token")?
        .trim()
        .to_string();

    if token.is_empty() {
        anyhow::bail!("Azure CLI returned an empty token");
    }

    Ok(token)
}

// ==================== ADO context from git remote ====================

/// ADO context extracted from the git remote URL.
#[derive(Debug, Clone)]
pub struct AdoContext {
    /// Organization URL (e.g., `https://dev.azure.com/myorg`)
    pub org_url: String,
    /// Project name
    pub project: String,
    /// Repository name
    pub repo_name: String,
}

/// Parse the ADO org, project, and repo from a git remote URL.
///
/// Supports:
/// - HTTPS: `https://dev.azure.com/{org}/{project}/_git/{repo}`
/// - HTTPS (legacy): `https://{org}.visualstudio.com/{project}/_git/{repo}`
/// - SSH: `git@ssh.dev.azure.com:v3/{org}/{project}/{repo}`
/// - SSH (legacy): `git@vs-ssh.visualstudio.com:v3/{org}/{project}/{repo}`
pub fn parse_ado_remote(remote_url: &str) -> Result<AdoContext> {
    let url = remote_url.trim();

    // SSH format: git@ssh.dev.azure.com:v3/{org}/{project}/{repo}
    // Also handles legacy: git@vs-ssh.visualstudio.com:v3/{org}/{project}/{repo}
    if let Some(rest) = url
        .strip_prefix("git@ssh.dev.azure.com:v3/")
        .or_else(|| url.strip_prefix("git@vs-ssh.visualstudio.com:v3/"))
    {
        let parts: Vec<&str> = rest.splitn(3, '/').collect();
        if parts.len() >= 3 {
            let repo_name = parts[2].trim_end_matches(".git");
            return Ok(AdoContext {
                org_url: format!("https://dev.azure.com/{}", parts[0]),
                project: parts[1].to_string(),
                repo_name: repo_name.to_string(),
            });
        }
    }

    // HTTPS format: https://dev.azure.com/{org}/{project}/_git/{repo}
    if url.contains("dev.azure.com") {
        let url_parsed =
            url::Url::parse(url).with_context(|| format!("Invalid URL: {}", url))?;
        let segments: Vec<&str> = url_parsed
            .path_segments()
            .map(|s| s.collect())
            .unwrap_or_default();

        // Expected: /{org}/{project}/_git/{repo}
        if segments.len() >= 4 && segments[2] == "_git" {
            let repo_name = segments[3].trim_end_matches(".git");
            return Ok(AdoContext {
                org_url: format!("https://dev.azure.com/{}", segments[0]),
                project: segments[1].to_string(),
                repo_name: repo_name.to_string(),
            });
        }
    }

    // Legacy format: https://{org}.visualstudio.com/{project}/_git/{repo}
    if url.contains(".visualstudio.com") {
        let url_parsed =
            url::Url::parse(url).with_context(|| format!("Invalid URL: {}", url))?;
        let host = url_parsed.host_str().unwrap_or("");
        let org = host.strip_suffix(".visualstudio.com").unwrap_or(host);
        let segments: Vec<&str> = url_parsed
            .path_segments()
            .map(|s| s.collect())
            .unwrap_or_default();

        // Expected: /{project}/_git/{repo}
        if segments.len() >= 3 && segments[1] == "_git" {
            let repo_name = segments[2].trim_end_matches(".git");
            return Ok(AdoContext {
                org_url: format!("https://dev.azure.com/{}", org),
                project: segments[0].to_string(),
                repo_name: repo_name.to_string(),
            });
        }
    }

    anyhow::bail!(
        "Could not parse ADO context from remote URL: {}. \
         Expected format: https://dev.azure.com/{{org}}/{{project}}/_git/{{repo}}",
        url
    )
}

/// Get the git remote URL for the repository at `repo_path`.
async fn get_git_remote_url(repo_path: &Path) -> Result<String> {
    let output = tokio::process::Command::new("git")
        .args(["remote", "get-url", "origin"])
        .current_dir(repo_path)
        .output()
        .await
        .context("Failed to run git command")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("git remote get-url origin failed: {}", stderr);
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

// ==================== ADO Build Definitions API ====================

/// Authentication method for ADO API calls.
/// PATs use HTTP Basic auth; Azure CLI tokens use Bearer auth.
#[derive(Clone)]
enum AdoAuth {
    Pat(String),
    Bearer(String),
}

impl AdoAuth {
    fn apply(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match self {
            AdoAuth::Pat(pat) => request.basic_auth("", Some(pat)),
            AdoAuth::Bearer(token) => request.bearer_auth(token),
        }
    }
}

/// Minimal subset of an ADO Build Definition for listing.
#[derive(Debug, Deserialize)]
struct DefinitionListResponse {
    value: Vec<DefinitionSummary>,
}

#[derive(Debug, Deserialize)]
struct DefinitionSummary {
    id: u64,
    name: String,
    process: Option<ProcessInfo>,
}

#[derive(Debug, Deserialize)]
struct ProcessInfo {
    #[serde(rename = "yamlFilename")]
    yaml_filename: Option<String>,
}

/// How a local YAML file was matched to an ADO pipeline definition.
#[derive(Debug, Clone)]
pub enum MatchMethod {
    YamlPath,
    PipelineName,
    Explicit,
}

impl std::fmt::Display for MatchMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MatchMethod::YamlPath => write!(f, "yaml-path"),
            MatchMethod::PipelineName => write!(f, "pipeline-name"),
            MatchMethod::Explicit => write!(f, "explicit"),
        }
    }
}

/// A matched pipeline definition from ADO.
#[derive(Debug, Clone)]
struct MatchedDefinition {
    id: u64,
    name: String,
    match_method: MatchMethod,
    yaml_path: String,
}

/// List all build definitions in the project, handling pagination.
async fn list_definitions(
    client: &reqwest::Client,
    ctx: &AdoContext,
    auth: &AdoAuth,
) -> Result<Vec<DefinitionSummary>> {
    let mut all_definitions = Vec::new();
    let mut continuation_token: Option<String> = None;

    loop {
        let base_url = format!(
            "{}/{}/_apis/build/definitions",
            ctx.org_url.trim_end_matches('/'),
            ctx.project
        );

        debug!("Listing definitions: {}", base_url);

        let mut request = auth.apply(client.get(&base_url))
            .query(&[("includeAllProperties", "true"), ("api-version", "7.1")]);
        if let Some(ref token) = continuation_token {
            request = request.query(&[("continuationToken", token)]);
        }

        let resp = request
            .send()
            .await
            .context("Failed to list pipeline definitions")?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!(
                "ADO API returned {} when listing definitions: {}",
                status,
                body
            );
        }

        // Check for continuation token in response headers
        let next_token = resp
            .headers()
            .get("x-ms-continuationtoken")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        let body = resp.text().await.context("Failed to read definitions response body")?;
        let response: DefinitionListResponse = serde_json::from_str(&body)
            .with_context(|| {
                let snippet: String = body.chars().take(500).collect();
                format!(
                    "Failed to parse definitions response as JSON. \
                     This usually means the PAT is invalid or expired. \
                     Response body (first 500 chars):\n{snippet}"
                )
            })?;

        all_definitions.extend(response.value);

        match next_token {
            Some(token) if !token.is_empty() => {
                continuation_token = Some(token);
            }
            _ => break,
        }
    }

    Ok(all_definitions)
}

/// Result of a fuzzy name match attempt.
#[derive(Debug, PartialEq)]
enum FuzzyMatchResult {
    /// Exactly one definition matched.
    Single(usize),
    /// Multiple definitions matched (ambiguous).
    Ambiguous(Vec<String>),
    /// No definitions matched.
    None,
}

/// Fuzzy-match an agent filename against pipeline definition names.
///
/// Checks if any definition name contains the agent name (with hyphens also
/// tried as spaces). Returns `Single(index)` for an unambiguous match,
/// `Ambiguous` when multiple definitions match, or `None` when nothing matches.
fn fuzzy_match_by_name(agent_name: &str, definitions: &[DefinitionSummary]) -> FuzzyMatchResult {
    if agent_name.is_empty() {
        return FuzzyMatchResult::None;
    }

    let agent_lower = agent_name.to_lowercase();
    let agent_spaced = agent_lower.replace('-', " ");
    let candidates: Vec<(usize, &DefinitionSummary)> = definitions
        .iter()
        .enumerate()
        .filter(|(_, d)| {
            let def_name_lower = d.name.to_lowercase();
            def_name_lower.contains(&agent_spaced) || def_name_lower.contains(&agent_lower)
        })
        .collect();

    match candidates.len() {
        1 => FuzzyMatchResult::Single(candidates[0].0),
        n if n > 1 => {
            let names = candidates.iter().map(|(_, d)| d.name.clone()).collect();
            FuzzyMatchResult::Ambiguous(names)
        }
        _ => FuzzyMatchResult::None,
    }
}

/// Normalize an ADO YAML filename for comparison with local paths.
///
/// ADO's Build Definitions API stores `yamlFilename` with a leading `/`
/// (e.g., `/.azdo/pipelines/agent.yml`) and may use backslashes on Windows.
/// This strips the leading `/` and normalizes separators to forward slashes.
fn normalize_ado_yaml_path(path: &str) -> String {
    path.replace('\\', "/").trim_start_matches('/').to_string()
}

/// Match detected pipeline YAML files to ADO pipeline definitions.
///
/// Strategy:
/// 1. Try to match by the `yamlFilename` field in the definition's process config
/// 2. Fall back to matching by pipeline name containing the agent name
async fn match_definitions(
    client: &reqwest::Client,
    ctx: &AdoContext,
    auth: &AdoAuth,
    detected: &[detect::DetectedPipeline],
) -> Result<Vec<MatchedDefinition>> {
    let definitions = list_definitions(client, ctx, auth).await?;
    info!(
        "Found {} pipeline definitions in {}/{}",
        definitions.len(),
        ctx.org_url,
        ctx.project
    );

    let mut matched = Vec::new();

    // Log all definition yaml paths for debugging
    for def in &definitions {
        let yaml_path = def
            .process
            .as_ref()
            .and_then(|p| p.yaml_filename.as_ref())
            .map(|f| normalize_ado_yaml_path(f));
        debug!(
            "ADO definition: '{}' (id={}) yamlFilename={:?} normalized={:?}",
            def.name, def.id,
            def.process.as_ref().and_then(|p| p.yaml_filename.as_ref()),
            yaml_path
        );
    }

    for pipeline in detected {
        let yaml_path_str = pipeline.yaml_path.to_string_lossy();
        let yaml_path_normalized = yaml_path_str.replace('\\', "/");
        debug!(
            "Matching local pipeline: raw={:?} normalized={:?} source={:?}",
            yaml_path_str, yaml_path_normalized, pipeline.source
        );

        // Strategy 1: Match by YAML filename in the definition.
        // ADO stores yamlFilename with a leading '/' (e.g., "/.azdo/pipelines/agent.yml"),
        // so we strip it before comparing to the locally-detected relative path.
        let path_match = definitions.iter().find(|d| {
            d.process
                .as_ref()
                .and_then(|p| p.yaml_filename.as_ref())
                .is_some_and(|f| normalize_ado_yaml_path(f) == yaml_path_normalized)
        });

        if let Some(def) = path_match {
            debug!(
                "Matched '{}' to definition '{}' (id={}) by YAML path",
                yaml_path_normalized, def.name, def.id
            );
            matched.push(MatchedDefinition {
                id: def.id,
                name: def.name.clone(),
                match_method: MatchMethod::YamlPath,
                yaml_path: yaml_path_normalized.to_string(),
            });
            continue;
        }

        // Strategy 2: Fall back to matching by pipeline name.
        // Only accept unambiguous matches — if multiple definitions match, skip.
        let agent_name = Path::new(&pipeline.source)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("");

        match fuzzy_match_by_name(agent_name, &definitions) {
            FuzzyMatchResult::Single(idx) => {
                let def = &definitions[idx];
                eprintln!(
                    "  Warning: '{}' matched to '{}' (id={}) by pipeline name (fuzzy match)",
                    yaml_path_normalized, def.name, def.id
                );
                matched.push(MatchedDefinition {
                    id: def.id,
                    name: def.name.clone(),
                    match_method: MatchMethod::PipelineName,
                    yaml_path: yaml_path_normalized.to_string(),
                });
                continue;
            }
            FuzzyMatchResult::Ambiguous(names) => {
                eprintln!(
                    "  Warning: '{}' has {} ambiguous name matches ({}), skipping",
                    yaml_path_normalized,
                    names.len(),
                    names.join(", ")
                );
                continue;
            }
            FuzzyMatchResult::None => {}
        }

        info!(
            "No ADO definition match for: {} (source: {})",
            yaml_path_normalized, pipeline.source
        );
    }

    Ok(matched)
}

/// Fetch the human-readable name of a pipeline definition by ID.
/// Returns `None` if the definition doesn't exist or the request fails.
async fn get_definition_name(
    client: &reqwest::Client,
    ctx: &AdoContext,
    auth: &AdoAuth,
    definition_id: u64,
) -> Option<String> {
    let url = format!(
        "{}/{}/_apis/build/definitions/{}?api-version=7.1",
        ctx.org_url.trim_end_matches('/'),
        ctx.project,
        definition_id
    );

    let resp = auth
        .apply(client.get(&url))
        .send()
        .await
        .ok()?;

    if !resp.status().is_success() {
        return None;
    }

    let body: serde_json::Value = resp.json().await.ok()?;
    body.get("name")
        .and_then(|n| n.as_str())
        .map(|s| s.to_string())
}

/// Update the GITHUB_TOKEN pipeline variable on a definition.
///
/// Note: The GET→PUT cycle is not atomic. Concurrent `configure` runs against
/// the same definition could overwrite each other's variables. This is acceptable
/// for a CLI tool typically run by a single operator.
async fn update_pipeline_variable(
    client: &reqwest::Client,
    ctx: &AdoContext,
    auth: &AdoAuth,
    definition_id: u64,
    variable_name: &str,
    variable_value: &str,
) -> Result<()> {
    let get_url = format!(
        "{}/{}/_apis/build/definitions/{}?api-version=7.1",
        ctx.org_url.trim_end_matches('/'),
        ctx.project,
        definition_id
    );

    debug!("Fetching definition {}: {}", definition_id, get_url);

    let resp = auth
        .apply(client.get(&get_url))
        .send()
        .await
        .context("Failed to get pipeline definition")?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!(
            "ADO API returned {} when getting definition {}: {}",
            status,
            definition_id,
            body
        );
    }

    let body = resp.text().await.context("Failed to read definition response body")?;
    let mut definition: serde_json::Value = serde_json::from_str(&body)
        .with_context(|| {
            let snippet: String = body.chars().take(500).collect();
            format!(
                "Failed to parse definition {} as JSON. \
                 This usually means the PAT is invalid or expired. \
                 Response body (first 500 chars):\n{snippet}",
                definition_id
            )
        })?;

    // Ensure variables object exists
    if definition.get("variables").is_none() {
        definition["variables"] = serde_json::json!({});
    }

    // Set the variable (mark as secret since it's a token).
    // Preserve existing allowOverride if the variable already exists,
    // otherwise default to false (stricter security posture).
    let allow_override = definition
        .get("variables")
        .and_then(|vars| vars.get(variable_name))
        .and_then(|var| var.get("allowOverride"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    definition["variables"][variable_name] = serde_json::json!({
        "value": variable_value,
        "isSecret": true,
        "allowOverride": allow_override
    });

    let put_url = format!(
        "{}/{}/_apis/build/definitions/{}?api-version=7.1",
        ctx.org_url.trim_end_matches('/'),
        ctx.project,
        definition_id
    );

    debug!("Updating definition {}: {}", definition_id, put_url);

    let resp = auth
        .apply(client.put(&put_url))
        .header("Content-Type", "application/json")
        .json(&definition)
        .send()
        .await
        .context("Failed to update pipeline definition")?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!(
            "ADO API returned {} when updating definition {}: {}",
            status,
            definition_id,
            body
        );
    }

    Ok(())
}

// ==================== Command orchestration ====================

/// Run the configure command.
pub async fn run(
    token: Option<&str>,
    org: Option<&str>,
    project: Option<&str>,
    pat: Option<&str>,
    path: Option<&Path>,
    dry_run: bool,
    definition_ids: Option<&[u64]>,
) -> Result<()> {
    let repo_path = match path {
        Some(p) => tokio::fs::canonicalize(p)
            .await
            .with_context(|| format!("Could not resolve path: {}", p.display()))?,
        None => tokio::fs::canonicalize(".")
            .await
            .context("Could not resolve current directory")?,
    };

    // Resolve token: CLI flag > env var (handled by clap) > interactive prompt
    let token = match token {
        Some(t) => t.to_string(),
        None => inquire::Password::new("Enter the new GITHUB_TOKEN:")
            .without_confirmation()
            .prompt()
            .context("Failed to read token from interactive prompt")?,
    };

    // Resolve auth: CLI flag > env var (handled by clap) > Azure CLI > interactive prompt
    let auth = match pat {
        Some(p) => {
            info!("Using PAT from --pat flag or AZURE_DEVOPS_EXT_PAT env var");
            AdoAuth::Pat(p.to_string())
        }
        None => {
            info!("No PAT provided, trying Azure CLI authentication...");
            match try_azure_cli_token().await {
                Ok(token) => {
                    println!("Using Azure CLI authentication (az account get-access-token)");
                    AdoAuth::Bearer(token)
                }
                Err(e) => {
                    warn!("Azure CLI auth failed: {:#}. Falling back to interactive prompt.", e);
                    let pat = inquire::Password::new("Enter your Azure DevOps PAT:")
                        .without_confirmation()
                        .prompt()
                        .context("Failed to read PAT from interactive prompt. Set AZURE_DEVOPS_EXT_PAT env var, log in with 'az login', or use --pat flag.")?;
                    AdoAuth::Pat(pat)
                }
            }
        }
    };

    // Resolve ADO context from git remote (best-effort), with CLI overrides
    let ado_ctx = match (get_git_remote_url(&repo_path).await.ok(), org, project) {
        // Git remote available — parse and apply overrides
        (Some(remote_url), org, project) => {
            info!("Git remote: {}", remote_url);
            let mut ctx = parse_ado_remote(&remote_url).with_context(|| {
                format!(
                    "Could not parse ADO context from remote '{}'. Use --org and --project to specify manually.",
                    remote_url
                )
            })?;
            if let Some(org) = org {
                ctx.org_url = org.to_string();
            }
            if let Some(project) = project {
                ctx.project = project.to_string();
            }
            ctx
        }
        // No git remote — require explicit --org and --project
        (None, Some(org), Some(project)) => {
            info!("No git remote; using --org and --project");
            AdoContext {
                org_url: org.to_string(),
                project: project.to_string(),
                repo_name: String::new(),
            }
        }
        (None, _, _) => {
            anyhow::bail!(
                "Could not determine ADO context: no git remote found and --org/--project not both provided.\n\
                 Use --org <org-url> --project <project> to specify manually."
            );
        }
    };

    println!(
        "ADO context: org={}, project={}{}",
        ado_ctx.org_url,
        ado_ctx.project,
        if ado_ctx.repo_name.is_empty() {
            String::new()
        } else {
            format!(", repo={}", ado_ctx.repo_name)
        }
    );
    println!();

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .context("Failed to create HTTP client")?;

    // Build the list of definitions to update — either from explicit IDs or auto-detection
    let matched = if let Some(ids) = definition_ids {
        println!("Using explicit definition IDs: {:?}", ids);

        let mut matched = Vec::new();
        for &id in ids {
            let name = get_definition_name(&client, &ado_ctx, &auth, id)
                .await
                .unwrap_or_else(|| format!("definition {}", id));
            matched.push(MatchedDefinition {
                id,
                name,
                match_method: MatchMethod::Explicit,
                yaml_path: String::new(),
            });
        }
        matched
    } else {
        // Auto-detect: scan local repo and match to ADO definitions
        println!("Scanning for agentic pipelines...");
        let detected = detect::detect_pipelines(&repo_path).await?;

        if detected.is_empty() {
            println!(
                "No agentic pipelines found. Make sure your pipelines were compiled with the latest ado-aw."
            );
            return Ok(());
        }

        println!("Found {} agentic pipeline(s):", detected.len());
        for p in &detected {
            println!(
                "  {} (source: {}, version: {})",
                p.yaml_path.display(),
                p.source,
                p.version
            );
        }
        println!();

        println!("Matching to Azure DevOps pipeline definitions...");
        match_definitions(&client, &ado_ctx, &auth, &detected).await?
    };

    if matched.is_empty() {
        println!("No matching ADO pipeline definitions found.");
        println!("Make sure your pipelines are registered in Azure DevOps and point to the detected YAML files.");
        return Ok(());
    }

    println!("{} definition(s) to update:", matched.len());
    for m in &matched {
        if m.yaml_path.is_empty() {
            println!(
                "  [{}] '{}' (id={})",
                m.match_method, m.name, m.id
            );
        } else {
            println!(
                "  [{}] '{}' (id={}) \u{2190} {}",
                m.match_method, m.name, m.id, m.yaml_path
            );
        }
    }
    println!();

    // Step 4: Update GITHUB_TOKEN
    if dry_run {
        println!("Dry run \u{2014} no changes applied.");
        println!(
            "Would update GITHUB_TOKEN on {} definition(s).",
            matched.len()
        );
        return Ok(());
    }

    println!("Updating GITHUB_TOKEN on matched definitions...");
    let mut success_count = 0;
    let mut failure_count = 0;

    for m in &matched {
        match update_pipeline_variable(
            &client,
            &ado_ctx,
            &auth,
            m.id,
            "GITHUB_TOKEN",
            &token,
        )
        .await
        {
            Ok(()) => {
                println!("  \u{2713} Updated '{}' (id={})", m.name, m.id);
                success_count += 1;
            }
            Err(e) => {
                eprintln!("  \u{2717} Failed to update '{}' (id={}): {}", m.name, m.id, e);
                failure_count += 1;
            }
        }
    }

    println!();
    println!("Done: {} updated, {} failed.", success_count, failure_count);

    if failure_count > 0 {
        anyhow::bail!("{} definition(s) failed to update", failure_count);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_ado_remote_https() {
        let url = "https://dev.azure.com/myorg/myproject/_git/myrepo";
        let ctx = parse_ado_remote(url).unwrap();
        assert_eq!(ctx.org_url, "https://dev.azure.com/myorg");
        assert_eq!(ctx.project, "myproject");
        assert_eq!(ctx.repo_name, "myrepo");
    }

    #[test]
    fn test_parse_ado_remote_https_with_git_suffix() {
        let url = "https://dev.azure.com/myorg/myproject/_git/myrepo.git";
        let ctx = parse_ado_remote(url).unwrap();
        assert_eq!(ctx.repo_name, "myrepo");
    }

    #[test]
    fn test_parse_ado_remote_ssh() {
        let url = "git@ssh.dev.azure.com:v3/myorg/myproject/myrepo";
        let ctx = parse_ado_remote(url).unwrap();
        assert_eq!(ctx.org_url, "https://dev.azure.com/myorg");
        assert_eq!(ctx.project, "myproject");
        assert_eq!(ctx.repo_name, "myrepo");
    }

    #[test]
    fn test_parse_ado_remote_legacy_visualstudio() {
        let url = "https://myorg.visualstudio.com/myproject/_git/myrepo";
        let ctx = parse_ado_remote(url).unwrap();
        assert_eq!(ctx.org_url, "https://dev.azure.com/myorg");
        assert_eq!(ctx.project, "myproject");
        assert_eq!(ctx.repo_name, "myrepo");
    }

    #[test]
    fn test_parse_ado_remote_legacy_ssh() {
        let url = "git@vs-ssh.visualstudio.com:v3/myorg/myproject/myrepo";
        let ctx = parse_ado_remote(url).unwrap();
        assert_eq!(ctx.org_url, "https://dev.azure.com/myorg");
        assert_eq!(ctx.project, "myproject");
        assert_eq!(ctx.repo_name, "myrepo");
    }

    #[test]
    fn test_parse_ado_remote_invalid() {
        assert!(parse_ado_remote("https://github.com/user/repo").is_err());
        assert!(parse_ado_remote("not-a-url").is_err());
    }

    // ==================== Fuzzy name matching ====================

    fn make_def(id: u64, name: &str) -> DefinitionSummary {
        DefinitionSummary {
            id,
            name: name.to_string(),
            process: None,
        }
    }

    fn make_def_with_yaml(id: u64, name: &str, yaml_filename: &str) -> DefinitionSummary {
        DefinitionSummary {
            id,
            name: name.to_string(),
            process: Some(ProcessInfo {
                yaml_filename: Some(yaml_filename.to_string()),
            }),
        }
    }

    // ==================== YAML path matching ====================

    #[test]
    fn test_yaml_path_match_strips_leading_slash() {
        // ADO stores yamlFilename with a leading '/'
        assert_eq!(
            normalize_ado_yaml_path("/.azdo/pipelines/agent.yml"),
            ".azdo/pipelines/agent.yml"
        );
    }

    #[test]
    fn test_yaml_path_match_without_leading_slash() {
        // Some ADO instances may store without leading '/'
        assert_eq!(
            normalize_ado_yaml_path(".azdo/pipelines/agent.yml"),
            ".azdo/pipelines/agent.yml"
        );
    }

    #[test]
    fn test_yaml_path_match_backslash_normalization() {
        assert_eq!(
            normalize_ado_yaml_path("\\.azdo\\pipelines\\agent.yml"),
            ".azdo/pipelines/agent.yml"
        );
    }

    #[test]
    fn test_yaml_path_match_finds_definition_by_yaml_filename() {
        let defs = vec![
            make_def(1, "Unrelated Pipeline"),
            make_def_with_yaml(2, "My Agent", "/.azdo/pipelines/agent.yml"),
            make_def(3, "Another Pipeline"),
        ];
        let local_path = ".azdo/pipelines/agent.yml";
        let path_match = defs.iter().find(|d| {
            d.process
                .as_ref()
                .and_then(|p| p.yaml_filename.as_ref())
                .is_some_and(|f| normalize_ado_yaml_path(f) == local_path)
        });
        assert!(path_match.is_some());
        assert_eq!(path_match.unwrap().id, 2);
    }

    #[test]
    fn test_yaml_path_match_no_match_when_process_is_none() {
        let defs = vec![
            make_def(1, "Classic Pipeline"),
            make_def(2, "Another Classic"),
        ];
        let local_path = ".azdo/pipelines/agent.yml";
        let path_match = defs.iter().find(|d| {
            d.process
                .as_ref()
                .and_then(|p| p.yaml_filename.as_ref())
                .is_some_and(|f| normalize_ado_yaml_path(f) == local_path)
        });
        assert!(path_match.is_none());
    }

    #[test]
    fn test_fuzzy_match_single_unambiguous() {
        let defs = vec![
            make_def(1, "Daily Code Review"),
            make_def(2, "Build Pipeline"),
            make_def(3, "Release Pipeline"),
        ];
        // "daily-code-review" → hyphens become spaces → "daily code review" matches def 1
        let result = fuzzy_match_by_name("daily-code-review", &defs);
        assert_eq!(result, FuzzyMatchResult::Single(0));
    }

    #[test]
    fn test_fuzzy_match_ambiguous_multiple() {
        let defs = vec![
            make_def(1, "Build and Test"),
            make_def(2, "Build Validation"),
            make_def(3, "Release Pipeline"),
        ];
        // "build" matches both def 1 ("Build and Test") and def 2 ("Build Validation")
        let result = fuzzy_match_by_name("build", &defs);
        assert!(
            matches!(result, FuzzyMatchResult::Ambiguous(ref names) if names.len() == 2),
            "Expected Ambiguous with 2 candidates, got: {:?}",
            result
        );
    }

    #[test]
    fn test_fuzzy_match_no_match() {
        let defs = vec![
            make_def(1, "Build Pipeline"),
            make_def(2, "Release Pipeline"),
        ];
        let result = fuzzy_match_by_name("security-scanner", &defs);
        assert_eq!(result, FuzzyMatchResult::None);
    }

    #[test]
    fn test_fuzzy_match_empty_agent_name() {
        let defs = vec![make_def(1, "Build Pipeline")];
        let result = fuzzy_match_by_name("", &defs);
        assert_eq!(result, FuzzyMatchResult::None);
    }

    #[test]
    fn test_fuzzy_match_case_insensitive() {
        let defs = vec![
            make_def(1, "CODE REVIEW Agent"),
            make_def(2, "Deploy Pipeline"),
        ];
        let result = fuzzy_match_by_name("code-review", &defs);
        assert_eq!(result, FuzzyMatchResult::Single(0));
    }

    // ==================== MatchMethod display ====================

    #[test]
    fn test_match_method_explicit_display() {
        assert_eq!(format!("{}", MatchMethod::Explicit), "explicit");
    }

    #[test]
    fn test_match_method_all_variants_display() {
        assert_eq!(format!("{}", MatchMethod::YamlPath), "yaml-path");
        assert_eq!(format!("{}", MatchMethod::PipelineName), "pipeline-name");
        assert_eq!(format!("{}", MatchMethod::Explicit), "explicit");
    }
}
