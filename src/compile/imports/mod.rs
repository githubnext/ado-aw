//! Compile-time resolution for reusable `imports:`.
//!
//! Remote imports are resolved to immutable commit SHAs, cached under
//! `.ado-aw/imports`, and expanded transitively with bounded breadth-first
//! traversal. No repository resource or runtime checkout is generated.

#[cfg(test)]
mod integration_tests;
pub mod merge;
pub mod schema;

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fmt;
use std::fs;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result};
use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};

use crate::compile::imports::schema::apply_import_inputs;
use crate::compile::types::{ImportEntry, ImportSource, ParsedImportSpec};
use crate::hash::sha256_hex;
use crate::secure::CommitSha;

const MAX_UNIQUE_IMPORTS: usize = 20;
const MAX_IMPORT_DEPTH: usize = 5;
/// Upper bound on resolution attempts, including duplicates.
///
/// [`MAX_UNIQUE_IMPORTS`] already caps how many distinct components a workflow
/// may accept, but duplicates are resolved before they are deduplicated, so a
/// single crafted manifest declaring thousands of repeated `imports:` entries
/// would otherwise drive that many resolutions. Diamond dependencies stay well
/// inside this bound.
const MAX_IMPORT_RESOLUTIONS: usize = 256;
const MAX_MANIFEST_BYTES: usize = 256 * 1024;
const REF_METADATA_VERSION: u32 = 1;
const IMPORT_GITATTRIBUTES: &str = "# Mark all cached import files as generated\n\
* linguist-generated=true\n\
# Keep local cached versions on merge\n\
* merge=ours\n";

/// Typed not-found error used to implement optional remote imports safely.
///
/// Optional imports skip only this error. Authentication, malformed responses,
/// cache-integrity failures, and all other errors remain fatal.
#[derive(Debug)]
pub struct ImportNotFound {
    resource: String,
}

impl ImportNotFound {
    pub(crate) fn new(resource: impl Into<String>) -> Self {
        Self {
            resource: resource.into(),
        }
    }
}

impl fmt::Display for ImportNotFound {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "import resource not found: {}", self.resource)
    }
}

impl std::error::Error for ImportNotFound {}

fn is_import_not_found(error: &anyhow::Error) -> bool {
    error.downcast_ref::<ImportNotFound>().is_some()
}

fn not_found(resource: impl Into<String>) -> anyhow::Error {
    anyhow::Error::new(ImportNotFound::new(resource))
}

/// Compile-time remote import fetcher.
///
/// Ref resolution is separate from manifest fetching so the resolver can key
/// the committed cache by immutable SHA and retain requested-ref metadata.
#[async_trait]
pub trait ManifestFetcher: Send + Sync {
    async fn remote_identity(&self, spec: &ParsedImportSpec) -> Result<RemoteRepositoryIdentity> {
        RemoteRepositoryIdentity::from_spec(spec)
    }

    async fn ensure_repository_accessible(&self, _spec: &ParsedImportSpec) -> Result<()> {
        Ok(())
    }

    async fn resolve_ref(&self, spec: &ParsedImportSpec) -> Result<CommitSha>;
    async fn fetch(&self, spec: &ParsedImportSpec, resolved_sha: &CommitSha) -> Result<Vec<u8>>;
}

/// Effective remote repository identity used for cache and provenance keys.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteRepositoryIdentity {
    source: String,
    project: Option<String>,
}

impl RemoteRepositoryIdentity {
    fn from_spec(spec: &ParsedImportSpec) -> Result<Self> {
        let remote = remote_parts(spec)?;
        Ok(Self {
            source: remote.source.identity(),
            project: remote.project.map(str::to_string),
        })
    }
}

/// GitHub.com / GHES fetcher using the author's existing `gh` authentication.
pub struct GhCliFetcher;

#[async_trait]
impl ManifestFetcher for GhCliFetcher {
    async fn ensure_repository_accessible(&self, spec: &ParsedImportSpec) -> Result<()> {
        let remote = remote_parts(spec)?;
        let ImportSource::GitHub { host } = remote.source else {
            anyhow::bail!("internal routing error: GitHub fetcher received an Azure Repos import");
        };
        let owner = remote.project.context("GitHub imports require an owner")?;
        let route = format!(
            "repos/{}/{}",
            encode_path_segment(owner),
            encode_path_segment(remote.repository),
        );
        run_gh_api(&route, None, host.as_str(), false).await?;
        Ok(())
    }

    async fn resolve_ref(&self, spec: &ParsedImportSpec) -> Result<CommitSha> {
        let remote = remote_parts(spec)?;
        let ImportSource::GitHub { host } = remote.source else {
            anyhow::bail!("internal routing error: GitHub fetcher received an Azure Repos import");
        };
        let owner = remote.project.context("GitHub imports require an owner")?;
        let route = format!(
            "repos/{}/{}/commits/{}",
            encode_path_segment(owner),
            encode_path_segment(remote.repository),
            encode_path_segment(remote.requested_ref),
        );
        let output = run_gh_api(&route, Some(".sha"), host.as_str(), true).await?;
        CommitSha::parse(output.trim()).with_context(|| {
            format!(
                "GitHub returned an invalid commit SHA while resolving `{}`",
                remote.requested_ref
            )
        })
    }

    async fn fetch(&self, spec: &ParsedImportSpec, resolved_sha: &CommitSha) -> Result<Vec<u8>> {
        let remote = remote_parts(spec)?;
        let ImportSource::GitHub { host } = remote.source else {
            anyhow::bail!("internal routing error: GitHub fetcher received an Azure Repos import");
        };
        let owner = remote.project.context("GitHub imports require an owner")?;
        let route =
            github_contents_api_route(owner, remote.repository, remote.path, resolved_sha.as_str());
        let output = run_gh_api(&route, None, host.as_str(), true).await?;

        #[derive(Deserialize)]
        struct ContentsResponse {
            content: String,
            #[serde(default)]
            encoding: Option<String>,
        }

        let response: ContentsResponse = serde_json::from_slice(output.as_bytes())
            .with_context(|| format!("failed to parse GitHub Contents API response for {route}"))?;
        if response.encoding.as_deref().unwrap_or("base64") != "base64" {
            anyhow::bail!(
                "GitHub Contents API response for {route} used unsupported encoding {:?}",
                response.encoding
            );
        }
        let compact: String = response
            .content
            .chars()
            .filter(|character| !character.is_whitespace())
            .collect();
        STANDARD
            .decode(compact.as_bytes())
            .with_context(|| format!("failed to decode GitHub import `{route}`"))
    }
}

async fn run_gh_api(
    route: &str,
    jq: Option<&str>,
    host: &str,
    classify_not_found: bool,
) -> Result<String> {
    let mut command = tokio::process::Command::new("gh");
    command.args(["api", route]);
    if let Some(jq) = jq {
        command.args(["--jq", jq]);
    }
    command.env("GH_HOST", host);
    let output = command
        .output()
        .await
        .with_context(|| format!("failed to run `gh api {route}`"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let message = stderr.trim();
        if classify_not_found && github_error_is_not_found(message) {
            return Err(not_found(route));
        }
        anyhow::bail!(
            "`gh api {route}` failed with status {}: {message}",
            output.status
        );
    }
    String::from_utf8(output.stdout)
        .with_context(|| format!("`gh api {route}` returned non-UTF-8 output"))
}

fn github_error_is_not_found(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("http 404")
        || lower.contains("status 404")
        || lower.contains("not found (http 404)")
        || ((lower.contains("http 422") || lower.contains("status 422"))
            && (lower.contains("no commit found")
                || lower.contains("reference does not exist")
                || lower.contains("sha does not exist")))
}

fn github_contents_api_route(owner: &str, repo: &str, path: &str, reference: &str) -> String {
    let encoded_path = path
        .split('/')
        .map(encode_path_segment)
        .collect::<Vec<_>>()
        .join("/");
    format!(
        "repos/{}/{}/contents/{}?ref={}",
        encode_path_segment(owner),
        encode_path_segment(repo),
        encoded_path,
        percent_encoding::utf8_percent_encode(reference, crate::ado::QUERY_VALUE),
    )
}

fn encode_path_segment(value: &str) -> String {
    percent_encoding::utf8_percent_encode(value, crate::ado::PATH_SEGMENT).to_string()
}

/// Azure Repos fetcher using the consumer's non-interactive ADO credentials.
pub struct AdoRepoFetcher {
    client: reqwest::Client,
    repo_root: PathBuf,
    context: tokio::sync::OnceCell<std::result::Result<crate::ado::AdoContext, String>>,
    auth: tokio::sync::OnceCell<std::result::Result<crate::ado::AdoAuth, String>>,
}

impl AdoRepoFetcher {
    pub fn new(repo_root: PathBuf) -> Self {
        Self {
            client: reqwest::Client::new(),
            repo_root,
            context: tokio::sync::OnceCell::new(),
            auth: tokio::sync::OnceCell::new(),
        }
    }

    #[cfg(test)]
    fn with_resolved(
        context: std::result::Result<crate::ado::AdoContext, String>,
        auth: std::result::Result<crate::ado::AdoAuth, String>,
    ) -> Self {
        Self {
            client: reqwest::Client::new(),
            repo_root: PathBuf::new(),
            context: tokio::sync::OnceCell::new_with(Some(context)),
            auth: tokio::sync::OnceCell::new_with(Some(auth)),
        }
    }

    async fn context(&self) -> &std::result::Result<crate::ado::AdoContext, String> {
        self.context
            .get_or_init(|| async {
                if let Some(context) = ado_context_from_env(|name| std::env::var(name).ok()) {
                    return Ok(context);
                }
                crate::ado::resolve_ado_context(&self.repo_root, None, None)
                    .await
                    .map_err(|error| format!("{error:#}"))
            })
            .await
    }

    async fn auth(&self) -> &std::result::Result<crate::ado::AdoAuth, String> {
        self.auth
            .get_or_init(|| async {
                crate::ado::resolve_auth_non_interactive()
                    .await
                    .map_err(|error| format!("{error:#}"))
            })
            .await
    }

    async fn org_and_project<'a>(&'a self, remote: RemoteParts<'a>) -> Result<(String, String)> {
        let ImportSource::AzureRepos { collection } = remote.source else {
            anyhow::bail!("internal routing error: Azure Repos fetcher received a GitHub import");
        };
        let env_org = ado_org_url_from_env(|name| std::env::var(name).ok());
        let env_project = ado_project_from_env(|name| std::env::var(name).ok());
        let needs_context = (collection.is_none() && env_org.is_none())
            || (remote.project.is_none() && env_project.is_none());
        let context = if needs_context {
            Some(self.context().await.as_ref().map_err(|reason| {
                anyhow::anyhow!(
                    "cannot determine the consumer Azure DevOps context for import `{}`: {reason}",
                    remote.display()
                )
            })?)
        } else {
            None
        };
        let org = collection
            .as_ref()
            .map(|value| value.as_str().trim_end_matches('/').to_string())
            .or(env_org)
            .or_else(|| context.map(|value| value.org_url.clone()))
            .context("Azure Repos import has no organization")?;
        let project = remote
            .project
            .map(str::to_string)
            .or(env_project)
            .or_else(|| context.map(|value| value.project.clone()))
            .context("Azure Repos import has no project")?;
        Ok((org, project))
    }

    async fn request_text(&self, url: &str, resource: &str) -> Result<String> {
        let auth = self.auth().await.as_ref().map_err(|reason| {
            anyhow::anyhow!("cannot authenticate to Azure Repos for `{resource}`: {reason}")
        })?;
        let response = auth
            .apply(self.client.get(url))
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await
            .with_context(|| format!("failed to request Azure Repos resource `{resource}`"))?;
        let status = response.status();
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let body = response
            .text()
            .await
            .with_context(|| format!("failed to read Azure Repos response for `{resource}`"))?;
        if looks_like_ado_signin(status, content_type.as_deref(), &body) {
            anyhow::bail!(
                "Azure DevOps returned an interactive sign-in page for `{resource}`; \
                 configure SYSTEM_ACCESSTOKEN, AZURE_DEVOPS_EXT_PAT, or Azure CLI auth"
            );
        }
        if status == reqwest::StatusCode::NOT_FOUND {
            return Err(not_found(resource));
        }
        if !status.is_success() {
            anyhow::bail!(
                "Azure Repos API returned {status} for `{resource}`: {}",
                body.trim()
            );
        }
        Ok(body)
    }
}

#[async_trait]
impl ManifestFetcher for AdoRepoFetcher {
    async fn remote_identity(&self, spec: &ParsedImportSpec) -> Result<RemoteRepositoryIdentity> {
        let remote = remote_parts(spec)?;
        let (org, project) = self.org_and_project(remote).await?;
        Ok(RemoteRepositoryIdentity {
            source: format!(
                "azure-repos:{}",
                crate::ado::normalize_org_url(&org).trim_end_matches('/')
            ),
            project: Some(project),
        })
    }

    async fn ensure_repository_accessible(&self, spec: &ParsedImportSpec) -> Result<()> {
        let remote = remote_parts(spec)?;
        if !matches!(remote.source, ImportSource::AzureRepos { .. }) {
            anyhow::bail!("internal routing error: Azure Repos fetcher received a GitHub import");
        }
        let (org, project) = self.org_and_project(remote).await?;
        let url = format!(
            "{}/{}/_apis/git/repositories/{}?api-version=7.1",
            org.trim_end_matches('/'),
            encode_path_segment(&project),
            encode_path_segment(remote.repository),
        );
        self.request_text(&url, &format!("{} repository", remote.display()))
            .await?;
        Ok(())
    }

    async fn resolve_ref(&self, spec: &ParsedImportSpec) -> Result<CommitSha> {
        let remote = remote_parts(spec)?;
        if !matches!(remote.source, ImportSource::AzureRepos { .. }) {
            anyhow::bail!("internal routing error: Azure Repos fetcher received a GitHub import");
        }
        if let Ok(sha) = CommitSha::parse(remote.requested_ref) {
            return Ok(sha);
        }
        let (org, project) = self.org_and_project(remote).await?;
        for candidate in ado_ref_candidates(remote.requested_ref) {
            let url = format!(
                "{}/{}/_apis/git/repositories/{}/refs?filter={}\
                 &peelTags=true&api-version=7.1",
                org.trim_end_matches('/'),
                encode_path_segment(&project),
                encode_path_segment(remote.repository),
                percent_encoding::utf8_percent_encode(&candidate.filter, crate::ado::QUERY_VALUE),
            );
            let body = self
                .request_text(
                    &url,
                    &format!("{} ref {}", remote.display(), candidate.full_name),
                )
                .await?;

            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct RefValue {
                name: String,
                object_id: String,
                #[serde(default)]
                peeled_object_id: Option<String>,
            }
            #[derive(Deserialize)]
            struct RefResponse {
                #[serde(default)]
                value: Vec<RefValue>,
            }

            let response: RefResponse =
                serde_json::from_str(&body).context("failed to parse Azure Repos refs response")?;
            if let Some(reference) = response
                .value
                .into_iter()
                .find(|reference| reference.name == candidate.full_name)
            {
                if let Some(peeled) = reference.peeled_object_id
                    && let Ok(sha) = CommitSha::parse(peeled)
                {
                    return Ok(sha);
                }
                return CommitSha::parse(reference.object_id).with_context(|| {
                    format!(
                        "Azure Repos ref `{}` did not resolve to a full commit SHA",
                        candidate.full_name
                    )
                });
            }
        }
        Err(not_found(format!(
            "{} ref {}",
            remote.display(),
            remote.requested_ref
        )))
    }

    async fn fetch(&self, spec: &ParsedImportSpec, resolved_sha: &CommitSha) -> Result<Vec<u8>> {
        let remote = remote_parts(spec)?;
        let (org, project) = self.org_and_project(remote).await?;
        let url = format!(
            "{}/{}/_apis/git/repositories/{}/items?path={}\
             &versionDescriptor.version={}&versionDescriptor.versionType=commit\
             &includeContent=true&api-version=7.1",
            org.trim_end_matches('/'),
            encode_path_segment(&project),
            encode_path_segment(remote.repository),
            percent_encoding::utf8_percent_encode(remote.path, crate::ado::QUERY_VALUE),
            percent_encoding::utf8_percent_encode(resolved_sha.as_str(), crate::ado::QUERY_VALUE),
        );
        let body = self.request_text(&url, &remote.display()).await?;
        #[derive(Deserialize)]
        struct GitItem {
            content: Option<String>,
        }
        let item: GitItem = serde_json::from_str(&body)
            .with_context(|| format!("failed to parse Azure Repos item `{}`", remote.display()))?;
        item.content
            .map(String::into_bytes)
            .with_context(|| format!("Azure Repos item `{}` is not a file", remote.display()))
    }
}

fn ado_context_from_env(
    mut get: impl FnMut(&str) -> Option<String>,
) -> Option<crate::ado::AdoContext> {
    let org_url = ado_org_url_from_env(&mut get)?;
    let project = ado_project_from_env(&mut get)?;
    Some(crate::ado::AdoContext {
        org_url,
        project,
        repo_name: get("BUILD_REPOSITORY_NAME").unwrap_or_default(),
    })
}

fn ado_org_url_from_env(mut get: impl FnMut(&str) -> Option<String>) -> Option<String> {
    let org_url = ["AZURE_DEVOPS_ORG_URL", "SYSTEM_COLLECTIONURI"]
        .into_iter()
        .find_map(|name| {
            let value = get(name)?;
            (!value.trim().is_empty()).then(|| crate::ado::normalize_org_url(&value))
        })?;
    Some(org_url)
}

fn ado_project_from_env(mut get: impl FnMut(&str) -> Option<String>) -> Option<String> {
    let project = get("SYSTEM_TEAMPROJECT")?;
    (!project.trim().is_empty()).then(|| project.trim().to_string())
}

fn looks_like_ado_signin(
    status: reqwest::StatusCode,
    content_type: Option<&str>,
    body: &str,
) -> bool {
    status == reqwest::StatusCode::NON_AUTHORITATIVE_INFORMATION
        || content_type.is_some_and(|value| value.to_ascii_lowercase().contains("text/html"))
        || {
            let lower = body.trim_start().to_ascii_lowercase();
            lower.starts_with("<!doctype") || lower.starts_with("<html")
        }
}

struct AdoRefCandidate {
    filter: String,
    full_name: String,
}

fn ado_ref_candidates(reference: &str) -> Vec<AdoRefCandidate> {
    if let Some(branch) = reference.strip_prefix("refs/heads/") {
        return vec![AdoRefCandidate {
            filter: format!("heads/{branch}"),
            full_name: reference.to_string(),
        }];
    }
    if let Some(tag) = reference.strip_prefix("refs/tags/") {
        return vec![AdoRefCandidate {
            filter: format!("tags/{tag}"),
            full_name: reference.to_string(),
        }];
    }
    vec![
        AdoRefCandidate {
            filter: format!("heads/{reference}"),
            full_name: format!("refs/heads/{reference}"),
        },
        AdoRefCandidate {
            filter: format!("tags/{reference}"),
            full_name: format!("refs/tags/{reference}"),
        },
    ]
}

/// Routes imports by their typed compile-time source.
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

#[async_trait]
impl ManifestFetcher for RoutingFetcher {
    async fn remote_identity(&self, spec: &ParsedImportSpec) -> Result<RemoteRepositoryIdentity> {
        match remote_parts(spec)?.source {
            ImportSource::AzureRepos { .. } => self.ado.remote_identity(spec).await,
            ImportSource::GitHub { .. } => self.github.remote_identity(spec).await,
        }
    }

    async fn ensure_repository_accessible(&self, spec: &ParsedImportSpec) -> Result<()> {
        match remote_parts(spec)?.source {
            ImportSource::AzureRepos { .. } => self.ado.ensure_repository_accessible(spec).await,
            ImportSource::GitHub { .. } => self.github.ensure_repository_accessible(spec).await,
        }
    }

    async fn resolve_ref(&self, spec: &ParsedImportSpec) -> Result<CommitSha> {
        match remote_parts(spec)?.source {
            ImportSource::AzureRepos { .. } => self.ado.resolve_ref(spec).await,
            ImportSource::GitHub { .. } => self.github.resolve_ref(spec).await,
        }
    }

    async fn fetch(&self, spec: &ParsedImportSpec, resolved_sha: &CommitSha) -> Result<Vec<u8>> {
        match remote_parts(spec)?.source {
            ImportSource::AzureRepos { .. } => self.ado.fetch(spec, resolved_sha).await,
            ImportSource::GitHub { .. } => self.github.fetch(spec, resolved_sha).await,
        }
    }
}

/// Resolved import manifest and compile-time provenance.
#[derive(Debug, Clone)]
pub struct ResolvedImport {
    pub entry: ImportEntry,
    pub spec: ParsedImportSpec,
    pub front_matter: serde_yaml::Value,
    pub body: String,
    pub provenance: ImportProvenance,
}

/// Provenance stamped onto imported custom jobs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportProvenance {
    pub source: String,
    pub requested_ref: Option<String>,
    pub sha: Option<String>,
    pub manifest_digest: String,
}

#[derive(Debug, Clone)]
enum ResolutionContext {
    Local {
        base_dir: PathBuf,
    },
    Remote {
        source: ImportSource,
        project: Option<String>,
        repository: String,
        manifest_path: String,
        resolved_sha: CommitSha,
    },
}

#[derive(Debug, Clone)]
struct PendingImport {
    entry: ImportEntry,
    context: ResolutionContext,
    depth: usize,
    ancestry: Vec<Ancestor>,
}

#[derive(Debug, Clone)]
struct Ancestor {
    identity: ResolvedIdentity,
    display: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum ResolvedIdentity {
    Local {
        path: String,
        section: Option<String>,
    },
    Remote {
        source: String,
        project: Option<String>,
        repository: String,
        sha: String,
        path: String,
        section: Option<String>,
    },
}

#[derive(Debug)]
struct ResolvedNode {
    import: ResolvedImport,
    identity: ResolvedIdentity,
    child_context: ResolutionContext,
}

/// Resolve top-level and nested imports breadth-first.
pub async fn resolve_imports_with_repo_root(
    entries: &[ImportEntry],
    base_dir: &Path,
    repo_root: &Path,
    fetcher: &dyn ManifestFetcher,
) -> Result<Vec<ResolvedImport>> {
    let repo_root = fs::canonicalize(repo_root)
        .with_context(|| format!("failed to resolve repository root {}", repo_root.display()))?;
    let base_dir = fs::canonicalize(base_dir).with_context(|| {
        format!(
            "failed to resolve import base directory {}",
            base_dir.display()
        )
    })?;
    if !base_dir.starts_with(&repo_root) {
        anyhow::bail!(
            "import base directory {} is outside repository root {}",
            base_dir.display(),
            repo_root.display()
        );
    }

    let mut queue = VecDeque::new();
    for entry in entries {
        queue.push_back(PendingImport {
            entry: entry.clone(),
            context: ResolutionContext::Local {
                base_dir: base_dir.clone(),
            },
            depth: 1,
            ancestry: Vec::new(),
        });
    }

    let mut resolved = Vec::new();
    let mut seen: HashMap<ResolvedIdentity, serde_json::Map<String, serde_json::Value>> =
        HashMap::new();
    let mut graph: HashMap<ResolvedIdentity, Vec<ResolvedIdentity>> = HashMap::new();
    let mut identity_displays: HashMap<ResolvedIdentity, String> = HashMap::new();
    let mut ref_cache: HashMap<String, CommitSha> = HashMap::new();
    let mut ref_metadata = read_ref_metadata(&repo_root)?;

    let mut resolution_attempts = 0usize;
    while let Some(pending) = queue.pop_front() {
        resolution_attempts += 1;
        anyhow::ensure!(
            resolution_attempts <= MAX_IMPORT_RESOLUTIONS,
            "resolving this import graph requires more than {MAX_IMPORT_RESOLUTIONS} lookups; \
             reduce duplicate or fanned-out `imports:` entries"
        );
        if pending.depth > MAX_IMPORT_DEPTH {
            anyhow::bail!(
                "import nesting depth exceeds the maximum of {MAX_IMPORT_DEPTH}: {}",
                display_chain(&pending.ancestry, &pending.entry.uses)
            );
        }

        let spec = parse_entry_in_context(&pending.entry, &pending.context)
            .with_context(|| format!("failed to parse import `{}`", pending.entry.uses))?;
        let node = match resolve_spec(
            &pending.entry,
            spec,
            &pending.context,
            &repo_root,
            fetcher,
            &mut ref_cache,
            &mut ref_metadata,
        )
        .await
        .with_context(|| format!("failed to resolve import `{}`", pending.entry.uses))
        {
            Ok(Some(node)) => node,
            Ok(None) => continue,
            Err(error) => return Err(error),
        };
        identity_displays
            .entry(node.identity.clone())
            .or_insert_with(|| node.import.provenance.source.clone());

        if pending
            .ancestry
            .iter()
            .any(|ancestor| ancestor.identity == node.identity)
        {
            anyhow::bail!(
                "import cycle detected: {}",
                display_cycle(&pending.ancestry, &node.import.provenance.source)
            );
        }
        if let Some(parent) = pending.ancestry.last() {
            if let Some(path) = find_identity_path(&graph, &node.identity, &parent.identity) {
                let mut cycle = vec![parent.identity.clone()];
                cycle.extend(path);
                anyhow::bail!(
                    "import cycle detected: {}",
                    display_identity_path(&cycle, &identity_displays)
                );
            }
            let children = graph.entry(parent.identity.clone()).or_default();
            if !children.contains(&node.identity) {
                children.push(node.identity.clone());
            }
        }

        if let Some(previous_with) = seen.get(&node.identity) {
            if previous_with == &pending.entry.with {
                continue;
            }
            anyhow::bail!(
                "import `{}` resolves to the same component as an earlier import but uses \
                 different `with:` values",
                pending.entry.uses
            );
        }
        if seen.len() >= MAX_UNIQUE_IMPORTS {
            anyhow::bail!("a workflow may resolve at most {MAX_UNIQUE_IMPORTS} unique imports");
        }
        seen.insert(node.identity.clone(), pending.entry.with.clone());

        let (substituted_front_matter, _) = apply_import_inputs(
            &node.import.front_matter,
            &node.import.body,
            &node.import.entry.with,
        )
        .with_context(|| {
            format!(
                "failed to apply import inputs for `{}`",
                node.import.provenance.source
            )
        })?;
        let nested = nested_import_entries(&substituted_front_matter).with_context(|| {
            format!(
                "failed to parse nested imports from `{}`",
                node.import.provenance.source
            )
        })?;

        let mut ancestry = pending.ancestry;
        ancestry.push(Ancestor {
            identity: node.identity.clone(),
            display: node.import.provenance.source.clone(),
        });
        for entry in nested {
            queue.push_back(PendingImport {
                entry,
                context: node.child_context.clone(),
                depth: pending.depth + 1,
                ancestry: ancestry.clone(),
            });
        }
        resolved.push(node.import);
    }

    Ok(resolved)
}

async fn resolve_spec(
    entry: &ImportEntry,
    spec: ParsedImportSpec,
    context: &ResolutionContext,
    repo_root: &Path,
    fetcher: &dyn ManifestFetcher,
    ref_cache: &mut HashMap<String, CommitSha>,
    ref_metadata: &mut RefMetadata,
) -> Result<Option<ResolvedNode>> {
    match &spec {
        ParsedImportSpec::Local {
            path,
            section,
            optional,
        } => {
            let ResolutionContext::Local { base_dir } = context else {
                anyhow::bail!("internal error: unresolved local import has a remote origin");
            };
            let local_path = resolve_local_path(base_dir, repo_root, path)?;
            let canonical = match fs::canonicalize(&local_path) {
                Ok(path) => path,
                Err(error) if *optional && error.kind() == std::io::ErrorKind::NotFound => {
                    return Ok(None);
                }
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!("failed to resolve local import {}", local_path.display())
                    });
                }
            };
            ensure_path_within_repo(&canonical, repo_root)?;
            let bytes = match fs::read(&canonical) {
                Ok(bytes) => bytes,
                Err(error) if *optional && error.kind() == std::io::ErrorKind::NotFound => {
                    return Ok(None);
                }
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!("failed to read local import {}", canonical.display())
                    });
                }
            };
            enforce_manifest_size(bytes.len(), &canonical.display().to_string())?;
            let digest = sha256_hex(&bytes);
            let (front_matter, body) = parse_manifest_bytes(&bytes, section.as_deref())?;
            let identity_path = local_identity_path(&canonical);
            Ok(Some(ResolvedNode {
                import: ResolvedImport {
                    entry: entry.clone(),
                    spec: spec.clone(),
                    front_matter,
                    body,
                    provenance: ImportProvenance {
                        source: canonical.display().to_string(),
                        requested_ref: None,
                        sha: None,
                        manifest_digest: digest,
                    },
                },
                identity: ResolvedIdentity::Local {
                    path: identity_path,
                    section: section.clone(),
                },
                child_context: ResolutionContext::Local {
                    base_dir: canonical
                        .parent()
                        .unwrap_or_else(|| Path::new("."))
                        .to_path_buf(),
                },
            }))
        }
        ParsedImportSpec::Remote {
            source,
            project,
            repository,
            path,
            requested_ref,
            section,
            optional,
        } => {
            let remote_identity = fetcher.remote_identity(&spec).await?;
            if *optional {
                match fetcher.ensure_repository_accessible(&spec).await {
                    Ok(()) => {}
                    Err(error) if is_import_not_found(&error) => return Ok(None),
                    Err(error) => return Err(error),
                }
            }
            let ref_key = ref_metadata_key(&remote_identity, repository, requested_ref);
            let resolved_sha = match resolve_remote_sha(
                &spec,
                &remote_identity,
                &ref_key,
                repo_root,
                fetcher,
                ref_cache,
                ref_metadata,
            )
            .await
            {
                Ok(sha) => sha,
                Err(error) if *optional && is_import_not_found(&error) => return Ok(None),
                Err(error) => return Err(error),
            };
            // Persist the ref movement before fetching the path. If the new
            // commit no longer contains an optional manifest, retaining the
            // previous SHA here would let a later offline compile incorrectly
            // resurrect stale cached content.
            ref_metadata.record(&remote_identity, repository, requested_ref, &resolved_sha);
            write_ref_metadata(repo_root, ref_metadata)?;
            let bytes = match read_remote_manifest(
                repo_root,
                &spec,
                &remote_identity,
                &resolved_sha,
                fetcher,
            )
            .await
            {
                Ok(bytes) => bytes,
                Err(error) if *optional && is_import_not_found(&error) => return Ok(None),
                Err(error) => return Err(error),
            };

            let digest = sha256_hex(&bytes);
            let (front_matter, body) = parse_manifest_bytes(&bytes, section.as_deref())?;
            let display = resolved_remote_display(&remote_identity, repository, path);
            Ok(Some(ResolvedNode {
                import: ResolvedImport {
                    entry: entry.clone(),
                    spec: spec.clone(),
                    front_matter,
                    body,
                    provenance: ImportProvenance {
                        source: display,
                        requested_ref: Some(requested_ref.clone()),
                        sha: Some(resolved_sha.as_str().to_string()),
                        manifest_digest: digest,
                    },
                },
                identity: ResolvedIdentity::Remote {
                    source: remote_identity.source,
                    project: remote_identity.project,
                    repository: repository.clone(),
                    sha: resolved_sha.as_str().to_string(),
                    path: path.clone(),
                    section: section.clone(),
                },
                child_context: ResolutionContext::Remote {
                    source: source.clone(),
                    project: project.clone(),
                    repository: repository.clone(),
                    manifest_path: path.clone(),
                    resolved_sha,
                },
            }))
        }
    }
}

fn parse_entry_in_context(
    entry: &ImportEntry,
    context: &ResolutionContext,
) -> Result<ParsedImportSpec> {
    let ResolutionContext::Remote {
        source,
        project,
        repository,
        manifest_path,
        resolved_sha,
    } = context
    else {
        return entry.parse_source();
    };

    if !entry.uses.contains('@') {
        return match entry.parse_source()? {
            ParsedImportSpec::Local {
                path,
                section,
                optional,
            } => Ok(ParsedImportSpec::Remote {
                source: source.clone(),
                project: project.clone(),
                repository: repository.clone(),
                path: join_remote_relative_path(manifest_path, &path)?,
                requested_ref: resolved_sha.as_str().to_string(),
                section,
                optional,
            }),
            remote => Ok(remote),
        };
    }

    if entry.repository.is_none() && !looks_like_combined_shorthand(&entry.uses) {
        let mut inherited = entry.clone();
        inherited.repository = Some(match project {
            Some(project) => format!("{project}/{repository}"),
            None => repository.clone(),
        });
        inherited.source = Some(source.clone());
        inherited.uses = rewrite_remote_relative_uses(&entry.uses, manifest_path)?;
        return inherited.parse_source();
    }

    entry.parse_source()
}

fn looks_like_combined_shorthand(uses: &str) -> bool {
    let without_optional = uses.trim().strip_suffix('?').unwrap_or(uses.trim());
    let base = without_optional
        .split_once('#')
        .map(|(base, _)| base)
        .unwrap_or(without_optional);
    let before_ref = base
        .rsplit_once('@')
        .map(|(value, _)| value)
        .unwrap_or(base);
    if before_ref.starts_with("./") || before_ref.starts_with("../") {
        return false;
    }
    before_ref
        .split('/')
        .filter(|segment| !segment.is_empty() && *segment != ".")
        .count()
        >= 3
}

fn rewrite_remote_relative_uses(uses: &str, parent_manifest: &str) -> Result<String> {
    let raw = uses.trim();
    let (without_optional, optional) = raw
        .strip_suffix('?')
        .map(|value| (value, true))
        .unwrap_or((raw, false));
    let (base, section) = without_optional
        .split_once('#')
        .map(|(base, section)| (base, Some(section)))
        .unwrap_or((without_optional, None));
    let (path, reference) = base
        .rsplit_once('@')
        .context("relative remote import must contain `@`")?;
    let joined = join_remote_relative_path(parent_manifest, path)?;
    let mut rewritten = format!("{joined}@{reference}");
    if let Some(section) = section {
        rewritten.push('#');
        rewritten.push_str(section);
    }
    if optional {
        rewritten.push('?');
    }
    Ok(rewritten)
}

fn join_remote_relative_path(parent_manifest: &str, relative: &str) -> Result<String> {
    if relative.starts_with('/') || relative.contains('\\') {
        anyhow::bail!("nested remote import path must be relative, got `{relative}`");
    }
    let mut segments: Vec<&str> = parent_manifest.split('/').collect();
    segments.pop();
    for (index, segment) in relative.split('/').enumerate() {
        match segment {
            "" => {
                anyhow::bail!("nested remote import path contains an empty segment: `{relative}`")
            }
            "." if index == 0 => {}
            "." => anyhow::bail!(
                "nested remote import path contains an ambiguous `.` segment: `{relative}`"
            ),
            ".." => {
                if segments.pop().is_none() {
                    anyhow::bail!("nested remote import `{relative}` escapes the repository root");
                }
            }
            value => segments.push(value),
        }
    }
    if segments.is_empty() {
        anyhow::bail!("nested remote import path must name a file");
    }
    Ok(segments.join("/"))
}

fn nested_import_entries(front_matter: &serde_yaml::Value) -> Result<Vec<ImportEntry>> {
    let serde_yaml::Value::Mapping(mapping) = front_matter else {
        return Ok(Vec::new());
    };
    let Some(value) = mapping.get(serde_yaml::Value::String("imports".to_string())) else {
        return Ok(Vec::new());
    };
    serde_yaml::from_value(value.clone()).context("`imports:` must be a list of import entries")
}

async fn resolve_remote_sha(
    spec: &ParsedImportSpec,
    remote_identity: &RemoteRepositoryIdentity,
    metadata_key: &str,
    repo_root: &Path,
    fetcher: &dyn ManifestFetcher,
    ref_cache: &mut HashMap<String, CommitSha>,
    ref_metadata: &mut RefMetadata,
) -> Result<CommitSha> {
    let requested_ref = remote_parts(spec)?.requested_ref;
    if let Ok(sha) = CommitSha::parse(requested_ref) {
        return Ok(sha);
    }
    if let Some(sha) = ref_cache.get(metadata_key) {
        return Ok(sha.clone());
    }

    match fetcher.resolve_ref(spec).await {
        Ok(sha) => {
            ref_cache.insert(metadata_key.to_string(), sha.clone());
            Ok(sha)
        }
        Err(error) if is_import_not_found(&error) => {
            ref_cache.remove(metadata_key);
            if ref_metadata.remove(metadata_key) {
                write_ref_metadata(repo_root, ref_metadata)?;
            }
            Err(error)
        }
        Err(error) => {
            let Some(cached_sha) = ref_metadata.lookup(metadata_key) else {
                return Err(error).with_context(|| {
                    format!("failed to resolve remote import ref `{requested_ref}`")
                });
            };
            let cached_path = cache_path(repo_root, spec, remote_identity, &cached_sha)?;
            if !cached_path.exists() {
                return Err(error).with_context(|| {
                    format!(
                        "failed to resolve remote import ref `{requested_ref}` and its recorded \
                         SHA {} is not present in the committed cache",
                        cached_sha.as_str()
                    )
                });
            }
            eprintln!(
                "Warning: could not refresh import ref `{}`; using committed cached resolution {}",
                requested_ref,
                cached_sha.as_str()
            );
            ref_cache.insert(metadata_key.to_string(), cached_sha.clone());
            Ok(cached_sha)
        }
    }
}

async fn read_remote_manifest(
    repo_root: &Path,
    spec: &ParsedImportSpec,
    remote_identity: &RemoteRepositoryIdentity,
    resolved_sha: &CommitSha,
    fetcher: &dyn ManifestFetcher,
) -> Result<Vec<u8>> {
    let cache_path = cache_path(repo_root, spec, remote_identity, resolved_sha)?;
    if cache_path.exists() {
        return read_cached_manifest(&cache_path);
    }

    let bytes = fetcher.fetch(spec, resolved_sha).await?;
    enforce_manifest_size(bytes.len(), &remote_parts(spec)?.display())?;
    let parent = cache_path
        .parent()
        .context("import cache path unexpectedly has no parent")?;
    fs::create_dir_all(parent)
        .with_context(|| format!("failed to create import cache {}", parent.display()))?;
    ensure_import_gitattributes(repo_root)?;
    fs::write(&cache_path, &bytes)
        .with_context(|| format!("failed to write cached import {}", cache_path.display()))?;
    fs::write(digest_sidecar_path(&cache_path), sha256_hex(&bytes)).with_context(|| {
        format!(
            "failed to write import cache digest for {}",
            cache_path.display()
        )
    })?;
    Ok(bytes)
}

fn read_cached_manifest(cache_path: &Path) -> Result<Vec<u8>> {
    let bytes = fs::read(cache_path)
        .with_context(|| format!("failed to read cached import {}", cache_path.display()))?;
    enforce_manifest_size(bytes.len(), &cache_path.display().to_string())?;
    let sidecar = digest_sidecar_path(cache_path);
    match fs::read_to_string(&sidecar) {
        Ok(expected) => {
            let actual = sha256_hex(&bytes);
            if actual != expected.trim() {
                anyhow::bail!(
                    "cached import {} does not match its recorded digest (expected {}, got {})",
                    cache_path.display(),
                    expected.trim(),
                    actual
                );
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error).with_context(|| {
                format!("failed to read import cache digest {}", sidecar.display())
            });
        }
    }
    Ok(bytes)
}

fn cache_path(
    repo_root: &Path,
    spec: &ParsedImportSpec,
    remote_identity: &RemoteRepositoryIdentity,
    sha: &CommitSha,
) -> Result<PathBuf> {
    let remote = remote_parts(spec)?;
    let mut path = repo_root
        .join(".ado-aw")
        .join("imports")
        .join("cache")
        .join(cache_segment(&remote_identity.source))
        .join(cache_segment(
            remote_identity
                .project
                .as_deref()
                .unwrap_or("_current-project"),
        ))
        .join(cache_segment(remote.repository))
        .join(sha.as_str());
    for segment in remote.path.split('/') {
        if segment.is_empty() || matches!(segment, "." | "..") {
            anyhow::bail!(
                "remote import path contains an invalid segment: `{}`",
                remote.path
            );
        }
        path.push(cache_segment(segment));
    }
    Ok(path)
}

fn cache_segment(value: &str) -> String {
    let mut prefix: String = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-') {
                character
            } else {
                '_'
            }
        })
        .take(48)
        .collect();
    prefix = prefix.trim_matches(['.', ' ']).to_string();
    if prefix.is_empty() {
        prefix.push_str("value");
    }
    let digest = sha256_hex(value.as_bytes());
    format!("{prefix}--{}", &digest[..12])
}

fn digest_sidecar_path(cache_path: &Path) -> PathBuf {
    let mut value = cache_path.as_os_str().to_os_string();
    value.push(".sha256");
    PathBuf::from(value)
}

fn ensure_import_gitattributes(repo_root: &Path) -> Result<()> {
    let imports = repo_root.join(".ado-aw").join("imports");
    fs::create_dir_all(&imports)
        .with_context(|| format!("failed to create import cache {}", imports.display()))?;
    let attributes = imports.join(".gitattributes");
    if !attributes.exists() {
        fs::write(&attributes, IMPORT_GITATTRIBUTES)
            .with_context(|| format!("failed to write {}", attributes.display()))?;
    }
    Ok(())
}

#[derive(Debug, Serialize, Deserialize)]
struct RefMetadata {
    version: u32,
    #[serde(default)]
    entries: BTreeMap<String, RefMetadataEntry>,
}

impl Default for RefMetadata {
    fn default() -> Self {
        Self {
            version: REF_METADATA_VERSION,
            entries: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct RefMetadataEntry {
    source: String,
    repository: String,
    #[serde(rename = "requested-ref")]
    requested_ref: String,
    #[serde(rename = "resolved-sha")]
    resolved_sha: CommitSha,
}

impl RefMetadata {
    fn lookup(&self, key: &str) -> Option<CommitSha> {
        self.entries
            .get(key)
            .map(|entry| entry.resolved_sha.clone())
    }

    fn record(
        &mut self,
        remote_identity: &RemoteRepositoryIdentity,
        repository: &str,
        requested_ref: &str,
        resolved_sha: &CommitSha,
    ) {
        let key = ref_metadata_key(remote_identity, repository, requested_ref);
        self.entries.insert(
            key,
            RefMetadataEntry {
                source: remote_identity.source.clone(),
                repository: repository_identity(remote_identity.project.as_deref(), repository),
                requested_ref: requested_ref.to_string(),
                resolved_sha: resolved_sha.clone(),
            },
        );
    }

    fn remove(&mut self, key: &str) -> bool {
        self.entries.remove(key).is_some()
    }
}

fn ref_metadata_key(
    remote_identity: &RemoteRepositoryIdentity,
    repository: &str,
    requested_ref: &str,
) -> String {
    sha256_hex(
        format!(
            "{}\n{}\n{}",
            remote_identity.source,
            repository_identity(remote_identity.project.as_deref(), repository),
            requested_ref
        )
        .as_bytes(),
    )
}

fn repository_identity(project: Option<&str>, repository: &str) -> String {
    match project {
        Some(project) => format!("{project}/{repository}"),
        None => format!("_current-project/{repository}"),
    }
}

fn ref_metadata_path(repo_root: &Path) -> PathBuf {
    repo_root.join(".ado-aw").join("imports").join("refs.json")
}

fn read_ref_metadata(repo_root: &Path) -> Result<RefMetadata> {
    let path = ref_metadata_path(repo_root);
    match fs::read(&path) {
        Ok(bytes) => {
            let metadata: RefMetadata = serde_json::from_slice(&bytes)
                .with_context(|| format!("failed to parse {}", path.display()))?;
            if metadata.version != REF_METADATA_VERSION {
                anyhow::bail!(
                    "unsupported import ref metadata version {} in {}",
                    metadata.version,
                    path.display()
                );
            }
            Ok(metadata)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(RefMetadata::default()),
        Err(error) => Err(error).with_context(|| format!("failed to read {}", path.display())),
    }
}

fn write_ref_metadata(repo_root: &Path, metadata: &RefMetadata) -> Result<()> {
    ensure_import_gitattributes(repo_root)?;
    let path = ref_metadata_path(repo_root);
    let bytes = serde_json::to_vec_pretty(metadata).context("failed to serialize import refs")?;
    fs::write(&path, bytes).with_context(|| format!("failed to write {}", path.display()))
}

fn resolve_local_path(base_dir: &Path, repo_root: &Path, import_path: &str) -> Result<PathBuf> {
    if import_path.contains('\\') {
        anyhow::bail!("local import path must use `/`, got `{import_path}`");
    }
    for (index, segment) in import_path.split('/').enumerate() {
        if segment.is_empty() {
            anyhow::bail!("local import path contains an empty segment: `{import_path}`");
        }
        if segment == "." && index != 0 {
            anyhow::bail!("local import path contains an ambiguous `.` segment: `{import_path}`");
        }
    }
    let relative = Path::new(import_path);
    if relative.is_absolute() {
        anyhow::bail!("local import path must be relative, got `{import_path}`");
    }

    let base_relative = base_dir.strip_prefix(repo_root).with_context(|| {
        format!(
            "import base directory {} is outside repository root {}",
            base_dir.display(),
            repo_root.display()
        )
    })?;
    let mut segments = path_segments(base_relative)?;
    for component in relative.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if segments.pop().is_none() {
                    anyhow::bail!("local import `{import_path}` escapes the repository root");
                }
            }
            Component::Normal(value) => segments.push(value.to_os_string()),
            Component::RootDir | Component::Prefix(_) => {
                anyhow::bail!("local import path must be relative, got `{import_path}`");
            }
        }
    }
    if segments.is_empty() {
        anyhow::bail!("local import path must name a file");
    }
    let mut resolved = repo_root.to_path_buf();
    for segment in segments {
        resolved.push(segment);
    }
    Ok(resolved)
}

fn path_segments(path: &Path) -> Result<Vec<std::ffi::OsString>> {
    let mut segments = Vec::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if segments.pop().is_none() {
                    anyhow::bail!("import base directory escapes the repository root");
                }
            }
            Component::Normal(value) => segments.push(value.to_os_string()),
            Component::RootDir | Component::Prefix(_) => {}
        }
    }
    Ok(segments)
}

fn ensure_path_within_repo(path: &Path, repo_root: &Path) -> Result<()> {
    if !path.starts_with(repo_root) {
        anyhow::bail!(
            "local import {} resolves outside repository root {}",
            path.display(),
            repo_root.display()
        );
    }
    Ok(())
}

fn local_identity_path(path: &Path) -> String {
    let value = path.to_string_lossy().to_string();
    if cfg!(windows) {
        value.to_ascii_lowercase()
    } else {
        value
    }
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
                serde_yaml::Value::Mapping(_) | serde_yaml::Value::Null => value,
                other => anyhow::bail!(
                    "import YAML front matter must be a mapping/object, got {}",
                    yaml_value_kind(&other)
                ),
            }
        }
        None => serde_yaml::Value::Null,
    };
    let body = match section {
        Some(section) => extract_markdown_section(&parts.markdown_body, section)?,
        None => parts.markdown_body,
    };
    Ok((front_matter, body))
}

fn enforce_manifest_size(size: usize, source: &str) -> Result<()> {
    if size > MAX_MANIFEST_BYTES {
        anyhow::bail!(
            "import manifest {source} is {size} bytes, exceeding the {MAX_MANIFEST_BYTES} byte limit"
        );
    }
    Ok(())
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

fn extract_markdown_section(body: &str, section: &str) -> Result<String> {
    let lines: Vec<&str> = body.lines().collect();
    let start = lines
        .iter()
        .position(|line| markdown_heading(line).is_some_and(|(_, name)| name == section))
        .ok_or_else(|| anyhow::anyhow!("import section `{section}` was not found"))?;
    let level = markdown_heading(lines[start])
        .map(|(level, _)| level)
        .context("import heading could not be re-parsed")?;
    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find_map(|(index, line)| {
            markdown_heading(line).and_then(|(candidate, _)| (candidate <= level).then_some(index))
        })
        .unwrap_or(lines.len());
    Ok(lines[start..end].join("\n").trim().to_string())
}

fn markdown_heading(line: &str) -> Option<(usize, &str)> {
    let trimmed = line.trim_start();
    let level = trimmed.bytes().take_while(|byte| *byte == b'#').count();
    if !(1..=2).contains(&level) {
        return None;
    }
    let rest = &trimmed[level..];
    if !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let name = rest.trim().trim_end_matches('#').trim();
    (!name.is_empty()).then_some((level, name))
}

fn display_chain(ancestry: &[Ancestor], tail: &str) -> String {
    ancestry
        .iter()
        .map(|ancestor| ancestor.display.as_str())
        .chain(std::iter::once(tail))
        .collect::<Vec<_>>()
        .join(" -> ")
}

fn display_cycle(ancestry: &[Ancestor], repeated: &str) -> String {
    ancestry
        .iter()
        .map(|ancestor| ancestor.display.as_str())
        .chain(std::iter::once(repeated))
        .collect::<Vec<_>>()
        .join(" -> ")
}

fn find_identity_path(
    graph: &HashMap<ResolvedIdentity, Vec<ResolvedIdentity>>,
    start: &ResolvedIdentity,
    target: &ResolvedIdentity,
) -> Option<Vec<ResolvedIdentity>> {
    let mut stack = vec![(start.clone(), vec![start.clone()])];
    let mut visited = std::collections::HashSet::new();
    while let Some((current, path)) = stack.pop() {
        if &current == target {
            return Some(path);
        }
        if !visited.insert(current.clone()) {
            continue;
        }
        for child in graph.get(&current).into_iter().flatten().rev() {
            let mut child_path = path.clone();
            child_path.push(child.clone());
            stack.push((child.clone(), child_path));
        }
    }
    None
}

fn display_identity_path(
    path: &[ResolvedIdentity],
    displays: &HashMap<ResolvedIdentity, String>,
) -> String {
    path.iter()
        .map(|identity| {
            displays
                .get(identity)
                .cloned()
                .unwrap_or_else(|| format!("{identity:?}"))
        })
        .collect::<Vec<_>>()
        .join(" -> ")
}

fn remote_display(
    source: &ImportSource,
    project: Option<&str>,
    repository: &str,
    path: &str,
) -> String {
    format!(
        "{}/{}/{}",
        source.identity(),
        repository_identity(project, repository),
        path
    )
}

fn resolved_remote_display(
    remote_identity: &RemoteRepositoryIdentity,
    repository: &str,
    path: &str,
) -> String {
    format!(
        "{}/{}/{}",
        remote_identity.source,
        repository_identity(remote_identity.project.as_deref(), repository),
        path
    )
}

#[derive(Clone, Copy)]
struct RemoteParts<'a> {
    source: &'a ImportSource,
    project: Option<&'a str>,
    repository: &'a str,
    path: &'a str,
    requested_ref: &'a str,
}

impl RemoteParts<'_> {
    fn display(self) -> String {
        remote_display(self.source, self.project, self.repository, self.path)
    }
}

fn remote_parts(spec: &ParsedImportSpec) -> Result<RemoteParts<'_>> {
    match spec {
        ParsedImportSpec::Remote {
            source,
            project,
            repository,
            path,
            requested_ref,
            ..
        } => Ok(RemoteParts {
            source,
            project: project.as_deref(),
            repository,
            path,
            requested_ref,
        }),
        ParsedImportSpec::Local { .. } => {
            anyhow::bail!("internal error: remote fetcher received a local import")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    fn remote_spec(reference: &str) -> ParsedImportSpec {
        ParsedImportSpec::Remote {
            source: ImportSource::default(),
            project: Some("project".to_string()),
            repository: "repo".to_string(),
            path: "components/example.md".to_string(),
            requested_ref: reference.to_string(),
            section: None,
            optional: false,
        }
    }

    #[test]
    fn ado_ref_candidates_support_branch_tag_and_full_names() {
        let short = ado_ref_candidates("v1");
        assert_eq!(short[0].full_name, "refs/heads/v1");
        assert_eq!(short[1].full_name, "refs/tags/v1");
        let branch = ado_ref_candidates("refs/heads/main");
        assert_eq!(branch.len(), 1);
        assert_eq!(branch[0].filter, "heads/main");
    }

    #[test]
    fn nested_remote_paths_resolve_and_cannot_escape() {
        assert_eq!(
            join_remote_relative_path("components/a/main.md", "../shared.md").unwrap(),
            "components/shared.md"
        );
        assert!(join_remote_relative_path("main.md", "../escape.md").is_err());
        assert!(join_remote_relative_path("components/main.md", "a//b.md").is_err());
    }

    #[test]
    fn cache_key_includes_source_repo_sha_and_path() {
        let repo = tempfile::tempdir().unwrap();
        let path = cache_path(
            repo.path(),
            &remote_spec("main"),
            &RemoteRepositoryIdentity::from_spec(&remote_spec("main")).unwrap(),
            &CommitSha::parse(SHA).unwrap(),
        )
        .unwrap();
        let text = path.to_string_lossy();
        assert!(text.contains(SHA));
        assert!(text.contains("azure-repos"));
        assert!(text.contains("example.md"));
    }

    #[test]
    fn context_environment_requires_org_and_project() {
        let context = ado_context_from_env(|name| match name {
            "SYSTEM_COLLECTIONURI" => Some("https://dev.azure.com/acme/".to_string()),
            "SYSTEM_TEAMPROJECT" => Some("project".to_string()),
            _ => None,
        })
        .unwrap();
        assert_eq!(context.org_url, "https://dev.azure.com/acme");
        assert_eq!(context.project, "project");
    }

    #[test]
    fn org_and_project_environment_values_can_be_resolved_independently() {
        assert_eq!(
            ado_org_url_from_env(|name| (name == "AZURE_DEVOPS_ORG_URL")
                .then(|| "https://dev.azure.com/acme/".to_string()))
            .as_deref(),
            Some("https://dev.azure.com/acme")
        );
        assert_eq!(
            ado_project_from_env(|name| {
                (name == "SYSTEM_TEAMPROJECT").then(|| " Project A ".to_string())
            })
            .as_deref(),
            Some("Project A")
        );
    }

    #[test]
    fn github_contents_route_encodes_ref_and_path_segments() {
        assert_eq!(
            github_contents_api_route("o", "r", "dir one/a.md", "feature/x"),
            "repos/o/r/contents/dir%20one/a.md?ref=feature%2Fx"
        );
    }

    #[test]
    fn github_missing_ref_422_is_typed_as_not_found() {
        assert!(github_error_is_not_found(
            "gh: No commit found for SHA: missing (HTTP 422)"
        ));
        assert!(!github_error_is_not_found(
            "gh: Validation Failed: malformed request (HTTP 422)"
        ));
    }

    #[test]
    fn typed_not_found_is_detectable_through_anyhow() {
        let error = not_found("x").context("outer");
        assert!(is_import_not_found(&error));
    }

    #[test]
    fn ado_fetcher_rejects_github_source_before_network() {
        let fetcher =
            AdoRepoFetcher::with_resolved(Err("unused".to_string()), Err("unused".to_string()));
        let spec = ParsedImportSpec::Remote {
            source: ImportSource::GitHub {
                host: crate::secure::HostName::parse("github.com").unwrap(),
            },
            project: Some("owner".to_string()),
            repository: "repo".to_string(),
            path: "component.md".to_string(),
            requested_ref: SHA.to_string(),
            section: None,
            optional: false,
        };
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let error = runtime
            .block_on(fetcher.fetch(&spec, &CommitSha::parse(SHA).unwrap()))
            .unwrap_err();
        assert!(error.to_string().contains("internal routing error"));
    }
}
