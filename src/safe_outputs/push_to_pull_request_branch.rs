//! Exact-head guarded code changes to an existing, authorized ADO PR source ref.
use ado_aw_derive::SanitizeConfig;
use anyhow::{Context, ensure};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use tokio::process::Command;

use super::create_pull_request::{
    IfNoChanges, ProtectedFiles, count_patch_files, extract_paths_from_patch, find_protected_files,
    glob_match_simple, read_file_change, validate_patch_paths,
};
use super::pr_common::{
    PrMutationPolicy, PrTargetPolicy, PullRequestReference, collection_identity,
    describe_pr_reference, resolve_configured_pr_target,
};
use super::pr_mutations::UpdatePrContext;
use super::{
    ExecutionContext, ExecutionResult, Executor, ToolResult, Validate, authenticate_ado_request,
};
use crate::sanitize::SanitizeContent;
use crate::secure::{CommitSha, GitRefName, RelativeSafePath, StrictRelativePath};
use crate::tool_result;

const MAX_PATCH: usize = 5 * 1024 * 1024;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PushToPullRequestBranchParams {
    #[serde(default)]
    pub pull_request_id: Option<PullRequestReference>,
    /// Checkout alias from the source snapshot (self or an explicitly checked-out alias).
    pub repository: RelativeSafePath,
    /// Original PR source head from trusted preparation; never a merge commit or agent-created head.
    pub expected_head_sha: CommitSha,
}
impl Validate for PushToPullRequestBranchParams {
    fn validate(&self) -> anyhow::Result<()> {
        if let Some(reference) = &self.pull_request_id {
            super::pr_common::validate_reference(reference)?;
            ensure!(
                matches!(reference, PullRequestReference::Number(_)),
                "PR branch pushes require a pre-existing PR, not a temporary ID"
            );
        }
        ensure!(
            self.expected_head_sha.as_str() != "0000000000000000000000000000000000000000",
            "Expected head must identify an existing commit"
        );
        Ok(())
    }
}

#[derive(Deserialize, JsonSchema)]
struct PushFields {
    pull_request_id: Option<PullRequestReference>,
    repository: RelativeSafePath,
    expected_head_sha: CommitSha,
    patch_file: StrictRelativePath,
    patch_sha256: String,
}
impl Validate for PushFields {}

tool_result! {
    name = "push-to-pull-request-branch",
    write = true,
    params = PushFields,
    #[serde(deny_unknown_fields)]
    pub struct PushToPullRequestBranchResult {
        #[serde(default)]
        pull_request_id:Option<PullRequestReference>,
        repository:RelativeSafePath,
        expected_head_sha:CommitSha,
        patch_file:StrictRelativePath,
        patch_sha256:String,
    }
}
impl SanitizeContent for PushToPullRequestBranchResult {
    fn sanitize_content_fields(&mut self) {}
}

#[derive(Debug, Clone, Serialize, Deserialize, SanitizeConfig)]
#[serde(deny_unknown_fields)]
pub struct PushToPullRequestBranchConfig {
    #[serde(default)]
    #[sanitize_config(skip)]
    pub target: super::update_pull_request::UpdatePullRequestTarget,
    #[serde(default, rename = "target-repo")]
    pub target_repo: Option<String>,
    #[serde(default, rename = "allowed-repositories")]
    pub allowed_repositories: Vec<String>,
    #[serde(default, rename = "required-labels")]
    pub required_labels: Vec<String>,
    #[serde(default, rename = "required-title-prefix")]
    pub required_title_prefix: Option<String>,
    #[serde(default, rename = "allowed-branches")]
    pub allowed_branches: Vec<String>,
    #[serde(default = "default_max_files", rename = "max-files")]
    #[sanitize_config(skip)]
    pub max_files: usize,
    #[serde(default, rename = "excluded-files")]
    pub excluded_files: Vec<String>,
    #[serde(default = "default_protected", rename = "protected-files")]
    #[sanitize_config(skip)]
    pub protected_files: ProtectedFiles,
    #[serde(default = "default_no_changes", rename = "if-no-changes")]
    #[sanitize_config(skip)]
    pub if_no_changes: IfNoChanges,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[sanitize_config(skip)]
    pub max: Option<u32>,
}
fn default_max_files() -> usize {
    100
}
fn default_protected() -> ProtectedFiles {
    ProtectedFiles::Blocked
}
fn default_no_changes() -> IfNoChanges {
    IfNoChanges::Warn
}
impl Default for PushToPullRequestBranchConfig {
    fn default() -> Self {
        Self {
            target: Default::default(),
            target_repo: None,
            allowed_repositories: vec![],
            required_labels: vec![],
            required_title_prefix: None,
            allowed_branches: vec![],
            max_files: 100,
            excluded_files: vec![],
            protected_files: default_protected(),
            if_no_changes: default_no_changes(),
            max: None,
        }
    }
}
pub(crate) fn validate_push_config(config: &PushToPullRequestBranchConfig) -> anyhow::Result<()> {
    ensure!(
        !config.allowed_branches.is_empty(),
        "push-to-pull-request-branch requires explicit allowed-branches"
    );
    ensure!(
        config.max_files > 0 && config.max_files <= 1_000,
        "max-files must be between 1 and 1000"
    );
    for pattern in config.allowed_branches.iter().chain(&config.excluded_files) {
        ensure!(
            !pattern.trim().is_empty(),
            "Branch/file patterns must not be empty"
        );
        crate::validate::reject_pipeline_injection(pattern, "push policy")?;
    }
    Ok(())
}

#[derive(Deserialize)]
struct Repository {
    id: String,
    name: String,
    #[serde(rename = "defaultBranch")]
    default_branch: String,
    project: Project,
}
#[derive(Deserialize)]
struct Project {
    id: String,
    name: String,
}
#[derive(Deserialize)]
struct RepositoryId {
    id: String,
}
#[derive(Deserialize)]
struct PullRequest {
    #[serde(rename = "pullRequestId")]
    id: u64,
    status: String,
    #[serde(rename = "sourceRefName")]
    source: GitRefName,
    #[serde(rename = "targetRefName")]
    target: GitRefName,
    repository: RepositoryId,
    #[serde(rename = "forkSource")]
    fork: Option<Value>,
}

async fn source_state(
    op: &UpdatePrContext<'_>,
    config: &PushToPullRequestBranchConfig,
) -> anyhow::Result<(PullRequest, Repository, CommitSha)> {
    let base = op.repository_api_base();
    let repository: Repository =
        super::pr_comments::get_json(op, &format!("{base}?api-version=7.1")).await?;
    crate::secure::Guid::parse(&repository.id)
        .context("Invalid repository ID in source metadata")?;
    crate::secure::Guid::parse(&repository.project.id)
        .context("Invalid project ID in source metadata")?;
    ensure!(
        (op.target
            .project
            .eq_ignore_ascii_case(&repository.project.id)
            || op
                .target
                .project
                .eq_ignore_ascii_case(&repository.project.name))
            && (op.target.repository.eq_ignore_ascii_case(&repository.id)
                || op.target.repository.eq_ignore_ascii_case(&repository.name)),
        "Source metadata does not match the authorized repository target"
    );
    let pr: PullRequest = super::pr_comments::get_json(
        op,
        &format!("{base}/pullRequests/{}?api-version=7.1", op.pr_id),
    )
    .await?;
    ensure!(
        pr.id == op.pr_id && pr.repository.id.eq_ignore_ascii_case(&repository.id),
        "PR identity does not match its authorized repository"
    );
    ensure!(
        pr.status.eq_ignore_ascii_case("active"),
        "Only active PR source branches may be updated"
    );
    ensure!(
        pr.fork.is_none(),
        "Fork-backed PR branch pushes are not supported"
    );
    ensure!(
        pr.source.starts_with("refs/heads/") && pr.target.starts_with("refs/heads/"),
        "PR refs must be branch refs"
    );
    ensure!(
        pr.source != pr.target && pr.source.as_str() != repository.default_branch,
        "PR source must not be its target or the repository default branch"
    );
    let branch = pr
        .source
        .strip_prefix("refs/heads/")
        .context("Invalid PR source ref")?;
    ensure!(
        config
            .allowed_branches
            .iter()
            .any(|pattern| super::wildcard_match(pattern, branch)),
        "PR source branch is not in allowed-branches"
    );
    #[derive(Deserialize)]
    struct Ref {
        name: String,
        #[serde(rename = "objectId")]
        head: CommitSha,
    }
    #[derive(Deserialize)]
    struct Refs {
        value: Vec<Ref>,
    }
    let mut url = reqwest::Url::parse(&format!("{base}/refs"))?;
    url.query_pairs_mut()
        .append_pair(
            "filter",
            pr.source
                .strip_prefix("refs/")
                .context("Invalid source ref")?,
        )
        .append_pair("api-version", "7.1");
    let refs: Refs = super::pr_comments::get_json(op, url.as_str()).await?;
    let matches = refs
        .value
        .into_iter()
        .filter(|reference| reference.name == pr.source.as_str())
        .collect::<Vec<_>>();
    ensure!(matches.len() == 1, "PR source ref is missing or ambiguous");
    let head = matches
        .into_iter()
        .next()
        .context("Missing source ref")?
        .head;
    ensure!(
        head.as_str() != "0000000000000000000000000000000000000000",
        "PR source ref has no live commit"
    );
    Ok((pr, repository, head))
}

async fn git(repo: &Path, args: &[&str]) -> anyhow::Result<std::process::Output> {
    git_without_filters(repo)
        .await?
        .args(args)
        .output()
        .await
        .context("Failed to run PR patch git operation")
}

fn git_path(path: &Path) -> std::borrow::Cow<'_, str> {
    let text = path.to_string_lossy();
    #[cfg(windows)]
    if let Some(rest) = text.strip_prefix(r"\\?\") {
        return std::borrow::Cow::Owned(if let Some(unc) = rest.strip_prefix(r"UNC\") {
            format!(r"\\{unc}")
        } else {
            rest.to_string()
        });
    }
    text
}

async fn git_without_filters(repo: &Path) -> anyhow::Result<Command> {
    let filters = git_command(repo)
        .args([
            "config",
            "--name-only",
            "--get-regexp",
            r"^filter\..*\.(clean|smudge|process|required)$",
        ])
        .output()
        .await?;
    ensure!(
        filters.status.success() || filters.status.code() == Some(1),
        "Could not inspect git filter configuration"
    );
    let mut command = git_command(repo);
    for key in std::str::from_utf8(&filters.stdout)?.lines() {
        ensure!(
            key.bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_')),
            "Unsupported git filter configuration key"
        );
        command.arg("-c").arg(format!(
            "{key}={}",
            if key.ends_with(".required") {
                "false"
            } else {
                ""
            }
        ));
    }
    Ok(command)
}

fn git_command(repo: &Path) -> Command {
    let mut command = Command::new("git");
    command.env_clear();
    for name in [
        "PATH",
        "HOME",
        "USERPROFILE",
        "HOMEDRIVE",
        "HOMEPATH",
        "SYSTEMROOT",
        "WINDIR",
        "TEMP",
        "TMP",
        "TMPDIR",
        "LANG",
        "LC_ALL",
        "SSL_CERT_FILE",
        "SSL_CERT_DIR",
        "GIT_SSL_CAINFO",
    ] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    command
        .args(["-c", "core.hooksPath=/dev/null"])
        .env("GIT_TERMINAL_PROMPT", "0")
        .current_dir(repo);
    command
}

async fn ensure_source_commit(
    repo: &Path,
    head: &CommitSha,
    op: &UpdatePrContext<'_>,
    metadata: &Repository,
) -> anyhow::Result<()> {
    if git(repo, &["cat-file", "-e", &format!("{head}^{{commit}}")])
        .await?
        .status
        .success()
    {
        return Ok(());
    }
    let remote = git(repo, &["remote", "get-url", "origin"]).await?;
    ensure!(remote.status.success(), "Source checkout has no origin");
    let remote = String::from_utf8(remote.stdout)?.trim().to_string();
    let url = reqwest::Url::parse(&remote)
        .context("Source checkout origin must be HTTPS Azure DevOps")?;
    ensure!(
        url.scheme() == "https"
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none()
            && url.port().is_none(),
        "Invalid source checkout origin"
    );
    let host = url.host_str().context("Source origin host missing")?;
    ensure!(
        host == "dev.azure.com"
            || host
                == format!(
                    "{}.visualstudio.com",
                    op.target.organization.to_ascii_lowercase()
                ),
        "Source origin is not the authorized Azure DevOps organization"
    );
    ensure!(
        url.username().is_empty() || url.username().eq_ignore_ascii_case(&op.target.organization),
        "Source origin contains an unexpected user-info component"
    );
    let parts = url.path().trim_matches('/').split('/').collect::<Vec<_>>();
    ensure!(
        (host == "dev.azure.com" && parts.len() == 4 && parts[2] == "_git")
            || (host != "dev.azure.com" && parts.len() == 3 && parts[1] == "_git")
            || (host != "dev.azure.com"
                && parts.len() == 4
                && parts[0].eq_ignore_ascii_case("DefaultCollection")
                && parts[2] == "_git"),
        "Source origin must identify exactly one Azure DevOps repository"
    );
    let parsed = crate::ado::parse_ado_remote(&remote)?;
    ensure!(
        collection_identity(&parsed.org_url) == collection_identity(&op.target.organization_url),
        "Source origin organization mismatch"
    );
    let project = percent_encoding::percent_decode_str(&parsed.project).decode_utf8()?;
    let name = percent_encoding::percent_decode_str(&parsed.repo_name).decode_utf8()?;
    ensure!(
        (project.eq_ignore_ascii_case(&metadata.project.name)
            || project.eq_ignore_ascii_case(&metadata.project.id))
            && (name.eq_ignore_ascii_case(&metadata.name)
                || name.eq_ignore_ascii_case(&metadata.id)),
        "Source origin repository mismatch"
    );
    let rewrites = git(
        repo,
        &[
            "config",
            "--get-regexp",
            r"^url\..*\.(insteadof|pushinsteadof)$",
        ],
    )
    .await?;
    ensure!(
        rewrites.status.code() == Some(1),
        "Git URL rewrite configuration is not permitted for authenticated source fetches"
    );
    let mut command = git_command(repo);
    use base64::Engine;
    let header = match op.connection_type {
        Some(_) => format!("Authorization: bearer {}", op.token),
        None => format!(
            "Authorization: Basic {}",
            base64::engine::general_purpose::STANDARD.encode(format!(":{}", op.token))
        ),
    };
    let output = command
        .args([
            "-c",
            "credential.helper=",
            "-c",
            "http.followRedirects=false",
            "-c",
            "http.sslVerify=true",
            "fetch",
            "--no-tags",
            "--depth=1",
            "--",
            &remote,
            head.as_str(),
        ])
        .env("GIT_CONFIG_COUNT", "3")
        .env("GIT_CONFIG_KEY_0", "http.extraheader")
        .env("GIT_CONFIG_VALUE_0", "")
        .env("GIT_CONFIG_KEY_1", format!("http.{remote}.extraheader"))
        .env("GIT_CONFIG_VALUE_1", "")
        .env("GIT_CONFIG_KEY_2", format!("http.{remote}.extraheader"))
        .env("GIT_CONFIG_VALUE_2", header)
        .env("GIT_TERMINAL_PROMPT", "0")
        .current_dir(repo)
        .output()
        .await
        .context("Source commit fetch failed")?;
    ensure!(
        output.status.success(),
        "Source commit fetch failed; verify checkout and read authorization"
    );
    ensure!(
        git(repo, &["cat-file", "-e", &format!("{head}^{{commit}}")])
            .await?
            .status
            .success(),
        "Fetched source commit is unavailable"
    );
    Ok(())
}

struct Worktree {
    repo: PathBuf,
    path: PathBuf,
    cleaned: bool,
}
impl Worktree {
    async fn cleanup(&mut self) -> anyhow::Result<()> {
        let output = git(
            &self.repo,
            &[
                "worktree",
                "remove",
                "--force",
                &self.path.to_string_lossy(),
            ],
        )
        .await?;
        ensure!(
            output.status.success(),
            "PR push worktree cleanup failed; remote push was not attempted"
        );
        self.cleaned = true;
        Ok(())
    }
}
impl Drop for Worktree {
    fn drop(&mut self) {
        if self.cleaned {
            return;
        }
        if let Err(error) = std::process::Command::new("git")
            .args(["worktree", "remove", "--force"])
            .arg(&self.path)
            .current_dir(&self.repo)
            .output()
            .and_then(|result| {
                if result.status.success() {
                    Ok(())
                } else {
                    Err(std::io::Error::other("git worktree cleanup failed"))
                }
            })
        {
            log::warn!("PR push worktree cleanup failed: {error}");
        }
    }
}

async fn patch_changes(
    repo: &Path,
    head: &CommitSha,
    patch: &Path,
    config: &PushToPullRequestBranchConfig,
) -> anyhow::Result<Vec<Value>> {
    let excludes = config
        .excluded_files
        .iter()
        .map(|pattern| format!("--exclude={pattern}"))
        .collect::<Vec<_>>();
    let mut inspect = vec!["apply", "--numstat", "-z"];
    inspect.extend(excludes.iter().map(String::as_str));
    let patch_path = git_path(patch);
    inspect.push(&patch_path);
    let summary = git(repo, &inspect).await?;
    ensure!(
        summary.status.success(),
        "Could not inspect PR patch paths: {}",
        crate::sanitize::neutralize_pipeline_commands(&String::from_utf8_lossy(&summary.stderr))
    );
    let mut touched = Vec::new();
    for record in summary
        .stdout
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
    {
        let text = std::str::from_utf8(record)?;
        let path = text
            .splitn(3, '\t')
            .nth(2)
            .context("Invalid PR patch path summary")?;
        ensure!(
            !path.is_empty(),
            "PR patch renames must use delete/add records from the safe-output collector"
        );
        crate::secure::RelativeSafePath::parse(path)?;
        touched.push(path.to_string());
    }
    ensure!(
        touched.len() <= config.max_files,
        "PR patch path count exceeds max-files"
    );
    ensure!(touched.iter().all(|path|path!="aw-context"&&!path.starts_with("aw-context/")),
        "Compiler-owned aw-context data is not pushable");
    if touched.is_empty() {
        return Ok(Vec::new());
    }
    ensure!(
        config.protected_files == ProtectedFiles::Allowed
            || find_protected_files(
                &touched
                    .iter()
                    .filter(|path| !config
                        .excluded_files
                        .iter()
                        .any(|pattern| glob_match_simple(pattern, path)))
                    .cloned()
                    .collect::<Vec<_>>()
            )
            .is_empty(),
        "PR patch touches protected files"
    );
    let mut args = vec![
        "--literal-pathspecs",
        "ls-tree",
        "-l",
        "-z",
        head.as_str(),
        "--",
    ];
    args.extend(touched.iter().map(String::as_str));
    let sizes = git(repo, &args).await?;
    ensure!(
        sizes.status.success(),
        "Could not inspect source blob sizes"
    );
    let mut total = 0usize;
    for entry in sizes
        .stdout
        .split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty())
    {
        let text = std::str::from_utf8(entry)?;
        let header = text
            .split('\t')
            .next()
            .context("Invalid source tree metadata")?
            .split_whitespace()
            .collect::<Vec<_>>();
        ensure!(
            header.len() == 4 && header[1] == "blob",
            "Changed source path is not a regular blob"
        );
        let size: usize = header[3].parse().context("Invalid source blob size")?;
        ensure!(
            size <= MAX_PATCH,
            "Changed source file exceeds the 5 MB limit"
        );
        total = total
            .checked_add(size)
            .context("Source blob size overflow")?;
        ensure!(
            total <= 10 * 1024 * 1024,
            "Changed source files exceed the 10 MB aggregate bound"
        );
    }
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("worktree");
    let output = git(
        repo,
        &[
            "worktree",
            "add",
            "--detach",
            &path.to_string_lossy(),
            head.as_str(),
        ],
    )
    .await?;
    ensure!(
        output.status.success(),
        "Could not create an isolated source-head worktree"
    );
    let mut guard = Worktree {
        repo: repo.to_path_buf(),
        path: path.clone(),
        cleaned: false,
    };
    for check in [true, false] {
        let mut command = git_without_filters(&path).await?;
        command.args(["apply", "--index", "--binary"]);
        if check {
            command.arg("--check");
        }
        command
            .args(&excludes)
            .arg(patch_path.as_ref())
            .current_dir(&path);
        let output = command.output().await?;
        ensure!(
            output.status.success(),
            "Patch does not apply cleanly to the exact source head"
        );
    }
    let output = git(
        &path,
        &[
            "diff",
            "--cached",
            "--raw",
            "-z",
            "--no-renames",
            head.as_str(),
        ],
    )
    .await?;
    ensure!(
        output.status.success(),
        "Could not inspect applied PR patch"
    );
    let chunks = output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    ensure!(chunks.len() % 2 == 0, "Malformed git change records");
    let mut changes = Vec::new();
    let mut paths = Vec::new();
    for pair in chunks.chunks_exact(2) {
        let header = std::str::from_utf8(pair[0])?
            .split_whitespace()
            .collect::<Vec<_>>();
        ensure!(header.len() == 5, "Malformed git change metadata");
        let old = header[0].strip_prefix(':').context("Malformed git mode")?;
        let new = header[1];
        let file = std::str::from_utf8(pair[1])?;
        crate::secure::RelativeSafePath::parse(file)?;
        ensure!(
            matches!(old, "000000" | "100644" | "100755")
                && matches!(new, "000000" | "100644" | "100755"),
            "PR pushes do not support symlink or submodule changes"
        );
        ensure!(
            new == "000000" || old == new || (old == "000000" && new == "100644"),
            "PR push cannot represent file-mode changes"
        );
        paths.push(file.to_string());
        if new == "000000" {
            changes.push(json!({"changeType":"delete","item":{"path":format!("/{file}")}}));
        } else {
            let full = crate::validate::ensure_path_within_base(
                &path.join(file),
                &path,
                "applied PR file",
            )?;
            let size = tokio::fs::metadata(&full).await?.len();
            ensure!(
                size <= MAX_PATCH as u64,
                "Applied PR file exceeds the 5 MB limit"
            );
            changes.push(
                read_file_change(if old == "000000" { "add" } else { "edit" }, file, &full).await?,
            );
        }
    }
    ensure!(
        changes.len() <= config.max_files,
        "Applied changes exceed max-files"
    );
    ensure!(
        config.protected_files == ProtectedFiles::Allowed
            || find_protected_files(&paths).is_empty(),
        "Applied PR changes contain protected files"
    );
    if !paths.is_empty() {
        let mut attributes = vec!["check-attr", "--cached", "-z", "filter", "--"];
        attributes.extend(paths.iter().map(String::as_str));
        let output = git(&path, &attributes).await?;
        ensure!(
            output.status.success(),
            "Could not inspect applied file filters"
        );
        let values = output
            .stdout
            .split(|byte| *byte == 0)
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>();
        ensure!(values.len() % 3 == 0, "Malformed file-filter metadata");
        ensure!(
            values
                .chunks_exact(3)
                .all(|entry| matches!(entry[2], b"unspecified" | b"unset")),
            "PR pushes do not support LFS or custom-filtered file changes"
        );
    }
    ensure!(
        serde_json::to_vec(&changes)?.len() <= 10 * 1024 * 1024,
        "ADO push payload exceeds the 10 MB limit"
    );
    guard.cleanup().await?;
    Ok(changes)
}

#[async_trait::async_trait]
impl Executor for PushToPullRequestBranchResult {
    fn dry_run_summary(&self) -> String {
        format!(
            "push guarded code changes to {} at {}",
            describe_pr_reference(self.pull_request_id.as_ref()),
            self.expected_head_sha
        )
    }
    async fn execute_impl(&self, ctx: &ExecutionContext) -> anyhow::Result<ExecutionResult> {
        PushToPullRequestBranchParams {
            pull_request_id: self.pull_request_id.clone(),
            repository: self.repository.clone(),
            expected_head_sha: self.expected_head_sha.clone(),
        }
        .validate()?;
        let config: PushToPullRequestBranchConfig = ctx.get_tool_config(Self::NAME)?;
        validate_push_config(&config)?;
        let (pr_id, target) = match resolve_configured_pr_target(
            Self::NAME,
            self.pull_request_id.as_ref(),
            Some(self.repository.as_str()),
            ctx,
        )
        .await?
        {
            Ok(target) => target,
            Err(failure) => return Ok(failure),
        };
        let client = super::pr_comments::client()?;
        let op = UpdatePrContext {
            client: &client,
            target,
            pr_id,
            token: ctx
                .access_token
                .as_deref()
                .context("No access token available")?,
            connection_type: ctx.write_connection_type,
        };
        let (pr, metadata, head) = source_state(&op, &config).await?;
        ensure!(
            head.eq_ignore_ascii_case(&self.expected_head_sha),
            "PR source head changed; refusing stale patch"
        );
        let patch = crate::validate::ensure_path_within_base(
            &ctx.working_directory.join(self.patch_file.as_str()),
            &ctx.working_directory,
            "PR patch",
        )?;
        ensure!(
            tokio::fs::metadata(&patch).await?.len() <= MAX_PATCH as u64,
            "PR patch exceeds the 5 MB limit"
        );
        let bytes = tokio::fs::read(&patch).await?;
        ensure!(
            crate::hash::sha256_hex(&bytes) == self.patch_sha256,
            "PR patch SHA-256 mismatch"
        );
        let text =
            std::str::from_utf8(&bytes).context("PR patch must be a UTF-8 git binary patch")?;
        validate_patch_paths(text)?;
        validate_binary_bounds(text)?;
        let paths = extract_paths_from_patch(text)
            .into_iter()
            .filter(|path| {
                !config
                    .excluded_files
                    .iter()
                    .any(|pattern| glob_match_simple(pattern, path))
            })
            .collect::<Vec<_>>();
        ensure!(
            count_patch_files(text) <= config.max_files,
            "PR patch exceeds max-files"
        );
        ensure!(
            config.protected_files == ProtectedFiles::Allowed
                || find_protected_files(&paths).is_empty(),
            "PR patch contains protected files"
        );
        if text.trim().is_empty() {
            return Ok(empty_patch(&config));
        }
        let repo = super::resolve_repository_checkout_dir(&op.target.alias, ctx)?;
        ensure_source_commit(&repo, &head, &op, &metadata).await?;
        let changes = patch_changes(&repo, &head, &patch, &config).await?;
        if changes.is_empty() {
            return Ok(empty_patch(&config));
        }
        let (current, _, current_head) = source_state(&op, &config).await?;
        ensure!(
            current.source == pr.source && current.target == pr.target && current_head == head,
            "PR source changed during patch preparation"
        );
        let payload = json!({"refUpdates":[{"name":pr.source,"oldObjectId":head}],
            "commits":[{"comment":format!("Agentic workflow update for PR #{pr_id}"),"parents":[head],"changes":changes}]});
        let mut data = json!({"pull_request_id":pr_id,"repository":op.target.qualified_repository(),
            "source_ref":pr.source,"expected_head_sha":head,"push_status":"uncertain"});
        let response = authenticate_ado_request(
            client.post(format!(
                "{}/pushes?api-version=7.1",
                op.repository_api_base()
            )),
            op.token,
            op.connection_type,
        )
        .json(&payload)
        .send()
        .await;
        let response = match response {
            Ok(response) if response.status().is_success() => response,
            Ok(response) => {
                data["push_status"] = json!("failed");
                return Ok(ExecutionResult::failure_with_data(
                    format!(
                        "PR branch push failed (HTTP {}); no retry attempted",
                        response.status()
                    ),
                    data,
                ));
            }
            Err(error) => {
                return Ok(ExecutionResult::failure_with_data(
                    format!("PR push delivery is uncertain; no retry attempted: {error}"),
                    data,
                ));
            }
        };
        let pushed: Value = match response.json().await {
            Ok(value) => value,
            Err(error) => {
                return Ok(ExecutionResult::failure_with_data(
                    format!("PR push response is uncertain: {error}"),
                    data,
                ));
            }
        };
        let commit = pushed
            .pointer("/commits/0/commitId")
            .and_then(Value::as_str)
            .and_then(|value| CommitSha::parse(value).ok());
        let returned_ref = pushed.pointer("/refUpdates/0/name").and_then(Value::as_str);
        let Some(commit) = commit.filter(|commit| {
            returned_ref == Some(pr.source.as_str())
                && commit != &head
                && pushed
                    .pointer("/refUpdates/0/newObjectId")
                    .and_then(Value::as_str)
                    == Some(commit.as_str())
                && pushed
                    .pointer("/refUpdates/0/oldObjectId")
                    .and_then(Value::as_str)
                    == Some(head.as_str())
                && pushed.pointer("/commits/0/parents") == Some(&json!([head]))
        }) else {
            return Ok(ExecutionResult::failure_with_data(
                "PR push response did not confirm the expected ref/commit",
                data,
            ));
        };
        data["commit_id"] = json!(commit);
        match source_state(&op, &config).await {
            Ok((after, _, observed)) if after.source == pr.source && observed == commit => {
                data["push_status"] = json!("confirmed");
                Ok(ExecutionResult::success_with_data(
                    "PR source branch update confirmed",
                    data,
                ))
            }
            outcome => {
                let reason = match outcome {
                    Ok(_) => "source ref moved again".into(),
                    Err(error) => format!("{error:#}"),
                };
                Ok(ExecutionResult::failure_with_data(
                    format!("PR push was accepted but final state is unconfirmed: {reason}"),
                    data,
                ))
            }
        }
    }
}

pub(crate) async fn prepare_agent(
    ctx: &ExecutionContext,
    snapshot_path: &Path,
) -> anyhow::Result<()> {
    let config: PushToPullRequestBranchConfig =
        ctx.get_tool_config(PushToPullRequestBranchResult::NAME)?;
    validate_push_config(&config)?;
    let policy = PrMutationPolicy::parse(
        ctx.tool_configs
            .get(PushToPullRequestBranchResult::NAME)
            .context("Push tool is not configured")?,
    )?;
    if matches!(policy.target_policy()?, PrTargetPolicy::Explicit) {
        if let Some(parent) = snapshot_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(snapshot_path,serde_json::to_vec_pretty(&json!({
            "target":"*","source_prepared":false,
            "guidance":"Use an explicitly prepared checkout and the selected PR's original source head; no default PR was selected."
        }))?).await?;
        log::warn!(
            "Wildcard PR push targets require a checkout already based on the selected PR source head; no arbitrary branch is fetched"
        );
        return Ok(());
    }
    let (pr_id, target) =
        resolve_configured_pr_target(PushToPullRequestBranchResult::NAME, None, None, ctx)
            .await?
            .map_err(|failure| anyhow::anyhow!(failure.message))?;
    let client = super::pr_comments::client()?;
    let op = UpdatePrContext {
        client: &client,
        target,
        pr_id,
        token: ctx
            .access_token
            .as_deref()
            .context("Source preparation needs read authentication")?,
        connection_type: ctx.write_connection_type,
    };
    let (pr, metadata, head) = source_state(&op, &config).await?;
    let repo = super::resolve_repository_checkout_dir(&op.target.alias, ctx)?;
    let status = git(&repo, &["status", "--porcelain"]).await?;
    ensure!(
        status.status.success() && status.stdout.is_empty(),
        "PR source preparation refuses an invalid or dirty checkout"
    );
    ensure_source_commit(&repo, &head, &op, &metadata).await?;
    let checkout = git(&repo, &["checkout", "--detach", head.as_str()]).await?;
    ensure!(
        checkout.status.success(),
        "Could not select the exact PR source snapshot"
    );
    let snapshot = json!({"pull_request_id":pr_id,"repository":op.target.alias,"source_ref":pr.source,"expected_head_sha":head});
    if let Some(parent) = snapshot_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::write(snapshot_path, serde_json::to_vec_pretty(&snapshot)?).await?;
    println!(
        "Prepared exact PR source snapshot at {}",
        snapshot_path.display()
    );
    Ok(())
}

pub(crate) async fn capture_patch(repo: &Path, head: &CommitSha) -> anyhow::Result<Vec<u8>> {
    let sparse = git(repo, &["config", "--bool", "core.sparseCheckout"]).await?;
    ensure!(
        sparse.status.code() == Some(1)
            || String::from_utf8_lossy(&sparse.stdout).trim() == "false",
        "PR patch capture requires a full, non-sparse source checkout"
    );
    let commit = git(
        repo,
        &["rev-parse", "--verify", &format!("{head}^{{commit}}")],
    )
    .await?;
    ensure!(
        commit.status.success(),
        "Expected PR source commit is not available in this checkout; prepare the exact source snapshot first"
    );
    ensure!(
        git(
            repo,
            &["merge-base", "--is-ancestor", head.as_str(), "HEAD"]
        )
        .await?
        .status
        .success(),
        "Checkout is not based on the expected PR source head"
    );
    let merges = git(repo, &["rev-list", "--merges", &format!("{head}..HEAD")]).await?;
    ensure!(
        merges.status.success() && merges.stdout.is_empty(),
        "PR push capture refuses merge/synthetic-merge history; check out the actual PR source head before editing"
    );
    let scratch = tempfile::tempdir()?;
    let index = scratch.path().join("index");
    for args in [
        vec!["read-tree", "HEAD"],
        vec!["add", "-A", "--", ".", ":(top,exclude)aw-context"],
        vec!["reset", "--quiet", head.as_str(), "--", "aw-context"],
    ] {
        let output = git_without_filters(repo)
            .await?
            .args(args)
            .env("GIT_INDEX_FILE", &index)
            .current_dir(repo)
            .output()
            .await?;
        ensure!(
            output.status.success(),
            "Could not capture PR changes in an isolated index"
        );
    }
    let output = git_without_filters(repo)
        .await?
        .args([
            "diff",
            "--cached",
            "--binary",
            "--full-index",
            "--no-renames",
            "--no-ext-diff",
            "--no-textconv",
            head.as_str(),
            "--",
        ])
        .env("GIT_INDEX_FILE", &index)
        .current_dir(repo)
        .output()
        .await?;
    ensure!(
        output.status.success(),
        "Could not capture a PR source-head delta"
    );
    ensure!(
        output.stdout.len() <= MAX_PATCH,
        "PR patch exceeds the 5 MB limit"
    );
    Ok(output.stdout)
}

fn empty_patch(config: &PushToPullRequestBranchConfig) -> ExecutionResult {
    match config.if_no_changes {
        IfNoChanges::Warn => ExecutionResult::warning("PR patch has no effective changes"),
        IfNoChanges::Error => ExecutionResult::failure("PR patch has no effective changes"),
        IfNoChanges::Ignore => ExecutionResult::success("PR patch has no effective changes"),
    }
}

fn validate_binary_bounds(patch: &str) -> anyhow::Result<()> {
    use std::io::Read;
    const ALPHABET: &[u8] =
        b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz!#$%&()*+-;<=>?@^_`{|}~";
    let mut lines = patch.lines().peekable();
    let mut binary = false;
    let mut total = 0usize;
    while let Some(line) = lines.next() {
        if line.starts_with("diff --git ") {
            binary = false;
        }
        if line == "GIT binary patch" {
            binary = true;
            continue;
        }
        let Some((kind, length)) = line.split_once(' ') else {
            continue;
        };
        if !binary || !matches!(kind, "literal" | "delta") {
            continue;
        }
        let length: usize = length.parse().context("Invalid binary patch size")?;
        ensure!(
            length <= MAX_PATCH,
            "Binary patch declares more than 5 MB of data"
        );
        let mut compressed = Vec::new();
        while let Some(encoded) = lines.peek().copied().filter(|line| !line.is_empty()) {
            lines.next();
            let bytes = encoded.as_bytes();
            let count = match bytes[0] {
                b'A'..=b'Z' => usize::from(bytes[0] - b'A') + 1,
                b'a'..=b'z' => usize::from(bytes[0] - b'a') + 27,
                _ => anyhow::bail!("Invalid binary patch line length"),
            };
            ensure!(
                bytes.len() == 1 + count.div_ceil(4) * 5,
                "Malformed binary patch encoding"
            );
            let mut decoded = Vec::new();
            for group in bytes[1..].chunks_exact(5) {
                let mut value = 0u32;
                for byte in group {
                    let digit = u32::try_from(
                        ALPHABET
                            .iter()
                            .position(|candidate| candidate == byte)
                            .context("Invalid binary patch alphabet")?,
                    )?;
                    value = value
                        .checked_mul(85)
                        .and_then(|value| value.checked_add(digit))
                        .context("Binary patch encoding overflow")?;
                }
                decoded.extend_from_slice(&value.to_be_bytes());
            }
            compressed.extend_from_slice(&decoded[..count]);
            ensure!(
                compressed.len() <= MAX_PATCH,
                "Compressed binary patch exceeds the limit"
            );
        }
        let mut decoded = Vec::new();
        flate2::read::ZlibDecoder::new(compressed.as_slice())
            .take((MAX_PATCH + 1) as u64)
            .read_to_end(&mut decoded)
            .context("Malformed compressed binary patch")?;
        ensure!(
            decoded.len() == length,
            "Binary patch decompression size mismatch"
        );
        let expanded = if kind == "literal" {
            length
        } else {
            let mut cursor = 0;
            let source = delta_size(&decoded, &mut cursor)?;
            let target = delta_size(&decoded, &mut cursor)?;
            ensure!(
                source <= MAX_PATCH && target <= MAX_PATCH,
                "Binary delta expands beyond the 5 MB limit"
            );
            target
        };
        total = total
            .checked_add(expanded)
            .context("Binary patch size overflow")?;
        ensure!(
            total <= 10 * 1024 * 1024,
            "Aggregate binary patch expansion exceeds 10 MB"
        );
    }
    Ok(())
}

fn delta_size(bytes: &[u8], cursor: &mut usize) -> anyhow::Result<usize> {
    let mut size = 0usize;
    for shift in (0..usize::BITS).step_by(7) {
        let byte = *bytes
            .get(*cursor)
            .context("Truncated binary delta header")?;
        *cursor += 1;
        let part = usize::from(byte & 0x7f);
        ensure!(part <= usize::MAX >> shift, "Binary delta length overflow");
        size |= part << shift;
        if byte & 0x80 == 0 {
            return Ok(size);
        }
    }
    anyhow::bail!("Binary delta length overflow")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path},
    };

    const REPO: &str = "11111111-1111-1111-1111-111111111111";
    const PROJECT: &str = "22222222-2222-2222-2222-222222222222";

    fn command(repo: &Path, args: &[&str]) -> String {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(repo)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().to_string()
    }
    fn repository() -> (tempfile::TempDir, CommitSha) {
        let dir = tempfile::tempdir().unwrap();
        command(dir.path(), &["init", "--quiet", "--initial-branch=main"]);
        command(
            dir.path(),
            &["config", "user.email", "fixture@example.test"],
        );
        command(dir.path(), &["config", "user.name", "Fixture"]);
        command(dir.path(), &["config", "core.autocrlf", "false"]);
        std::fs::write(dir.path().join("src.txt"), "base\n").unwrap();
        command(dir.path(), &["add", "."]);
        command(dir.path(), &["commit", "--quiet", "-m", "base"]);
        command(dir.path(), &["checkout", "--quiet", "-b", "feature/test"]);
        std::fs::write(dir.path().join("pr-existing.txt"), "Existing PR change.\n").unwrap();
        command(dir.path(), &["add", "."]);
        command(dir.path(), &["commit", "--quiet", "-m", "existing PR"]);
        let head = CommitSha::parse(command(dir.path(), &["rev-parse", "HEAD"])).unwrap();
        (dir, head)
    }
    fn context(server: &MockServer, repo: &Path, output: &Path) -> ExecutionContext {
        let mut ctx = ExecutionContext {
            ado_org_url: Some(server.uri()),
            ado_organization: Some("org".into()),
            ado_project: Some("P".into()),
            repository_name: Some("repo".into()),
            source_directory: repo.into(),
            self_repository_directory: repo.into(),
            working_directory: output.into(),
            access_token: Some("token".into()),
            ..Default::default()
        };
        ctx.tool_configs.insert(
            PushToPullRequestBranchResult::NAME.into(),
            json!({"target":"*","allowed-branches":["feature/*"]}),
        );
        ctx
    }

    async fn service(server: &MockServer, head: &CommitSha, case: &str) -> Arc<Mutex<Vec<Value>>> {
        let current = Arc::new(Mutex::new(head.as_str().to_string()));
        Mock::given(method("GET")).and(path("/P/_apis/git/repositories/repo"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id":REPO,"name":"repo","defaultBranch":"refs/heads/main","project":{"id":PROJECT,"name":"P"}
            }))).mount(server).await;
        Mock::given(method("GET")).and(path("/P/_apis/git/repositories/repo/pullRequests/42"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "pullRequestId":42,"status":if case=="closed"{"completed"}else{"active"},
                "sourceRefName":"refs/heads/feature/test","targetRefName":"refs/heads/main","repository":{"id":REPO},
                "forkSource":if case=="fork"{json!({"repository":{"id":"other"}})}else{Value::Null}
            }))).mount(server).await;
        let refs = current.clone();
        let reads = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let race = case == "race";
        let stale = case == "stale";
        Mock::given(method("GET"))
            .and(path("/P/_apis/git/repositories/repo/refs"))
            .respond_with(move |_: &wiremock::Request| {
                let count = reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let sha = if stale || (race && count > 0) {
                    "c".repeat(40)
                } else {
                    refs.lock().unwrap().clone()
                };
                ResponseTemplate::new(200).set_body_json(
                    json!({"value":[{"name":"refs/heads/feature/test","objectId":sha}]}),
                )
            })
            .mount(server)
            .await;
        let updates = Arc::new(Mutex::new(Vec::new()));
        let saved = updates.clone();
        let old = head.as_str().to_string();
        let malformed = case == "malformed";
        let failed = case == "http-failed";
        Mock::given(method("POST")).and(path("/P/_apis/git/repositories/repo/pushes"))
            .respond_with(move |request:&wiremock::Request|{
                let value:Value=serde_json::from_slice(&request.body).unwrap();
                assert_eq!(value["refUpdates"][0]["oldObjectId"],old);
                assert_eq!(value["commits"][0]["parents"],json!([old]));
                saved.lock().unwrap().push(value);
                if failed{return ResponseTemplate::new(403);}
                let new="d".repeat(40);
                *current.lock().unwrap()=new.clone();
                if malformed {return ResponseTemplate::new(200).set_body_string("{malformed");}
                ResponseTemplate::new(200).set_body_json(json!({
                    "commits":[{"commitId":new,"parents":[old]}],
                    "refUpdates":[{"name":"refs/heads/feature/test","oldObjectId":old,"newObjectId":new}]
                }))
            }).mount(server).await;
        updates
    }

    #[tokio::test]
    async fn capture_includes_only_agent_delta_and_preserves_head_index_and_worktree() {
        let (repo, head) = repository();
        std::fs::write(repo.path().join("src.txt"), "agent commit\n").unwrap();
        command(repo.path(), &["add", "."]);
        command(repo.path(), &["commit", "--quiet", "-m", "agent"]);
        std::fs::write(repo.path().join("staged.txt"), "staged content\n").unwrap();
        command(repo.path(), &["add", "staged.txt"]);
        std::fs::write(repo.path().join("untracked.txt"), "untracked content\n").unwrap();
        std::fs::write(repo.path().join("image.bin"), [0, 255, 0, 128]).unwrap();
        std::fs::create_dir_all(repo.path().join("aw-context")).unwrap();
        std::fs::write(repo.path().join("aw-context").join("run.json"),"Compiler-generated context.\n").unwrap();
        let before = command(repo.path(), &["status", "--porcelain"]);
        let committed = command(repo.path(), &["rev-parse", "HEAD"]);
        let index = command(repo.path(), &["diff", "--cached", "--binary"]);
        let bytes = capture_patch(repo.path(), &head).await.unwrap();
        let patch = String::from_utf8(bytes).unwrap();
        assert!(
            patch.contains("agent commit")
                && patch.contains("staged content")
                && patch.contains("untracked content")
        );
        assert!(!patch.contains("pr-existing.txt"));
        assert!(!patch.contains("aw-context"));
        assert!(patch.contains("GIT binary patch"));
        validate_binary_bounds(&patch).unwrap();
        assert_eq!(command(repo.path(), &["status", "--porcelain"]), before);
        assert_eq!(command(repo.path(), &["rev-parse", "HEAD"]), committed);
        assert_eq!(
            command(repo.path(), &["diff", "--cached", "--binary"]),
            index
        );
    }

    #[tokio::test]
    async fn exact_head_pushes_preserve_base_and_fail_closed_on_races_or_uncertain_responses() {
        for case in [
            "success",
            "stale",
            "race",
            "fork",
            "closed",
            "malformed",
            "http-failed",
        ] {
            let (repo, head) = repository();
            std::fs::write(repo.path().join("src.txt"), "agent update\n").unwrap();
            std::fs::write(repo.path().join("new.bin"), [0, 255, 0, 128]).unwrap();
            let patch = capture_patch(repo.path(), &head).await.unwrap();
            let output = tempfile::tempdir().unwrap();
            std::fs::write(output.path().join("patch.diff"), &patch).unwrap();
            let server = MockServer::start().await;
            let updates = service(&server, &head, case).await;
            let ctx = context(&server, repo.path(), output.path());
            let outcome=crate::execute::execute_safe_output(&json!({
                "name":"push-to-pull-request-branch","pull_request_id":42,"repository":"self",
                "expected_head_sha":head,"patch_file":"patch.diff","patch_sha256":crate::hash::sha256_hex(&patch)
            }),&ctx).await;
            let recorded = updates.lock().unwrap();
            if matches!(case, "success" | "malformed" | "http-failed") {
                assert_eq!(recorded.len(), 1, "{case}: {outcome:?}");
                let result = outcome.unwrap().1;
                assert_eq!(
                    result.success,
                    case == "success",
                    "{case}: {}",
                    result.message
                );
                let changes = recorded[0]["commits"][0]["changes"].as_array().unwrap();
                assert_eq!(changes.len(), 2);
                assert!(
                    !changes
                        .iter()
                        .any(|change| change["item"]["path"] == "/pr-existing.txt")
                );
                if case == "success" {
                    assert_eq!(result.data.unwrap()["push_status"], "confirmed");
                }
            } else {
                assert!(outcome.is_err(), "{case}");
                assert!(recorded.is_empty(), "{case}");
            }
            assert_eq!(
                command(repo.path(), &["worktree", "list", "--porcelain"])
                    .matches("worktree ")
                    .count(),
                1
            );
        }
    }

    #[tokio::test]
    async fn trusted_preparation_and_empty_capture_do_not_write_remote_refs() {
        let (repo, head) = repository();
        let server = MockServer::start().await;
        let updates = service(&server, &head, "success").await;
        let output = tempfile::tempdir().unwrap();
        let mut ctx = context(&server, repo.path(), output.path());
        ctx.tool_configs
            .get_mut(PushToPullRequestBranchResult::NAME)
            .unwrap()["target"] = json!(42);
        command(repo.path(), &["checkout", "--quiet", "main"]);
        let snapshot = output.path().join("snapshot.json");
        prepare_agent(&ctx, &snapshot).await.unwrap();
        assert_eq!(command(repo.path(), &["rev-parse", "HEAD"]), head.as_str());
        let value: Value = serde_json::from_slice(&std::fs::read(snapshot).unwrap()).unwrap();
        assert_eq!(value["expected_head_sha"], head.as_str());
        assert_eq!(value["repository"], "self");
        assert!(capture_patch(repo.path(), &head).await.unwrap().is_empty());
        assert!(updates.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn guarded_push_rejects_protected_or_tampered_inputs_and_handles_empty_patch() {
        for case in ["protected", "bad-hash", "empty"] {
            let (repo, head) = repository();
            if case != "empty" {
                std::fs::write(
                    repo.path().join(if case == "protected" {
                        "package.json"
                    } else {
                        "new.txt"
                    }),
                    "Changed content.\n",
                )
                .unwrap();
            }
            let patch = capture_patch(repo.path(), &head).await.unwrap();
            let output = tempfile::tempdir().unwrap();
            std::fs::write(output.path().join("patch.diff"), &patch).unwrap();
            let server = MockServer::start().await;
            let updates = service(&server, &head, "success").await;
            let ctx = context(&server, repo.path(), output.path());
            let outcome=crate::execute::execute_safe_output(&json!({
                "name":"push-to-pull-request-branch","pull_request_id":42,"repository":"self",
                "expected_head_sha":head,"patch_file":"patch.diff",
                "patch_sha256":if case=="bad-hash"{"0".repeat(64)}else{crate::hash::sha256_hex(&patch)}
            }),&ctx).await;
            if case == "empty" {
                assert!(outcome.unwrap().1.is_warning());
            } else {
                let error = outcome.unwrap_err().to_string();
                assert!(
                    error.contains(if case == "protected" {
                        "protected"
                    } else {
                        "SHA-256"
                    }),
                    "{error}"
                );
            }
            assert!(updates.lock().unwrap().is_empty());
        }
    }

    #[tokio::test]
    async fn failed_push_blocks_same_pr_followups_but_not_diagnostics_or_other_prs() {
        let directory = tempfile::tempdir().unwrap();
        let mut ctx = ExecutionContext {
            ado_org_url: Some("https://dev.azure.com/org".into()),
            ado_organization: Some("org".into()),
            ado_project: Some("P".into()),
            repository_name: Some("repo".into()),
            dry_run: true,
            ..Default::default()
        };
        for tool in [
            "push-to-pull-request-branch",
            "submit-pull-request-review",
            "mark-pull-request-as-ready-for-review",
        ] {
            ctx.tool_configs.insert(tool.into(), json!({"target":"*"}));
        }
        let entries = [
            json!({"name":"push-to-pull-request-branch","pull_request_id":42,"repository":"self","unexpected":true}),
            json!({"name":"submit-pull-request-review","pull_request_id":42,"event":"approve"}),
            json!({"name":"mark-pull-request-as-ready-for-review","pull_request_id":42}),
            json!({"name":"submit-pull-request-review","pull_request_id":43,"event":"approve"}),
            json!({"name":"noop","context":"Diagnostic output still runs."}),
        ];
        std::fs::write(
            directory.path().join(crate::ndjson::SAFE_OUTPUT_FILENAME),
            entries
                .iter()
                .map(|entry| format!("{entry}\n"))
                .collect::<String>(),
        )
        .unwrap();
        let results = crate::execute::execute_safe_outputs(
            directory.path(),
            &ctx,
            &crate::execute::ToolFilter::default(),
        )
        .await
        .unwrap();
        assert_eq!(results.len(), 5);
        assert!(!results[0].success);
        assert!(results[1].message.contains("earlier code push"));
        assert!(results[2].message.contains("earlier code push"));
        assert!(results[3].success);
        assert!(results[4].success);
    }

    #[test]
    fn binary_size_guards_reject_declared_and_encoded_expansion_limits() {
        assert!(validate_binary_bounds("GIT binary patch\nliteral 999999999\n").is_err());
        assert!(validate_binary_bounds("GIT binary patch\ndelta 2\nAbad\n").is_err());
        assert!(delta_size(&[0xff; 20], &mut 0).is_err());
        assert!(delta_size(&[0x80], &mut 0).is_err());
        assert_eq!(delta_size(&[0x81, 0x01], &mut 0).unwrap(), 129);
    }
}
