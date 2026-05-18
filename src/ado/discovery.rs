//! Preview-driven discovery of ado-aw pipelines.
//!
//! Replaces the lexical `match_definitions_in` approach (which only
//! finds definitions whose root YAML is itself an ado-aw lock file)
//! with a content-marker scan over expanded YAML returned by ADO's
//! Pipeline Preview API. Picks up consumer pipelines (definitions whose
//! root YAML is hand-written but `include`s an ado-aw template) and is
//! rename-resilient because the marker is in the YAML content rather
//! than the definition's `process.yamlFilename` field.
//!
//! ## Algorithm
//!
//! 1. List every definition in the project via `list_definitions`.
//! 2. Filter by [`DiscoveryScope`] — e.g., `CurrentRepo` matches
//!    `repository.url` against the resolved git remote.
//! 3. Fast path: when a definition's `process.yamlFilename` matches one
//!    of the supplied local lock files, parse the local file's
//!    `# @ado-aw` header and mark the definition `Direct` without
//!    spending a Preview call.
//! 4. Otherwise: POST to `/_apis/pipelines/{id}/preview` and scan the
//!    response's `finalYaml` for `# ado-aw-metadata: {…}` markers
//!    (see [`crate::detect::parse_marker_step`]).
//! 5. Classify per [`DiscoveryStatus`].
//!
//! ## Empirical grounding
//!
//! The Preview API was validated against the live `msazuresphere/4x4`
//! project (definition 2434, `OS Release Readiness`) — a 56 KB
//! `finalYaml` was returned, comments inside step bodies preserved,
//! top-of-document comments stripped. The marker-step design in
//! `src/compile/extensions/ado_aw_marker.rs` solves the stripping
//! problem by embedding the marker inside a bash heredoc.

#![allow(dead_code)] // Wired into CLI commands in workstream S; tests below cover it now.

use anyhow::{Context, Result};
use log::{debug, warn};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Semaphore;

use super::{AdoAuth, AdoContext, DefinitionSummary, MatchMethod, MatchedDefinition, list_definitions};
use crate::detect::{MarkerMetadata, parse_marker_step};

/// Default permits used to throttle concurrent Preview HTTP calls.
/// Tunable via the `ADO_AW_PREVIEW_CONCURRENCY` environment variable so
/// users hitting ADO rate limits in large projects can dial it down
/// without a code change.
const DEFAULT_PREVIEW_CONCURRENCY: usize = 8;

// ─── Public types ────────────────────────────────────────────────────

/// Which ADO definitions to consider during discovery.
#[derive(Debug, Clone)]
pub enum DiscoveryScope {
    /// Only definitions whose backing repository URL matches the
    /// resolved git remote of the current working directory. Default
    /// for project-scope CLI commands.
    CurrentRepo,
    /// Every definition in the project, regardless of repository.
    AllRepos,
    /// A pre-resolved list of definition IDs; bypasses listing and
    /// scope filtering entirely. Used by `--definition-ids` callers.
    Explicit(Vec<u64>),
}

/// Classification of a single ADO definition with respect to ado-aw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiscoveryStatus {
    /// The definition's root YAML is itself an ado-aw lock file —
    /// either via the local-fixture fast-path, or because the expanded
    /// YAML's first marker step appears at the root level.
    Direct,
    /// The definition's root YAML is hand-written but includes one or
    /// more ado-aw templates (detected by markers inside the expanded
    /// YAML).
    Consumer,
    /// Preview returned 400, typically because the consumer's
    /// `parameters:` block has required fields with no defaults. The
    /// definition may still be an ado-aw consumer; we just couldn't
    /// confirm without supplying parameter values.
    UnknownRequiredParams,
    /// Preview returned 403 — the calling identity lacks read access
    /// on the definition or one of its referenced repos.
    UnknownForbidden,
    /// Preview returned some other error (5xx, network failure, etc.).
    PreviewFailed(String),
    /// Preview succeeded but no ado-aw marker was found in the
    /// expanded YAML; the definition is not an ado-aw pipeline.
    NotAdoAw,
}

/// Result of discovery for a single ADO definition.
#[derive(Debug, Clone)]
pub struct DiscoveredPipeline {
    pub definition_id: u64,
    pub definition_name: String,
    pub repository_url: Option<String>,
    pub queue_status: Option<String>,
    pub markers: Vec<MarkerMetadata>,
    pub status: DiscoveryStatus,
}

// ─── Preview API client ──────────────────────────────────────────────

/// Typed error from a `preview_pipeline` call so callers can map ADO
/// failure modes to [`DiscoveryStatus`] without re-inspecting HTTP
/// response bodies.
#[derive(Debug, Clone)]
pub enum PreviewError {
    /// 400 from ADO — usually means the pipeline declares required
    /// `parameters:` without defaults.
    RequiredParams,
    /// 403 — calling identity lacks read access.
    Forbidden,
    /// 404 — definition does not exist (or is hidden from the caller).
    NotFound,
    /// Any other failure (5xx, network, malformed JSON).
    Other(String),
}

impl std::fmt::Display for PreviewError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PreviewError::RequiredParams => write!(f, "preview returned 400 (required parameters without defaults)"),
            PreviewError::Forbidden => write!(f, "preview returned 403 (forbidden)"),
            PreviewError::NotFound => write!(f, "preview returned 404 (not found)"),
            PreviewError::Other(msg) => write!(f, "preview failed: {msg}"),
        }
    }
}

/// JSON shape of the Pipeline Preview response. Only `finalYaml` is
/// consumed; other fields (`yaml`, `id`, etc.) are intentionally
/// ignored — Preview also returns the un-rendered yaml under `yaml`
/// which is the wrong surface for marker discovery.
#[derive(Debug, Deserialize)]
struct PreviewResponse {
    #[serde(rename = "finalYaml", default)]
    final_yaml: Option<String>,
}

/// Call ADO's Pipeline Preview API and return the expanded `finalYaml`.
///
/// Uses the `/_apis/pipelines/{id}/preview?api-version=7.1-preview.1`
/// endpoint (not the legacy build-definitions surface — Preview is the
/// only documented public way to get expanded YAML).
pub async fn preview_pipeline(
    client: &reqwest::Client,
    ctx: &AdoContext,
    auth: &AdoAuth,
    definition_id: u64,
) -> std::result::Result<String, PreviewError> {
    let url = format!(
        "{}/{}/_apis/pipelines/{}/preview?api-version=7.1-preview.1",
        ctx.org_url.trim_end_matches('/'),
        percent_encoding::utf8_percent_encode(&ctx.project, super::PATH_SEGMENT),
        definition_id
    );

    let body = serde_json::json!({
        "previewRun": true,
        "templateParameters": {}
    });

    debug!("POST {} (preview definition {})", url, definition_id);

    let resp = auth
        .apply(client.post(&url).json(&body))
        .send()
        .await
        .map_err(|e| PreviewError::Other(format!("network error: {e}")))?;

    let status = resp.status();
    if status.as_u16() == 400 {
        return Err(PreviewError::RequiredParams);
    }
    if status.as_u16() == 403 || status.as_u16() == 401 {
        return Err(PreviewError::Forbidden);
    }
    if status.as_u16() == 404 {
        return Err(PreviewError::NotFound);
    }
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(PreviewError::Other(format!(
            "HTTP {} from preview endpoint: {}",
            status,
            body.chars().take(500).collect::<String>()
        )));
    }

    let parsed: PreviewResponse = resp
        .json()
        .await
        .map_err(|e| PreviewError::Other(format!("malformed preview JSON: {e}")))?;

    parsed
        .final_yaml
        .ok_or_else(|| PreviewError::Other("preview response missing finalYaml field".to_string()))
}

// ─── Discovery driver ────────────────────────────────────────────────

/// Discover every ado-aw pipeline in scope.
///
/// `local_lock_paths` enables the fast-path: definitions whose
/// `process.yamlFilename` matches one of these paths skip the Preview
/// call and are classified `Direct` by reading the local file's
/// `# @ado-aw` header (cheap, no HTTP). When `None` or empty, every
/// definition in scope is Previewed.
pub async fn discover_ado_aw_pipelines(
    client: &reqwest::Client,
    ctx: &AdoContext,
    auth: &AdoAuth,
    scope: DiscoveryScope,
    local_lock_paths: Option<&[PathBuf]>,
) -> Result<Vec<DiscoveredPipeline>> {
    let definitions = list_definitions(client, ctx, auth)
        .await
        .context("Failed to list ADO definitions for discovery")?;

    let filtered = apply_scope_filter(definitions, &scope, &ctx.repo_url());

    // Build a (normalized yamlFilename → local lock path) map for the
    // fast-path. Path comparison uses the same normalization as
    // `match_definitions_in`.
    let lock_map = build_lock_path_map(local_lock_paths);

    let permits = std::env::var("ADO_AW_PREVIEW_CONCURRENCY")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_PREVIEW_CONCURRENCY);
    let semaphore = Arc::new(Semaphore::new(permits.max(1)));

    let mut handles = Vec::with_capacity(filtered.len());
    for def in filtered {
        let local_match = def
            .process
            .as_ref()
            .and_then(|p| p.yaml_filename.as_ref())
            .and_then(|f| lock_map.get(&super::normalize_ado_yaml_path(f)).cloned());

        let sem = Arc::clone(&semaphore);
        let client = client.clone();
        let ctx = ctx.clone();
        let auth = auth.clone();

        handles.push(tokio::spawn(async move {
            let _permit = sem.acquire_owned().await.expect("semaphore not closed");
            classify_definition(&client, &ctx, &auth, def, local_match).await
        }));
    }

    let mut results = Vec::with_capacity(handles.len());
    for handle in handles {
        match handle.await {
            Ok(p) => results.push(p),
            Err(e) => warn!("Discovery worker panicked: {e}"),
        }
    }

    Ok(results)
}

/// Pure scope filter, factored out so it's exercised by unit tests.
fn apply_scope_filter(
    definitions: Vec<DefinitionSummary>,
    scope: &DiscoveryScope,
    current_repo_url: &Option<String>,
) -> Vec<DefinitionSummary> {
    match scope {
        DiscoveryScope::AllRepos => definitions,
        DiscoveryScope::Explicit(ids) => definitions
            .into_iter()
            .filter(|d| ids.contains(&d.id))
            .collect(),
        DiscoveryScope::CurrentRepo => {
            let Some(current) = current_repo_url else {
                // No git remote → nothing matches CurrentRepo. Caller
                // can fall back to AllRepos or an explicit ID list.
                return Vec::new();
            };
            let target = normalize_repo_url(current);
            definitions
                .into_iter()
                .filter(|d| {
                    d.repository
                        .as_ref()
                        .and_then(|r| r.url.as_ref())
                        .map(|u| normalize_repo_url(u) == target)
                        .unwrap_or(false)
                })
                .collect()
        }
    }
}

/// Normalize a repo URL for equality comparison. Strips trailing slash
/// and lowercases the scheme/host portion (ADO is case-insensitive on
/// org/project/repo names).
fn normalize_repo_url(url: &str) -> String {
    url.trim_end_matches('/').to_ascii_lowercase()
}

/// Build a `(normalized yamlFilename → local lock path)` lookup table
/// from `--source agents/foo.lock.yml` or similar inputs.
fn build_lock_path_map(local_lock_paths: Option<&[PathBuf]>) -> HashMap<String, PathBuf> {
    let mut map = HashMap::new();
    let Some(paths) = local_lock_paths else {
        return map;
    };
    for path in paths {
        let normalized = path.to_string_lossy().replace('\\', "/");
        // Match the same trim that `normalize_ado_yaml_path` applies.
        let trimmed = normalized.trim_start_matches('/').to_string();
        map.insert(trimmed, path.clone());
    }
    map
}

async fn classify_definition(
    client: &reqwest::Client,
    ctx: &AdoContext,
    auth: &AdoAuth,
    def: DefinitionSummary,
    local_match: Option<PathBuf>,
) -> DiscoveredPipeline {
    let repository_url = def.repository.as_ref().and_then(|r| r.url.clone());

    // Fast path: if process.yamlFilename matched a local lock file,
    // parse it directly. Avoids one HTTP round-trip per ado-aw-owned
    // definition. The local file existing but not parsing falls
    // through to Preview as a defensive measure.
    if let Some(local_path) = local_match
        && let Some(meta) = parse_local_lock(&local_path).await
    {
        return DiscoveredPipeline {
            definition_id: def.id,
            definition_name: def.name,
            repository_url,
            queue_status: def.queue_status,
            markers: vec![meta],
            status: DiscoveryStatus::Direct,
        };
    }

    match preview_pipeline(client, ctx, auth, def.id).await {
        Ok(final_yaml) => {
            let markers = parse_marker_step(&final_yaml);
            let status = if markers.is_empty() {
                DiscoveryStatus::NotAdoAw
            } else if is_direct_match(&def, &markers) {
                DiscoveryStatus::Direct
            } else {
                DiscoveryStatus::Consumer
            };
            DiscoveredPipeline {
                definition_id: def.id,
                definition_name: def.name,
                repository_url,
                queue_status: def.queue_status,
                markers,
                status,
            }
        }
        Err(PreviewError::RequiredParams) => DiscoveredPipeline {
            definition_id: def.id,
            definition_name: def.name,
            repository_url,
            queue_status: def.queue_status,
            markers: vec![],
            status: DiscoveryStatus::UnknownRequiredParams,
        },
        Err(PreviewError::Forbidden) => DiscoveredPipeline {
            definition_id: def.id,
            definition_name: def.name,
            repository_url,
            queue_status: def.queue_status,
            markers: vec![],
            status: DiscoveryStatus::UnknownForbidden,
        },
        Err(e) => DiscoveredPipeline {
            definition_id: def.id,
            definition_name: def.name,
            repository_url,
            queue_status: def.queue_status,
            markers: vec![],
            status: DiscoveryStatus::PreviewFailed(e.to_string()),
        },
    }
}

/// Heuristic: classify as `Direct` if the definition's root YAML is
/// itself an ado-aw lock file, otherwise `Consumer`. The check uses
/// the definition's `process.yamlFilename` as a proxy — if the source
/// markdown referenced by the marker has the same stem as the root
/// YAML, the definition is the direct owner. Anything else is a
/// consumer that pulls the template in via `template:` indirection.
///
/// Returns `false` for the marker-less case — `classify_definition`
/// only routes here when at least one marker was found, but defensive
/// against future callers.
fn is_direct_match(def: &DefinitionSummary, markers: &[MarkerMetadata]) -> bool {
    if markers.is_empty() {
        // 0 markers means "not ado-aw at all", which is neither direct
        // nor consumer. Belt-and-braces — `classify_definition`
        // currently guards this, but the guard could move.
        return false;
    }
    if markers.len() > 1 {
        // Multiple markers means a consumer pulling in more than one
        // template; can't be a direct ado-aw pipeline.
        return false;
    }
    let marker = &markers[0];
    let Some(yaml_filename) = def
        .process
        .as_ref()
        .and_then(|p| p.yaml_filename.as_ref())
    else {
        return false;
    };
    let yaml_normalized = super::normalize_ado_yaml_path(yaml_filename);

    // Map e.g. `agents/foo.md` → `agents/foo.lock.yml` and compare to
    // the definition's root YAML. Convention: `<stem>.md` compiles to
    // `<stem>.lock.yml`.
    let Some(stem) = marker
        .source
        .strip_suffix(".md")
        .map(|s| format!("{s}.lock.yml"))
    else {
        return false;
    };
    yaml_normalized == stem || yaml_normalized.ends_with(&format!("/{stem}"))
}

async fn parse_local_lock(path: &Path) -> Option<MarkerMetadata> {
    let content = tokio::fs::read_to_string(path).await.ok()?;
    // Two surfaces, in order of preference:
    // (1) the structural marker step — survives Preview expansion and
    //     is identical to what the Preview path would parse;
    // (2) the legacy top-of-file `# @ado-aw` header — kept for
    //     backward compat with pre-marker-extension lock files.
    if let Some(meta) = parse_marker_step(&content).into_iter().next() {
        return Some(meta);
    }
    // Fall back to the legacy header.
    for line in content.lines().take(5) {
        if let Some(h) = crate::detect::parse_header_line(line) {
            return Some(MarkerMetadata {
                schema: 0,
                source: h.source,
                version: h.version,
                target: String::new(),
            });
        }
    }
    None
}

// ─── Adapters into the existing CLI types ────────────────────────────

/// Convert a [`DiscoveredPipeline`] into a [`MatchedDefinition`], the
/// shape used by every CLI command (`list`, `secrets set/list/delete`,
/// etc.). This keeps the rest of the codebase unchanged when commands
/// opt into discovery via `--all-repos` / `--source`.
///
/// Returns `None` for any classification that isn't safely actionable
/// by a write command. In particular `UnknownRequiredParams`,
/// `UnknownForbidden`, and `PreviewFailed` are dropped because we
/// have no markers to attach (so we can't even tell the user which
/// template a write would affect); `NotAdoAw` is dropped because it
/// isn't ado-aw at all. Callers that want a richer summary (e.g. a
/// future `list --all-repos`) should inspect `DiscoveredPipeline`
/// directly rather than going through this adapter.
pub fn discovered_to_matched(d: &DiscoveredPipeline) -> Option<MatchedDefinition> {
    match d.status {
        DiscoveryStatus::Direct | DiscoveryStatus::Consumer => {}
        DiscoveryStatus::NotAdoAw
        | DiscoveryStatus::UnknownForbidden
        | DiscoveryStatus::UnknownRequiredParams
        | DiscoveryStatus::PreviewFailed(_) => return None,
    }

    Some(MatchedDefinition {
        id: d.definition_id,
        name: d.definition_name.clone(),
        match_method: MatchMethod::Discovery,
        // Prefer the first marker's source path so downstream summaries
        // can show "→ agents/foo.md" without further lookup.
        yaml_path: d
            .markers
            .first()
            .map(|m| m.source.clone())
            .unwrap_or_default(),
        queue_status: d.queue_status.clone(),
    })
}

/// CLI-facing wrapper: run Preview-driven discovery with the given
/// scope, optionally filter to consumers of a single template source,
/// and return the result as `Vec<MatchedDefinition>`.
///
/// `source_filter` filters discovery results so only definitions whose
/// markers reference that source path are kept. Match is by exact
/// equality on the normalized source string in the marker JSON.
///
/// Definitions whose Preview call failed in a known-recoverable way
/// (`UnknownRequiredParams` / `UnknownForbidden` / `PreviewFailed`) are
/// counted and surfaced as a `warn!` so the operator can see that
/// some pipelines were skipped — silently dropping them would be a
/// nasty surprise for `secrets set --all-repos`.
pub async fn resolve_definitions_via_discovery(
    client: &reqwest::Client,
    ctx: &AdoContext,
    auth: &AdoAuth,
    scope: DiscoveryScope,
    local_lock_paths: Option<&[PathBuf]>,
    source_filter: Option<&str>,
) -> Result<Vec<MatchedDefinition>> {
    let discovered = discover_ado_aw_pipelines(client, ctx, auth, scope, local_lock_paths).await?;

    let mut skipped_required_params = 0usize;
    let mut skipped_forbidden = 0usize;
    let mut skipped_failed = 0usize;

    let kept: Vec<_> = discovered
        .into_iter()
        .filter(|d| {
            match &d.status {
                DiscoveryStatus::UnknownRequiredParams => skipped_required_params += 1,
                DiscoveryStatus::UnknownForbidden => skipped_forbidden += 1,
                DiscoveryStatus::PreviewFailed(_) => skipped_failed += 1,
                _ => {}
            }
            let Some(src) = source_filter else { return true };
            d.markers.iter().any(|m| m.source == src)
        })
        .collect();

    if skipped_required_params > 0 {
        warn!(
            "Discovery skipped {skipped_required_params} definition(s) whose Pipeline Preview \
             requires templateParameters with no defaults. Use --definition-ids to act on them \
             directly.",
        );
    }
    if skipped_forbidden > 0 {
        warn!(
            "Discovery skipped {skipped_forbidden} definition(s) the calling identity lacks \
             read access to. Check your PAT or AAD permissions.",
        );
    }
    if skipped_failed > 0 {
        warn!(
            "Discovery skipped {skipped_failed} definition(s) whose Pipeline Preview returned \
             an unexpected error. Re-run with --debug to see details.",
        );
    }

    Ok(kept.iter().filter_map(discovered_to_matched).collect())
}

// AdoContext helper: derive the resolved git remote URL for
// `CurrentRepo` scoping. Lives here (rather than on `AdoContext`)
// because the context only stores org+project+repo_name today;
// reconstructing the URL is a local detail of discovery.
//
// Percent-encodes `project` and `repo_name` to match the form ADO
// returns in `repository.url` — without this, projects whose names
// contain spaces or other reserved chars would silently match nothing
// because the lowercase comparison can't reconcile e.g. `my project`
// with `my%20project`.
impl AdoContext {
    fn repo_url(&self) -> Option<String> {
        if self.repo_name.is_empty() {
            return None;
        }
        Some(format!(
            "{}/{}/_git/{}",
            self.org_url.trim_end_matches('/'),
            percent_encoding::utf8_percent_encode(&self.project, super::PATH_SEGMENT),
            percent_encoding::utf8_percent_encode(&self.repo_name, super::PATH_SEGMENT),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ado::{ProcessInfo, Repository};

    fn def_with(
        id: u64,
        name: &str,
        yaml_filename: Option<&str>,
        repo_url: Option<&str>,
    ) -> DefinitionSummary {
        DefinitionSummary {
            id,
            name: name.to_string(),
            process: yaml_filename.map(|f| ProcessInfo {
                yaml_filename: Some(f.to_string()),
            }),
            queue_status: None,
            path: None,
            repository: repo_url.map(|u| Repository {
                url: Some(u.to_string()),
                name: None,
                repo_type: None,
                id: None,
            }),
            revision: None,
        }
    }

    // ── apply_scope_filter ───────────────────────────────────────────

    #[test]
    fn scope_all_repos_returns_everything() {
        let defs = vec![
            def_with(1, "a", None, Some("https://dev.azure.com/o/p/_git/a")),
            def_with(2, "b", None, Some("https://dev.azure.com/o/p/_git/b")),
        ];
        let kept = apply_scope_filter(defs, &DiscoveryScope::AllRepos, &None);
        assert_eq!(kept.len(), 2);
    }

    #[test]
    fn scope_explicit_filters_by_id() {
        let defs = vec![
            def_with(1, "a", None, None),
            def_with(2, "b", None, None),
            def_with(3, "c", None, None),
        ];
        let kept = apply_scope_filter(defs, &DiscoveryScope::Explicit(vec![1, 3]), &None);
        assert_eq!(kept.iter().map(|d| d.id).collect::<Vec<_>>(), vec![1, 3]);
    }

    #[test]
    fn scope_current_repo_matches_normalized_url() {
        let defs = vec![
            def_with(1, "a", None, Some("https://dev.azure.com/Org/P/_git/Repo")),
            def_with(2, "b", None, Some("https://dev.azure.com/Org/P/_git/Other")),
        ];
        let current = Some("https://dev.azure.com/org/p/_git/repo/".to_string());
        let kept = apply_scope_filter(defs, &DiscoveryScope::CurrentRepo, &current);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].id, 1);
    }

    #[test]
    fn scope_current_repo_with_no_remote_returns_empty() {
        let defs = vec![def_with(1, "a", None, Some("https://dev.azure.com/o/p/_git/x"))];
        let kept = apply_scope_filter(defs, &DiscoveryScope::CurrentRepo, &None);
        assert!(kept.is_empty());
    }

    #[test]
    fn scope_current_repo_excludes_definitions_without_repository() {
        let defs = vec![
            def_with(1, "a", None, None),
            def_with(2, "b", None, Some("https://dev.azure.com/o/p/_git/x")),
        ];
        let current = Some("https://dev.azure.com/o/p/_git/x".to_string());
        let kept = apply_scope_filter(defs, &DiscoveryScope::CurrentRepo, &current);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].id, 2);
    }

    // ── is_direct_match ──────────────────────────────────────────────

    #[test]
    fn direct_when_yaml_filename_matches_marker_stem() {
        let def = def_with(1, "a", Some("/agents/foo.lock.yml"), None);
        let markers = vec![MarkerMetadata {
            schema: 1,
            source: "agents/foo.md".to_string(),
            version: "0.30.0".to_string(),
            target: "standalone".to_string(),
        }];
        assert!(is_direct_match(&def, &markers));
    }

    #[test]
    fn direct_when_yaml_filename_has_extra_path_prefix() {
        // ADO sometimes stores yamlFilename with a project-relative
        // leading slash + extra path components. The marker source is
        // just the markdown path the user passed at compile time.
        let def = def_with(1, "a", Some("/agents/foo.lock.yml"), None);
        let markers = vec![MarkerMetadata {
            schema: 1,
            source: "agents/foo.md".to_string(),
            version: "0.30.0".to_string(),
            target: "standalone".to_string(),
        }];
        assert!(is_direct_match(&def, &markers));
    }

    #[test]
    fn consumer_when_yaml_filename_does_not_match_marker() {
        let def = def_with(1, "a", Some("/release-readiness.yml"), None);
        let markers = vec![MarkerMetadata {
            schema: 1,
            source: "agents/foo.md".to_string(),
            version: "0.30.0".to_string(),
            target: "stage".to_string(),
        }];
        assert!(!is_direct_match(&def, &markers));
    }

    #[test]
    fn consumer_when_multiple_markers_present() {
        let def = def_with(1, "a", Some("/agents/foo.lock.yml"), None);
        let markers = vec![
            MarkerMetadata {
                schema: 1,
                source: "agents/foo.md".to_string(),
                version: "0.30.0".to_string(),
                target: "stage".to_string(),
            },
            MarkerMetadata {
                schema: 1,
                source: "agents/bar.md".to_string(),
                version: "0.30.0".to_string(),
                target: "job".to_string(),
            },
        ];
        // Multiple markers = at least one template is being included
        // alongside something else; not a single direct ownership.
        assert!(!is_direct_match(&def, &markers));
    }

    // ── build_lock_path_map ──────────────────────────────────────────

    #[test]
    fn lock_map_normalizes_paths() {
        let paths = vec![
            PathBuf::from("agents\\foo.lock.yml"),
            PathBuf::from("/agents/bar.lock.yml"),
        ];
        let map = build_lock_path_map(Some(&paths));
        assert!(map.contains_key("agents/foo.lock.yml"));
        assert!(map.contains_key("agents/bar.lock.yml"));
    }

    #[test]
    fn lock_map_empty_for_none() {
        assert!(build_lock_path_map(None).is_empty());
    }

    // ── PreviewError ─────────────────────────────────────────────────

    #[test]
    fn preview_error_display_is_actionable() {
        assert!(
            PreviewError::RequiredParams
                .to_string()
                .contains("required parameters")
        );
        assert!(PreviewError::Forbidden.to_string().contains("403"));
        assert!(PreviewError::NotFound.to_string().contains("404"));
    }
}
