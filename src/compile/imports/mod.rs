//! Compile-time resolution for `imports:` entries.
//!
//! This module deliberately stops at resolution: it fetches/loads the imported
//! markdown manifest, parses its front matter and body, and records provenance.
//! Merging imported content into the consumer workflow is a later compile pass.
#![allow(dead_code)]

pub mod alias;
#[cfg(test)]
mod integration_tests;
pub mod merge;
pub mod schema;

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::Deserialize;

use crate::compile::types::{ImportEndpoint, ImportEntry, ImportSource, ParsedImportSpec};
use crate::hash::sha256_hex;

const MAX_IMPORTS_PER_WORKFLOW: usize = 20;
const MAX_MANIFEST_BYTES: usize = 256 * 1024;
const IMPORT_GITATTRIBUTES: &str = "# Mark all cached import files as generated\n\
* linguist-generated=true\n\
# Keep local cached versions on merge\n\
* merge=ours\n";

/// Fetches a single SHA-pinned component manifest.
///
/// `Send + Sync` so a `&dyn ManifestFetcher` can be held across an await in a
/// `Send` future (e.g. `build_pipeline_ir`, which the `mcp-author` tool router
/// spawns on a multi-threaded runtime).
#[async_trait]
pub trait ManifestFetcher: Send + Sync {
    async fn fetch(&self, spec: &ParsedImportSpec) -> Result<Vec<u8>>;
}

/// GitHub Contents API-backed manifest fetcher using the author's `gh` auth.
///
/// Handles both GitHub.com ([`ImportEndpoint::GitHub`]) and GitHub Enterprise
/// ([`ImportEndpoint::GitHubEnterprise`]) sources; for GHE the target API host
/// is passed to `gh` via the `GH_HOST` environment variable.
pub struct GhCliFetcher;

#[async_trait]
impl ManifestFetcher for GhCliFetcher {
    async fn fetch(&self, spec: &ParsedImportSpec) -> Result<Vec<u8>> {
        let route = format!(
            "repos/{}/{}/contents/{}?ref={}",
            spec.owner,
            spec.repo,
            spec.path,
            spec.sha.as_str()
        );

        let mut command = tokio::process::Command::new("gh");
        command.args(["api", &route]);
        // GitHub Enterprise: target the configured API host. `GH_HOST` makes
        // `gh api` resolve the relative route against that instance.
        if let Some(ImportEndpoint::GitHubEnterprise { host, .. }) = &spec.endpoint {
            command.env("GH_HOST", host.as_str());
        }

        let output = command
            .output()
            .await
            .with_context(|| format!("failed to run `gh api {route}` for import manifest"))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!(
                "`gh api {}` failed with status {}: {}",
                route,
                output.status,
                stderr.trim()
            );
        }

        #[derive(Deserialize)]
        struct ContentsResponse {
            content: String,
            #[serde(default)]
            encoding: Option<String>,
        }

        let response: ContentsResponse = serde_json::from_slice(&output.stdout)
            .with_context(|| format!("failed to parse GitHub Contents API response for {route}"))?;
        if response.encoding.as_deref().unwrap_or("base64") != "base64" {
            anyhow::bail!(
                "GitHub Contents API response for {} used unsupported encoding {:?}",
                route,
                response.encoding
            );
        }

        let compact_content: String = response
            .content
            .chars()
            .filter(|ch| !ch.is_whitespace())
            .collect();
        STANDARD
            .decode(compact_content.as_bytes())
            .with_context(|| {
                format!("failed to base64-decode GitHub Contents API response for {route}")
            })
    }
}

/// Azure Repos-backed manifest fetcher — the **primary** compile-time source.
///
/// Fetches a SHA-pinned manifest from the ADO Git Items API. Endpoint-less
/// imports resolve against the consumer's own organization (from `repo_root`);
/// [`ImportEndpoint::AzureReposCrossOrg`] imports resolve against the
/// organization named in the endpoint. The import spec's `owner` maps to the
/// ADO **project** and `repo` to the repository name.
///
/// The consumer org URL and non-interactive auth are resolved **lazily on the
/// first actual fetch** (and cached), so a fully-vendored committed cache — as
/// used by `ado-aw check` — performs no `git`/`az` subprocess or network work
/// (the cache is consulted before `fetch` is ever called). A resolution failure
/// is surfaced **fail-closed** at fetch time; an Azure-Repos-typed import never
/// silently falls back to GitHub.
pub struct AdoRepoFetcher {
    client: reqwest::Client,
    /// Repo root used to infer the consumer org for same-org imports.
    repo_root: PathBuf,
    /// Lazily-resolved consumer organization collection URL (same-org imports).
    context_org_url: tokio::sync::OnceCell<std::result::Result<String, String>>,
    /// Lazily-resolved non-interactive ADO auth.
    auth: tokio::sync::OnceCell<std::result::Result<crate::ado::AdoAuth, String>>,
}

impl AdoRepoFetcher {
    /// Construct a fetcher that resolves org/auth lazily on first fetch, using
    /// `repo_root` to infer the consumer organization for same-org imports.
    pub fn new(repo_root: PathBuf) -> Self {
        Self {
            client: reqwest::Client::new(),
            repo_root,
            context_org_url: tokio::sync::OnceCell::new(),
            auth: tokio::sync::OnceCell::new(),
        }
    }

    /// Test constructor with the org URL + auth pre-resolved (no subprocess).
    #[cfg(test)]
    pub fn with_resolved(
        context_org_url: std::result::Result<String, String>,
        auth: std::result::Result<crate::ado::AdoAuth, String>,
    ) -> Self {
        Self {
            client: reqwest::Client::new(),
            repo_root: PathBuf::new(),
            context_org_url: tokio::sync::OnceCell::new_with(Some(context_org_url)),
            auth: tokio::sync::OnceCell::new_with(Some(auth)),
        }
    }

    /// Consumer org URL for same-org imports, resolved + cached on first use.
    async fn context_org_url(&self) -> &std::result::Result<String, String> {
        self.context_org_url
            .get_or_init(|| async {
                crate::ado::resolve_ado_context(&self.repo_root, None, None)
                    .await
                    .map(|ctx| ctx.org_url)
                    .map_err(|e| format!("{e:#}"))
            })
            .await
    }

    /// Non-interactive ADO auth, resolved + cached on first use.
    async fn auth(&self) -> &std::result::Result<crate::ado::AdoAuth, String> {
        self.auth
            .get_or_init(|| async {
                crate::ado::resolve_auth_non_interactive()
                    .await
                    .map_err(|e| format!("{e:#}"))
            })
            .await
    }
}

#[async_trait]
impl ManifestFetcher for AdoRepoFetcher {
    async fn fetch(&self, spec: &ParsedImportSpec) -> Result<Vec<u8>> {
        // Fail-closed BEFORE any lazy org/auth resolution: a GitHub/GHE-typed
        // import must never reach the Azure Repos fetcher.
        if let Some(other @ (ImportEndpoint::GitHub { .. } | ImportEndpoint::GitHubEnterprise { .. })) =
            &spec.endpoint
        {
            anyhow::bail!(
                "internal routing error: Azure Repos fetcher received a {:?} import",
                other
            );
        }

        let org_url = match &spec.endpoint {
            None => self.context_org_url().await.as_deref().map_err(|reason| {
                anyhow::anyhow!(
                    "cannot fetch same-org Azure Repos import `{}/{}/{}`: {}. \
                     Set AZURE_DEVOPS_ORG_URL / SYSTEM_COLLECTIONURI or run from an \
                     Azure Repos checkout; to import from GitHub, add an `endpoint:`.",
                    spec.owner,
                    spec.repo,
                    spec.path,
                    reason
                )
            })?,
            Some(ImportEndpoint::AzureReposCrossOrg { org, .. }) => org.as_str(),
            Some(_) => unreachable!("github/ghe rejected above"),
        };

        let auth = self.auth().await.as_ref().map_err(|reason| {
            anyhow::anyhow!(
                "cannot authenticate to Azure Repos for import `{}/{}/{}`: {}",
                spec.owner,
                spec.repo,
                spec.path,
                reason
            )
        })?;

        crate::ado::fetch_git_item(
            &self.client,
            org_url,
            &spec.owner,
            &spec.repo,
            &spec.path,
            spec.sha.as_str(),
            auth,
        )
        .await
        .with_context(|| {
            format!(
                "failed to fetch Azure Repos import manifest `{}/{}/{}@{}`",
                spec.owner,
                spec.repo,
                spec.path,
                spec.sha.as_str()
            )
        })
    }
}

/// Routes each import to the correct fetcher based on its typed endpoint,
/// guaranteeing the compile-time fetch source matches the runtime checkout
/// source (see [`crate::compile::imports::alias`]).
///
/// - endpoint-less / [`ImportEndpoint::AzureReposCrossOrg`] → [`AdoRepoFetcher`]
/// - [`ImportEndpoint::GitHub`] / [`ImportEndpoint::GitHubEnterprise`] → [`GhCliFetcher`]
///
/// **Fail-closed:** an Azure-Repos-intended (endpoint-less) import can never be
/// silently served by GitHub, eliminating the source-confusion class of bug.
pub struct RoutingFetcher {
    ado: AdoRepoFetcher,
    github: GhCliFetcher,
}

impl RoutingFetcher {
    pub fn new(ado: AdoRepoFetcher) -> Self {
        Self {
            ado,
            github: GhCliFetcher,
        }
    }
}

/// Which fetcher a given endpoint routes to. Extracted as a pure function so
/// the source-confusion guard (an Azure-Repos-intended import must never be
/// served by GitHub, and vice-versa) can be unit-tested without any network.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FetcherKind {
    /// Azure Repos (same-org endpoint-less, or cross-org).
    AzureRepos,
    /// GitHub.com or GitHub Enterprise.
    GitHub,
}

/// Classify an import endpoint to its fetcher. Endpoint-less imports are
/// same-org Azure Repos (primary); this is the single source of truth that
/// keeps compile-time fetch routing aligned with the runtime checkout in
/// [`crate::compile::imports::alias`].
pub fn route_endpoint(endpoint: &Option<ImportEndpoint>) -> FetcherKind {
    match endpoint {
        None | Some(ImportEndpoint::AzureReposCrossOrg { .. }) => FetcherKind::AzureRepos,
        Some(ImportEndpoint::GitHub { .. }) | Some(ImportEndpoint::GitHubEnterprise { .. }) => {
            FetcherKind::GitHub
        }
    }
}

#[async_trait]
impl ManifestFetcher for RoutingFetcher {
    async fn fetch(&self, spec: &ParsedImportSpec) -> Result<Vec<u8>> {
        match route_endpoint(&spec.endpoint) {
            FetcherKind::AzureRepos => self.ado.fetch(spec).await,
            FetcherKind::GitHub => self.github.fetch(spec).await,
        }
    }
}

/// A resolved import manifest plus source provenance.
#[derive(Debug, Clone)]
pub struct ResolvedImport {
    pub entry: ImportEntry,
    pub source: ImportSource,
    pub front_matter: serde_yaml::Value,
    pub body: String,
    pub provenance: ImportProvenance,
}

/// Audit provenance for a resolved import manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportProvenance {
    pub source: String,
    pub sha: Option<String>,
    pub manifest_digest: String,
}

/// Resolve top-level imports using `base_dir` for local paths and cache root.
///
/// Prefer [`resolve_imports_with_repo_root`] when the workflow directory is not
/// the repository root. This function is kept as the simple public entry point
/// for callers that compile from the repo root.
pub async fn resolve_imports(
    entries: &[ImportEntry],
    base_dir: &Path,
    fetcher: &dyn ManifestFetcher,
) -> Result<Vec<ResolvedImport>> {
    resolve_imports_with_repo_root(entries, base_dir, base_dir, fetcher).await
}

/// Resolve top-level imports using an explicit repo root for the committed
/// `.ado-aw/imports` cache.
///
/// TODO: nested imports (depth<=3). This pass intentionally resolves only the
/// workflow's declared top-level `imports:` list; transitive resolution will
/// layer on this entry point in a later merge pass.
pub async fn resolve_imports_with_repo_root(
    entries: &[ImportEntry],
    base_dir: &Path,
    repo_root: &Path,
    fetcher: &dyn ManifestFetcher,
) -> Result<Vec<ResolvedImport>> {
    if entries.len() > MAX_IMPORTS_PER_WORKFLOW {
        anyhow::bail!(
            "imports per workflow must be <= {}, got {}",
            MAX_IMPORTS_PER_WORKFLOW,
            entries.len()
        );
    }

    let mut resolved = Vec::new();
    for entry in entries {
        if let Some(import) = resolve_one(entry, base_dir, repo_root, fetcher)
            .await
            .with_context(|| format!("failed to resolve import `{}`", entry.uses))?
        {
            resolved.push(import);
        }
    }
    Ok(resolved)
}

async fn resolve_one(
    entry: &ImportEntry,
    base_dir: &Path,
    repo_root: &Path,
    fetcher: &dyn ManifestFetcher,
) -> Result<Option<ResolvedImport>> {
    let source = entry.parse_source()?;
    match &source {
        ImportSource::Local {
            path,
            section,
            optional,
        } => {
            let local_path = resolve_local_path(base_dir, path)?;
            let bytes = match fs::read(&local_path) {
                Ok(bytes) => bytes,
                Err(err) if *optional && err.kind() == std::io::ErrorKind::NotFound => {
                    return Ok(None);
                }
                Err(err) => {
                    return Err(err).with_context(|| {
                        format!("failed to read local import {}", local_path.display())
                    });
                }
            };
            let digest = sha256_hex(&bytes);
            let (front_matter, body) = parse_manifest_bytes(&bytes, section.as_deref())?;
            Ok(Some(ResolvedImport {
                entry: entry.clone(),
                source: source.clone(),
                front_matter,
                body,
                provenance: ImportProvenance {
                    source: path.clone(),
                    sha: None,
                    manifest_digest: digest,
                },
            }))
        }
        ImportSource::Remote(spec) => {
            let bytes = read_remote_manifest(repo_root, spec, fetcher).await?;
            let digest = sha256_hex(&bytes);
            let (front_matter, body) = parse_manifest_bytes(&bytes, spec.section.as_deref())?;
            Ok(Some(ResolvedImport {
                entry: entry.clone(),
                source: source.clone(),
                front_matter,
                body,
                provenance: ImportProvenance {
                    source: format!("{}/{}/{}", spec.owner, spec.repo, spec.path),
                    sha: Some(spec.sha.as_str().to_string()),
                    manifest_digest: digest,
                },
            }))
        }
    }
}

fn resolve_local_path(base_dir: &Path, import_path: &str) -> Result<PathBuf> {
    let path = Path::new(import_path);
    if path.is_absolute() {
        anyhow::bail!("local import path must be relative, got `{}`", import_path);
    }
    // Reject path-traversal: a `..`/`.` segment (or a backslash) would let a
    // local import escape the workflow directory and read arbitrary files at
    // compile time. Mirrors the guard `validate_import_path_segments` applies to
    // remote import paths.
    if import_path.contains('\\')
        || import_path
            .split('/')
            .any(|segment| segment.is_empty() || segment == "." || segment == "..")
    {
        anyhow::bail!(
            "local import path `{}` contains an invalid segment; `.`, `..`, empty \
             segments, and backslashes are not allowed",
            import_path
        );
    }
    Ok(base_dir.join(path))
}

async fn read_remote_manifest(
    repo_root: &Path,
    spec: &ParsedImportSpec,
    fetcher: &dyn ManifestFetcher,
) -> Result<Vec<u8>> {
    let cache_path = cache_path(repo_root, spec)?;
    if cache_path.exists() {
        let bytes = fs::read(&cache_path)
            .with_context(|| format!("failed to read cached import {}", cache_path.display()))?;
        enforce_manifest_size(bytes.len(), &cache_path.display().to_string())?;
        // Defense-in-depth: if a digest sidecar exists (written when ado-aw
        // populated the cache), verify the cached bytes still hash to it. This
        // detects tampering of the committed cache file — which GitHub collapses
        // in diffs (`linguist-generated`) — before the manifest is trusted at
        // compile time. A missing sidecar (older cache) is tolerated.
        let sidecar = digest_sidecar_path(&cache_path);
        if let Ok(expected) = fs::read_to_string(&sidecar) {
            let actual = sha256_hex(&bytes);
            if actual != expected.trim() {
                anyhow::bail!(
                    "cached import {} does not match its recorded digest (expected {}, got {}); \
                     the committed cache may have been tampered with — delete it to re-fetch",
                    cache_path.display(),
                    expected.trim(),
                    actual
                );
            }
        }
        return Ok(bytes);
    }

    let bytes = fetcher.fetch(spec).await?;
    enforce_manifest_size(
        bytes.len(),
        &format!("{}/{}/{}", spec.owner, spec.repo, spec.path),
    )?;

    let parent = cache_path
        .parent()
        .context("import cache path unexpectedly has no parent")?;
    fs::create_dir_all(parent).with_context(|| {
        format!(
            "failed to create import cache directory {}",
            parent.display()
        )
    })?;
    ensure_import_gitattributes(repo_root)?;
    fs::write(&cache_path, &bytes)
        .with_context(|| format!("failed to write cached import {}", cache_path.display()))?;
    // Record the digest sidecar so a later read can detect cache tampering.
    fs::write(digest_sidecar_path(&cache_path), sha256_hex(&bytes)).with_context(|| {
        format!(
            "failed to write import cache digest sidecar for {}",
            cache_path.display()
        )
    })?;
    Ok(bytes)
}

/// The `.sha256` sidecar path recording a cached manifest's content digest.
fn digest_sidecar_path(cache_path: &Path) -> PathBuf {
    let mut os = cache_path.as_os_str().to_os_string();
    os.push(".sha256");
    PathBuf::from(os)
}

fn enforce_manifest_size(size: usize, source: &str) -> Result<()> {
    if size > MAX_MANIFEST_BYTES {
        anyhow::bail!(
            "import manifest {} is {} bytes, exceeding the {} byte limit",
            source,
            size,
            MAX_MANIFEST_BYTES
        );
    }
    Ok(())
}

fn cache_path(repo_root: &Path, spec: &ParsedImportSpec) -> Result<PathBuf> {
    validate_cache_segment("owner", &spec.owner)?;
    validate_cache_segment("repo", &spec.repo)?;
    let mut path = repo_root
        .join(".ado-aw")
        .join("imports")
        .join(&spec.owner)
        .join(&spec.repo)
        .join(spec.sha.as_str());
    // Preserve the component's directory structure under the SHA dir (mirrors
    // the source repo) rather than flattening `/` -> `_`. Flattening is NOT
    // injective (`a/b.md` and `a_b.md` collide onto the same cache file), which
    // would silently serve one component's manifest for another from the same
    // repo+SHA and bypass the digest sidecar. `validate_import_path_segments`
    // enforces the same traversal guard so joining each segment is safe.
    for segment in validate_import_path_segments(&spec.path)? {
        path.push(segment);
    }
    Ok(path)
}

fn validate_cache_segment(label: &str, value: &str) -> Result<()> {
    if value.is_empty()
        || value == "."
        || value == ".."
        || value.contains('\\')
        || value.contains('/')
    {
        anyhow::bail!(
            "remote import {} contains an invalid path segment: `{}`",
            label,
            value
        );
    }
    Ok(())
}

/// Validate a remote import path and return its `/`-separated segments.
///
/// Rejects backslashes and any empty / `.` / `..` segment so the returned
/// segments can be joined onto the cache directory without escaping it.
fn validate_import_path_segments(path: &str) -> Result<Vec<&str>> {
    if path.is_empty() || path.contains('\\') {
        anyhow::bail!("remote import path contains an invalid segment: `{}`", path);
    }
    let segments: Vec<&str> = path.split('/').collect();
    if segments
        .iter()
        .any(|segment| segment.is_empty() || *segment == "." || *segment == "..")
    {
        anyhow::bail!("remote import path contains an invalid segment: `{}`", path);
    }
    Ok(segments)
}

fn ensure_import_gitattributes(repo_root: &Path) -> Result<()> {
    let imports_dir = repo_root.join(".ado-aw").join("imports");
    fs::create_dir_all(&imports_dir).with_context(|| {
        format!(
            "failed to create import cache attributes directory {}",
            imports_dir.display()
        )
    })?;
    let attributes_path = imports_dir.join(".gitattributes");
    if !attributes_path.exists() {
        fs::write(&attributes_path, IMPORT_GITATTRIBUTES).with_context(|| {
            format!(
                "failed to write import cache attributes {}",
                attributes_path.display()
            )
        })?;
    }
    Ok(())
}

fn parse_manifest_bytes(
    bytes: &[u8],
    section: Option<&str>,
) -> Result<(serde_yaml::Value, String)> {
    enforce_manifest_size(bytes.len(), "resolved import manifest")?;
    let content =
        std::str::from_utf8(bytes).context("import manifest must be valid UTF-8 markdown")?;
    let parts = super::common::split_markdown_front_matter(content, false)?;
    let front_matter = match parts.yaml_raw {
        Some(yaml) => {
            let value: serde_yaml::Value =
                serde_yaml::from_str(&yaml).context("failed to parse import YAML front matter")?;
            match value {
                serde_yaml::Value::Mapping(ref mapping) => {
                    // Transitive/nested imports are not yet resolved (only a
                    // workflow's own top-level `imports:` is). Rather than
                    // silently ignore a component's own `imports:` — which would
                    // drop tools/config it depends on with no diagnostic — fail
                    // loudly until nested resolution lands.
                    if mapping.contains_key(serde_yaml::Value::String("imports".to_string())) {
                        anyhow::bail!(
                            "imported component declares its own `imports:`, but nested \
                             (transitive) imports are not yet supported; flatten the \
                             component or inline the nested import"
                        );
                    }
                    value
                }
                serde_yaml::Value::Null => value,
                other => {
                    anyhow::bail!(
                        "import YAML front matter must be a mapping/object, got {}",
                        yaml_value_kind(&other)
                    );
                }
            }
        }
        None => serde_yaml::Value::Null,
    };

    let body = match section {
        Some(name) => extract_markdown_section(&parts.markdown_body, name)?,
        None => parts.markdown_body,
    };
    Ok((front_matter, body))
}

pub(super) fn yaml_value_kind(value: &serde_yaml::Value) -> &'static str {
    match value {
        serde_yaml::Value::Null => "null",
        serde_yaml::Value::Bool(_) => "boolean",
        serde_yaml::Value::Number(_) => "number",
        serde_yaml::Value::String(_) => "string",
        serde_yaml::Value::Sequence(_) => "sequence/array",
        serde_yaml::Value::Mapping(_) => "mapping/object",
        serde_yaml::Value::Tagged(_) => "tagged value",
    }
}

/// Extract a markdown `# Name` / `## Name` section, including its heading.
fn extract_markdown_section(body: &str, section: &str) -> Result<String> {
    let lines: Vec<&str> = body.lines().collect();
    let start = lines
        .iter()
        .position(|line| markdown_heading(line).is_some_and(|(_, name)| name == section))
        .ok_or_else(|| anyhow::anyhow!("import section `{}` was not found", section))?;
    let start_level = markdown_heading(lines[start])
        .map(|(level, _)| level)
        .ok_or_else(|| {
            anyhow::anyhow!("import section `{}` heading could not be re-parsed", section)
        })?;

    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find_map(|(idx, line)| match markdown_heading(line) {
            Some((level, _)) if level <= start_level => Some(idx),
            _ => None,
        })
        .unwrap_or(lines.len());

    Ok(lines[start..end].join("\n").trim().to_string())
}

fn markdown_heading(line: &str) -> Option<(usize, &str)> {
    let trimmed = line.trim_start();
    let level = trimmed.bytes().take_while(|byte| *byte == b'#').count();
    if !(level == 1 || level == 2) {
        return None;
    }
    let rest = &trimmed[level..];
    if !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let name = rest
        .trim()
        .trim_end_matches('#')
        .trim_end()
        .trim()
        .trim_end_matches('\r');
    if name.is_empty() {
        return None;
    }
    Some((level, name))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    struct StaticFetcher {
        bytes: Vec<u8>,
    }

    #[async_trait]
    impl ManifestFetcher for StaticFetcher {
        async fn fetch(&self, _spec: &ParsedImportSpec) -> Result<Vec<u8>> {
            Ok(self.bytes.clone())
        }
    }

    struct PanicFetcher;

    #[async_trait]
    impl ManifestFetcher for PanicFetcher {
        async fn fetch(&self, _spec: &ParsedImportSpec) -> Result<Vec<u8>> {
            panic!("fetcher must not be called on cache hit")
        }
    }

    fn import_entry(uses: &str) -> ImportEntry {
        ImportEntry {
            uses: uses.to_string(),
            with: serde_json::Map::new(),
            endpoint: None,
        }
    }

    fn manifest() -> &'static [u8] {
        b"---\nimport-schema:\n  region:\n    type: string\n---\n# Imported\nBody\n"
    }

    #[tokio::test]
    async fn local_import_resolves_front_matter_body_and_provenance() {
        let repo = tempfile::tempdir().unwrap();
        let workflow_dir = repo.path().join("workflows");
        fs::create_dir_all(&workflow_dir).unwrap();
        let import_path = workflow_dir.join("component.md");
        fs::write(&import_path, manifest()).unwrap();

        let resolved = resolve_imports_with_repo_root(
            &[import_entry("component.md")],
            &workflow_dir,
            repo.path(),
            &PanicFetcher,
        )
        .await
        .unwrap();

        assert_eq!(resolved.len(), 1);
        assert!(resolved[0].front_matter["import-schema"].is_mapping());
        assert_eq!(resolved[0].body, "# Imported\nBody");
        assert_eq!(resolved[0].provenance.source, "component.md");
        assert_eq!(resolved[0].provenance.sha, None);
        assert_eq!(
            resolved[0].provenance.manifest_digest,
            sha256_hex(manifest())
        );
    }

    #[tokio::test]
    async fn remote_import_fetches_writes_cache_attributes_and_records_digest() {
        let repo = tempfile::tempdir().unwrap();
        let entry = import_entry(&format!("acme/shared/components/deploy.md@{SHA}"));
        let fetcher = StaticFetcher {
            bytes: manifest().to_vec(),
        };

        let resolved =
            resolve_imports_with_repo_root(&[entry], repo.path(), repo.path(), &fetcher)
                .await
                .unwrap();

        let cache_file = repo
            .path()
            .join(".ado-aw")
            .join("imports")
            .join("acme")
            .join("shared")
            .join(SHA)
            .join("components")
            .join("deploy.md");
        assert!(cache_file.exists());
        assert_eq!(fs::read(&cache_file).unwrap(), manifest());
        let attributes = fs::read_to_string(
            repo.path()
                .join(".ado-aw")
                .join("imports")
                .join(".gitattributes"),
        )
        .unwrap();
        assert_eq!(attributes, IMPORT_GITATTRIBUTES);
        assert_eq!(
            resolved[0].provenance.source,
            "acme/shared/components/deploy.md"
        );
        assert_eq!(resolved[0].provenance.sha.as_deref(), Some(SHA));
        assert_eq!(
            resolved[0].provenance.manifest_digest,
            sha256_hex(manifest())
        );
    }

    #[tokio::test]
    async fn remote_import_uses_cache_before_fetching() {
        let repo = tempfile::tempdir().unwrap();
        let entry = import_entry(&format!("acme/shared/components/deploy.md@{SHA}"));
        let cache_dir = repo
            .path()
            .join(".ado-aw")
            .join("imports")
            .join("acme")
            .join("shared")
            .join(SHA);
        fs::create_dir_all(cache_dir.join("components")).unwrap();
        fs::write(cache_dir.join("components").join("deploy.md"), manifest()).unwrap();

        let resolved =
            resolve_imports_with_repo_root(&[entry], repo.path(), repo.path(), &PanicFetcher)
                .await
                .unwrap();

        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].body, "# Imported\nBody");
    }

    #[tokio::test]
    async fn colliding_flattened_paths_get_distinct_cache_files() {
        // Regression: `a/b.md` and `a_b.md` from the same repo+SHA previously
        // flattened to the same cache file (`a_b.md`), silently serving one
        // component's manifest for the other. The preserved directory structure
        // gives them distinct cache paths.
        let repo = tempfile::tempdir().unwrap();
        let slash = import_entry(&format!("acme/shared/a/b.md@{SHA}"));
        let under = import_entry(&format!("acme/shared/a_b.md@{SHA}"));
        let fetcher = StaticFetcher {
            bytes: manifest().to_vec(),
        };
        resolve_imports_with_repo_root(
            std::slice::from_ref(&slash),
            repo.path(),
            repo.path(),
            &fetcher,
        )
        .await
        .unwrap();
        resolve_imports_with_repo_root(
            std::slice::from_ref(&under),
            repo.path(),
            repo.path(),
            &fetcher,
        )
        .await
        .unwrap();

        let base = repo
            .path()
            .join(".ado-aw")
            .join("imports")
            .join("acme")
            .join("shared")
            .join(SHA);
        // `a/b.md` -> nested; `a_b.md` -> flat sibling. Distinct files.
        assert!(base.join("a").join("b.md").exists());
        assert!(base.join("a_b.md").exists());
    }

    #[tokio::test]
    async fn size_cap_rejects_large_manifest() {
        let repo = tempfile::tempdir().unwrap();
        let entry = import_entry(&format!("acme/shared/component.md@{SHA}"));
        let fetcher = StaticFetcher {
            bytes: vec![b'x'; MAX_MANIFEST_BYTES + 1],
        };

        let err = resolve_imports_with_repo_root(&[entry], repo.path(), repo.path(), &fetcher)
            .await
            .unwrap_err();

        assert!(format!("{err:#}").contains("exceeding the 262144 byte limit"));
    }

    #[tokio::test]
    async fn nested_imports_in_component_are_rejected() {
        let repo = tempfile::tempdir().unwrap();
        fs::write(
            repo.path().join("component.md"),
            b"---\nimports:\n  - other.md\n---\n# Body\n",
        )
        .unwrap();

        let err = resolve_imports_with_repo_root(
            &[import_entry("component.md")],
            repo.path(),
            repo.path(),
            &PanicFetcher,
        )
        .await
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("nested (transitive) imports are not yet supported"),
            "got: {err:#}"
        );
    }

    #[tokio::test]
    async fn tampered_cache_is_rejected_via_digest_sidecar() {
        let repo = tempfile::tempdir().unwrap();
        let entry = import_entry(&format!("acme/shared/components/deploy.md@{SHA}"));
        let fetcher = StaticFetcher {
            bytes: manifest().to_vec(),
        };
        // First resolve populates the cache + digest sidecar.
        resolve_imports_with_repo_root(std::slice::from_ref(&entry), repo.path(), repo.path(), &fetcher)
            .await
            .unwrap();

        // Tamper with the committed cache file (sidecar still records the
        // original digest).
        let cache_file = repo
            .path()
            .join(".ado-aw")
            .join("imports")
            .join("acme")
            .join("shared")
            .join(SHA)
            .join("components")
            .join("deploy.md");
        fs::write(&cache_file, b"---\n{}\n---\n# Tampered\nevil\n").unwrap();

        let err =
            resolve_imports_with_repo_root(&[entry], repo.path(), repo.path(), &PanicFetcher)
                .await
                .unwrap_err();
        assert!(
            format!("{err:#}").contains("does not match its recorded digest"),
            "got: {err:#}"
        );
    }

    #[tokio::test]
    async fn section_selector_extracts_only_that_section() {
        let repo = tempfile::tempdir().unwrap();
        let workflow_dir = repo.path();
        fs::write(
            workflow_dir.join("component.md"),
            b"---\n{}\n---\n# One\none\n## Two\ntwo\n### Detail\nkeep\n## Three\nthree\n",
        )
        .unwrap();

        let resolved = resolve_imports_with_repo_root(
            &[import_entry("component.md#Two")],
            workflow_dir,
            repo.path(),
            &PanicFetcher,
        )
        .await
        .unwrap();

        assert_eq!(resolved[0].body, "## Two\ntwo\n### Detail\nkeep");
    }

    #[tokio::test]
    async fn optional_missing_local_import_is_skipped_and_required_missing_errors() {
        let repo = tempfile::tempdir().unwrap();

        let optional = resolve_imports_with_repo_root(
            &[import_entry("missing.md?")],
            repo.path(),
            repo.path(),
            &PanicFetcher,
        )
        .await
        .unwrap();
        assert!(optional.is_empty());

        let err = resolve_imports_with_repo_root(
            &[import_entry("missing.md")],
            repo.path(),
            repo.path(),
            &PanicFetcher,
        )
        .await
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("failed to resolve import `missing.md`")
        );
    }

    #[tokio::test]
    async fn local_import_rejects_path_traversal() {
        let repo = tempfile::tempdir().unwrap();
        for spec in ["../secret.md", "../../etc/passwd.md", "a/../../b.md", "./x.md"] {
            let err = resolve_imports_with_repo_root(
                &[import_entry(spec)],
                repo.path(),
                repo.path(),
                &PanicFetcher,
            )
            .await
            .unwrap_err();
            assert!(
                format!("{err:#}").contains("invalid segment"),
                "spec `{spec}` should be rejected as traversal, got: {err:#}"
            );
        }
    }

    #[tokio::test]
    async fn imports_per_workflow_limit_is_enforced() {
        let repo = tempfile::tempdir().unwrap();
        let entries: Vec<ImportEntry> = (0..=MAX_IMPORTS_PER_WORKFLOW)
            .map(|idx| import_entry(&format!("component-{idx}.md?")))
            .collect();

        let err = resolve_imports_with_repo_root(&entries, repo.path(), repo.path(), &PanicFetcher)
            .await
            .unwrap_err();

        assert!(
            err.to_string()
                .contains("imports per workflow must be <= 20")
        );
    }

    use crate::compile::types::ImportEndpoint;
    use crate::secure::{CommitSha, HostName};

    fn remote_spec(endpoint: Option<ImportEndpoint>) -> ParsedImportSpec {
        ParsedImportSpec {
            owner: "proj".to_string(),
            repo: "repo".to_string(),
            path: "component.md".to_string(),
            sha: CommitSha::parse(SHA).unwrap(),
            section: None,
            optional: false,
            endpoint,
        }
    }

    #[test]
    fn route_endpoint_maps_azure_repos_sources_to_azure_fetcher() {
        // Endpoint-less => same-org Azure Repos (the primary, default source).
        assert_eq!(route_endpoint(&None), FetcherKind::AzureRepos);
        // Cross-org Azure Repos.
        assert_eq!(
            route_endpoint(&Some(ImportEndpoint::AzureReposCrossOrg {
                name: "conn".to_string(),
                org: "https://dev.azure.com/other".to_string(),
            })),
            FetcherKind::AzureRepos
        );
    }

    #[test]
    fn route_endpoint_maps_github_sources_to_github_fetcher() {
        assert_eq!(
            route_endpoint(&Some(ImportEndpoint::GitHub {
                name: "gh-conn".to_string(),
            })),
            FetcherKind::GitHub
        );
        assert_eq!(
            route_endpoint(&Some(ImportEndpoint::GitHubEnterprise {
                name: "ghe-conn".to_string(),
                host: HostName::parse("api.acme.ghe.com").unwrap(),
            })),
            FetcherKind::GitHub
        );
    }

    /// Fail-closed guard: an endpoint-less (same-org Azure Repos) import whose
    /// org/auth could not be resolved must hard-error — it must NEVER silently
    /// fall through to GitHub. Regression guard for the source-confusion bug.
    #[tokio::test]
    async fn ado_fetcher_endpoint_less_without_org_fails_closed() {
        let fetcher = AdoRepoFetcher::with_resolved(
            Err("no ADO remote".to_string()),
            Err("no creds".to_string()),
        );
        let err = fetcher
            .fetch(&remote_spec(None))
            .await
            .expect_err("must fail closed without org");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("same-org Azure Repos") && msg.contains("no ADO remote"),
            "unexpected error: {msg}"
        );
    }

    /// A GitHub-typed spec must never be accepted by the Azure Repos fetcher.
    #[tokio::test]
    async fn ado_fetcher_rejects_github_typed_spec() {
        let fetcher = AdoRepoFetcher::with_resolved(
            Ok("https://dev.azure.com/org".to_string()),
            Ok(crate::ado::AdoAuth::Pat("x".to_string())),
        );
        let err = fetcher
            .fetch(&remote_spec(Some(ImportEndpoint::GitHub {
                name: "gh".to_string(),
            })))
            .await
            .expect_err("azure fetcher must reject a github-typed import");
        assert!(
            format!("{err:#}").contains("internal routing error"),
            "unexpected error: {err:#}"
        );
    }
}
