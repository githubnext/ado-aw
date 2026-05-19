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
    /// scope filtering entirely.
    ///
    /// **Reserved for future use.** No production callsite constructs
    /// this variant today — the `--definition-ids` CLI flag is handled
    /// by `crate::ado::resolve_definitions` (the legacy lexical
    /// matcher), which short-circuits before discovery is invoked. The
    /// variant exists so callers that want to feed an explicit ID list
    /// into the discovery pipeline (e.g. for future automation that
    /// has pre-filtered definitions) don't need a parallel API.
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
    /// Preview returned 404 — the definition disappeared between
    /// `list_definitions` and `preview_pipeline` (race with a
    /// concurrent delete). Tracked as a distinct status so it can be
    /// filtered out of the skip-warning counts: there's no operator
    /// action to take for a definition that no longer exists.
    NotFound,
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

    // Resolve concurrency from env; warn (not silently clamp) when an
    // operator sets `=0`, since the deadlock-avoidance `.max(1)` would
    // mask the typo and leave the user wondering why throughput hasn't
    // changed.
    let raw_permits = std::env::var("ADO_AW_PREVIEW_CONCURRENCY")
        .ok()
        .and_then(|v| v.parse::<usize>().ok());
    let permits = match raw_permits {
        Some(0) => {
            warn!(
                "ADO_AW_PREVIEW_CONCURRENCY=0 would deadlock the Preview semaphore; \
                 clamping to 1. Set a positive integer to control concurrency.",
            );
            1
        }
        Some(n) => n,
        None => DEFAULT_PREVIEW_CONCURRENCY,
    };
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

/// Normalize a repo URL for equality comparison.
///
/// Two normalizations are applied so the comparison is robust to the
/// shape ADO returns:
///
///  1. **Percent-decode** the URL so a project named e.g. `My Project`
///     compares equal whether ADO returned `My%20Project` or the (rare
///     but legal) decoded `My Project`. Lossy decoding on invalid UTF-8
///     keeps us forward-compatible — anything ADO can return, we can
///     compare.
///  2. **ASCII-lowercase** because ADO is case-insensitive on org /
///     project / repo identifiers, and trim any trailing `/`.
///
/// Without (1), the comparison would silently fail for any project
/// name containing a percent-reserved character if `ado_ctx.repo_url()`
/// emitted the encoded form and `repository.url` returned the decoded
/// form (or vice-versa).
fn normalize_repo_url(url: &str) -> String {
    let decoded = percent_encoding::percent_decode_str(url)
        .decode_utf8_lossy()
        .into_owned();
    decoded.trim_end_matches('/').to_ascii_lowercase()
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
        Err(PreviewError::NotFound) => {
            // Definition was deleted between `list_definitions` and the
            // Preview call (TOCTOU race with a concurrent delete).
            // Track as a distinct status so it's excluded from the
            // "Preview Failed" warning — there's no operator action to
            // take for a definition that no longer exists.
            debug!(
                "Definition {} ({}) disappeared between list and preview (404); skipping",
                def.id, def.name
            );
            DiscoveredPipeline {
                definition_id: def.id,
                definition_name: def.name,
                repository_url,
                queue_status: def.queue_status,
                markers: vec![],
                status: DiscoveryStatus::NotFound,
            }
        }
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
    //
    // Equality is required — an earlier version also accepted
    // `yaml_normalized.ends_with("/{stem}")` for a defensive
    // tail-match, but that produced false-positives when an unrelated
    // pipeline happened to live under a same-named lock file in a
    // different directory (e.g. marker `agents/foo.md` + yamlFilename
    // `other/agents/foo.lock.yml` would mislabel a Consumer as Direct).
    // Both `marker.source` and the post-`normalize_ado_yaml_path`
    // form of `yaml_filename` are repo-root-relative without a leading
    // slash, so strict equality is the correct check.
    //
    // Non-`.md` sources are treated conservatively as `Consumer`: this
    // branch is unreachable today (the compiler always emits `.md`
    // source paths) but stays defensive against future extensions that
    // allow `.yaml` / `.json` / etc. agent sources. Returning `false`
    // here means the definition will be classified as `Consumer`
    // rather than `Direct`, which is the safe default — a write
    // command still acts on it, just labelled differently in the
    // summary.
    let Some(stem) = marker
        .source
        .strip_suffix(".md")
        .map(|s| format!("{s}.lock.yml"))
    else {
        return false;
    };
    yaml_normalized == stem
}

/// Decide whether a marker's `(org, repo)` identifies the same
/// repository as the discovery context. Empty marker fields (legacy
/// markers produced before the org/repo embed landed, or markers from
/// non-ADO compile environments) are treated as wildcards so existing
/// deployments are not silently excluded. Once those lock files are
/// recompiled, the match becomes strict.
fn marker_origin_matches(
    marker: &MarkerMetadata,
    current_org_lc: &str,
    current_repo_lc: &str,
) -> bool {
    if marker.org.is_empty() && marker.repo.is_empty() {
        return true;
    }
    // Marker fields are already lower-cased at emit time. Be defensive
    // anyway — round-tripping through serde_json doesn't change case
    // but a hand-edited fixture or future producer might.
    marker.org.eq_ignore_ascii_case(current_org_lc)
        && marker.repo.eq_ignore_ascii_case(current_repo_lc)
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
                org: String::new(),
                repo: String::new(),
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
        | DiscoveryStatus::NotFound
        | DiscoveryStatus::UnknownForbidden
        | DiscoveryStatus::UnknownRequiredParams
        | DiscoveryStatus::PreviewFailed(_) => return None,
    }

    // Join every marker's source path so consumers that include
    // multiple templates show up honestly in the CLI summary instead
    // of silently truncating to whichever marker happened to be
    // first. Also apply the canonical pipeline-command neutraliser:
    // the `yaml_path` ends up in `print_matched_summary` (which writes
    // to stdout), and if `ado-aw secrets set --all-repos` is ever
    // invoked from inside an ADO pipeline step, an attacker-controlled
    // marker source path containing `##vso[` would otherwise be
    // processed by the agent's logging-command scanner. Reusing the
    // shared helper keeps this in sync with every other sanitisation
    // surface (front matter, safe outputs, agent stats).
    let yaml_path = if d.markers.is_empty() {
        String::new()
    } else {
        let joined = d
            .markers
            .iter()
            .map(|m| m.source.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        crate::sanitize::neutralize_pipeline_commands(&joined)
    };

    Some(MatchedDefinition {
        id: d.definition_id,
        name: d.definition_name.clone(),
        match_method: MatchMethod::Discovery,
        yaml_path,
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
/// Normalisation is applied to the user-supplied value too
/// ([`crate::compile::normalize_source_path`] — forward-slash separators,
/// CR/LF stripping, leading `./` collapsed) so the common variants
/// (`./agents/foo.md`, `agents\foo.md` on Windows) match. Matching is
/// **case-sensitive** even on Windows; pass the path in the same case
/// it was compiled with.
///
/// Source-only matching is ambiguous when two repos in the same ADO
/// project happen to define a file of the same name (e.g. both have
/// `agents/foo.md`). To disambiguate, the marker carries the ADO
/// `org` and `repo` of the compiling repository (lower-cased). When
/// `source_filter` is active, the marker's `(org, repo)` must also
/// equal `ctx`'s — i.e. the operator gets only consumers whose
/// template originated in the **current repo**. Markers with empty
/// `org` / `repo` (legacy or non-ADO compilers) match leniently so
/// pre-existing deployments are not silently excluded; once everything
/// is recompiled with this version, the match becomes strict.
///
/// Skip-summary warnings are emitted differently depending on whether
/// `source_filter` is active:
///
/// - **Unfiltered (`--all-repos` alone)**: every `UnknownRequiredParams`
///   / `UnknownForbidden` / `PreviewFailed` definition is counted —
///   under `--all-repos` the user is operating on every ado-aw pipeline
///   in scope, so each failure represents a real skip.
///
/// - **Filtered (`--source <path>`)**: we can't tell whether a failed
///   definition would have been a consumer of `path` because we never
///   got markers out of it. Emitting per-status counts would mislead
///   the user into thinking they're missing consumers of their
///   template. Instead, emit a single conservative warning ("N
///   definitions could not be inspected; consumers of `<path>` among
///   them may have been silently skipped") so the operator is informed
///   without being told false specifics.
pub async fn resolve_definitions_via_discovery(
    client: &reqwest::Client,
    ctx: &AdoContext,
    auth: &AdoAuth,
    scope: DiscoveryScope,
    local_lock_paths: Option<&[PathBuf]>,
    source_filter: Option<&str>,
) -> Result<Vec<MatchedDefinition>> {
    let discovered = discover_ado_aw_pipelines(client, ctx, auth, scope, local_lock_paths).await?;

    // Normalize the user-supplied `--source` value through the same
    // canonical form the compiler uses for the marker JSON's `source`
    // field. Without this, `--source ./agents/foo.md` or
    // `--source agents\foo.md` (Windows) silently matches nothing
    // because the marker stores `agents/foo.md`.
    let normalized_filter: Option<String> = source_filter
        .map(|s| crate::compile::normalize_source_path(Path::new(s)));

    // Origin scoping: when filtering by `--source`, also require the
    // marker's (org, repo) to identify the current repository. This
    // disambiguates the source field when two repos in the same
    // project define files of the same name. Lower-cased to align with
    // the marker's lower-casing at emit time (ADO identifiers are
    // case-insensitive). Markers with empty fields (legacy / non-ADO
    // compiles) match leniently so already-deployed pipelines remain
    // discoverable until they are recompiled.
    let current_org = ctx
        .org_name()
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();
    let current_repo = ctx.repo_name.to_ascii_lowercase();

    // Pass 1: classify each discovered definition into "keep / skip
    // silently / skip with reason". The previous shape stuffed all of
    // this into a side-effecting `.filter()` closure that mutated
    // counters while deciding inclusion — explicit two-pass form keeps
    // the counts honestly derived from the same iteration and makes it
    // obvious what ends up in the returned vec.
    //
    // The per-status counters (`uninspectable_required_params` /
    // `_forbidden` / `_failed`) tally Preview failures by reason. They
    // intentionally do NOT distinguish "ado-aw consumer" from
    // "unrelated pipeline" — Preview failed for these, so we have no
    // markers to tell which is which. A non-ado-aw project may have
    // hundreds of definitions that legitimately require
    // templateParameters; we can't claim any of them were ado-aw
    // consumers without inspecting them, so the warning text below is
    // written to be honest about that uncertainty.
    let mut uninspectable_required_params = 0usize;
    let mut uninspectable_forbidden = 0usize;
    let mut uninspectable_failed = 0usize;
    let mut selected: Vec<DiscoveredPipeline> = Vec::with_capacity(discovered.len());

    for d in discovered {
        let matches_filter = match normalized_filter.as_deref() {
            Some(src) => d.markers.iter().any(|m| {
                m.source == src && marker_origin_matches(m, &current_org, &current_repo)
            }),
            None => true,
        };

        match d.status {
            DiscoveryStatus::UnknownRequiredParams => uninspectable_required_params += 1,
            DiscoveryStatus::UnknownForbidden => uninspectable_forbidden += 1,
            DiscoveryStatus::PreviewFailed(_) => uninspectable_failed += 1,
            _ => {}
        }

        if matches_filter {
            selected.push(d);
        }
    }

    let uninspectable =
        uninspectable_required_params + uninspectable_forbidden + uninspectable_failed;

    // Pass 2: emit a single warning that's honest about uncertainty,
    // and surface the per-status breakdown at debug level for
    // operators who want to know whether the misses were
    // permission-related or template-parameter-related.
    //
    // Previously we emitted three separate warn-level messages keyed
    // on the per-status counts (e.g. "Discovery skipped N definitions
    // whose Pipeline Preview requires templateParameters") — but in
    // `--all-repos` mode that's misleading: a project with hundreds of
    // non-ado-aw pipelines that legitimately require parameters would
    // make the operator think they'd missed N ado-aw consumers, when
    // none of them were ado-aw in the first place. We can't tell
    // which is which without successful Preview output.
    if uninspectable > 0 {
        match normalized_filter.as_deref() {
            Some(src) => warn!(
                "Discovery could not inspect {uninspectable} definition(s) (Preview failure, \
                 forbidden, or required-parameters); any consumers of `{src}` among them have \
                 been silently skipped. Re-run with --debug for per-definition reasons.",
            ),
            None => warn!(
                "Discovery could not inspect {uninspectable} definition(s) (Preview failure, \
                 forbidden, or required-parameters); any ado-aw pipelines among them have been \
                 silently skipped. Re-run with --debug for per-definition reasons.",
            ),
        }
        debug!(
            "Uninspectable breakdown: {uninspectable_required_params} required-parameters, \
             {uninspectable_forbidden} forbidden, {uninspectable_failed} other Preview errors.",
        );
    }

    Ok(selected.iter().filter_map(discovered_to_matched).collect())
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
            ..Default::default()
        }];
        assert!(is_direct_match(&def, &markers));
    }

    #[test]
    fn direct_when_yaml_filename_has_leading_slash() {
        // ADO sometimes returns yamlFilename with a leading slash. The
        // `normalize_ado_yaml_path` helper strips it, so equality with
        // the derived `<stem>.lock.yml` still holds.
        let def = def_with(1, "a", Some("/agents/foo.lock.yml"), None);
        let markers = vec![MarkerMetadata {
            schema: 1,
            source: "agents/foo.md".to_string(),
            version: "0.30.0".to_string(),
            target: "standalone".to_string(),
            ..Default::default()
        }];
        assert!(is_direct_match(&def, &markers));
    }

    #[test]
    fn consumer_when_same_stem_in_different_directory() {
        // Regression: previously `yaml_normalized.ends_with("/{stem}")`
        // would mislabel a Consumer pipeline as Direct whenever a
        // same-named lock file lived under any unrelated prefix
        // (e.g. marker `agents/foo.md` + yamlFilename
        // `other/agents/foo.lock.yml`). The fix requires strict
        // equality after normalisation.
        let def = def_with(1, "a", Some("other/agents/foo.lock.yml"), None);
        let markers = vec![MarkerMetadata {
            schema: 1,
            source: "agents/foo.md".to_string(),
            version: "0.30.0".to_string(),
            target: "standalone".to_string(),
            ..Default::default()
        }];
        assert!(!is_direct_match(&def, &markers));
    }

    #[test]
    fn consumer_when_yaml_filename_does_not_match_marker() {
        let def = def_with(1, "a", Some("/release-readiness.yml"), None);
        let markers = vec![MarkerMetadata {
            schema: 1,
            source: "agents/foo.md".to_string(),
            version: "0.30.0".to_string(),
            target: "stage".to_string(),
            ..Default::default()
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
                ..Default::default()
            },
            MarkerMetadata {
                schema: 1,
                source: "agents/bar.md".to_string(),
                version: "0.30.0".to_string(),
                target: "job".to_string(),
                ..Default::default()
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

    // ── source_filter normalization ──────────────────────────────────

    #[test]
    fn source_filter_normalization_matches_marker_form() {
        // The marker stores normalized form (`agents/foo.md`). Verify
        // that the same normalization applied to user input produces
        // matchable strings for the common variants.
        use crate::compile::normalize_source_path;
        use std::path::Path;

        let canonical = normalize_source_path(Path::new("agents/foo.md"));
        assert_eq!(canonical, "agents/foo.md");

        // Leading `./` is stripped.
        assert_eq!(
            normalize_source_path(Path::new("./agents/foo.md")),
            canonical
        );

        // Backslashes are normalized to forward slashes.
        assert_eq!(
            normalize_source_path(Path::new(r"agents\foo.md")),
            canonical
        );
    }

    // ── marker_origin_matches ────────────────────────────────────────

    #[test]
    fn origin_matches_strict_when_marker_has_org_and_repo() {
        let marker = MarkerMetadata {
            org: "myorg".to_string(),
            repo: "templates-a".to_string(),
            source: "agents/foo.md".to_string(),
            ..Default::default()
        };
        assert!(marker_origin_matches(&marker, "myorg", "templates-a"));
        assert!(!marker_origin_matches(&marker, "myorg", "templates-b"));
        assert!(!marker_origin_matches(&marker, "otherorg", "templates-a"));
    }

    #[test]
    fn origin_matches_case_insensitively() {
        // ADO identifiers are case-insensitive. Marker fields are
        // lower-cased at emit time, but a fixture or hand-edited
        // marker might carry uppercase — accept either.
        let marker = MarkerMetadata {
            org: "MyOrg".to_string(),
            repo: "Templates-A".to_string(),
            source: "agents/foo.md".to_string(),
            ..Default::default()
        };
        assert!(marker_origin_matches(&marker, "myorg", "templates-a"));
    }

    #[test]
    fn origin_matches_leniently_when_marker_org_repo_empty() {
        // Legacy markers (pre-org/repo embed) and markers compiled
        // outside an ADO checkout carry empty org/repo. Match anything
        // so existing deployments keep working until recompiled.
        let marker = MarkerMetadata {
            source: "agents/foo.md".to_string(),
            ..Default::default()
        };
        assert!(marker_origin_matches(&marker, "myorg", "templates-a"));
        assert!(marker_origin_matches(&marker, "", ""));
    }

    #[test]
    fn origin_matches_strictly_when_only_one_field_empty() {
        // If only one half of (org, repo) is set, we treat the marker
        // as non-legacy and require both to match. Pre-empts a
        // malformed fixture passing through the lenient path.
        let half_marker = MarkerMetadata {
            org: "myorg".to_string(),
            repo: String::new(),
            source: "agents/foo.md".to_string(),
            ..Default::default()
        };
        assert!(!marker_origin_matches(&half_marker, "myorg", "templates-a"));
    }

    // ── discovered_to_matched ────────────────────────────────────────

    fn discovered(status: DiscoveryStatus) -> DiscoveredPipeline {
        DiscoveredPipeline {
            definition_id: 42,
            definition_name: "test".to_string(),
            repository_url: None,
            queue_status: None,
            markers: vec![],
            status,
        }
    }

    #[test]
    fn discovered_to_matched_drops_not_found() {
        // 404 from Preview (definition deleted in flight) must not
        // surface as a matched definition that a write command would
        // act on — there's nothing to act on.
        assert!(discovered_to_matched(&discovered(DiscoveryStatus::NotFound)).is_none());
    }

    #[test]
    fn discovered_to_matched_drops_unactionable_statuses() {
        for status in [
            DiscoveryStatus::NotAdoAw,
            DiscoveryStatus::NotFound,
            DiscoveryStatus::UnknownForbidden,
            DiscoveryStatus::UnknownRequiredParams,
            DiscoveryStatus::PreviewFailed("boom".to_string()),
        ] {
            assert!(
                discovered_to_matched(&discovered(status.clone())).is_none(),
                "expected {status:?} to map to None"
            );
        }
    }

    #[test]
    fn discovered_to_matched_keeps_direct_and_consumer() {
        assert!(discovered_to_matched(&discovered(DiscoveryStatus::Direct)).is_some());
        assert!(discovered_to_matched(&discovered(DiscoveryStatus::Consumer)).is_some());
    }

    #[test]
    fn discovered_to_matched_joins_multiple_marker_sources() {
        // A consumer that includes two templates must surface both
        // sources in the yaml_path summary, not silently truncate to
        // whichever happened to be first.
        let mut d = discovered(DiscoveryStatus::Consumer);
        d.markers = vec![
            MarkerMetadata {
                schema: 1,
                source: "agents/a.md".to_string(),
                version: "1.0".to_string(),
                target: "job".to_string(),
                ..Default::default()
            },
            MarkerMetadata {
                schema: 1,
                source: "agents/b.md".to_string(),
                version: "1.0".to_string(),
                target: "stage".to_string(),
                ..Default::default()
            },
        ];
        let matched = discovered_to_matched(&d).expect("Consumer kept");
        assert!(
            matched.yaml_path.contains("agents/a.md")
                && matched.yaml_path.contains("agents/b.md"),
            "expected both marker sources in yaml_path, got: {}",
            matched.yaml_path
        );
    }

    #[test]
    fn discovered_to_matched_sanitises_vso_in_yaml_path() {
        // The yaml_path ends up in stdout via print_matched_summary.
        // If the CLI is invoked from inside an ADO pipeline step, an
        // attacker-controlled marker source path containing `##vso[`
        // would otherwise be processed by the agent's logging-command
        // scanner.
        //
        // The canonical `neutralize_pipeline_commands` wraps the
        // prefix in backticks (`` `##vso[` ``) — the literal `##vso[`
        // token no longer matches the agent's scanner. The canonical
        // helper's own behaviour is exhaustively tested in
        // `src/sanitize.rs`; this test is just the integration point.
        let mut d = discovered(DiscoveryStatus::Consumer);
        d.markers = vec![MarkerMetadata {
            schema: 1,
            source: "agents/##vso[task.setvariable variable=X]value.md".to_string(),
            version: "1.0".to_string(),
            target: "job".to_string(),
            ..Default::default()
        }];
        let matched = discovered_to_matched(&d).expect("Consumer kept");
        assert!(
            !matched.yaml_path.contains("agents/##vso["),
            "raw ##vso[ leaked into yaml_path: {}",
            matched.yaml_path,
        );
        assert!(
            matched.yaml_path.contains("`##vso[`"),
            "expected `##vso[` neutralised via canonical backtick-wrap: {}",
            matched.yaml_path,
        );
    }

    // ── normalize_repo_url ───────────────────────────────────────────

    #[test]
    fn normalize_repo_url_is_encoding_independent() {
        // ADO usually returns percent-encoded URLs (`My%20Project`),
        // but the comparison must work whichever shape both sides
        // happen to be in.
        let encoded = "https://dev.azure.com/Org/My%20Project/_git/Repo";
        let decoded = "https://dev.azure.com/Org/My Project/_git/Repo";
        assert_eq!(normalize_repo_url(encoded), normalize_repo_url(decoded));
    }

    #[test]
    fn normalize_repo_url_is_case_insensitive_and_trims_trailing_slash() {
        assert_eq!(
            normalize_repo_url("https://dev.azure.com/Org/P/_git/Repo/"),
            normalize_repo_url("https://dev.azure.com/org/p/_git/repo")
        );
    }
}
