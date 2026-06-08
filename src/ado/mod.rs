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
use std::io::Write;
use std::path::Path;

use crate::detect;

pub mod discovery;

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
                "/C",
                "az",
                "account",
                "get-access-token",
                "--resource",
                ADO_RESOURCE_ID,
                "--query",
                "accessToken",
                "-o",
                "tsv",
            ])
            .output()
            .await
    } else {
        tokio::process::Command::new("az")
            .args([
                "account",
                "get-access-token",
                "--resource",
                ADO_RESOURCE_ID,
                "--query",
                "accessToken",
                "-o",
                "tsv",
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

impl AdoContext {
    /// Extract just the org slug from `org_url` (e.g.
    /// `https://dev.azure.com/MyOrg/` → `Some("MyOrg")`). Mirrors the
    /// inline parse in `CompileContext::ado_org`; lives here so
    /// non-compile callers (Preview-driven discovery) can reuse it.
    pub fn org_name(&self) -> Option<&str> {
        let org = self.org_url.trim_end_matches('/').rsplit('/').next()?;
        if org.is_empty() { None } else { Some(org) }
    }
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
        let url_parsed = url::Url::parse(url).with_context(|| format!("Invalid URL: {}", url))?;
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
        let url_parsed = url::Url::parse(url).with_context(|| format!("Invalid URL: {}", url))?;
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

// ==================== Source identity (RepoSource) ====================

/// Which forge a pipeline's source markdown lives on.
///
/// `AdoGit` and `Github` are the only providers supported in v1. The
/// `repository.url` form ADO stores for a build definition uses a
/// different shape per provider — see [`RepoSource::url`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepoProvider {
    /// Azure DevOps Git (TfsGit).
    AdoGit,
    /// GitHub on `github.com` (GitHub Enterprise is out of scope for
    /// v1; see `docs/cli.md` for the follow-up roadmap).
    Github,
}

/// Where the agent markdown for a compiled pipeline lives.
///
/// `RepoSource` is the **source identity** half of the two-namespace
/// model — distinct from [`AdoContext`], which carries the ADO
/// **deployment target** (`org_url`, `project`). They coincide for
/// ADO-source pipelines (both halves derived from the same git remote)
/// and diverge for GitHub-source pipelines (the source lives on
/// GitHub but the pipeline runs in some ADO project supplied via
/// `--org`/`--project`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoSource {
    /// Which forge hosts the repository.
    pub provider: RepoProvider,
    /// For `AdoGit`: the ADO organisation name (e.g. `myorg`).
    /// For `Github`: the GitHub owner (e.g. `githubnext`).
    pub owner: String,
    /// Bare repository name in both cases (e.g. `templates-a`,
    /// `ado-aw`). Never `owner/repo` — composing the slashed form is
    /// the caller's job when the API requires it (e.g. the GitHub
    /// build-definition body's `repository.name` field).
    pub repo: String,
    /// `Some("MyProject")` for `AdoGit` (ADO organisations contain
    /// projects). `None` for `Github`.
    pub project: Option<String>,
}

impl RepoSource {
    /// Return the canonical browse URL ADO surfaces in
    /// `repository.url` for a build definition backed by this source.
    ///
    /// Returned in the form expected by [`crate::ado::discovery`]'s
    /// `normalize_repo_url` (no trailing `.git` — the discovery
    /// normalizer also strips it). Not currently wired into the
    /// `CurrentRepo` URL filter for GitHub source — see the PR
    /// landing this type for the deferred-work rationale.
    ///
    /// Invariant: `provider == AdoGit` implies `project.is_some()`
    /// (every `AdoGit`-yielding `parse_git_remote` path sets it).
    /// A debug assertion guards against a future producer that
    /// forgets to populate the field — release builds fall back to
    /// an empty path segment rather than panicking, since the URL is
    /// only used for comparison and a mismatch is the right failure
    /// mode there.
    pub fn url(&self) -> String {
        match self.provider {
            RepoProvider::AdoGit => {
                let project = self.project.as_deref().unwrap_or_default();
                debug_assert!(
                    !project.is_empty(),
                    "RepoSource::url(): AdoGit invariant violated — project must be Some"
                );
                format!(
                    "https://dev.azure.com/{}/{}/_git/{}",
                    self.owner, project, self.repo,
                )
            }
            RepoProvider::Github => {
                format!("https://github.com/{}/{}", self.owner, self.repo)
            }
        }
    }
}

/// Parse a git remote URL into a [`RepoSource`].
///
/// Tries [`parse_ado_remote`] first (regression-safe for every existing
/// ADO call site), then `github.com` HTTPS / SSH. Bails with a unified
/// "could not parse as ADO Git or GitHub" message when neither matches.
///
/// Accepted GitHub forms:
/// - `https://github.com/{owner}/{repo}` (with or without `.git`)
/// - `git@github.com:{owner}/{repo}` (with or without `.git`)
///
/// GitHub Enterprise (`github.example.com`) is **not** matched in v1.
pub fn parse_git_remote(remote_url: &str) -> Result<RepoSource> {
    if let Ok(ctx) = parse_ado_remote(remote_url) {
        // Defensive: every shape `parse_ado_remote` accepts populates
        // `org_url` with a `/{org}` segment, so `org_name()` should
        // always return `Some`. Bail explicitly on the unreachable
        // path rather than silently producing an empty `owner` that
        // would surface later as a confusing ADO API error.
        let owner = ctx.org_name().ok_or_else(|| {
            anyhow::anyhow!(
                "Parsed '{}' as an ADO remote but could not extract the org segment from '{}'",
                remote_url,
                ctx.org_url
            )
        })?.to_string();
        return Ok(RepoSource {
            provider: RepoProvider::AdoGit,
            owner,
            repo: ctx.repo_name,
            project: Some(ctx.project),
        });
    }

    if let Some(source) = parse_github_remote(remote_url) {
        return Ok(source);
    }

    anyhow::bail!(
        "Could not parse '{}' as either an Azure DevOps Git remote \
         (https://dev.azure.com/{{org}}/{{project}}/_git/{{repo}}) or a \
         GitHub remote (https://github.com/{{owner}}/{{repo}} or \
         git@github.com:{{owner}}/{{repo}})",
        remote_url
    )
}

/// Try to parse `remote_url` as a `github.com` HTTPS or SSH URL.
///
/// Returns `None` on any other host (including GitHub Enterprise) so
/// the caller can fall through to its own error path.
fn parse_github_remote(remote_url: &str) -> Option<RepoSource> {
    let url = remote_url.trim();

    // SSH: git@github.com:owner/repo(.git)
    if let Some(rest) = url.strip_prefix("git@github.com:") {
        let (owner, repo) = split_owner_repo(rest)?;
        return Some(RepoSource {
            provider: RepoProvider::Github,
            owner,
            repo,
            project: None,
        });
    }

    // HTTPS: https://github.com/owner/repo(.git)
    let parsed = url::Url::parse(url).ok()?;
    if parsed.host_str() != Some("github.com") {
        return None;
    }
    let path = parsed.path().trim_start_matches('/');
    let (owner, repo) = split_owner_repo(path)?;
    Some(RepoSource {
        provider: RepoProvider::Github,
        owner,
        repo,
        project: None,
    })
}

/// Split a `owner/repo[.git][/...]` fragment, trimming the `.git`
/// suffix on the repo half. Returns `None` if either half is empty.
fn split_owner_repo(path: &str) -> Option<(String, String)> {
    let mut parts = path.splitn(3, '/');
    let owner = parts.next()?.trim();
    let repo_raw = parts.next()?.trim();
    if owner.is_empty() || repo_raw.is_empty() {
        return None;
    }
    let repo = repo_raw.trim_end_matches(".git");
    if repo.is_empty() {
        return None;
    }
    Some((owner.to_string(), repo.to_string()))
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
    /// ADO folder path (e.g. `\smoke`, `\`). Populated when
    /// `includeAllProperties=true`. May be absent on older API versions.
    #[serde(default)]
    pub path: Option<String>,
    /// Backing git repository (URL, name, type, id). Populated by ADO's
    /// list endpoint without any extra query parameters. Used by
    /// project-scope discovery to filter definitions by the current
    /// git remote (`DiscoveryScope::CurrentRepo`).
    #[serde(default)]
    pub repository: Option<Repository>,
    /// Monotonic revision counter ADO bumps on every definition edit.
    /// Deserialised here so a future Preview-driven discovery cache
    /// can key on `(definition_id, revision)`. **No caching is
    /// implemented yet** — see the discovery module for the current
    /// in-process behaviour. Track in a follow-up before depending on
    /// this for staleness checks.
    #[serde(default)]
    pub revision: Option<u64>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Repository {
    /// Browse URL of the backing repo (e.g.
    /// `https://dev.azure.com/{org}/{project}/_git/{repo}`). Used for
    /// `DiscoveryScope::CurrentRepo` filtering.
    #[serde(default)]
    pub url: Option<String>,
    /// Human-readable repo name.
    #[serde(default)]
    pub name: Option<String>,
    /// Repository provider (e.g. `"TfsGit"`, `"GitHub"`).
    #[serde(rename = "type", default)]
    pub repo_type: Option<String>,
    /// Backing repository ID (GUID for TfsGit, owner/repo for GitHub).
    #[serde(default)]
    pub id: Option<String>,
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
    /// Found via Preview-driven discovery (Workstream P). Uniquely
    /// identifies definitions that ADO knows about but where the
    /// caller has no corresponding local lock file — i.e. consumer
    /// pipelines and ado-aw definitions in other repos.
    Discovery,
}

impl std::fmt::Display for MatchMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MatchMethod::YamlPath => write!(f, "yaml-path"),
            MatchMethod::PipelineName => write!(f, "pipeline-name"),
            MatchMethod::Explicit => write!(f, "explicit"),
            MatchMethod::Discovery => write!(f, "discovery"),
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
    /// `enabled`, `disabled`, `paused`, or `None` when the matcher
    /// couldn't read the field (explicit-ID matches, older API
    /// responses). Populated from `DefinitionSummary::queue_status`
    /// when available, so command-level decision logic can skip
    /// already-at-target definitions without an extra HTTP round-trip.
    pub queue_status: Option<String>,
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
            percent_encoding::utf8_percent_encode(&ctx.project, PATH_SEGMENT)
        );

        debug!("Listing definitions: {}", base_url);

        let mut request = auth
            .apply(client.get(&base_url))
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

        let body = resp
            .text()
            .await
            .context("Failed to read definitions response body")?;
        let response: DefinitionListResponse = serde_json::from_str(&body).with_context(|| {
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
pub fn fuzzy_match_by_name(
    agent_name: &str,
    definitions: &[DefinitionSummary],
) -> FuzzyMatchResult {
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
pub fn match_definitions_in(
    definitions: &[DefinitionSummary],
    detected: &[detect::DetectedPipeline],
) -> Vec<MatchedDefinition> {
    let mut matched = Vec::new();

    // Log all definition yaml paths for debugging
    for def in definitions {
        let yaml_path = def
            .process
            .as_ref()
            .and_then(|p| p.yaml_filename.as_ref())
            .map(|f| normalize_ado_yaml_path(f));
        debug!(
            "ADO definition: '{}' (id={}) yamlFilename={:?} normalized={:?}",
            def.name,
            def.id,
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
                queue_status: def.queue_status.clone(),
            });
            continue;
        }

        // Strategy 2: Fall back to matching by pipeline name.
        // Only accept unambiguous matches — if multiple definitions match, skip.
        let agent_name = Path::new(&pipeline.source)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("");

        match fuzzy_match_by_name(agent_name, definitions) {
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
                    queue_status: def.queue_status.clone(),
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

    matched
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

    Ok(match_definitions_in(&definitions, detected))
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
        percent_encoding::utf8_percent_encode(&ctx.project, PATH_SEGMENT),
        definition_id
    );

    let resp = match auth.apply(client.get(&url)).send().await {
        Ok(r) => r,
        Err(e) => {
            debug!(
                "Failed to fetch name for definition {}: {:?}",
                definition_id, e
            );
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
            debug!(
                "Failed to parse response for definition {}: {:?}",
                definition_id, e
            );
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
///
/// ADO returns existing secret variables from definition GETs as masked
/// `***`. For definition PUTs, `null` preserves the stored secret value,
/// while the literal mask would overwrite it. Normalize the masked form
/// before mutating the definition and PUTting it back.
pub(crate) fn normalize_masked_secret_variable_values(definition: &mut serde_json::Value) {
    let Some(vars) = definition
        .get_mut("variables")
        .and_then(|v| v.as_object_mut())
    else {
        return;
    };

    for var in vars.values_mut() {
        let is_masked_secret = var.get("isSecret").and_then(|v| v.as_bool()) == Some(true)
            && var.get("value").and_then(|v| v.as_str()) == Some("***");
        if !is_masked_secret {
            continue;
        }

        if let Some(obj) = var.as_object_mut() {
            obj.insert("value".to_string(), serde_json::Value::Null);
        }
    }
}

pub async fn update_pipeline_variable(
    client: &reqwest::Client,
    ctx: &AdoContext,
    auth: &AdoAuth,
    definition_id: u64,
    variable_name: &str,
    variable_value: &str,
) -> Result<()> {
    let mut definition = get_definition_full(client, ctx, auth, definition_id)
        .await
        .with_context(|| {
            format!(
                "Failed to fetch definition {} before updating",
                definition_id
            )
        })?;
    normalize_masked_secret_variable_values(&mut definition);

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
        percent_encoding::utf8_percent_encode(&ctx.project, PATH_SEGMENT),
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
                    warn!(
                        "Azure CLI auth failed: {:#}. Falling back to interactive prompt.",
                        e
                    );
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
    let remote_ctx = get_git_remote_url(repo_path).await.ok().and_then(|url| {
        info!("Git remote: {}", url);
        match parse_ado_remote(&url) {
            Ok(ctx) => Some(ctx),
            Err(e) => {
                debug!("Git remote is not an ADO URL: {:#}", e);
                None
            }
        }
    });

    let mut ctx = match (remote_ctx, org, project) {
        // Git remote parsed — apply overrides
        (Some(mut ctx), org, project) => {
            if let Some(org) = org {
                ctx.org_url = normalize_org_url(org);
            }
            if let Some(project) = project {
                ctx.project = project.to_string();
            }
            ctx
        }
        // No usable remote — require explicit --org and --project
        (None, Some(org), Some(project)) => {
            info!("No ADO git remote; using --org and --project");
            AdoContext {
                org_url: normalize_org_url(org),
                project: project.to_string(),
                repo_name: String::new(),
            }
        }
        (None, _, _) => {
            anyhow::bail!(
                "Could not determine ADO context: no ADO git remote found and --org/--project not both provided.\n\
                 When using --definition-ids outside an ADO repo, both --org and --project are required."
            );
        }
    };

    apply_test_org_url_override(&mut ctx);
    Ok(ctx)
}

/// Test-only override that lets the integration tests in `tests/audit_it.rs`
/// redirect ADO REST calls at a mock server via the `ADO_AW_TEST_ORG_URL`
/// environment variable.
///
/// **Compiled out of release builds.** All published artifacts ship with
/// `cargo build --release`, which sets `debug_assertions = false` and
/// replaces the body of this function with a no-op via the
/// `#[cfg(not(debug_assertions))]` branch below. This prevents an
/// attacker-controlled env var (a leftover from a debugging session, a
/// hostile CI environment, etc.) from silently redirecting production
/// ADO API calls. Debug builds — used by `cargo test`, integration
/// tests, and `cargo run` during development — keep the override
/// available, and emit a `warn!` on every invocation so the override is
/// loud and obvious in logs.
#[cfg(debug_assertions)]
fn apply_test_org_url_override(ctx: &mut AdoContext) {
    if let Ok(org_url) = std::env::var("ADO_AW_TEST_ORG_URL") {
        let org_url = org_url.trim().trim_end_matches('/');
        if !org_url.is_empty() {
            log::warn!(
                "ADO_AW_TEST_ORG_URL test override active: redirecting ADO REST calls \
                 from {} to {} (this branch is compiled out of release builds)",
                ctx.org_url,
                org_url
            );
            ctx.org_url = org_url.to_string();
        }
    }
}

#[cfg(not(debug_assertions))]
fn apply_test_org_url_override(_: &mut AdoContext) {
    // Release builds intentionally ignore ADO_AW_TEST_ORG_URL so that a
    // stray env var cannot redirect production ADO API calls.
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
                queue_status: None,
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
pub const PATH_SEGMENT: &percent_encoding::AsciiSet = &percent_encoding::CONTROLS
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

/// Characters that must be percent-encoded when used as a URL
/// **query-string value** (the bit after `?key=` and before `&` /
/// `#`). Preserves the RFC 3986 unreserved set (`A-Z`, `a-z`, `0-9`,
/// `-`, `_`, `.`, `~`) so common identifiers like `ado-aw-github` are
/// emitted literally rather than as `ado%2Daw%2Dgithub` — strictly
/// correct ADO/RFC behaviour either way, but the unencoded form is
/// what humans read in debug logs and what most servers' strict
/// matchers prefer. Encodes:
///
/// - query-syntax metachars (`&`, `=`, `#`, `?`) that would split
///   the parameter,
/// - `%` (escape char) and `+` (form-encoding space alias),
/// - whitespace and structural ASCII (`/`, `:`, `@`, `<`, `>`, `"`,
///   `'`, `{`, `}`, `` ` ``).
///
/// Stricter than `NON_ALPHANUMERIC` (which would encode `-`/`_`/`.`/`~`)
/// and weaker than `PATH_SEGMENT` (which does *not* encode `&`/`=`).
pub const QUERY_VALUE: &percent_encoding::AsciiSet = &percent_encoding::CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'\'')
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
    .add(b'&')
    .add(b'=')
    .add(b'+');

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

/// Resolve a GitHub service-connection identifier to its GUID.
///
/// Accepts either a raw UUID (returned unchanged — no API call) or a
/// human-readable endpoint name (e.g. `ado-aw-github`) which is
/// resolved via the project-scoped service-endpoint API.
///
/// Used by `ado-aw enable` when registering GitHub-source build
/// definitions: the ADO `repository.properties.connectedServiceId`
/// field requires a GUID, but operators prefer typing the friendly
/// name they set up in the portal.
///
/// Returns a useful error on 0-match (the connection doesn't exist in
/// this project) or >1-match (rare, but possible if two endpoints
/// happen to share a display name; operator must pass the GUID
/// directly to disambiguate).
pub async fn resolve_service_connection_id(
    client: &reqwest::Client,
    ctx: &AdoContext,
    auth: &AdoAuth,
    value: &str,
) -> Result<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        anyhow::bail!("--service-connection value is empty");
    }
    if is_uuid_like(trimmed) {
        return Ok(trimmed.to_string());
    }

    let url = format!(
        "{}/{}/_apis/serviceendpoint/endpoints?type=github&endpointNames={}&api-version=7.1-preview.4",
        ctx.org_url.trim_end_matches('/'),
        percent_encoding::utf8_percent_encode(&ctx.project, PATH_SEGMENT),
        // The endpoint name is a query-string value (not a path
        // segment), so `&` and `=` must be percent-encoded or they'd
        // split the parameter. Use `QUERY_VALUE` rather than
        // `PATH_SEGMENT` (leaves `&`/`=` unencoded) or
        // `NON_ALPHANUMERIC` (over-encodes `-`/`_`/`.`/`~`); the
        // common `ado-aw-github` style emits literally.
        percent_encoding::utf8_percent_encode(trimmed, QUERY_VALUE),
    );

    debug!("Looking up GitHub service connection '{}': {}", trimmed, url);

    let resp = auth
        .apply(client.get(&url))
        .send()
        .await
        .with_context(|| {
            format!(
                "Failed to look up GitHub service connection '{}' in project '{}'",
                trimmed, ctx.project
            )
        })?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!(
            "ADO API returned {} when looking up service connection '{}': {}",
            status,
            trimmed,
            body
        );
    }

    let body: serde_json::Value = resp.json().await.with_context(|| {
        format!("Failed to parse service-endpoint response for '{}'", trimmed)
    })?;

    pick_service_endpoint_id(&body, trimmed, &ctx.project)
}

/// Returns `true` when `value` looks like a UUID
/// (`xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx`, hex only). Case-insensitive.
///
/// The check is intentionally lenient — ADO accepts any GUID, so we
/// only need to recognise the canonical hyphenated form well enough to
/// skip the resolution API call. A false negative (resolver runs even
/// though `value` was a GUID) is harmless; the API returns the same
/// id back.
fn is_uuid_like(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 36 {
        return false;
    }
    for (i, b) in bytes.iter().enumerate() {
        let is_hyphen_position = matches!(i, 8 | 13 | 18 | 23);
        if is_hyphen_position {
            if *b != b'-' {
                return false;
            }
        } else if !b.is_ascii_hexdigit() {
            return false;
        }
    }
    true
}

/// Pick the single service-endpoint `id` from the
/// `/_apis/serviceendpoint/endpoints` list response.
///
/// Factored out for unit-testability: the JSON shape is stable
/// (`{ count, value: [ { id, name, … } ] }`) and the 0 / 1 / >1 result
/// triage logic is the interesting part.
fn pick_service_endpoint_id(
    body: &serde_json::Value,
    name: &str,
    project: &str,
) -> Result<String> {
    let entries = body
        .get("value")
        .and_then(|v| v.as_array())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "service-endpoint response missing `value` array (got: {})",
                body
            )
        })?;

    match entries.len() {
        0 => anyhow::bail!(
            "No GitHub service connection named '{}' was found in project '{}'. \
             Create one under Project settings → Service connections → GitHub, then re-run.",
            name,
            project
        ),
        1 => entries[0]
            .get("id")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .ok_or_else(|| {
                anyhow::anyhow!("Service connection '{}' has no `id` field", name)
            }),
        n => anyhow::bail!(
            "{} GitHub service connections named '{}' in project '{}'. \
             Pass the GUID directly to --service-connection to disambiguate.",
            n,
            name,
            project
        ),
    }
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
    normalize_masked_secret_variable_values(&mut definition);

    definition["queueStatus"] = serde_json::Value::String(status.to_string());

    let put_url = format!(
        "{}/{}/_apis/build/definitions/{}?api-version=7.1",
        ctx.org_url.trim_end_matches('/'),
        percent_encoding::utf8_percent_encode(&ctx.project, PATH_SEGMENT),
        id
    );

    debug!(
        "PUT definition {} with queueStatus={}: {}",
        id, status, put_url
    );

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
        percent_encoding::utf8_percent_encode(&ctx.project, PATH_SEGMENT),
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
    client: &reqwest::Client,
    ctx: &AdoContext,
    auth: &AdoAuth,
    definition_id: u64,
    branch: Option<&str>,
    parameters: &serde_json::Map<String, serde_json::Value>,
) -> Result<u64> {
    let url = format!(
        "{}/{}/_apis/build/builds?api-version=7.1",
        ctx.org_url.trim_end_matches('/'),
        percent_encoding::utf8_percent_encode(&ctx.project, PATH_SEGMENT),
    );

    let mut body = serde_json::json!({
        "definition": { "id": definition_id }
    });
    if let Some(b) = branch {
        body["sourceBranch"] = serde_json::Value::String(b.to_string());
    }
    if !parameters.is_empty() {
        body["templateParameters"] = serde_json::Value::Object(parameters.clone());
    }

    debug!("POST queue build for definition {}: {}", definition_id, url);

    let resp = auth
        .apply(client.post(&url))
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .with_context(|| format!("Failed to queue build for definition {}", definition_id))?;

    let status = resp.status();
    if !status.is_success() {
        let resp_body = resp.text().await.unwrap_or_default();
        anyhow::bail!(
            "ADO API returned {} when queuing build for definition {}: {}",
            status,
            definition_id,
            resp_body
        );
    }

    let resp_body: serde_json::Value = resp
        .json()
        .await
        .context("Failed to parse queue-build response")?;

    resp_body
        .get("id")
        .and_then(|v| v.as_u64())
        .context("queue_build response has no numeric 'id' field")
}

/// Fetch the full JSON body of a build.
///
/// Calls `GET /_apis/build/builds/{id}?api-version=7.1`.
pub async fn get_build(
    client: &reqwest::Client,
    ctx: &AdoContext,
    auth: &AdoAuth,
    build_id: u64,
) -> Result<serde_json::Value> {
    let url = format!(
        "{}/{}/_apis/build/builds/{}?api-version=7.1",
        ctx.org_url.trim_end_matches('/'),
        percent_encoding::utf8_percent_encode(&ctx.project, PATH_SEGMENT),
        build_id
    );

    debug!("GET build {}: {}", build_id, url);

    let resp = auth
        .apply(client.get(&url))
        .send()
        .await
        .with_context(|| format!("Failed to fetch build {}", build_id))?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!(
            "ADO API returned {} when fetching build {}: {}",
            status,
            build_id,
            body
        );
    }

    resp.json()
        .await
        .with_context(|| format!("Failed to parse build {} response", build_id))
}

/// Fetch the most recent build for a definition.
///
/// Calls `GET /_apis/build/builds?definitions={id}&$top=1&api-version=7.1`
/// and returns the first result (or `None` if the definition has never run).
pub async fn get_latest_build(
    client: &reqwest::Client,
    ctx: &AdoContext,
    auth: &AdoAuth,
    definition_id: u64,
) -> Result<Option<serde_json::Value>> {
    let url = format!(
        "{}/{}/_apis/build/builds?definitions={}&$top=1&api-version=7.1",
        ctx.org_url.trim_end_matches('/'),
        percent_encoding::utf8_percent_encode(&ctx.project, PATH_SEGMENT),
        definition_id,
    );

    debug!("GET latest build for definition {}: {}", definition_id, url);

    let resp = auth.apply(client.get(&url)).send().await.with_context(|| {
        format!(
            "Failed to fetch latest build for definition {}",
            definition_id
        )
    })?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!(
            "ADO API returned {} when fetching latest build for definition {}: {}",
            status,
            definition_id,
            body
        );
    }

    let body: serde_json::Value = resp
        .json()
        .await
        .with_context(|| format!("Failed to parse builds response for {}", definition_id))?;

    Ok(body
        .get("value")
        .and_then(|v| v.as_array())
        .and_then(|a| a.first())
        .cloned())
}

/// A single build artifact returned by the ADO REST API.
///
/// Shape comes from `GET _apis/build/builds/{buildId}/artifacts`.
/// We surface only the fields the audit consumes; unknown fields are
/// dropped on deserialization.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildArtifact {
    pub id: u64,
    pub name: String,
    pub source: Option<String>,
    pub resource: BuildArtifactResource,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildArtifactResource {
    /// "PipelineArtifact" for `- publish:` steps, "Container" for legacy.
    #[serde(rename = "type")]
    pub kind: String,
    pub data: Option<String>,
    pub properties: Option<serde_json::Value>,
    pub url: Option<String>,
    pub download_url: Option<String>,
}

/// List all artifacts published by a build.
///
/// Calls `GET /_apis/build/builds/{buildId}/artifacts?api-version=7.1`.
/// Returns the full `value` array — callers filter by `name` themselves.
///
/// Returns an empty vec when the build has not published any artifacts
/// (HTTP 200 with `value: []`) — that is NOT an error.
///
/// Mirrors the style of `get_build` (status-code check, body capture,
/// debug! logging).
pub async fn list_build_artifacts(
    client: &reqwest::Client,
    ctx: &AdoContext,
    auth: &AdoAuth,
    build_id: u64,
) -> Result<Vec<BuildArtifact>> {
    #[derive(Deserialize)]
    struct BuildArtifactListResponse {
        value: Vec<BuildArtifact>,
    }

    let url = format!(
        "{}/{}/_apis/build/builds/{}/artifacts?api-version=7.1",
        ctx.org_url.trim_end_matches('/'),
        percent_encoding::utf8_percent_encode(&ctx.project, PATH_SEGMENT),
        build_id
    );

    debug!("GET build artifacts for build {}: {}", build_id, url);

    let resp = auth
        .apply(client.get(&url))
        .send()
        .await
        .with_context(|| format!("Failed to fetch build artifacts for build {}", build_id))?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(anyhow::anyhow!(
                "ADO API returned {} when listing build artifacts for build {}: {}. This call requires PAT scopes Build (Read) and Build Artifacts (Read). As a manual alternative, try `az pipelines runs artifact list --run-id {}`.",
                status,
                build_id,
                body,
                build_id
            ));
        }
        anyhow::bail!(
            "ADO API returned {} when listing build artifacts for build {}: {}",
            status,
            build_id,
            body
        );
    }

    let body = resp.text().await.with_context(|| {
        format!(
            "Failed to read build artifacts response body for build {}",
            build_id
        )
    })?;
    let response: BuildArtifactListResponse = serde_json::from_str(&body).with_context(|| {
        let snippet: String = body.chars().take(500).collect();
        format!(
            "Failed to parse build artifacts response for build {} as JSON. Response body (first 500 chars):\n{snippet}",
            build_id
        )
    })?;

    Ok(response.value)
}

/// Download a single build artifact and unzip it into `dest_dir`.
///
/// ADO PipelineArtifacts are delivered as a zip; this helper follows the
/// signed `downloadUrl`, streams the response, and extracts it under
/// `dest_dir/{artifact.name}/...`.
///
/// On HTTP 401/403, returns a structured error whose message lists the
/// required PAT scopes (`Build (Read)`, `Build Artifacts (Read)`) and
/// suggests the `az pipelines runs artifact download --run-id <id>
/// --artifact-name <name> --path <dir>` escape hatch.
///
/// If `artifact.resource.download_url` is `None`, returns an error
/// explaining that the artifact resource type is not downloadable
/// (legacy `Container` artifacts use a different endpoint we do not
/// support yet).
pub async fn download_build_artifact(
    client: &reqwest::Client,
    auth: &AdoAuth,
    artifact: &BuildArtifact,
    dest_dir: &std::path::Path,
) -> Result<()> {
    let download_url = artifact.resource.download_url.as_deref().with_context(|| {
        format!(
            "Build artifact '{}' has no download URL. Artifact resource type '{}' is not downloadable via this helper yet (legacy Container artifacts use a different endpoint).",
            artifact.name,
            artifact.resource.kind
        )
    })?;

    std::fs::create_dir_all(dest_dir).with_context(|| {
        format!(
            "Failed to create artifact destination directory '{}'",
            dest_dir.display()
        )
    })?;

    let artifact_dir = dest_dir.join(&artifact.name);
    if artifact_dir.exists() {
        std::fs::remove_dir_all(&artifact_dir).with_context(|| {
            format!(
                "Failed to remove existing artifact directory '{}'",
                artifact_dir.display()
            )
        })?;
    }
    std::fs::create_dir_all(&artifact_dir).with_context(|| {
        format!(
            "Failed to create artifact extraction directory '{}'",
            artifact_dir.display()
        )
    })?;

    debug!(
        "Downloading build artifact '{}' from {}",
        artifact.name, download_url
    );

    let mut resp = auth
        .apply(client.get(download_url))
        .send()
        .await
        .with_context(|| format!("Failed to download build artifact '{}'", artifact.name))?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            let run_id_hint = artifact.source.as_deref().unwrap_or("<build-id>");
            return Err(anyhow::anyhow!(
                "ADO API returned {} when downloading build artifact '{}': {}. This call requires PAT scopes Build (Read) and Build Artifacts (Read). As a manual alternative, try `az pipelines runs artifact download --run-id {} --artifact-name {} --path {}`.",
                status,
                artifact.name,
                body,
                run_id_hint,
                artifact.name,
                dest_dir.display()
            ));
        }
        anyhow::bail!(
            "ADO API returned {} when downloading build artifact '{}': {}",
            status,
            artifact.name,
            body
        );
    }

    let mut temp_zip = tempfile::Builder::new()
        .prefix(&format!(".tmp-{}-", artifact.id))
        .suffix(".zip")
        .tempfile_in(dest_dir)
        .with_context(|| {
            format!(
                "Failed to create temp zip for build artifact '{}'",
                artifact.name
            )
        })?;

    while let Some(chunk) = resp
        .chunk()
        .await
        .with_context(|| format!("Failed to stream build artifact '{}'", artifact.name))?
    {
        temp_zip.write_all(&chunk).with_context(|| {
            format!(
                "Failed to write temp zip for build artifact '{}'",
                artifact.name
            )
        })?;
    }
    temp_zip.flush().with_context(|| {
        format!(
            "Failed to flush temp zip for build artifact '{}'",
            artifact.name
        )
    })?;

    let archive_file = temp_zip.reopen().with_context(|| {
        format!(
            "Failed to reopen temp zip for build artifact '{}'",
            artifact.name
        )
    })?;
    let mut archive = zip::ZipArchive::new(archive_file).with_context(|| {
        format!(
            "Failed to read downloaded zip for build artifact '{}'",
            artifact.name
        )
    })?;

    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).with_context(|| {
            format!(
                "Failed to read zip entry {} from build artifact '{}'",
                index, artifact.name
            )
        })?;
        let entry_name = entry.name().to_string();
        let relative_path = entry
            .enclosed_name()
            .map(|path| path.to_owned())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Refusing to extract unsafe path '{}' from build artifact '{}'",
                    entry_name,
                    artifact.name
                )
            })?;
        let output_path = artifact_dir.join(&relative_path);

        if entry.is_dir() {
            std::fs::create_dir_all(&output_path).with_context(|| {
                format!(
                    "Failed to create extracted directory '{}'",
                    output_path.display()
                )
            })?;
            continue;
        }

        if let Some(parent) = output_path.parent() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("Failed to create parent directory '{}'", parent.display())
            })?;
        }

        let mut output = std::fs::File::create(&output_path).with_context(|| {
            format!(
                "Failed to create extracted file '{}'",
                output_path.display()
            )
        })?;
        std::io::copy(&mut entry, &mut output).with_context(|| {
            format!(
                "Failed to extract '{}' from build artifact '{}'",
                entry_name, artifact.name
            )
        })?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_masked_secret_variable_values_rewrites_masked_secret_to_null() {
        let mut def = serde_json::json!({
            "variables": {
                "SECRET": { "value": "***", "isSecret": true, "allowOverride": false },
                "PLAIN": { "value": "visible", "isSecret": false, "allowOverride": false }
            }
        });

        normalize_masked_secret_variable_values(&mut def);

        assert!(def["variables"]["SECRET"]["value"].is_null());
        assert_eq!(def["variables"]["PLAIN"]["value"], "visible");
    }

    #[test]
    fn normalize_masked_secret_variable_values_leaves_non_secret_mask_alone() {
        let mut def = serde_json::json!({
            "variables": {
                "LITERAL": { "value": "***", "isSecret": false, "allowOverride": false },
                "SECRET": { "value": "new-value", "isSecret": true, "allowOverride": false }
            }
        });

        normalize_masked_secret_variable_values(&mut def);

        assert_eq!(def["variables"]["LITERAL"]["value"], "***");
        assert_eq!(def["variables"]["SECRET"]["value"], "new-value");
    }

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

    // ==================== RepoSource / parse_git_remote ====================

    #[test]
    fn parse_git_remote_ado_https() {
        let source =
            parse_git_remote("https://dev.azure.com/myorg/MyProject/_git/myrepo").unwrap();
        assert_eq!(source.provider, RepoProvider::AdoGit);
        assert_eq!(source.owner, "myorg");
        assert_eq!(source.repo, "myrepo");
        assert_eq!(source.project.as_deref(), Some("MyProject"));
    }

    #[test]
    fn parse_git_remote_ado_ssh() {
        let source =
            parse_git_remote("git@ssh.dev.azure.com:v3/myorg/MyProject/myrepo").unwrap();
        assert_eq!(source.provider, RepoProvider::AdoGit);
        assert_eq!(source.owner, "myorg");
        assert_eq!(source.repo, "myrepo");
        assert_eq!(source.project.as_deref(), Some("MyProject"));
    }

    #[test]
    fn parse_git_remote_ado_legacy_visualstudio() {
        let source =
            parse_git_remote("https://myorg.visualstudio.com/MyProject/_git/myrepo").unwrap();
        assert_eq!(source.provider, RepoProvider::AdoGit);
        assert_eq!(source.owner, "myorg");
        assert_eq!(source.repo, "myrepo");
        assert_eq!(source.project.as_deref(), Some("MyProject"));
    }

    #[test]
    fn parse_git_remote_github_https() {
        let source = parse_git_remote("https://github.com/githubnext/ado-aw").unwrap();
        assert_eq!(source.provider, RepoProvider::Github);
        assert_eq!(source.owner, "githubnext");
        assert_eq!(source.repo, "ado-aw");
        assert!(source.project.is_none());
    }

    #[test]
    fn parse_git_remote_github_https_dotgit_suffix() {
        let source = parse_git_remote("https://github.com/githubnext/ado-aw.git").unwrap();
        assert_eq!(source.provider, RepoProvider::Github);
        assert_eq!(source.owner, "githubnext");
        assert_eq!(source.repo, "ado-aw");
    }

    #[test]
    fn parse_git_remote_github_ssh() {
        let source = parse_git_remote("git@github.com:githubnext/ado-aw.git").unwrap();
        assert_eq!(source.provider, RepoProvider::Github);
        assert_eq!(source.owner, "githubnext");
        assert_eq!(source.repo, "ado-aw");
    }

    #[test]
    fn parse_git_remote_github_ssh_no_dotgit() {
        let source = parse_git_remote("git@github.com:githubnext/ado-aw").unwrap();
        assert_eq!(source.provider, RepoProvider::Github);
        assert_eq!(source.owner, "githubnext");
        assert_eq!(source.repo, "ado-aw");
    }

    #[test]
    fn parse_git_remote_rejects_ghes() {
        // GitHub Enterprise is out of scope for v1; the parser must
        // not silently accept a non-github.com host as Github.
        assert!(parse_git_remote("https://github.example.com/owner/repo").is_err());
    }

    #[test]
    fn parse_git_remote_rejects_unrelated() {
        assert!(parse_git_remote("https://gitlab.com/owner/repo").is_err());
        assert!(parse_git_remote("not-a-url").is_err());
        // Missing repo half.
        assert!(parse_git_remote("https://github.com/owner").is_err());
        assert!(parse_git_remote("git@github.com:owner").is_err());
    }

    #[test]
    fn repo_source_url_ado_shape() {
        let source = RepoSource {
            provider: RepoProvider::AdoGit,
            owner: "myorg".to_string(),
            repo: "myrepo".to_string(),
            project: Some("MyProject".to_string()),
        };
        assert_eq!(
            source.url(),
            "https://dev.azure.com/myorg/MyProject/_git/myrepo"
        );
    }

    #[test]
    fn repo_source_url_github_shape() {
        let source = RepoSource {
            provider: RepoProvider::Github,
            owner: "githubnext".to_string(),
            repo: "ado-aw".to_string(),
            project: None,
        };
        assert_eq!(source.url(), "https://github.com/githubnext/ado-aw");
    }

    /// In release builds (where `debug_assert!` is a no-op), an
    /// `AdoGit` `RepoSource` with `project: None` must still produce
    /// a well-formed-ish URL — we tolerate the empty middle segment
    /// rather than panicking, because `url()` is comparison-only.
    /// `parse_git_remote` never produces this shape in practice; this
    /// test only documents the defensive fallback.
    #[cfg(not(debug_assertions))]
    #[test]
    fn repo_source_url_ado_without_project_release_fallback() {
        let source = RepoSource {
            provider: RepoProvider::AdoGit,
            owner: "myorg".to_string(),
            repo: "myrepo".to_string(),
            project: None,
        };
        // Empty project segment is the documented fallback.
        assert_eq!(source.url(), "https://dev.azure.com/myorg//_git/myrepo");
    }

    // ==================== Service-connection resolver ====================

    #[test]
    fn is_uuid_like_accepts_canonical() {
        assert!(is_uuid_like("12345678-1234-1234-1234-1234567890ab"));
        assert!(is_uuid_like("ABCDEF12-3456-7890-ABCD-EF1234567890"));
    }

    #[test]
    fn is_uuid_like_rejects_non_uuid() {
        assert!(!is_uuid_like("ado-aw-github"));
        assert!(!is_uuid_like(""));
        // Wrong length.
        assert!(!is_uuid_like("12345678-1234-1234-1234-1234567890"));
        // Missing hyphens.
        assert!(!is_uuid_like("123456781234123412341234567890ab"));
        // Hyphens in wrong positions.
        assert!(!is_uuid_like("12345678-12341-1234-1234-1234567890a"));
        // Non-hex character.
        assert!(!is_uuid_like("12345678-1234-1234-1234-1234567890zz"));
    }

    #[test]
    fn pick_service_endpoint_id_single_match() {
        let body = serde_json::json!({
            "count": 1,
            "value": [
                { "id": "abc-123", "name": "ado-aw-github" }
            ]
        });
        let id = pick_service_endpoint_id(&body, "ado-aw-github", "AgentPlayground").unwrap();
        assert_eq!(id, "abc-123");
    }

    #[test]
    fn pick_service_endpoint_id_no_match_bails_with_useful_message() {
        let body = serde_json::json!({ "count": 0, "value": [] });
        let err = pick_service_endpoint_id(&body, "missing-conn", "AgentPlayground")
            .unwrap_err()
            .to_string();
        assert!(err.contains("No GitHub service connection named 'missing-conn'"));
        assert!(err.contains("'AgentPlayground'"));
        assert!(err.contains("Project settings"));
    }

    #[test]
    fn pick_service_endpoint_id_multi_match_bails_with_disambiguation_hint() {
        let body = serde_json::json!({
            "count": 2,
            "value": [
                { "id": "abc-1", "name": "shared-name" },
                { "id": "abc-2", "name": "shared-name" }
            ]
        });
        let err = pick_service_endpoint_id(&body, "shared-name", "AgentPlayground")
            .unwrap_err()
            .to_string();
        assert!(err.contains("2 GitHub service connections named 'shared-name'"));
        assert!(err.contains("GUID"));
    }

    #[test]
    fn pick_service_endpoint_id_missing_value_array_bails() {
        let body = serde_json::json!({ "count": 0 });
        let err = pick_service_endpoint_id(&body, "any", "AgentPlayground")
            .unwrap_err()
            .to_string();
        assert!(err.contains("missing `value` array"));
    }

    /// The `endpointNames=` query-string value must encode `&` and `=`
    /// — otherwise an endpoint named e.g. `foo&bar=baz` would
    /// shatter into two `endpointNames` params and a stray `baz=` on
    /// the URL. We use a custom `QUERY_VALUE` set rather than
    /// `NON_ALPHANUMERIC` (over-encodes `-`/`_`/`.`/`~`) or
    /// `PATH_SEGMENT` (leaves `&`/`=` unencoded). Test pins both:
    /// query metachars escape, RFC 3986 unreserved chars don't.
    #[test]
    fn service_connection_query_value_encodes_query_metacharacters() {
        let encoded =
            percent_encoding::utf8_percent_encode("foo&bar=baz", QUERY_VALUE).to_string();
        assert!(!encoded.contains('&'), "must encode '&' in query value");
        assert!(!encoded.contains('='), "must encode '=' in query value");
        assert!(encoded.contains("foo"));
        assert!(encoded.contains("bar"));
        assert!(encoded.contains("baz"));
    }

    #[test]
    fn service_connection_query_value_preserves_unreserved_chars() {
        // RFC 3986 unreserved set: A-Z a-z 0-9 - _ . ~ — these must
        // pass through literally. Pins that we don't regress to
        // `NON_ALPHANUMERIC` which would over-encode them.
        let encoded =
            percent_encoding::utf8_percent_encode("ado-aw_github.v1~beta", QUERY_VALUE)
                .to_string();
        assert_eq!(encoded, "ado-aw_github.v1~beta");
    }

    // ==================== match_definitions_in (provider-agnostic) ====================

    /// `match_definitions_in` only consults `process.yamlFilename` and
    /// `name` — both stable across provider. A GitHub-source
    /// `DefinitionSummary` (one with `repository.type = "GitHub"` in
    /// the ADO response) must match a locally-detected lock file
    /// exactly as well as a TfsGit one does. This pins that contract
    /// so a future refactor of `match_definitions_in` can't
    /// accidentally start gating on provider.
    #[test]
    fn match_definitions_in_works_for_github_source_definition() {
        use std::path::PathBuf;

        let definitions = vec![DefinitionSummary {
            id: 99,
            name: "Daily smoke noop".to_string(),
            process: Some(ProcessInfo {
                yaml_filename: Some("/tests/safe-outputs/noop.lock.yml".to_string()),
            }),
            queue_status: Some("enabled".to_string()),
            path: Some("\\smoke".to_string()),
            repository: Some(Repository {
                url: Some("https://github.com/githubnext/ado-aw".to_string()),
                name: Some("githubnext/ado-aw".to_string()),
                repo_type: Some("GitHub".to_string()),
                id: None,
            }),
            revision: Some(1),
        }];
        let detected = vec![crate::detect::DetectedPipeline {
            yaml_path: PathBuf::from("tests/safe-outputs/noop.lock.yml"),
            source: "tests/safe-outputs/noop.md".to_string(),
            version: "0.32.0".to_string(),
        }];

        let matched = match_definitions_in(&definitions, &detected);
        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0].id, 99);
        assert_eq!(matched[0].name, "Daily smoke noop");
    }

    // ==================== Org URL normalization ====================

    #[test]
    fn normalize_org_url_accepts_bare_name() {
        assert_eq!(normalize_org_url("myorg"), "https://dev.azure.com/myorg");
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
            path: None,
            repository: None,
            revision: None,
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
            path: None,
            repository: None,
            revision: None,
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
        let defs = [
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
        let defs = [
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
        let encoded = percent_encoding::utf8_percent_encode("café-π", PATH_SEGMENT).to_string();
        // Non-ASCII bytes get encoded per UTF-8.
        assert_eq!(encoded, "caf%C3%A9-%CF%80");
    }

    #[test]
    fn build_artifact_deserializes_pipeline_artifact_response() {
        #[derive(Deserialize)]
        struct BuildArtifactListResponse {
            value: Vec<BuildArtifact>,
        }

        let raw = serde_json::json!({
            "count": 1,
            "value": [
                {
                    "id": 1,
                    "name": "agent_outputs_42",
                    "source": "42",
                    "resource": {
                        "type": "PipelineArtifact",
                        "data": "#/123/agent_outputs_42",
                        "url": "https://dev.azure.com/example/project/_apis/build/builds/42/artifacts?artifactName=agent_outputs_42",
                        "downloadUrl": "https://example.invalid/download/agent_outputs_42.zip"
                    }
                }
            ]
        });

        let response: BuildArtifactListResponse = serde_json::from_value(raw).unwrap();
        let artifact = &response.value[0];
        assert_eq!(artifact.id, 1);
        assert_eq!(artifact.name, "agent_outputs_42");
        assert_eq!(artifact.source.as_deref(), Some("42"));
        assert_eq!(artifact.resource.kind, "PipelineArtifact");
        assert_eq!(
            artifact.resource.download_url.as_deref(),
            Some("https://example.invalid/download/agent_outputs_42.zip")
        );
    }

    #[tokio::test]
    async fn download_build_artifact_errors_when_download_url_is_missing() {
        let artifact = BuildArtifact {
            id: 1,
            name: "safe_outputs".to_string(),
            source: Some("42".to_string()),
            resource: BuildArtifactResource {
                kind: "Container".to_string(),
                data: None,
                properties: None,
                url: None,
                download_url: None,
            },
        };
        let client = reqwest::Client::new();
        let temp_dir = tempfile::tempdir().unwrap();

        let error = download_build_artifact(
            &client,
            &AdoAuth::Pat("test".to_string()),
            &artifact,
            temp_dir.path(),
        )
        .await
        .unwrap_err();
        let message = error.to_string();
        assert!(
            message.contains("no download URL") || message.contains("not downloadable"),
            "unexpected error message: {message}"
        );
    }
}
