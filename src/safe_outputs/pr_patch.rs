//! Shared limits, Git-object reads and patch policy for PR code outputs.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io::{self, Write};
use std::path::Path;
use std::process::{Output, Stdio};
use std::str::FromStr;

use anyhow::{Context, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

use super::create_pull_request::{ProtectedFiles, find_protected_files, glob_match_simple};
use crate::secure::{CommitSha, RelativeSafePath};

mod parse;
use parse::{Document, Kind};
#[cfg(test)]
pub(crate) mod tests;

pub(crate) const MAX_SOURCE_BYTES: usize = 10 * 1024 * 1024;
pub(crate) const MAX_REQUEST_BYTES: usize = 10 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "usize", into = "usize")]
pub struct PatchSizeKiB(usize);

impl Default for PatchSizeKiB {
    fn default() -> Self {
        Self(4096)
    }
}

impl TryFrom<usize> for PatchSizeKiB {
    type Error = anyhow::Error;
    fn try_from(value: usize) -> anyhow::Result<Self> {
        ensure!(
            (1..=10240).contains(&value),
            "max-patch-size must be an integer between 1 and 10240 KiB"
        );
        ensure!(
            value.checked_mul(1024).is_some(),
            "max-patch-size does not fit this platform"
        );
        Ok(Self(value))
    }
}

impl From<PatchSizeKiB> for usize {
    fn from(value: PatchSizeKiB) -> Self {
        value.0
    }
}

impl fmt::Display for PatchSizeKiB {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for PatchSizeKiB {
    type Err = String;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let value = value
            .parse::<usize>()
            .map_err(|_| "max-patch-size must be an integer in KiB".to_string())?;
        Self::try_from(value).map_err(|error| error.to_string())
    }
}

impl PatchSizeKiB {
    pub(crate) fn bytes(self) -> usize {
        self.0 * 1024
    }
}

async fn read_limited(reader: impl AsyncRead + Unpin, maximum: usize) -> anyhow::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader
        .take(u64::try_from(maximum)? + 1)
        .read_to_end(&mut bytes)
        .await?;
    ensure!(
        bytes.len() <= maximum,
        "Git output exceeds the {maximum}-byte bound"
    );
    Ok(bytes)
}

pub(crate) async fn bounded_output(
    command: &mut Command,
    maximum: usize,
    input: Option<&[u8]>,
) -> anyhow::Result<Output> {
    command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if input.is_some() {
        command.stdin(Stdio::piped());
    } else {
        command.stdin(Stdio::null());
    }
    let mut child = command.spawn().context("Failed to launch Git")?;
    let stdout = child.stdout.take().context("Git stdout unavailable")?;
    let stderr = child.stderr.take().context("Git stderr unavailable")?;
    let stdin = child.stdin.take();
    let write_input = async {
        if let (Some(mut stdin), Some(input)) = (stdin, input) {
            stdin.write_all(input).await?;
            stdin.shutdown().await?;
        }
        anyhow::Ok(())
    };
    let result = tokio::try_join!(
        read_limited(stdout, maximum),
        read_limited(stderr, 64 * 1024),
        write_input,
        async { child.wait().await.context("Failed to wait for Git") },
    );
    match result {
        Ok((stdout, stderr, _, status)) => Ok(Output {
            status,
            stdout,
            stderr,
        }),
        Err(error) => {
            if child.try_wait()?.is_none() {
                child
                    .kill()
                    .await
                    .context("Failed to stop bounded Git command")?;
            }
            child
                .wait()
                .await
                .context("Failed to reap bounded Git command")?;
            Err(error)
        }
    }
}

pub(crate) fn git_path(path: &Path) -> std::borrow::Cow<'_, str> {
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

pub(crate) fn git_command(repo: &Path) -> Command {
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
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "submodule.recurse=false",
            "-c",
            "apply.ignoreWhitespace=false",
        ])
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_NO_LAZY_FETCH", "1")
        .current_dir(repo);
    command
}

pub(crate) async fn git_without_filters(repo: &Path) -> anyhow::Result<Command> {
    let output = bounded_output(
        git_command(repo).args([
            "config",
            "--name-only",
            "--get-regexp",
            r"^filter\..*\.(clean|smudge|process|required)$",
        ]),
        64 * 1024,
        None,
    )
    .await?;
    ensure!(
        output.status.success() || output.status.code() == Some(1),
        "Could not inspect git filter configuration"
    );
    let mut command = git_command(repo);
    for key in std::str::from_utf8(&output.stdout)?.lines() {
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

pub(crate) async fn git(repo: &Path, args: &[&str]) -> anyhow::Result<Output> {
    bounded_output(
        git_without_filters(repo).await?.args(args),
        MAX_SOURCE_BYTES,
        None,
    )
    .await
}

pub(crate) async fn read_patch(path: &Path, limit: PatchSizeKiB) -> anyhow::Result<Vec<u8>> {
    let file = tokio::fs::File::open(path)
        .await
        .context("Failed to open PR patch")?;
    let mut bytes = Vec::new();
    file.take(u64::try_from(limit.bytes())? + 1)
        .read_to_end(&mut bytes)
        .await?;
    ensure!(
        bytes.len() <= limit.bytes(),
        "PR patch exceeds max-patch-size ({limit} KiB)"
    );
    Ok(bytes)
}

pub(crate) fn inspected_paths(bytes: &[u8]) -> anyhow::Result<BTreeSet<String>> {
    Ok(Document::parse(bytes)?.paths())
}

pub(crate) fn finish_scratch<T>(
    scratch: tempfile::TempDir,
    result: anyhow::Result<T>,
) -> anyhow::Result<T> {
    match (result, scratch.close()) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(error)) => Err(error).context("Failed to clean private patch files"),
        (Err(error), Err(cleanup)) => {
            Err(error.context(format!("Private patch cleanup also failed: {cleanup}")))
        }
    }
}

struct BoundedJson(Vec<u8>);
impl Write for BoundedJson {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > MAX_REQUEST_BYTES.saturating_sub(self.0.len()) {
            return Err(io::Error::other("Encoded ADO push payload exceeds 10 MiB"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub(crate) fn request_bytes(value: &impl Serialize) -> anyhow::Result<Vec<u8>> {
    let mut output = BoundedJson(Vec::new());
    serde_json::to_writer(&mut output, value)
        .context("Failed to serialize bounded ADO push request")?;
    Ok(output.0)
}

#[derive(Clone, Debug)]
struct Blob {
    oid: String,
    mode: String,
    size: usize,
}

async fn source_blobs(
    repo: &Path,
    base: &CommitSha,
    paths: &BTreeSet<String>,
) -> anyhow::Result<BTreeMap<String, Blob>> {
    let paths = paths.iter().collect::<Vec<_>>();
    let mut blobs = BTreeMap::new();
    for group in paths.chunks(64) {
        let mut args = vec![
            "--literal-pathspecs",
            "ls-tree",
            "-l",
            "-z",
            base.as_str(),
            "--",
        ];
        args.extend(group.iter().map(|path| path.as_str()));
        let output = git(repo, &args).await?;
        ensure!(
            output.status.success(),
            "Could not inspect source blob metadata"
        );
        for record in output
            .stdout
            .split(|byte| *byte == 0)
            .filter(|record| !record.is_empty())
        {
            let text = std::str::from_utf8(record)?;
            let (metadata, path) = text
                .split_once('\t')
                .context("Malformed source tree record")?;
            let fields = metadata.split_whitespace().collect::<Vec<_>>();
            ensure!(
                fields.len() == 4 && fields[1] == "blob",
                "Changed source path is not a blob"
            );
            validate_oid(fields[2])?;
            let size: usize = fields[3].parse().context("Invalid source blob size")?;
            ensure!(
                size <= MAX_SOURCE_BYTES,
                "Source blob exceeds the 10 MiB source-processing bound"
            );
            ensure!(
                blobs
                    .insert(
                        path.into(),
                        Blob {
                            oid: fields[2].into(),
                            mode: fields[0].into(),
                            size
                        }
                    )
                    .is_none(),
                "Ambiguous source blob metadata"
            );
        }
    }
    Ok(blobs)
}

fn validate_oid(oid: &str) -> anyhow::Result<()> {
    ensure!(
        oid.len() == 40 && oid.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "Git returned an invalid object ID"
    );
    Ok(())
}

async fn referenced_blobs(
    repo: &Path,
    references: &BTreeSet<String>,
) -> anyhow::Result<BTreeMap<String, Blob>> {
    if references.is_empty() {
        return Ok(BTreeMap::new());
    }
    let input = references
        .iter()
        .map(|oid| format!("{oid}\n"))
        .collect::<String>();
    let output = bounded_output(
        git_command(repo).args(["cat-file", "--batch-check"]),
        MAX_SOURCE_BYTES,
        Some(input.as_bytes()),
    )
    .await?;
    ensure!(
        output.status.success(),
        "Could not inspect referenced patch preimages"
    );
    let lines = std::str::from_utf8(&output.stdout)?
        .lines()
        .collect::<Vec<_>>();
    ensure!(
        lines.len() == references.len(),
        "Incomplete referenced-object metadata"
    );
    let mut blobs = BTreeMap::new();
    for (reference, line) in references.iter().zip(lines) {
        if line == format!("{reference} missing") {
            continue;
        }
        let fields = line.split_whitespace().collect::<Vec<_>>();
        ensure!(
            fields.len() == 3 && fields[1] == "blob",
            "Ambiguous or non-blob patch object reference"
        );
        validate_oid(fields[0])?;
        let size: usize = fields[2].parse()?;
        ensure!(
            size <= MAX_SOURCE_BYTES,
            "Referenced patch blob exceeds 10 MiB"
        );
        blobs.insert(
            reference.clone(),
            Blob {
                oid: fields[0].into(),
                size,
                mode: String::new(),
            },
        );
    }
    Ok(blobs)
}

pub(crate) async fn ensure_commit(
    repo: &Path,
    head: &CommitSha,
    target: &super::result::AdoRepositoryTarget,
    client: &reqwest::Client,
    token: &str,
    connection_type: Option<crate::compile::types::WriteConnectionType>,
) -> anyhow::Result<()> {
    if git(repo, &["cat-file", "-e", &format!("{head}^{{commit}}")])
        .await?
        .status
        .success()
    {
        return Ok(());
    }
    #[derive(Deserialize)]
    struct ProjectIdentity {
        id: String,
        name: String,
    }
    #[derive(Deserialize)]
    struct RepositoryIdentity {
        id: String,
        name: String,
        project: ProjectIdentity,
    }
    use super::pr_http::BoundedPrResponse;
    let response = super::authenticate_ado_request(
        client.get(format!(
            "{}?api-version=7.1",
            super::pr_common::repository_api_base(target)
        )),
        token,
        connection_type,
    )
    .send()
    .await
    .context("Failed to read repository identity for source fetch")?;
    ensure!(
        response.status().is_success(),
        "Failed to read repository identity for source fetch (HTTP {})",
        response.status()
    );
    let metadata: RepositoryIdentity = response.bounded_json().await?;
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
                    target.organization.to_ascii_lowercase()
                ),
        "Source origin is not the authorized Azure DevOps organization"
    );
    ensure!(
        url.username().is_empty() || url.username().eq_ignore_ascii_case(&target.organization),
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
        super::pr_common::collection_identity(&parsed.org_url)
            == super::pr_common::collection_identity(&target.organization_url),
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
    let header = match connection_type {
        Some(_) => format!("Authorization: bearer {}", token),
        None => format!(
            "Authorization: Basic {}",
            base64::engine::general_purpose::STANDARD.encode(format!(":{}", token))
        ),
    };
    command
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
        .current_dir(repo);
    let output = bounded_output(&mut command, MAX_SOURCE_BYTES, None)
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

pub(crate) struct PatchPolicy<'a> {
    pub limit: PatchSizeKiB,
    pub max_files: usize,
    pub excluded_files: &'a [String],
    pub protected_files: ProtectedFiles,
    pub exact: bool,
}

/// Only successful preflight constructs this value; application uses its verified bytes.
pub(crate) struct PreparedPatch {
    pub bytes: Vec<u8>,
    pub batches: Vec<Vec<u8>>,
    pub omitted: Vec<OmittedOperation>,
    paths: BTreeSet<String>,
    limit: PatchSizeKiB,
    max_files: usize,
    exact: bool,
}

pub(crate) async fn prepare(
    repo: &Path,
    base: &CommitSha,
    bytes: &[u8],
    policy: &PatchPolicy<'_>,
) -> anyhow::Result<PreparedPatch> {
    ensure!(
        bytes.len() <= policy.limit.bytes(),
        "PR patch exceeds max-patch-size ({} KiB)",
        policy.limit
    );
    let document = Document::parse(bytes)?;
    ensure!(
        document.paths().len() <= policy.max_files,
        "PR patch exceeds max-files"
    );
    let mut selected = Vec::new();
    let mut omitted = Vec::new();
    let mut paths = BTreeSet::new();
    let mut unavailable = BTreeSet::new();
    let mut selected_indices = BTreeSet::new();
    for (index, operation) in document.operations.iter().enumerate() {
        let endpoints = operation.paths();
        let excluded = endpoints.iter().any(|path| {
            policy
                .excluded_files
                .iter()
                .any(|pattern| glob_match_simple(pattern, path))
        });
        if excluded {
            omitted.push(OmittedOperation {
                operation: match operation.kind {
                    Kind::Add => "add",
                    Kind::Delete => "delete",
                    Kind::Modify => "modify",
                    Kind::Copy => "copy",
                    Kind::Rename => "rename",
                },
                source: operation.source.clone(),
                destination: operation.destination.clone(),
            });
            if let Some(destination) = &operation.destination {
                unavailable.insert(destination.clone());
            }
            continue;
        }
        ensure!(
            !operation
                .source
                .as_ref()
                .is_some_and(|path| unavailable.contains(path)),
            "Retained patch operation depends on an excluded operation"
        );
        if policy.exact {
            ensure!(
                endpoints
                    .iter()
                    .all(|path| path != "aw-context" && !path.starts_with("aw-context/")),
                "Compiler-owned aw-context data is not pushable"
            );
            ensure!(
                operation
                    .old_mode
                    .as_deref()
                    .is_none_or(|mode| matches!(mode, "100644" | "100755"))
                    && operation
                        .new_mode
                        .as_deref()
                        .is_none_or(|mode| matches!(mode, "100644" | "100755")),
                "PR pushes do not support symlink or submodule changes"
            );
        }
        ensure!(
            operation.new_mode.as_deref() != Some("120000")
                || (!policy.exact
                    && operation.old_mode.as_deref() == Some("120000")
                    && operation.kind == Kind::Modify),
            "Patch introduces a symlink"
        );
        if policy.protected_files != ProtectedFiles::Allowed {
            ensure!(
                find_protected_files(&endpoints).is_empty(),
                "PR patch contains protected files"
            );
        }
        paths.extend(endpoints);
        selected.push(operation);
        selected_indices.insert(index);
    }
    let sources = selected
        .iter()
        .filter_map(|op| op.source.clone())
        .collect::<BTreeSet<_>>();
    let initial = source_blobs(repo, base, &sources).await?;
    let references = selected
        .iter()
        .flat_map(|operation| operation.old_oid.iter().chain(&operation.new_oid))
        .filter(|oid| !oid.bytes().all(|byte| byte == b'0'))
        .cloned()
        .collect::<BTreeSet<_>>();
    let referenced = referenced_blobs(repo, &references).await?;
    let mut source_bytes = 0usize;
    for blob in initial.values() {
        source_bytes = source_bytes
            .checked_add(blob.size)
            .context("Source size overflow")?;
        ensure!(
            source_bytes <= MAX_SOURCE_BYTES,
            "Source blobs exceed the 10 MiB source-processing bound"
        );
    }
    let initial_oids = initial
        .values()
        .map(|blob| blob.oid.as_str())
        .collect::<BTreeSet<_>>();
    let mut charged = BTreeSet::new();
    for blob in referenced.values() {
        if !initial_oids.contains(blob.oid.as_str()) && charged.insert(&blob.oid) {
            source_bytes = source_bytes
                .checked_add(blob.size)
                .context("Referenced source size overflow")?;
            ensure!(
                source_bytes <= MAX_SOURCE_BYTES,
                "Referenced blobs exceed the 10 MiB source-processing bound"
            );
        }
    }
    let mut state = initial
        .iter()
        .map(|(path, blob)| {
            (
                path.clone(),
                (blob.size, Some(blob.oid.clone()), blob.mode.clone()),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let mut result_sizes = BTreeMap::<String, usize>::new();
    for batch in selected.chunk_by(|left, right| left.group == right.group) {
        // Git diff endpoints refer to the tree before the entire commit, including swaps.
        let mut updates = BTreeMap::new();
        for operation in batch {
            let preimage = operation
                .source
                .as_ref()
                .map(|source| {
                    state
                        .get(source)
                        .cloned()
                        .with_context(|| format!("Patch preimage is unavailable: {source}"))
                })
                .transpose()?;
            let (source_size, source_oid, source_mode) =
                preimage.unwrap_or((0, None, "100644".into()));
            if operation.source.is_some() && source_oid.is_none() {
                source_bytes = source_bytes
                    .checked_add(source_size)
                    .context("Intermediate source size overflow")?;
                ensure!(
                    source_bytes <= MAX_SOURCE_BYTES,
                    "Intermediate blobs exceed the 10 MiB source-processing bound"
                );
            }
            if matches!(operation.kind, Kind::Copy | Kind::Rename) {
                ensure!(
                    matches!(source_mode.as_str(), "100644" | "100755"),
                    "Copy/rename source must be a regular file"
                );
            }
            let exact_preimage = policy.exact
                || operation.old_oid.as_ref().is_some_and(|oid| {
                    source_oid
                        .as_ref()
                        .is_some_and(|actual| actual.starts_with(oid))
                });
            let mut output_size =
                operation.output_bound(source_size, exact_preimage, policy.limit.bytes())?;
            if operation.kind != Kind::Delete
                && let Some(blob) = operation
                    .new_oid
                    .as_ref()
                    .and_then(|oid| referenced.get(oid))
            {
                output_size = output_size.max(blob.size);
            }
            if let Some(destination) = &operation.destination {
                updates.insert(
                    destination.clone(),
                    (
                        output_size,
                        None,
                        operation.new_mode.clone().unwrap_or(source_mode),
                    ),
                );
            }
        }
        for operation in batch {
            if matches!(operation.kind, Kind::Delete | Kind::Rename)
                && let Some(source) = &operation.source
            {
                state.remove(source);
                result_sizes.remove(source);
            }
        }
        for (path, value) in updates {
            result_sizes.insert(path.clone(), value.0);
            state.insert(path, value);
        }
        let expanded = result_sizes
            .values()
            .try_fold(0usize, |sum, size| sum.checked_add(*size))
            .context("Expanded patch size overflow")?;
        ensure!(
            expanded <= policy.limit.bytes(),
            "PR pre-application expansion exceeds max-patch-size ({} KiB)",
            policy.limit
        );
    }
    Ok(PreparedPatch {
        bytes: document.filtered_bytes(&selected_indices)?,
        batches: document.batches(&selected_indices),
        omitted,
        paths,
        limit: policy.limit,
        max_files: policy.max_files,
        exact: policy.exact,
    })
}

impl PreparedPatch {
    pub(crate) fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }

    pub(crate) async fn apply_to_index(
        &self,
        repo: &Path,
        base: &CommitSha,
    ) -> anyhow::Result<IndexChanges> {
        let scratch = tempfile::tempdir()?;
        let index = scratch.path().join("index");
        let result = async {
            let read = bounded_output(
                git_without_filters(repo)
                    .await?
                    .args(["read-tree", base.as_str()])
                    .env("GIT_INDEX_FILE", &index),
                MAX_SOURCE_BYTES,
                None,
            )
            .await?;
            ensure!(read.status.success(), "Could not seed isolated PR index");
            if self.exact {
                for path in &self.paths {
                    ensure_unfiltered(repo, Some(&index), path).await?;
                }
            }
            for batch in &self.batches {
                for check in [true, false] {
                    let mut command = git_without_filters(repo).await?;
                    command.args(["apply", "--cached", "--binary", "--whitespace=nowarn"]);
                    if check {
                        command.arg("--check");
                    }
                    command.env("GIT_INDEX_FILE", &index);
                    let output =
                        bounded_output(&mut command, MAX_SOURCE_BYTES, Some(batch)).await?;
                    ensure!(
                        output.status.success(),
                        "Patch does not apply cleanly to the exact source head"
                    );
                }
            }
            self.collect(repo, base, Some(&index), None).await
        }
        .await;
        finish_scratch(scratch, result)
    }

    pub(crate) async fn collect(
        &self,
        repo: &Path,
        base: &CommitSha,
        index: Option<&Path>,
        tip: Option<&str>,
    ) -> anyhow::Result<IndexChanges> {
        let mut command = git_without_filters(repo).await?;
        command.args([
            "diff",
            "--raw",
            "-z",
            "--no-abbrev",
            "--no-ext-diff",
            "--no-textconv",
            "--no-renames",
        ]);
        if tip.is_none() {
            command.arg("--cached");
        }
        command.arg(base.as_str());
        if let Some(tip) = tip {
            command.arg(tip);
        }
        if let Some(index) = index {
            command.env("GIT_INDEX_FILE", index);
        }
        let output = bounded_output(&mut command, MAX_SOURCE_BYTES, None).await?;
        ensure!(
            output.status.success(),
            "Could not inspect resulting PR blobs"
        );
        let records = output
            .stdout
            .split(|byte| *byte == 0)
            .filter(|record| !record.is_empty())
            .collect::<Vec<_>>();
        let mut changes = Vec::new();
        let mut changed_paths = BTreeSet::new();
        let mut skipped_symlinks = Vec::new();
        let mut size = 0usize;
        let mut encoded_size = 2usize;
        let mut cursor = 0;
        while cursor < records.len() {
            let fields = std::str::from_utf8(records[cursor])?
                .split_whitespace()
                .collect::<Vec<_>>();
            ensure!(fields.len() == 5, "Malformed resulting Git metadata");
            ensure!(
                matches!(fields[4], "A" | "D" | "M" | "T"),
                "Unmerged or unsupported resulting Git change"
            );
            let old_mode = fields[0]
                .strip_prefix(':')
                .context("Malformed old Git mode")?;
            let mode = fields[1];
            let path = std::str::from_utf8(
                records
                    .get(cursor + 1)
                    .context("Missing resulting Git path")?,
            )?;
            cursor += 2;
            RelativeSafePath::parse(path)?;
            changed_paths.insert(path.to_string());
            ensure!(
                self.paths.contains(path),
                "Resulting Git path was not authorized by patch selection: {path}"
            );
            if self.exact {
                ensure!(
                    matches!(old_mode, "000000" | "100644" | "100755"),
                    "PR pushes do not support symlink or submodule changes"
                );
            }
            if mode == "000000" {
                append_change(
                    &mut changes,
                    &mut encoded_size,
                    json!({"changeType":"delete","item":{"path":format!("/{path}")}}),
                )?;
                continue;
            }
            if mode == "120000" && !self.exact {
                skipped_symlinks.push(path.into());
                continue;
            }
            ensure!(
                matches!(mode, "100644" | "100755"),
                "PR output is not a regular file"
            );
            if self.exact {
                ensure!(
                    old_mode == mode || (old_mode == "000000" && mode == "100644"),
                    "PR push cannot represent file-mode changes"
                );
                ensure_unfiltered(repo, index, path).await?;
            }
            validate_oid(fields[3])?;
            let metadata = git(repo, &["cat-file", "-s", fields[3]]).await?;
            ensure!(
                metadata.status.success(),
                "Could not inspect resulting blob size"
            );
            let length: usize = std::str::from_utf8(&metadata.stdout)?.trim().parse()?;
            size = size
                .checked_add(length)
                .context("Resulting blob size overflow")?;
            ensure!(
                size <= self.limit.bytes(),
                "Expanded PR content exceeds max-patch-size ({} KiB)",
                self.limit
            );
            let blob = bounded_output(
                git_without_filters(repo)
                    .await?
                    .args(["cat-file", "blob", fields[3]]),
                length,
                None,
            )
            .await?;
            ensure!(
                blob.status.success() && blob.stdout.len() == length,
                "Resulting blob read was incomplete"
            );
            let content = match String::from_utf8(blob.stdout) {
                Ok(text) => json!({"content":text,"contentType":"rawtext"}),
                Err(error) => {
                    use base64::Engine;
                    let bytes = error.into_bytes();
                    ensure!(
                        bytes
                            .len()
                            .div_ceil(3)
                            .checked_mul(4)
                            .is_some_and(|size| size <= MAX_REQUEST_BYTES),
                        "Base64 PR content exceeds the 10 MiB encoded payload bound"
                    );
                    json!({"content":base64::engine::general_purpose::STANDARD.encode(bytes),"contentType":"base64encoded"})
                }
            };
            append_change(
                &mut changes,
                &mut encoded_size,
                json!({
                    "changeType":if old_mode == "000000" { "add" } else { "edit" },
                    "item":{"path":format!("/{path}")},"newContent":content,
                }),
            )?;
        }
        ensure!(
            changed_paths.len() <= self.max_files,
            "Resulting PR changes exceed max-files"
        );
        Ok(IndexChanges {
            changes,
            skipped_symlinks,
            omitted: self.omitted.clone(),
        })
    }
}

async fn ensure_unfiltered(repo: &Path, index: Option<&Path>, path: &str) -> anyhow::Result<()> {
    let mut attrs = git_without_filters(repo).await?;
    attrs.args([
        "--literal-pathspecs",
        "check-attr",
        "--cached",
        "-z",
        "filter",
        "--",
        path,
    ]);
    if let Some(index) = index {
        attrs.env("GIT_INDEX_FILE", index);
    }
    let checked = bounded_output(&mut attrs, MAX_SOURCE_BYTES, None).await?;
    ensure!(
        checked.status.success(),
        "Could not inspect PR file filters"
    );
    let values = checked
        .stdout
        .split(|byte| *byte == 0)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    ensure!(
        values.len() == 3 && matches!(values[2], b"unspecified" | b"unset"),
        "PR pushes do not support LFS or custom-filtered file changes"
    );
    Ok(())
}

fn append_change(changes: &mut Vec<Value>, size: &mut usize, change: Value) -> anyhow::Result<()> {
    let bytes = request_bytes(&change)?;
    *size = size
        .checked_add(bytes.len())
        .and_then(|size| size.checked_add(usize::from(!changes.is_empty())))
        .context("Encoded change size overflow")?;
    ensure!(
        *size <= MAX_REQUEST_BYTES,
        "Encoded ADO push payload exceeds 10 MiB"
    );
    changes.push(change);
    Ok(())
}

#[derive(Debug)]
pub(crate) struct IndexChanges {
    pub changes: Vec<Value>,
    pub skipped_symlinks: Vec<String>,
    pub omitted: Vec<OmittedOperation>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct OmittedOperation {
    operation: &'static str,
    source: Option<String>,
    destination: Option<String>,
}
