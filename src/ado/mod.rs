//! Shared Azure DevOps REST helpers.
//!
//! Lifted from `src/configure.rs` so that every ADO-touching command
//! (`configure`/`secrets`, `enable`, `disable`, `remove`, `list`, `run`,
//! `status`, …) can draw from a single well instead of `pub use`-ing each
//! other's internals.
//!
//! Uses the same authentication patterns as the existing tools in
//! `src/tools/` (reqwest + `.basic_auth("", Some(token))` for PAT auth,
//! `.bearer_auth(token)` for AAD).

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
pub async fn try_azure_cli_token() -> Result<String> {
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
pub async fn get_git_remote_url(repo_path: &Path) -> Result<String> {
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
pub enum AdoAuth {
    Pat(String),
    Bearer(String),
}

impl AdoAuth {
    pub fn apply(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match self {
            AdoAuth::Pat(pat) => request.basic_auth("", Some(pat)),
            AdoAuth::Bearer(token) => request.bearer_auth(token),
        }
    }
}

/// Minimal subset of an ADO Build Definition for listing.
#[derive(Debug, Deserialize)]
pub struct DefinitionListResponse {
    pub value: Vec<DefinitionSummary>,
}

#[derive(Debug, Deserialize)]
pub struct DefinitionSummary {
    pub id: u64,
    pub name: String,
    pub process: Option<ProcessInfo>,
    /// `enabled`, `disabled`, or `paused`. Populated when `list_definitions`
    /// is called with `includeAllProperties=true` (the default in
    /// [`list_definitions`]). Older/cached responses may omit it.
    #[serde(rename = "queueStatus")]
    pub queue_status: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ProcessInfo {
    #[serde(rename = "yamlFilename")]
    pub yaml_filename: Option<String>,
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
pub struct MatchedDefinition {
    pub id: u64,
    pub name: String,
    pub match_method: MatchMethod,
    pub yaml_path: String,
}

/// List all build definitions in the project, handling pagination.
pub async fn list_definitions(
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
pub enum FuzzyMatchResult {
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
pub fn fuzzy_match_by_name(agent_name: &str, definitions: &[DefinitionSummary]) -> FuzzyMatchResult {
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
pub fn normalize_ado_yaml_path(path: &str) -> String {
    path.replace('\\', "/").trim_start_matches('/').to_string()
}

/// Match detected pipeline YAML files to ADO pipeline definitions.
///
/// Strategy:
/// 1. Try to match by the `yamlFilename` field in the definition's process config
/// 2. Fall back to matching by pipeline name containing the agent name
pub async fn match_definitions(
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
pub async fn get_definition_name(
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

    let resp = match auth.apply(client.get(&url)).send().await {
        Ok(r) => r,
        Err(e) => {
            debug!("Failed to fetch name for definition {}: {:?}", definition_id, e);
            return None;
        }
    };

    if !resp.status().is_success() {
        debug!(
            "Failed to fetch name for definition {}: HTTP {}",
            definition_id,
            resp.status()
        );
        return None;
    }

    let body: serde_json::Value = match resp.json().await {
        Ok(b) => b,
        Err(e) => {
            debug!("Failed to parse response for definition {}: {:?}", definition_id, e);
            return None;
        }
    };

    body.get("name")
        .and_then(|n| n.as_str())
        .map(|s| s.to_string())
}

/// Update a pipeline variable on a definition. The variable is marked
/// `isSecret: true` so values are stored encrypted in ADO.
///
/// Note: The GET→PUT cycle is not atomic. Concurrent callers against
/// the same definition could overwrite each other's variables. This is
/// acceptable for a CLI tool typically run by a single operator.
pub async fn update_pipeline_variable(
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

/// Resolves ADO authentication: PAT flag > Azure CLI > interactive prompt.
pub async fn resolve_auth(pat: Option<&str>) -> Result<AdoAuth> {
    match pat {
        Some(p) => {
            info!("Using PAT from --pat flag or AZURE_DEVOPS_EXT_PAT env var");
            Ok(AdoAuth::Pat(p.to_string()))
        }
        None => {
            info!("No PAT provided, trying Azure CLI authentication...");
            match try_azure_cli_token().await {
                Ok(token) => {
                    println!("Using Azure CLI authentication (az account get-access-token)");
                    Ok(AdoAuth::Bearer(token))
                }
                Err(e) => {
                    warn!("Azure CLI auth failed: {:#}. Falling back to interactive prompt.", e);
                    let pat = inquire::Password::new("Enter your Azure DevOps PAT:")
                        .without_confirmation()
                        .prompt()
                        .context("Failed to read PAT from interactive prompt. Set AZURE_DEVOPS_EXT_PAT env var, log in with 'az login', or use --pat flag.")?;
                    Ok(AdoAuth::Pat(pat))
                }
            }
        }
    }
}

/// Normalize a `--org` value to a full ADO organization URL.
///
/// Users commonly pass just the org name (e.g. `myorg`) instead of the full
/// URL (`https://dev.azure.com/myorg`). Accept both forms by prefixing the
/// canonical `https://dev.azure.com/` host when the input has no scheme.
///
/// Also accepts the legacy `{org}.visualstudio.com` form and rewrites it to
/// the modern `dev.azure.com/{org}` form for consistency with `parse_ado_remote`.
///
/// Inputs that contain a dot but no scheme (for example `my-corp.com`) are
/// treated as already-normalized and returned unchanged. This preserves
/// historical behavior and avoids guessing how to interpret ambiguous values.
pub fn normalize_org_url(org: &str) -> String {
    let trimmed = org.trim().trim_end_matches('/');

    // Bare org name: no scheme, no dots — assume it's just the org.
    if !trimmed.contains("://") && !trimmed.contains('/') && !trimmed.contains('.') {
        return format!("https://dev.azure.com/{}", trimmed);
    }

    // Legacy `https://{org}.visualstudio.com` → `https://dev.azure.com/{org}`.
    if let Ok(url) = url::Url::parse(trimmed)
        && let Some(host) = url.host_str()
        && let Some(org) = host.strip_suffix(".visualstudio.com")
    {
        return format!("https://dev.azure.com/{}", org);
    }

    trimmed.to_string()
}

/// Resolves the ADO context from the git remote (best-effort) with CLI overrides.
/// Falls back to explicit `--org`/`--project` when the remote is absent or non-ADO.
pub async fn resolve_ado_context(
    repo_path: &Path,
    org: Option<&str>,
    project: Option<&str>,
) -> Result<AdoContext> {
    let remote_ctx = get_git_remote_url(repo_path)
        .await
        .ok()
        .and_then(|url| {
            info!("Git remote: {}", url);
            match parse_ado_remote(&url) {
                Ok(ctx) => Some(ctx),
                Err(e) => {
                    debug!("Git remote is not an ADO URL: {:#}", e);
                    None
                }
            }
        });

    match (remote_ctx, org, project) {
        // Git remote parsed — apply overrides
        (Some(mut ctx), org, project) => {
            if let Some(org) = org {
                ctx.org_url = normalize_org_url(org);
            }
            if let Some(project) = project {
                ctx.project = project.to_string();
            }
            Ok(ctx)
        }
        // No usable remote — require explicit --org and --project
        (None, Some(org), Some(project)) => {
            info!("No ADO git remote; using --org and --project");
            Ok(AdoContext {
                org_url: normalize_org_url(org),
                project: project.to_string(),
                repo_name: String::new(),
            })
        }
        (None, _, _) => {
            anyhow::bail!(
                "Could not determine ADO context: no ADO git remote found and --org/--project not both provided.\n\
                 When using --definition-ids outside an ADO repo, both --org and --project are required."
            );
        }
    }
}

/// Builds the list of definitions to update from explicit IDs or auto-detection.
/// Returns `None` when auto-detection finds no agentic pipelines (caller should exit cleanly).
pub async fn resolve_definitions(
    client: &reqwest::Client,
    ado_ctx: &AdoContext,
    auth: &AdoAuth,
    definition_ids: Option<&[u64]>,
    repo_path: &Path,
) -> Result<Option<Vec<MatchedDefinition>>> {
    if let Some(ids) = definition_ids {
        println!("Using explicit definition IDs: {:?}", ids);
        let mut matched = Vec::new();
        for &id in ids {
            let name = get_definition_name(client, ado_ctx, auth, id)
                .await
                .unwrap_or_else(|| format!("definition {}", id));
            matched.push(MatchedDefinition {
                id,
                name,
                match_method: MatchMethod::Explicit,
                yaml_path: String::new(),
            });
        }
        return Ok(Some(matched));
    }

    // Auto-detect: scan local repo and match to ADO definitions
    println!("Scanning for agentic pipelines...");
    let detected = detect::detect_pipelines(repo_path).await?;

    if detected.is_empty() {
        println!(
            "No agentic pipelines found. Make sure your pipelines were compiled with the latest ado-aw."
        );
        return Ok(None);
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
    Ok(Some(
        match_definitions(client, ado_ctx, auth, &detected).await?,
    ))
}

// ==================== Stubs for forthcoming lifecycle commands ====================
//
// These are signature placeholders filled in by PRs 2–8 of the Phase 1 CLI
// overhaul. Locking the surface here lets the parallel command PRs depend on
// stable function signatures from day one.

/// Characters that must be percent-encoded when used in a URL path
/// segment. Built from RFC 3986 §3.3: `pchar` allows unreserved
/// characters (`A-Z`, `a-z`, `0-9`, `-`, `_`, `.`, `~`),
/// percent-encoded triplets, sub-delims, and `:` / `@`. We additionally
/// encode `:`, `@`, `%`, and `/` so a repository name containing any
/// of those does not break out of the segment, and the U+0021 (`!`)
/// just for symmetry with common path-encoding tables. Notably this
/// preserves `-`, `_`, `.`, `~` which `NON_ALPHANUMERIC` would over-
/// encode (e.g. `my-repo` → `my%2Drepo`).
const PATH_SEGMENT: &percent_encoding::AsciiSet = &percent_encoding::CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'`')
    .add(b'{')
    .add(b'}')
    .add(b'/')
    .add(b'%')
    .add(b'@')
    .add(b':')
    .add(b'!');

/// Look up an ADO Git repository's GUID by name.
///
/// Calls `GET /_apis/git/repositories/{repoName}?api-version=7.1` and reads
/// the `id` field. Required for `create_definition`, which needs a
/// `repository.id` (not just a name) on the POST body.
pub async fn get_repository_id(
    client: &reqwest::Client,
    ctx: &AdoContext,
    auth: &AdoAuth,
    repo_name: &str,
) -> Result<String> {
    let url = format!(
        "{}/{}/_apis/git/repositories/{}?api-version=7.1",
        ctx.org_url.trim_end_matches('/'),
        percent_encoding::utf8_percent_encode(&ctx.project, PATH_SEGMENT),
        percent_encoding::utf8_percent_encode(repo_name, PATH_SEGMENT),
    );

    debug!("Looking up repository '{}': {}", repo_name, url);

    let resp = auth
        .apply(client.get(&url))
        .send()
        .await
        .with_context(|| format!("Failed to look up repository '{}'", repo_name))?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!(
            "ADO API returned {} when looking up repository '{}': {}",
            status,
            repo_name,
            body
        );
    }

    let body: serde_json::Value = resp
        .json()
        .await
        .with_context(|| format!("Failed to parse repository response for '{}'", repo_name))?;

    body.get("id")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .with_context(|| format!("Repository '{}' response has no 'id' field", repo_name))
}

/// Fetch the full JSON body of a build definition.
///
/// Calls `GET /_apis/build/definitions/{id}?api-version=7.1` and returns
/// the raw `serde_json::Value` so callers can mutate specific fields and
/// PUT the result back (the standard GET → mutate → PUT cycle).
pub async fn get_definition_full(
    client: &reqwest::Client,
    ctx: &AdoContext,
    auth: &AdoAuth,
    id: u64,
) -> Result<serde_json::Value> {
    let url = format!(
        "{}/{}/_apis/build/definitions/{}?api-version=7.1",
        ctx.org_url.trim_end_matches('/'),
        percent_encoding::utf8_percent_encode(&ctx.project, PATH_SEGMENT),
        id
    );

    let resp = auth
        .apply(client.get(&url))
        .send()
        .await
        .with_context(|| format!("Failed to fetch definition {}", id))?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!(
            "ADO API returned {} when fetching definition {}: {}",
            status,
            id,
            body
        );
    }

    let body = resp
        .text()
        .await
        .with_context(|| format!("Failed to read definition {} response body", id))?;

    serde_json::from_str(&body).with_context(|| {
        let snippet: String = body.chars().take(500).collect();
        format!(
            "Failed to parse definition {} as JSON. \
             This usually means the PAT is invalid or expired. \
             Response body (first 500 chars):\n{snippet}",
            id
        )
    })
}

/// PATCH the `queueStatus` field on a build definition.
///
/// `status` must be one of `"enabled"`, `"disabled"`, or `"paused"`.
/// Implements the GET → mutate → PUT cycle internally; the full definition
/// is round-tripped to satisfy the PUT API's "you must send the whole
/// document" requirement.
pub async fn patch_queue_status(
    client: &reqwest::Client,
    ctx: &AdoContext,
    auth: &AdoAuth,
    id: u64,
    status: &str,
) -> Result<()> {
    match status {
        "enabled" | "disabled" | "paused" => {}
        other => anyhow::bail!(
            "patch_queue_status: invalid status '{}', expected one of enabled/disabled/paused",
            other
        ),
    }

    let mut definition = get_definition_full(client, ctx, auth, id)
        .await
        .with_context(|| format!("Failed to fetch definition {} before patching", id))?;

    definition["queueStatus"] = serde_json::Value::String(status.to_string());

    let put_url = format!(
        "{}/{}/_apis/build/definitions/{}?api-version=7.1",
        ctx.org_url.trim_end_matches('/'),
        percent_encoding::utf8_percent_encode(&ctx.project, PATH_SEGMENT),
        id
    );

    debug!("PUT definition {} with queueStatus={}: {}", id, status, put_url);

    let resp = auth
        .apply(client.put(&put_url))
        .header("Content-Type", "application/json")
        .json(&definition)
        .send()
        .await
        .with_context(|| format!("Failed to update queueStatus on definition {}", id))?;

    let resp_status = resp.status();
    if !resp_status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!(
            "ADO API returned {} when updating queueStatus on definition {}: {}",
            resp_status,
            id,
            body
        );
    }

    Ok(())
}

/// Delete a build definition.
///
/// Calls `DELETE /_apis/build/definitions/{id}?api-version=7.1`.
pub async fn delete_definition(
    client: &reqwest::Client,
    ctx: &AdoContext,
    auth: &AdoAuth,
    id: u64,
) -> Result<()> {
    let url = format!(
        "{}/{}/_apis/build/definitions/{}?api-version=7.1",
        ctx.org_url.trim_end_matches('/'),
        ctx.project,
        id
    );

    debug!("DELETE definition {}: {}", id, url);

    let resp = auth
        .apply(client.delete(&url))
        .send()
        .await
        .with_context(|| format!("Failed to delete definition {}", id))?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!(
            "ADO API returned {} when deleting definition {}: {}",
            status,
            id,
            body
        );
    }

    Ok(())
}

/// Create a new build definition.
///
/// Calls `POST /_apis/build/definitions?api-version=7.1` with the supplied
/// JSON body and returns the new definition's `id`.
pub async fn create_definition(
    client: &reqwest::Client,
    ctx: &AdoContext,
    auth: &AdoAuth,
    body: &serde_json::Value,
) -> Result<u64> {
    let url = format!(
        "{}/{}/_apis/build/definitions?api-version=7.1",
        ctx.org_url.trim_end_matches('/'),
        percent_encoding::utf8_percent_encode(&ctx.project, PATH_SEGMENT),
    );

    debug!("POST new definition: {}", url);

    let resp = auth
        .apply(client.post(&url))
        .header("Content-Type", "application/json")
        .json(body)
        .send()
        .await
        .context("Failed to create build definition")?;

    let status = resp.status();
    if !status.is_success() {
        let resp_body = resp.text().await.unwrap_or_default();
        anyhow::bail!(
            "ADO API returned {} when creating definition: {}",
            status,
            resp_body
        );
    }

    let resp_body: serde_json::Value = resp
        .json()
        .await
        .context("Failed to parse create-definition response")?;

    resp_body
        .get("id")
        .and_then(|v| v.as_u64())
        .context("create_definition response has no numeric 'id' field")
}

/// Queue a build for a definition.
///
/// Calls `POST /_apis/build/builds?api-version=7.1` and returns the queued
/// build's `id`. `branch` defaults to the definition's `defaultBranch` when
/// `None`. `parameters` are passed through as ADO `templateParameters`.
pub async fn queue_build(
    _client: &reqwest::Client,
    _ctx: &AdoContext,
    _auth: &AdoAuth,
    _definition_id: u64,
    _branch: Option<&str>,
    _parameters: &serde_json::Map<String, serde_json::Value>,
) -> Result<u64> {
    anyhow::bail!("not yet implemented: filled in by PR 6 (ado-aw run)")
}

/// Fetch the full JSON body of a build.
///
/// Calls `GET /_apis/build/builds/{id}?api-version=7.1`.
pub async fn get_build(
    _client: &reqwest::Client,
    _ctx: &AdoContext,
    _auth: &AdoAuth,
    _build_id: u64,
) -> Result<serde_json::Value> {
    anyhow::bail!("not yet implemented: filled in by PR 6 (ado-aw run)")
}

/// Fetch the most recent build for a definition.
///
/// Calls `GET /_apis/build/builds?definitions={id}&$top=1&api-version=7.1`
/// and returns the first result (or `None` if the definition has never run).
pub async fn get_latest_build(
    _client: &reqwest::Client,
    _ctx: &AdoContext,
    _auth: &AdoAuth,
    _definition_id: u64,
) -> Result<Option<serde_json::Value>> {
    anyhow::bail!("not yet implemented: filled in by PR 5 (ado-aw list) or PR 7 (ado-aw status)")
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

    // ==================== Org URL normalization ====================

    #[test]
    fn normalize_org_url_accepts_bare_name() {
        assert_eq!(
            normalize_org_url("myorg"),
            "https://dev.azure.com/myorg"
        );
    }

    #[test]
    fn normalize_org_url_preserves_full_url() {
        assert_eq!(
            normalize_org_url("https://dev.azure.com/myorg"),
            "https://dev.azure.com/myorg"
        );
    }

    #[test]
    fn normalize_org_url_strips_trailing_slash() {
        assert_eq!(
            normalize_org_url("https://dev.azure.com/myorg/"),
            "https://dev.azure.com/myorg"
        );
    }

    #[test]
    fn normalize_org_url_rewrites_legacy_visualstudio() {
        assert_eq!(
            normalize_org_url("https://myorg.visualstudio.com"),
            "https://dev.azure.com/myorg"
        );
        assert_eq!(
            normalize_org_url("https://myorg.visualstudio.com/"),
            "https://dev.azure.com/myorg"
        );
    }

    #[test]
    fn normalize_org_url_trims_whitespace() {
        assert_eq!(
            normalize_org_url("  myorg  "),
            "https://dev.azure.com/myorg"
        );
    }

    #[test]
    fn normalize_org_url_preserves_ambiguous_dotted_value() {
        assert_eq!(normalize_org_url("my-corp.com"), "my-corp.com");
    }

    // ==================== Fuzzy name matching ====================

    fn make_def(id: u64, name: &str) -> DefinitionSummary {
        DefinitionSummary {
            id,
            name: name.to_string(),
            process: None,
            queue_status: None,
        }
    }

    fn make_def_with_yaml(id: u64, name: &str, yaml_filename: &str) -> DefinitionSummary {
        DefinitionSummary {
            id,
            name: name.to_string(),
            process: Some(ProcessInfo {
                yaml_filename: Some(yaml_filename.to_string()),
            }),
            queue_status: None,
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

    // ==================== DefinitionSummary deserialization ====================

    #[test]
    fn definition_summary_deserializes_queue_status() {
        let raw = serde_json::json!({
            "id": 42,
            "name": "Daily noop",
            "queueStatus": "disabled",
            "process": { "yamlFilename": "/tests/noop.lock.yml" }
        });
        let def: DefinitionSummary = serde_json::from_value(raw).unwrap();
        assert_eq!(def.id, 42);
        assert_eq!(def.queue_status.as_deref(), Some("disabled"));
        assert_eq!(
            def.process
                .as_ref()
                .and_then(|p| p.yaml_filename.as_deref()),
            Some("/tests/noop.lock.yml")
        );
    }

    #[test]
    fn definition_summary_queue_status_missing_is_none() {
        let raw = serde_json::json!({ "id": 1, "name": "x" });
        let def: DefinitionSummary = serde_json::from_value(raw).unwrap();
        assert!(def.queue_status.is_none());
    }

    // ==================== PATH_SEGMENT percent-encoding ====================

    #[test]
    fn path_segment_preserves_rfc3986_unreserved_chars() {
        // RFC 3986 unreserved set: A-Z / a-z / 0-9 / - / _ / . / ~
        // These MUST NOT be percent-encoded in a URL path segment.
        let encoded =
            percent_encoding::utf8_percent_encode("my-repo_name.with~tilde", PATH_SEGMENT)
                .to_string();
        assert_eq!(encoded, "my-repo_name.with~tilde");
    }

    #[test]
    fn path_segment_encodes_space_and_reserved_punctuation() {
        let encoded =
            percent_encoding::utf8_percent_encode("my repo/with?special#chars", PATH_SEGMENT)
                .to_string();
        // Spaces become %20, slashes %2F, ? becomes %3F, # becomes %23.
        assert_eq!(encoded, "my%20repo%2Fwith%3Fspecial%23chars");
    }

    #[test]
    fn path_segment_handles_non_ascii() {
        let encoded =
            percent_encoding::utf8_percent_encode("café-π", PATH_SEGMENT).to_string();
        // Non-ASCII bytes get encoded per UTF-8.
        assert_eq!(encoded, "caf%C3%A9-%CF%80");
    }
}
