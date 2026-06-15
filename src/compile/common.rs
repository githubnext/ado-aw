//! Common helper functions shared across all compile targets.

use anyhow::{Context, Result};
use std::collections::{HashMap, HashSet};
use std::path::Path;

use super::extensions::{
    CompilerExtension, Declarations, McpgConfig, McpgGatewayConfig, McpgServerConfig,
};
use super::types::{
    CompileTarget, FrontMatter, PipelineParameter, PoolConfig, ReposItem, Repository,
};
use crate::allowed_hosts::{CORE_ALLOWED_HOSTS, mcp_required_hosts};
use crate::compile::types::McpConfig;
use crate::ecosystem_domains::{
    get_ecosystem_domains, is_ecosystem_identifier, is_known_ecosystem,
};
use crate::validate;

/// Atomically write `contents` to `path`.
///
/// Uses [`tempfile::NamedTempFile`] in the destination's parent
/// directory so the final `persist` is a same-filesystem rename. This
/// guarantees readers either see the old file or the new file in full —
/// never a half-written state.
///
/// Behavior:
///
/// - Creates the tempfile in `path.parent()` (falls back to `.` when
///   the parent is empty, matching `tokio::fs::write` semantics).
/// - On Unix, preserves the existing file's mode if the target exists.
///   Otherwise the tempfile keeps its default mode (0o600 from
///   `tempfile`'s implementation).
/// - When the destination is a symlink, the rename replaces the
///   symlink with a regular file (matches `tokio::fs::write`; the
///   symlink target is *not* followed).
pub async fn atomic_write(path: &Path, contents: &str) -> Result<()> {
    let path = path.to_path_buf();
    let owned_contents = contents.to_string();
    // tempfile is sync; do the whole thing on a blocking task so we
    // don't block the async runtime on large writes / fsync.
    tokio::task::spawn_blocking(move || atomic_write_blocking(&path, &owned_contents))
        .await
        .context("atomic_write task panicked")?
}

fn atomic_write_blocking(path: &Path, contents: &str) -> Result<()> {
    use std::io::Write;

    // Determine the directory to create the tempfile in. We MUST use
    // a path on the same filesystem as the destination so that the
    // final `persist` rename is atomic (otherwise it fails with
    // EXDEV on Linux when /tmp is a separate tmpfs mount).
    //
    // - `path.parent() == Some(non-empty)` -> use that parent.
    // - `path.parent() == Some("")` (bare filename like "agent.md")
    //   or `None` -> use the current directory ("."), which is the
    //   same filesystem as where the file will land.
    let parent = path.parent().filter(|p| !p.as_os_str().is_empty());
    let parent_dir: &Path = parent.unwrap_or_else(|| Path::new("."));
    let mut tmp = tempfile::NamedTempFile::new_in(parent_dir).with_context(|| {
        format!(
            "failed to create temporary file in {}",
            parent_dir.display()
        )
    })?;

    tmp.write_all(contents.as_bytes())
        .with_context(|| format!("failed to write temporary file for {}", path.display()))?;
    tmp.as_file()
        .sync_all()
        .with_context(|| format!("failed to fsync temporary file for {}", path.display()))?;

    // On Unix, copy the existing file's mode onto the tempfile so
    // permissions are preserved across the atomic rename.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(path) {
            let mode = meta.permissions().mode();
            std::fs::set_permissions(tmp.path(), std::fs::Permissions::from_mode(mode))
                .with_context(|| {
                    format!(
                        "failed to copy permissions from {} to temp file",
                        path.display()
                    )
                })?;
        }
    }

    tmp.persist(path)
        .with_context(|| format!("failed to atomically rename into {}", path.display()))?;
    Ok(())
}

/// Detailed parse result. Holds enough information to rewrite the
/// source on disk byte-faithfully when codemods apply.
///
/// See [`parse_markdown_detailed`].
#[derive(Debug)]
pub struct ParsedSource {
    /// Typed front matter, after codemods have been applied to the
    /// underlying mapping.
    pub front_matter: FrontMatter,
    /// Body for compilation, with leading/trailing whitespace trimmed
    /// (matches the legacy `parse_markdown` second tuple element).
    pub markdown_body: String,
    /// Codemod outcome.
    pub codemods: super::codemods::CodemodReport,
    /// The codemod-rewritten front-matter mapping. Used to
    /// reconstruct the source for the on-disk rewrite.
    pub front_matter_mapping: serde_yaml::Mapping,
    /// Whitespace bytes that appeared before the opening `---` fence,
    /// preserved verbatim so that source rewrite is byte-faithful.
    /// Empty in the typical case where the file starts with `---`.
    pub leading_whitespace: String,
    /// The body region exactly as it appeared after the closing `---`,
    /// byte-for-byte (no trim). Includes any leading newline.
    pub body_raw: String,
    /// SHA-256 of the original source bytes (lost-update protection).
    pub source_sha256: [u8; 32],
}

/// Parse the markdown file, run the codemod registry on the front
/// matter in memory, and return both the typed `FrontMatter` and the
/// raw fragments needed to rewrite the source on disk byte-faithfully.
///
/// Use this from callers that may rewrite the source (the `compile`
/// command). Callers that only want the typed view of the front matter
/// should use the backward-compatible [`parse_markdown`] wrapper.
pub fn parse_markdown_detailed(content: &str) -> Result<ParsedSource> {
    parse_markdown_detailed_with_registry(content, super::codemods::CODEMODS)
}

/// Variant of [`parse_markdown_detailed`] that allows injecting an
/// explicit codemod registry. Used by tests; production callers go
/// through the no-arg version that reads the global
/// [`super::codemods::CODEMODS`].
pub(crate) fn parse_markdown_detailed_with_registry(
    content: &str,
    registry: &[&'static super::codemods::Codemod],
) -> Result<ParsedSource> {
    use sha2::Digest;

    // Lost-update protection: hash the raw input as it was provided, so
    // a rewrite path can later re-read the file and compare.
    let mut hasher = sha2::Sha256::new();
    hasher.update(content.as_bytes());
    let source_sha256: [u8; 32] = hasher.finalize().into();

    // Allow leading whitespace before the opening fence (preserves
    // historical leniency). We compute a byte offset into `content` so
    // that `body_raw` extraction is purely byte-faithful, and we keep
    // the whitespace prefix around so that source rewrites preserve
    // anything the user (or their editor) put before the opening
    // fence.
    let leading_ws = content
        .bytes()
        .take_while(|b| b.is_ascii_whitespace())
        .count();
    let leading_whitespace = content[..leading_ws].to_string();
    let after_lead = &content[leading_ws..];
    if !after_lead.starts_with("---") {
        anyhow::bail!("Markdown file must start with YAML front matter (---)");
    }

    let after_open = &after_lead[3..];
    let end_idx = after_open
        .find("\n---")
        .context("Could not find closing --- for front matter")?;

    let yaml_str = &after_open[..end_idx];
    let body_raw_slice = &after_open[end_idx + 4..];
    let body_raw = body_raw_slice.to_string();
    let markdown_body = body_raw_slice.trim().to_string();

    // Stage 1: parse to untyped Value, reject non-mapping at top level.
    let parsed_value: serde_yaml::Value =
        serde_yaml::from_str(yaml_str).context("Failed to parse YAML front matter")?;
    let mut mapping = match parsed_value {
        serde_yaml::Value::Mapping(m) => m,
        other => {
            anyhow::bail!(
                "YAML front matter must be a mapping/object, got {}",
                yaml_value_kind(&other)
            );
        }
    };

    // Stage 2: run the codemod registry against the untyped mapping.
    let report = super::codemods::apply_codemods_with(&mut mapping, registry)
        .context("Failed to apply codemods")?;

    // Stage 3: deserialize the (possibly modified) mapping into the
    // typed FrontMatter. Errors here mean either the user wrote an
    // unsupported shape or a codemod produced invalid output. The
    // error context differs by case so the user can tell which.
    let front_matter: FrontMatter =
        serde_yaml::from_value(serde_yaml::Value::Mapping(mapping.clone())).with_context(|| {
            if report.changed() {
                let ids = report.applied_ids().join(", ");
                format!(
                    "Failed to parse YAML front matter after applying codemods ({}); \
                 a codemod likely produced an invalid shape",
                    ids
                )
            } else {
                "Failed to parse YAML front matter".to_string()
            }
        })?;

    Ok(ParsedSource {
        front_matter,
        markdown_body,
        codemods: report,
        front_matter_mapping: mapping,
        leading_whitespace,
        body_raw,
        source_sha256,
    })
}

/// Reconstruct full source content from codemod outputs.
///
/// Takes the individual fragments rather than the full
/// [`ParsedSource`] so callers that have already destructured the
/// parse don't have to round-trip a fresh `front_matter` through
/// serde just to satisfy the typed field.
///
/// Output shape:
/// - `leading_whitespace` (typically empty)
/// - `---\n`
/// - the codemod-rewritten YAML mapping (`serde_yaml::to_string`
///   always ends with `\n`); the mapping's existing key order is
///   preserved so user-authored keys keep their original positions
/// - `---`
/// - the original body region byte-for-byte (`body_raw`)
pub fn reconstruct_source(
    leading_whitespace: &str,
    front_matter_mapping: &serde_yaml::Mapping,
    body_raw: &str,
) -> Result<String> {
    let yaml_serialized = serde_yaml::to_string(front_matter_mapping)
        .context("Failed to serialize codemod-rewritten front matter")?;
    // Defensive: the format string assumes the serialized YAML ends
    // with `\n` so the closing `---` lands on a new line. This is
    // serde_yaml's documented behavior for non-empty mappings, but
    // hard-fail loudly if a future version breaks the assumption
    // rather than silently producing malformed YAML.
    anyhow::ensure!(
        yaml_serialized.ends_with('\n'),
        "serde_yaml::to_string produced output without trailing newline; \
         cannot reconstruct front-matter block safely"
    );
    Ok(format!(
        "{}---\n{}---{}",
        leading_whitespace, yaml_serialized, body_raw
    ))
}

fn yaml_value_kind(v: &serde_yaml::Value) -> &'static str {
    match v {
        serde_yaml::Value::Null => "null",
        serde_yaml::Value::Bool(_) => "bool",
        serde_yaml::Value::Number(_) => "number",
        serde_yaml::Value::String(_) => "string",
        serde_yaml::Value::Sequence(_) => "sequence",
        serde_yaml::Value::Mapping(_) => "mapping",
        serde_yaml::Value::Tagged(_) => "tagged",
    }
}

/// Backward-compatible parse: returns the typed front matter and the
/// trimmed body. New callers that may rewrite the source on disk
/// should use [`parse_markdown_detailed`] instead.
#[allow(dead_code)]
pub fn parse_markdown(content: &str) -> Result<(FrontMatter, String)> {
    let parsed = parse_markdown_detailed(content)?;
    Ok((parsed.front_matter, parsed.markdown_body))
}

/// Construct a guaranteed-unique heredoc sentinel for shell content.
///
/// Returns `<base>_<12-hex-chars-of-sha256(content)>`. The SHA suffix
/// makes the sentinel deterministic per content (so lock files stay
/// stable across recompiles) and astronomically unlikely to appear
/// inside the content (a random 48-bit prefix collision has ~2^-48
/// probability).
///
/// As defense in depth, validates that the content does not contain
/// the resulting sentinel as a standalone line and returns `Err` if
/// it does — converting a worst-case bash-injection silent failure
/// into a typed compile error. In practice the error branch is
/// unreachable without a deliberate SHA-256 prefix-collision attack
/// on the content body.
///
/// # Why
///
/// Bash heredocs (`cat > file <<'EOF' ... EOF`) are terminated by a
/// line whose entire content equals the sentinel. If `content`
/// contains such a line, everything after it executes as bash
/// instead of being captured into the file. With user-controlled
/// content (e.g. resolved agent markdown, front-matter description),
/// a fixed sentinel like `EOF` or `AGENT_PROMPT_EOF` is a latent
/// shell-injection vector: a malicious agent file can break out of
/// the heredoc and execute arbitrary commands in the Detection /
/// Agent jobs.
pub(crate) fn heredoc_sentinel(base: &str, content: &str) -> Result<String> {
    let hash = crate::hash::sha256_hex(content.as_bytes());
    let sentinel = format!("{base}_{}", &hash[..12]);
    if content.lines().any(|line| line == sentinel) {
        anyhow::bail!(
            "heredoc sentinel '{sentinel}' would terminate the heredoc early — \
             the content contains the sentinel as a standalone line. This requires \
             a SHA-256 prefix collision on the content body; investigate if seen."
        );
    }
    Ok(sentinel)
}

/// Round-trip a YAML body through `serde_yaml::from_str` ➜ `to_string` to
/// produce a canonical form (deterministic key order via `Mapping`'s preserved
/// insertion order, normalised quoting, normalised indentation).
///
/// This is the **prep-PR normalisation pass** that the IR refactor (see
/// `docs/ir.md` once it lands) relies on: by establishing a canonical
/// serde_yaml-formatted baseline *before* the IR work, the IR PR's diff
/// becomes purely structural — every line of churn after this point is a
/// real change, not a cosmetic re-quoting.
///
/// Behaviour:
///
/// - Input must be a single top-level YAML document. Multi-document streams
///   (`---` separated) are rejected.
/// - Leading `#` comment lines and blank lines are preserved verbatim and
///   prepended back onto the normalised body. This keeps the per-file
///   `# This file is auto-generated …` / `# @ado-aw …` header intact while
///   normalising everything below it.
/// - YAML comments *between* mapping keys (e.g. the `# Disable PR triggers`
///   line emitted by the PR trigger builder) are dropped — serde_yaml does
///   not preserve them. This is intentional and accepted as part of the
///   canonical-form definition.
/// - Comments *inside* literal block scalars (e.g. bash `#` comments inside
///   `script: |` blocks) are not affected, because they are string content
///   from the YAML parser's perspective.
///
/// Used by IR emitters just before the leading header comment is prepended.
pub fn normalize_yaml(input: &str) -> Result<String> {
    // Split off any leading comment / blank lines and preserve them
    // verbatim. The first non-comment, non-blank line marks the start of
    // the YAML body. Anything before it round-trips through `serde_yaml`
    // would be lost (comments are not preserved); we put them back
    // unchanged after the normalisation pass.
    let mut header_end = 0usize;
    for line in input.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            header_end += line.len();
        } else {
            break;
        }
    }
    let header = &input[..header_end];
    let body = &input[header_end..];

    if body.trim().is_empty() {
        // No body to normalise — return input unchanged.
        return Ok(input.to_string());
    }

    let value: serde_yaml::Value =
        serde_yaml::from_str(body).context("normalize_yaml: failed to parse YAML body")?;
    let mut normalised = serde_yaml::to_string(&value)
        .context("normalize_yaml: failed to serialise canonical YAML")?;
    if !normalised.ends_with('\n') {
        normalised.push('\n');
    }

    Ok(format!("{header}{normalised}"))
}

/// Validate front matter `name` and `description` fields.
///
/// These values are substituted directly into the pipeline YAML template and must not
/// contain ADO expressions (`${{`, `$(`, `$[`), the compiler's own template marker
/// delimiter (`{{`), or newlines — any of which could disclose secrets or manipulate
/// pipeline logic via second-order injection.
pub fn validate_front_matter_identity(front_matter: &FrontMatter) -> Result<()> {
    for (field, value) in [
        ("name", &front_matter.name),
        ("description", &front_matter.description),
    ] {
        validate::reject_pipeline_injection(value, field)?;
    }
    if let Some(workspace) = &front_matter.workspace {
        validate::reject_pipeline_injection(workspace, "workspace")?;
    }

    // Validate trigger.pipeline fields for newlines and ADO expressions
    if let Some(trigger_config) = &front_matter.on_config {
        if let Some(pipeline) = &trigger_config.pipeline {
            validate::reject_pipeline_injection(&pipeline.name, "on.pipeline.name")?;
            if let Some(project) = &pipeline.project {
                validate::reject_pipeline_injection(project, "on.pipeline.project")?;
            }
            for branch in &pipeline.branches {
                validate::reject_pipeline_injection(
                    branch,
                    &format!("on.pipeline.branches entry {:?}", branch),
                )?;
            }
        }

        // Validate on.pr branch/path filters for newlines and ADO expressions
        if let Some(pr) = &trigger_config.pr {
            if let Some(branches) = &pr.branches {
                for b in &branches.include {
                    validate::reject_pipeline_injection(
                        b,
                        &format!("on.pr.branches.include entry {:?}", b),
                    )?;
                }
                for b in &branches.exclude {
                    validate::reject_pipeline_injection(
                        b,
                        &format!("on.pr.branches.exclude entry {:?}", b),
                    )?;
                }
            }
            if let Some(paths) = &pr.paths {
                for p in &paths.include {
                    validate::reject_pipeline_injection(
                        p,
                        &format!("on.pr.paths.include entry {:?}", p),
                    )?;
                }
                for p in &paths.exclude {
                    validate::reject_pipeline_injection(
                        p,
                        &format!("on.pr.paths.exclude entry {:?}", p),
                    )?;
                }
            }
        }
    }

    Ok(())
}

/// Build the final parameters list by combining user-defined parameters
/// with auto-injected parameters.
///
/// Auto-injected parameters:
/// - `clearMemory` (boolean) — when `has_memory` is true and the user has
///   not already defined a parameter with the same name.
/// - `dependsOn` (object, default `[]`) and `condition` (string, default
///   `''`) — when `is_template_target` is true. These are applied at the
///   template call site via `parameters:` and let the parent pipeline
///   inject external stage/job ordering that ADO's `template:` invocation
///   syntax does not otherwise support. See `docs/targets.md` and the ADO
///   `stages.template` / `jobs.template` schemas.
///
/// Errors when `is_template_target` is true and the user has declared
/// front-matter parameters with the reserved names `dependsOn` or
/// `condition` — those names are reserved for template-invocation use.
pub fn build_parameters(
    user_params: &[PipelineParameter],
    has_memory: bool,
    is_template_target: bool,
) -> Result<Vec<PipelineParameter>> {
    if is_template_target {
        for reserved in ["dependsOn", "condition"] {
            if let Some(collide) = user_params.iter().find(|p| p.name == reserved) {
                anyhow::bail!(
                    "Parameter name '{}' is reserved for template-invocation use (target: job/stage). \
                     Rename the parameter '{}' to something else, or change the target.",
                    reserved,
                    collide.name
                );
            }
        }
    }

    let mut params = user_params.to_vec();

    // Auto-inject clearMemory parameter when memory is configured,
    // unless the user already defined one with the same name.
    if has_memory && !params.iter().any(|p| p.name == "clearMemory") {
        params.insert(
            0,
            PipelineParameter {
                name: "clearMemory".to_string(),
                display_name: Some("Clear agent memory".to_string()),
                param_type: Some("boolean".to_string()),
                default: Some(serde_yaml::Value::Bool(false)),
                values: None,
            },
        );
    }

    // Auto-inject dependsOn / condition template parameters for template
    // targets so callers can specify external stage/job ordering at the
    // template invocation site (ADO does not permit dependsOn:/condition:
    // as bare keys on a template: call — only `parameters:`).
    if is_template_target {
        // Prepend so they appear first in the rendered parameters block,
        // ahead of user-defined params and clearMemory.
        params.insert(
            0,
            PipelineParameter {
                name: "condition".to_string(),
                display_name: None,
                param_type: Some("string".to_string()),
                default: Some(serde_yaml::Value::String(String::new())),
                values: None,
            },
        );
        params.insert(
            0,
            PipelineParameter {
                name: "dependsOn".to_string(),
                display_name: None,
                param_type: Some("object".to_string()),
                default: Some(serde_yaml::Value::Sequence(Vec::new())),
                values: None,
            },
        );
    }

    Ok(params)
}

// ──────────────────────────────────────────────────────────────────────────────
// Compact `repos:` lowering
// ──────────────────────────────────────────────────────────────────────────────

/// Lower a `repos:` list into the internal `(Vec<Repository>, Vec<String>)` pair
/// consumed by the rest of the compiler. Also validates aliases for collisions.
pub fn lower_repos(items: &[ReposItem]) -> Result<(Vec<Repository>, Vec<String>)> {
    let mut repositories: Vec<Repository> = Vec::new();
    let mut checkout: Vec<String> = Vec::new();
    let mut seen_aliases: HashSet<String> = HashSet::new();

    for item in items {
        let (name, alias, repo_type, repo_ref, do_checkout) = match item {
            ReposItem::Shorthand(s) => {
                let (alias, name) = parse_shorthand(s)?;
                (
                    name,
                    alias,
                    "git".to_string(),
                    "refs/heads/main".to_string(),
                    true,
                )
            }
            ReposItem::Full(entry) => {
                let alias = match &entry.alias {
                    Some(a) => a.clone(),
                    None => derive_alias(&entry.name)?,
                };
                (
                    entry.name.clone(),
                    alias,
                    entry.repo_type.clone(),
                    entry.repo_ref.clone(),
                    entry.checkout,
                )
            }
        };

        // Reject duplicate aliases
        if !seen_aliases.insert(alias.clone()) {
            anyhow::bail!(
                "Duplicate repository alias '{}' in repos. \
                Use the `alias` field (or `alias=org/repo` shorthand) to disambiguate.",
                alias
            );
        }

        // Reject reserved names
        if RESERVED_WORKSPACE_NAMES.contains(&alias.as_str()) {
            anyhow::bail!(
                "Repository alias '{}' is reserved by the 'workspace:' resolver ({:?}). \
                Rename the alias to avoid ambiguity.",
                alias,
                RESERVED_WORKSPACE_NAMES
            );
        }

        repositories.push(Repository {
            repository: alias.clone(),
            repo_type,
            name,
            repo_ref,
        });

        if do_checkout {
            checkout.push(alias);
        }
    }

    Ok((repositories, checkout))
}

/// Parse a shorthand string: `"org/repo"` → (derived alias, name), or
/// `"alias=org/repo"` → (alias, name).
fn parse_shorthand(s: &str) -> Result<(String, String)> {
    if let Some((alias, name)) = s.split_once('=') {
        let alias = alias.trim().to_string();
        let name = name.trim().to_string();
        if alias.is_empty() {
            anyhow::bail!("repos shorthand '{}' has an empty alias before '='", s);
        }
        if name.is_empty() {
            anyhow::bail!("repos shorthand '{}' has an empty name after '='", s);
        }
        Ok((alias, name))
    } else {
        let alias = derive_alias(s)?;
        Ok((alias, s.to_string()))
    }
}

/// Derive the alias from a full `org/repo` name (last path segment).
fn derive_alias(name: &str) -> Result<String> {
    // Trim trailing slashes to handle "org/repo/" gracefully
    let trimmed = name.trim_end_matches('/');
    let alias = trimmed.rsplit('/').next().unwrap_or(trimmed).to_string();
    if alias.is_empty() {
        anyhow::bail!(
            "Cannot derive a repository alias from '{}'. \
            Provide an explicit `alias` field.",
            name
        );
    }
    Ok(alias)
}

/// Resolve the `repos:` field in a `FrontMatter` into the canonical
/// `(Vec<Repository>, Vec<String>)` pair consumed by the rest of the compiler.
///
/// The legacy `repositories:` + `checkout:` fields are converted to `repos:`
/// by the `repos_unified` codemod (`src/compile/codemods/0001_repos_unified.rs`)
/// before typed deserialization, so by the time this function runs the only
/// shape it sees is `repos:`.
pub fn resolve_repos(front_matter: &FrontMatter) -> Result<(Vec<Repository>, Vec<String>)> {
    if front_matter.repos.is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }
    lower_repos(&front_matter.repos)
}

/// Names that are reserved by the `workspace:` resolver and therefore cannot
/// be used as repository aliases / `checkout:` entries. If a user defines a
/// repository named `repo` and writes `workspace: repo`, the special-cased
/// reserved arm would silently win over the alias resolution, producing the
/// wrong working directory. We reject this at compile time instead.
const RESERVED_WORKSPACE_NAMES: &[&str] = &["root", "repo", "self"];

/// Validate that no entry in `checkout` resolves to the same on-disk
/// directory as the `self` checkout.
///
/// In ADO multi-repo checkout, both `checkout: self` and an additional
/// `checkout: <alias>` land in `s/<RepositoryName>`, where
/// `<RepositoryName>` is `Build.Repository.Name` for `self` and the
/// trailing path segment of the `name:` field for each `repositories:`
/// entry. When these collide, the second checkout runs `git clean -ffdx`
/// and resets to its configured ref, silently wiping files that exist on
/// the trigger branch but not on the workspace ref. Failing fast at
/// compile time is much more discoverable than the resulting runtime
/// "file not found" errors downstream.
///
/// `self_repo_name` is the trigger repo's `Build.Repository.Name` —
/// usually the trailing segment of the trigger repo's full name, inferred
/// from the local git remote. When `None` (e.g. compiling outside an ADO
/// clone, or in unit tests) the check is skipped because we have no
/// reliable identity for `self`.
pub fn validate_checkout_self_collision(
    repositories: &[Repository],
    checkout: &[String],
    self_repo_name: Option<&str>,
) -> Result<()> {
    let Some(self_name) = self_repo_name else {
        return Ok(());
    };
    if checkout.is_empty() {
        return Ok(());
    }

    for alias in checkout {
        let Some(repo) = repositories.iter().find(|r| r.repository == *alias) else {
            // Unknown aliases are reported by `validate_checkout_list`.
            continue;
        };
        // `rsplit('/').next()` on any &str always yields `Some` — even for
        // names without a slash the whole string is returned.
        let last_segment = repo
            .name
            .rsplit('/')
            .next()
            .expect("rsplit always yields one item");
        // ADO is case-insensitive on Windows agents and case-sensitive on
        // Linux. Use a case-insensitive comparison so the collision is
        // caught regardless of agent OS — the resulting pipeline would
        // break on at least one platform either way.
        if last_segment.eq_ignore_ascii_case(self_name) {
            anyhow::bail!(
                "Checkout entry '{}' (repository name '{}') resolves to the same \
                directory ('s/{}') as the trigger repository checked out as 'self'. \
                The second checkout would overwrite the first, replacing files \
                from the trigger branch with the workspace ref. Remove '{}' from \
                'checkout:' — the 'self' checkout already provides access to this \
                repository.",
                alias,
                repo.name,
                self_name,
                alias,
            );
        }
    }

    Ok(())
}

/// Validate that all entries in checkout list exist in repositories
pub fn validate_checkout_list(repositories: &[Repository], checkout: &[String]) -> Result<()> {
    if checkout.is_empty() {
        return Ok(());
    }

    let repo_names: std::collections::HashSet<_> =
        repositories.iter().map(|r| r.repository.as_str()).collect();

    for name in checkout {
        if !repo_names.contains(name.as_str()) {
            anyhow::bail!(
                "Checkout entry '{}' not found in repositories. Available: {:?}",
                name,
                repo_names
            );
        }
        if RESERVED_WORKSPACE_NAMES.contains(&name.as_str()) {
            anyhow::bail!(
                "Checkout entry '{}' uses a name reserved by the 'workspace:' resolver \
                ({:?}). Rename the repository alias to avoid ambiguity with \
                'workspace: {}'.",
                name,
                RESERVED_WORKSPACE_NAMES,
                name
            );
        }
    }

    Ok(())
}

/// Sentinel prefix used to encode a repository-alias workspace selection
/// in the string returned by [`compute_effective_workspace`]. The prefix is
/// only ever produced internally by `compute_effective_workspace` from a
/// user-supplied alias that has just been checked against the `checkout:`
/// list, so the encoded value never round-trips back through user input.
const WORKSPACE_ALIAS_PREFIX: &str = "alias:";

/// Compute the effective workspace based on explicit setting and checkout configuration.
///
/// Accepted values for `explicit_workspace`:
/// - `"root"` — `$(Build.SourcesDirectory)` (the checkout root)
/// - `"repo"` or `"self"` — the trigger repository's subfolder
/// - any repository alias listed in `checkout` — that repository's subfolder
///
/// Returns an encoded string that [`generate_working_directory`] resolves to
/// the actual ADO path expression.
pub fn compute_effective_workspace(
    explicit_workspace: &Option<String>,
    checkout: &[String],
    agent_name: &str,
) -> Result<String> {
    let has_additional_checkouts = !checkout.is_empty();

    match explicit_workspace {
        Some(ws) => {
            let ws = ws.as_str();
            match ws {
                "root" => Ok("root".to_string()),
                "repo" | "self" => {
                    if !has_additional_checkouts {
                        eprintln!(
                            "Warning: Agent '{}' has workspace: {} but no additional repositories in checkout. \
                            When only 'self' is checked out, $(Build.SourcesDirectory) already contains the repository content. \
                            The workspace setting has no effect in this case.",
                            agent_name, ws
                        );
                    }
                    Ok("repo".to_string())
                }
                alias => {
                    // Defense in depth: even though aliases are constrained
                    // by `validate_checkout_list` to match a `repository:`
                    // name, refuse anything that could escape the workspace
                    // root once embedded into the working directory path.
                    if !validate::is_safe_path_segment(alias) {
                        anyhow::bail!(
                            "Agent '{}' has workspace: '{}' which is not a safe path \
                            segment. Repository aliases must match [A-Za-z0-9._-], \
                            must not contain '..', '/', '\\\\', and must not start with '.'.",
                            agent_name,
                            alias
                        );
                    }
                    // A single contains() check covers both "alias not in
                    // checkout" and "checkout is empty" — produce one error
                    // message that clearly lists what would have been valid.
                    if !checkout.iter().any(|c| c == alias) {
                        if checkout.is_empty() {
                            anyhow::bail!(
                                "Agent '{}' has workspace: '{}' but no additional repositories are checked out. \
                                A repository alias for workspace is only valid when at least one repository appears in 'checkout:'. \
                                Use 'root', 'repo' (or 'self'), or add the repository to the 'checkout:' list.",
                                agent_name,
                                alias
                            );
                        }
                        anyhow::bail!(
                            "Agent '{}' has workspace: '{}' which does not match any checked-out repository. \
                            Valid values: 'root', 'repo' (or 'self'), or one of {:?}",
                            agent_name,
                            alias,
                            checkout
                        );
                    }
                    Ok(format!("{}{}", WORKSPACE_ALIAS_PREFIX, alias))
                }
            }
        }
        None if has_additional_checkouts => Ok("repo".to_string()),
        None => Ok("root".to_string()),
    }
}

/// Generate the directory where the trigger ("self") repository is checked out.
///
/// This is independent of `workspace:` — it depends only on whether any
/// additional repositories are checked out:
/// - No additional checkouts → `$(Build.SourcesDirectory)` (ADO checks `self`
///   into the root).
/// - One or more additional checkouts → `$(Build.SourcesDirectory)/$(Build.Repository.Name)`
///   (ADO puts each checked-out repo, including `self`, into a subfolder named
///   after the repository).
///
/// Used to anchor paths to files that ship in the trigger repo (e.g. the agent
/// markdown source and the compiled pipeline yaml itself), regardless of where
/// `workspace:` points the agent.
pub fn generate_trigger_repo_directory(checkout: &[String]) -> String {
    if checkout.is_empty() {
        "$(Build.SourcesDirectory)".to_string()
    } else {
        "$(Build.SourcesDirectory)/$(Build.Repository.Name)".to_string()
    }
}

/// Generate working directory based on workspace setting
pub fn generate_working_directory(effective_workspace: &str) -> String {
    if let Some(alias) = effective_workspace.strip_prefix(WORKSPACE_ALIAS_PREFIX) {
        return format!("$(Build.SourcesDirectory)/{}", alias);
    }
    match effective_workspace {
        "repo" => "$(Build.SourcesDirectory)/$(Build.Repository.Name)".to_string(),
        "root" => "$(Build.SourcesDirectory)".to_string(),
        // compute_effective_workspace only ever returns "root", "repo", or an
        // "alias:<name>" sentinel; any other value indicates a programming
        // error rather than user input. Fall back to the safest path.
        other => {
            debug_assert!(false, "unexpected effective workspace value: {other}");
            "$(Build.SourcesDirectory)".to_string()
        }
    }
}

const ADO_BUILD_NUMBER_MAX_LEN: usize = 255;
pub(crate) const ADO_BUILD_ID_SUFFIX: &str = "-$(BuildID)";

/// Sanitize front-matter agent name for ADO build-number format strings.
///
/// Rules enforced:
/// - Remove characters disallowed by Azure DevOps build numbers:
///   `"`, `/`, `:`, `<`, `>`, `\`, `|`, `?`, `@`, `*`
/// - Trim leading/trailing whitespace
/// - Ensure the resulting build number format (`<name>-$(BuildID)`) fits in 255 chars
/// - Ensure the name fragment does not end with `.`
pub fn sanitize_pipeline_agent_name(name: &str) -> String {
    let mut sanitized = String::with_capacity(name.len());
    for ch in name.trim().chars() {
        if matches!(
            ch,
            '"' | '/' | ':' | '<' | '>' | '\\' | '|' | '?' | '@' | '*'
        ) {
            continue;
        }
        sanitized.push(ch);
    }

    let mut sanitized = sanitized.trim().trim_end_matches('.').to_string();
    let max_agent_len = ADO_BUILD_NUMBER_MAX_LEN.saturating_sub(ADO_BUILD_ID_SUFFIX.len());
    if sanitized.chars().count() > max_agent_len {
        sanitized = sanitized.chars().take(max_agent_len).collect();
        sanitized = sanitized.trim_end_matches('.').to_string();
    }

    if sanitized.is_empty() {
        "pipeline".to_string()
    } else {
        sanitized
    }
}

/// Default self-hosted pool for 1ES templates.
pub const DEFAULT_ONEES_POOL: &str = "AZS-1ES-L-MMS-ubuntu-22.04";
/// Default Microsoft-hosted VM image for non-1ES templates.
pub const DEFAULT_VM_IMAGE_POOL: &str = "ubuntu-22.04";

/// Resolve a typed [`crate::compile::ir::job::Pool`] for IR target builders.
pub fn resolve_pool_typed(
    target: CompileTarget,
    pool: Option<&PoolConfig>,
) -> Result<crate::compile::ir::job::Pool> {
    use crate::compile::ir::job::Pool;
    match target {
        CompileTarget::OneES => {
            let (name, os) = match pool {
                None => (DEFAULT_ONEES_POOL.to_string(), "linux".to_string()),
                Some(PoolConfig::Name(name)) => (name.clone(), "linux".to_string()),
                Some(PoolConfig::Full(full)) => {
                    if let (Some(name), Some(vm_image)) =
                        (full.name.as_deref(), full.vm_image.as_deref())
                    {
                        anyhow::bail!(
                            "pool cannot specify both `name` and `vmImage` (got name='{}', vmImage='{}')",
                            name,
                            vm_image
                        );
                    }
                    if let Some(vm_image) = full.vm_image.as_deref() {
                        anyhow::bail!(
                            "target: 1es does not support `pool.vmImage` ('{}'); use `pool.name` for a 1ES pool",
                            vm_image
                        );
                    }
                    (
                        full.name
                            .as_deref()
                            .unwrap_or(DEFAULT_ONEES_POOL)
                            .to_string(),
                        full.os.as_deref().unwrap_or("linux").to_string(),
                    )
                }
            };
            Ok(Pool::Named {
                name,
                image: None,
                os: Some(os),
            })
        }
        _ => {
            let Some(pool) = pool else {
                return Ok(Pool::VmImage(DEFAULT_VM_IMAGE_POOL.to_string()));
            };
            match pool {
                PoolConfig::Name(name) => Ok(Pool::Named {
                    name: name.clone(),
                    image: None,
                    os: None,
                }),
                PoolConfig::Full(full) => match (full.name.as_deref(), full.vm_image.as_deref()) {
                    (Some(name), Some(vm_image)) => anyhow::bail!(
                        "pool cannot specify both `name` and `vmImage` (got name='{}', vmImage='{}')",
                        name,
                        vm_image
                    ),
                    (Some(name), None) => Ok(Pool::Named {
                        name: name.to_string(),
                        image: None,
                        os: None,
                    }),
                    (None, Some(vm_image)) => Ok(Pool::VmImage(vm_image.to_string())),
                    (None, None) => Ok(Pool::VmImage(DEFAULT_VM_IMAGE_POOL.to_string())),
                },
            }
        }
    }
}

/// Derive a valid ADO identifier from the agent name for use as a job-name
/// prefix and stage name. Converts to PascalCase, stripping non-alphanumeric
/// characters.
///
/// Examples:
/// - `"Daily Code Review"` → `"DailyCodeReview"`
/// - `"my-agent-123"` → `"MyAgent123"`
/// - `""` → `"Agent"` (fallback)
/// - `"123start"` → `"_123start"` (prefix underscore for leading digit)
/// - `"über-agent"` → `"BerAgent"` (non-ASCII stripped; ADO requires `[A-Za-z0-9_]`)
pub fn generate_stage_prefix(name: &str) -> String {
    // Warn if any Unicode alphanumeric characters are present — they will be
    // treated as word-separator boundaries and stripped from the output, which
    // may surprise users whose agent name starts with a non-ASCII letter.
    if name
        .chars()
        .any(|c| c.is_alphanumeric() && !c.is_ascii_alphanumeric())
    {
        log::warn!(
            "Agent name '{}' contains non-ASCII alphanumeric characters; \
             these are dropped from the ADO job-name prefix because ADO identifiers \
             require [A-Za-z0-9_]. Rename the agent to use ASCII characters only \
             if the prefix is important.",
            name
        );
    }

    let pascal: String = name
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|s| !s.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                None => String::new(),
                Some(first) => {
                    let upper = first.to_uppercase().to_string();
                    upper + chars.as_str()
                }
            }
        })
        .collect();

    if pascal.is_empty() {
        "Agent".to_string()
    } else if pascal.starts_with(|c: char| c.is_ascii_digit()) {
        format!("_{}", pascal)
    } else {
        pascal
    }
}

/// Version of the AWF (Agentic Workflow Firewall) binary to download from GitHub Releases.
/// Update this when upgrading to a new AWF release.
/// See: <https://github.com/github/gh-aw-firewall/releases>
pub const AWF_VERSION: &str = "0.25.65";

/// Prefix used to identify agentic pipeline YAML files generated by ado-aw.
pub const HEADER_MARKER: &str = "# @ado-aw";

/// Generate the header comment block prepended to all compiled pipeline YAML.
///
/// The header includes:
/// - A human-readable "do not edit" warning
/// - A machine-readable `@ado-aw` marker with source path and compiler version
///
/// The source path is stored as a relative path so that `--source` filters
/// and auto-discovery recompile work regardless of how the user invoked the
/// compiler (relative path, absolute path, etc.). Path separators are
/// normalized to forward slashes for cross-platform consistency.
///
/// Normalise a source markdown path into the canonical form embedded
/// in the `# ado-aw-metadata:` JSON marker and the `# @ado-aw` YAML
/// header comment.
///
/// Applies forward-slash separator normalisation, strips CR/LF, and
/// collapses any leading `./`. Does **not** escape `"` — JSON encoding
/// is the caller's job (`serde_json::json!` handles it for the marker;
/// [`generate_header_comment`] escapes for its YAML comment surface).
/// Previously this also escaped `"`, but that produced a literal `\"`
/// inside the marker JSON (serde_json then escaped the leading
/// backslash on top, storing a spurious extra backslash on every
/// round-trip). The header comment now applies its own escape inline.
///
/// Shared between [`generate_header_comment`], the always-on
/// `ado-aw-marker` compiler extension, and the `--source` filter in
/// Preview-driven discovery so all surfaces agree on the canonical
/// form of a source path.
///
/// Absolute inputs (e.g. `ado-aw compile /repo/agents/foo.md`) are
/// converted to a path relative to the current working directory so that
/// `--source agents/foo.md` filters can match them. When the absolute path
/// is not under the CWD (unusual), falls back to the original input rather
/// than silently producing a wrong value.
pub fn normalize_source_path(input_path: &std::path::Path) -> String {
    let relative: std::borrow::Cow<std::path::Path> = if input_path.is_absolute() {
        std::env::current_dir()
            .ok()
            .and_then(|cwd| input_path.strip_prefix(&cwd).ok().map(|p| p.to_path_buf()))
            .map(std::borrow::Cow::Owned)
            .unwrap_or(std::borrow::Cow::Borrowed(input_path))
    } else {
        std::borrow::Cow::Borrowed(input_path)
    };

    let mut source_path = relative
        .to_string_lossy()
        .replace('\\', "/")
        .replace(['\n', '\r'], "");

    // Strip redundant leading "./" prefixes to prevent accumulation when
    // compile_all_pipelines re-joins paths through Path::new(".").join(source).
    while source_path.starts_with("./") {
        source_path = source_path[2..].to_string();
    }

    source_path
}

pub fn generate_header_comment(input_path: &std::path::Path) -> String {
    let version = env!("CARGO_PKG_VERSION");
    // The header comment embeds the source inside double quotes
    // (`source="…"`); escape `"` so legacy `parse_header_line` consumers
    // can still recover the original. The JSON marker does not need
    // this — serde_json escapes JSON-meaningful chars on its own.
    let source_path = normalize_source_path(input_path).replace('"', "\\\"");

    format!(
        "# This file is auto-generated by ado-aw. Do not edit manually.\n\
         # @ado-aw source=\"{}\" version={}\n",
        source_path, version
    )
}

/// Docker image and version for the MCP Gateway (gh-aw-mcpg).
/// Update this when upgrading to a new MCPG release.
/// See: <https://github.com/github/gh-aw-mcpg/releases>
pub const MCPG_VERSION: &str = "0.3.23";

/// Docker image for the MCPG container.
pub const MCPG_IMAGE: &str = "ghcr.io/github/gh-aw-mcpg";

/// Default port MCPG listens on inside the container (host network mode).
pub const MCPG_PORT: u16 = 80;

/// Domain that the AWF-sandboxed agent uses to reach MCPG on the host.
/// Docker's `host.docker.internal` resolves to the host loopback from
/// inside containers running with `--network host` or via Docker DNS.
pub const MCPG_DOMAIN: &str = "host.docker.internal";

/// Docker base image for the Azure DevOps MCP container.
pub const ADO_MCP_IMAGE: &str = "node:20-slim";

/// Default entrypoint for the Azure DevOps MCP container.
pub const ADO_MCP_ENTRYPOINT: &str = "npx";

/// Default entrypoint args for the Azure DevOps MCP npm package.
pub const ADO_MCP_PACKAGE: &str = "@azure-devops/mcp";

/// Reserved MCPG server name for the auto-configured ADO MCP.
pub const ADO_MCP_SERVER_NAME: &str = "azure-devops";

/// Generate the agent markdown source path for Stage 3 execution.
///
/// Returns a path using `{{ trigger_repo_directory }}` as the base. The agent
/// markdown lives in the trigger ("self") repo, so this anchor is independent
/// of the user's `workspace:` setting (which may point at a different
/// checked-out repo where the agent runs).
///
/// The full relative path of the input file is preserved so that agents compiled
/// from subdirectories (e.g. `ado-aw compile agents/ctf.md`) produce a correct
/// runtime path rather than one that drops the directory component.
///
/// Absolute paths fall back to using only the filename to avoid embedding
/// machine-specific paths in the generated pipeline.
pub fn generate_source_path(input_path: &std::path::Path) -> String {
    let relative = normalize_relative_path(input_path).unwrap_or_else(|| {
        input_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("agent.md")
            .to_string()
    });

    format!("{{{{ trigger_repo_directory }}}}/{}", relative)
}

/// Generate the "Verify pipeline integrity" step for the pipeline YAML.
///
/// When `skip` is `false` (the default), returns the full bash step that
/// downloads the ado-aw compiler and runs `ado-aw check` against the
/// pipeline path.
///
/// The step sets `workingDirectory: {{ trigger_repo_directory }}` so that:
/// 1. The relative `{{ pipeline_path }}` argument resolves correctly when
///    `checkout:` produces a multi-repo `$(Build.SourcesDirectory)` layout.
/// 2. `ado-aw check`'s recompile step has access to the trigger repo's
///    `.git` directory, which is required to infer the ADO org from the
///    git remote (used by `tools.azure-devops`).
///
/// When `skip` is `true` (developer builds with `--skip-integrity`),
/// returns an empty string and the step is omitted from the pipeline.
pub fn generate_integrity_check(skip: bool) -> String {
    if skip {
        return String::new();
    }

    // Indentation is handled by replace_with_indent at the call site.
    r#"- bash: |
    AGENTIC_PIPELINES_PATH="$(Pipeline.Workspace)/agentic-pipeline-compiler/ado-aw"
    chmod +x "$AGENTIC_PIPELINES_PATH"
    $AGENTIC_PIPELINES_PATH check "{{ pipeline_path }}"
  workingDirectory: {{ trigger_repo_directory }}
  displayName: "Verify pipeline integrity""#
        .to_string()
}

/// Returns `true` when the agent's front matter sets
/// `ado-aw-debug.create-issue:` — the gate that activates the debug-only
/// `create-issue` safe output.
pub(crate) fn debug_create_issue_enabled(front_matter: &FrontMatter) -> bool {
    front_matter
        .ado_aw_debug
        .as_ref()
        .and_then(|d| d.create_issue.as_ref())
        .is_some()
}

/// Validate the `ado-aw-debug:` section.
///
/// When `create-issue:` is present:
/// * `target-repo` is required and must be `owner/repo`-shaped.
/// * Operator-supplied strings (target-repo, title-prefix, labels,
///   allowed-labels, assignees) must not contain ADO pipeline-injection
///   sequences (per `reject_pipeline_injection`).
///
/// Independently, `safe-outputs:` must NOT contain any `DEBUG_ONLY_TOOLS`
/// keys. The MCP layer ignores them, but their config would otherwise
/// flow into `ctx.tool_configs` and create a path for forged NDJSON
/// entries to bypass the `ado-aw-debug` gate at Stage 3.
///
/// Pure config check — no I/O, runs at compile time.
pub fn validate_ado_aw_debug_config(front_matter: &FrontMatter) -> Result<()> {
    use crate::safeoutputs::DEBUG_ONLY_TOOLS;

    // Defence-in-depth: reject any debug-only tool name appearing under the
    // regular safe-outputs surface. There is no legitimate reason for it to
    // be there — it's either a typo or an attempt to smuggle a debug-only
    // tool into a non-debug pipeline.
    for debug_tool in DEBUG_ONLY_TOOLS {
        if front_matter.safe_outputs.contains_key(*debug_tool) {
            anyhow::bail!(
                "safe-outputs.{0} is a debug-only tool and must be configured \
                 under `ado-aw-debug.{0}` instead of `safe-outputs.{0}`. \
                 The MCP layer hides debug-only tools by default; the \
                 `ado-aw-debug:` section is the only place to enable them.",
                debug_tool
            );
        }
    }

    let Some(debug) = front_matter.ado_aw_debug.as_ref() else {
        return Ok(());
    };
    let Some(ci) = debug.create_issue.as_ref() else {
        return Ok(());
    };

    crate::safeoutputs::validate_target_repo(&ci.target_repo)?;

    crate::validate::reject_pipeline_injection(
        &ci.target_repo,
        "ado-aw-debug.create-issue.target-repo",
    )?;
    if let Some(prefix) = ci.title_prefix.as_deref() {
        crate::validate::reject_pipeline_injection(
            prefix,
            "ado-aw-debug.create-issue.title-prefix",
        )?;
    }
    for label in &ci.labels {
        crate::validate::reject_pipeline_injection(label, "ado-aw-debug.create-issue.labels")?;
    }
    for label in &ci.allowed_labels {
        crate::validate::reject_pipeline_injection(
            label,
            "ado-aw-debug.create-issue.allowed-labels",
        )?;
    }
    for assignee in &ci.assignees {
        crate::validate::reject_pipeline_injection(
            assignee,
            "ado-aw-debug.create-issue.assignees",
        )?;
    }
    Ok(())
}

/// Generate the pipeline YAML path for integrity checking at ADO runtime.
///
/// Returns the path **relative** to the trigger repository root. The integrity
/// check step itself sets `workingDirectory: {{ trigger_repo_directory }}` so
/// that the path resolves correctly and so that `ado-aw check`'s recompile
/// step has access to the trigger repo's `.git` directory (needed to infer
/// the ADO org for `tools.azure-devops`).
///
/// The full relative path is preserved so that pipelines compiled into
/// subdirectories (e.g. `agents/ctf.yml`) produce a correct runtime path
/// rather than one that drops the directory component.
///
/// Absolute paths fall back to using only the filename to avoid embedding
/// machine-specific paths in the generated pipeline.
pub fn generate_pipeline_path(output_path: &std::path::Path) -> String {
    normalize_relative_path(output_path).unwrap_or_else(|| {
        output_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("pipeline.yml")
            .to_string()
    })
}

/// Normalize a path for embedding in a generated pipeline.
///
/// Returns `Some(String)` when `path` is relative, with:
/// - Backslashes converted to forward slashes
/// - Redundant leading `./` prefixes stripped
///
/// For absolute paths the function first tries to compute a relative path from
/// the nearest git repository root (found by walking up the directory tree
/// looking for a `.git` entry).  This preserves the directory structure when
/// the user passes an absolute path — e.g.
/// `/home/user/repo/agents/ctf.md` → `agents/ctf.md`.
///
/// Falls back to `None` (callers use filename-only) only when no git root is
/// found, to avoid embedding machine-specific absolute paths in the generated
/// pipeline YAML.
///
/// Note: `..` components in relative paths are passed through unchanged.
/// Callers are responsible for ensuring the path does not traverse outside the
/// repository checkout.
fn normalize_relative_path(path: &std::path::Path) -> Option<String> {
    if path.is_absolute() {
        // Try to make the path relative to the nearest git repo root so that
        // directory structure (e.g. `agents/ctf.md`) is preserved even when
        // the user invokes the compiler with an absolute path.
        if let Some(git_root) = find_git_root(path)
            && let Ok(rel) = path.strip_prefix(&git_root)
        {
            let s = rel.to_string_lossy().replace('\\', "/");
            return Some(s);
        }
        return None;
    }

    let mut s = path.to_string_lossy().replace('\\', "/");
    while let Some(stripped) = s.strip_prefix("./") {
        s = stripped.to_string();
    }
    Some(s)
}

/// Walk up the directory tree from `path` looking for a `.git` entry.
///
/// Returns the first ancestor directory that contains `.git`, or `None` if the
/// traversal reaches the filesystem root without finding one.
fn find_git_root(path: &std::path::Path) -> Option<std::path::PathBuf> {
    // Start from the file's parent directory (or the path itself if it is a dir).
    let start: &std::path::Path = if path.is_dir() { path } else { path.parent()? };

    let mut current = start.to_path_buf();
    loop {
        if current.join(".git").exists() {
            return Some(current);
        }
        match current.parent() {
            Some(parent) => current = parent.to_path_buf(),
            None => return None,
        }
    }
}

// ==================== Permission helpers ====================

/// ADO resource ID for minting ADO-scoped tokens via Azure CLI.
const ADO_RESOURCE_ID: &str = "499b84ac-1321-427f-aa17-267ca6975798";

/// Generate an AzureCLI@2 step to acquire an ADO-scoped token from an ARM service connection.
/// The `variable_name` parameter controls which pipeline variable the token is stored in
/// (e.g. "SC_READ_TOKEN" for the agent, "SC_WRITE_TOKEN" for the executor).
/// Returns empty string if no service connection is provided.
pub fn generate_acquire_ado_token(service_connection: Option<&str>, variable_name: &str) -> String {
    match service_connection {
        Some(sc) => {
            let mut lines = Vec::new();
            lines.push("- task: AzureCLI@2".to_string());
            lines.push(format!(
                r#"  displayName: "Acquire ADO token ({variable_name})""#
            ));
            lines.push("  inputs:".to_string());
            lines.push(format!(
                "    azureSubscription: '{}'",
                sc.replace('\'', "''")
            ));
            lines.push("    scriptType: 'bash'".to_string());
            lines.push("    scriptLocation: 'inlineScript'".to_string());
            lines.push("    addSpnToEnvironment: true".to_string());
            lines.push("    inlineScript: |".to_string());
            lines.push("      ADO_TOKEN=$(az account get-access-token \\".to_string());
            lines.push(format!("        --resource {} \\", ADO_RESOURCE_ID));
            lines.push("        --query accessToken -o tsv)".to_string());
            lines.push(format!(
                "      echo \"##vso[task.setvariable variable={variable_name};issecret=true]$ADO_TOKEN\""
            ));
            // Trailing newline ensures the inlineScript block scalar value
            // preserves its terminating newline through round-trip parse/emit;
            // without it serde_yaml strips the newline and switches to the
            // `|-` chomping indicator (semantically identical, but produces
            // a textual diff against the committed lock files).
            format!("{}\n", lines.join("\n"))
        }
        None => String::new(),
    }
}

/// Generate the env block entries for the executor step (Stage 3 Execution).
///
/// Always emits a non-empty `env:` block containing at minimum
/// `SYSTEM_ACCESSTOKEN`, which the Stage 3 executor uses to authenticate ADO
/// REST calls for write-bearing safe-output tools (create PR, create work
/// item, etc.).
///
/// Sources:
/// * `SYSTEM_ACCESSTOKEN: $(SC_WRITE_TOKEN)` when `write_service_connection`
///   is `Some` — write-capable ADO token minted via an ARM service connection.
///   Use this for cross-org / cross-project writes or when you need
///   named-identity attribution instead of the default
///   `Project Collection Build Service` identity.
/// * `SYSTEM_ACCESSTOKEN: $(System.AccessToken)` (default) — the pipeline's
///   built-in OAuth token, scoped by the pipeline's "Limit job authorization
///   scope" settings. Avoids the operational overhead of an ARM service
///   connection. The agent (Stage 1) never maps this variable, so the
///   token remains executor-only.
/// * `ADO_AW_DEBUG_GITHUB_TOKEN: $(ADO_AW_DEBUG_GITHUB_TOKEN)` when
///   `debug_create_issue_enabled` is `true` — GitHub PAT used by the
///   `ado-aw-debug.create-issue` safe output. Sourced from a dedicated
///   pipeline variable so it stays separate from the read-only `GITHUB_TOKEN`
///   the agent (Stage 1) sees.
pub fn generate_executor_ado_env(
    write_service_connection: Option<&str>,
    debug_create_issue_enabled: bool,
) -> String {
    let mut lines: Vec<String> = Vec::new();
    if write_service_connection.is_some() {
        lines.push("SYSTEM_ACCESSTOKEN: $(SC_WRITE_TOKEN)".to_string());
    } else {
        lines.push("SYSTEM_ACCESSTOKEN: $(System.AccessToken)".to_string());
    }
    if debug_create_issue_enabled {
        lines.push("ADO_AW_DEBUG_GITHUB_TOKEN: $(ADO_AW_DEBUG_GITHUB_TOKEN)".to_string());
    }
    // The two-space indent on each value line is the YAML relative indent for
    // a key nested under `env:`. replace_with_indent prepends the base
    // indentation from the marker's position in the template to each
    // subsequent line.
    let body = lines
        .into_iter()
        .map(|l| format!("  {}", l))
        .collect::<Vec<_>>()
        .join("\n");
    format!("env:\n{}", body)
}

/// Generate `--enabled-tools` CLI args for the SafeOutputs MCP server.
///
/// Derives the tool list from `safe-outputs:` front matter keys plus always-on
/// diagnostic tools, plus any debug-only safe outputs activated via the
/// `ado-aw-debug:` section (e.g. `create-issue`).
///
/// If `safe-outputs:` is empty AND no `ado-aw-debug` debug-only tool is
/// configured, returns an empty string (all non-debug tools enabled for
/// backward compatibility — debug-only tools remain stripped at the MCP
/// layer regardless).
///
/// Tool names are validated to contain only ASCII alphanumerics and hyphens
/// to prevent shell injection when the args are embedded in bash commands.
/// Unrecognized tool names emit a compile-time warning and are skipped.
pub fn generate_enabled_tools_args(front_matter: &FrontMatter) -> String {
    use crate::safeoutputs::{ALL_KNOWN_SAFE_OUTPUTS, ALWAYS_ON_TOOLS, NON_MCP_SAFE_OUTPUT_KEYS};
    use std::collections::HashSet;

    let debug_create_issue = debug_create_issue_enabled(front_matter);

    if front_matter.safe_outputs.is_empty() && !debug_create_issue {
        return String::new();
    }

    // `seen` deduplicates across user keys, ALWAYS_ON_TOOLS, and debug-only
    // tools (e.g. if the user configures `noop` explicitly, it shouldn't
    // appear twice in the output).
    let mut seen = HashSet::new();
    let mut tools: Vec<String> = Vec::new();
    let mut effective_mcp_tool_count = 0usize;
    for key in front_matter.safe_outputs.keys() {
        if !validate::is_safe_tool_name(key) {
            eprintln!(
                "Warning: skipping invalid safe-output tool name '{}' (must be ASCII alphanumeric/hyphens only)",
                key
            );
            continue;
        }
        if NON_MCP_SAFE_OUTPUT_KEYS.contains(&key.as_str()) {
            continue;
        }
        if key == "memory" {
            eprintln!(
                "Warning: Agent '{}': 'safe-outputs: memory:' has moved to \
                 'tools: cache-memory:'. Update your front matter to restore memory support.",
                front_matter.name
            );
            continue;
        }
        // Unreachable in practice: validate_safe_outputs_keys bails before
        // the pipeline reaches this point. The check is kept as a defensive
        // guard for callers that bypass the validation phase.
        if !ALL_KNOWN_SAFE_OUTPUTS.contains(&key.as_str()) {
            continue;
        }
        effective_mcp_tool_count += 1;
        if seen.insert(key.clone()) {
            tools.push(key.clone());
        }
    }

    // Debug-only tools must be added explicitly — they're stripped from the
    // MCP layer by default and only become reachable when listed here.
    if debug_create_issue && seen.insert("create-issue".to_string()) {
        tools.push("create-issue".to_string());
        effective_mcp_tool_count += 1;
    }

    if effective_mcp_tool_count == 0 {
        // Every user-specified key was either a non-MCP key or a guard path
        // from the defensive check above. Return empty to keep all tools
        // available (backward compat).
        return String::new();
    }

    // Always include diagnostic tools
    for tool in ALWAYS_ON_TOOLS {
        let name = tool.to_string();
        if seen.insert(name.clone()) {
            tools.push(name);
        }
    }

    tools.sort();

    let args = tools
        .iter()
        .map(|t| format!("--enabled-tools {}", t))
        .collect::<Vec<_>>()
        .join(" ");

    // Trailing space so the args don't concatenate with the next positional
    // argument when embedded inline in the shell template.
    // `args` is never empty here because ALWAYS_ON_TOOLS always contributes entries.
    args + " "
}

/// Validate that comment-on-work-item has a required `target` field when configured.
pub fn validate_comment_target(front_matter: &FrontMatter) -> Result<()> {
    if let Some(config_value) = front_matter.safe_outputs.get("comment-on-work-item") {
        // Check that "target" key is present in the config
        if let Some(obj) = config_value.as_object() {
            if !obj.contains_key("target") {
                anyhow::bail!(
                    "safe-outputs.comment-on-work-item requires a 'target' field to scope \
                     which work items the agent can comment on. Options:\n\n  \
                     target: \"*\"           # any work item (unrestricted)\n  \
                     target: 12345          # specific work item ID\n  \
                     target: [12345, 67890] # list of work item IDs\n  \
                     target: \"Path\\\\Sub\"   # work items under area path prefix\n"
                );
            }
        } else {
            // If the value is not an object (e.g., `comment-on-work-item: true`), that's invalid
            anyhow::bail!(
                "safe-outputs.comment-on-work-item must be a configuration object with at \
                 least a 'target' field. Example:\n\n  \
                 safe-outputs:\n    comment-on-work-item:\n      target: \"*\"\n"
            );
        }
    }
    Ok(())
}

/// Validate that update-work-item has a required `target` field when configured.
pub fn validate_update_work_item_target(front_matter: &FrontMatter) -> Result<()> {
    if let Some(config_value) = front_matter.safe_outputs.get("update-work-item") {
        if let Some(obj) = config_value.as_object() {
            if !obj.contains_key("target") {
                anyhow::bail!(
                    "safe-outputs.update-work-item requires a 'target' field to scope \
                     which work items the agent can update. Options:\n\n  \
                     target: \"*\"   # any work item (unrestricted)\n  \
                     target: 42    # specific work item ID\n"
                );
            }
        } else {
            anyhow::bail!(
                "safe-outputs.update-work-item must be a configuration object with at \
                 least a 'target' field. Example:\n\n  \
                 safe-outputs:\n    update-work-item:\n      target: \"*\"\n      title: true\n"
            );
        }
    }
    Ok(())
}

/// Validate that submit-pr-review has a required `allowed-events` field when configured.
///
/// An empty or missing `allowed-events` list would allow agents to cast any review vote,
/// including auto-approvals. Operators must explicitly opt in to each allowed event.
pub fn validate_submit_pr_review_events(front_matter: &FrontMatter) -> Result<()> {
    if let Some(config_value) = front_matter.safe_outputs.get("submit-pr-review") {
        if let Some(obj) = config_value.as_object() {
            let allowed_events = obj.get("allowed-events");
            let is_empty = match allowed_events {
                None => true,
                Some(v) => v.as_array().is_none_or(|a| a.is_empty()),
            };
            if is_empty {
                anyhow::bail!(
                    "safe-outputs.submit-pr-review requires a non-empty 'allowed-events' list \
                     to prevent agents from casting unrestricted review votes. Example:\n\n  \
                     safe-outputs:\n    submit-pr-review:\n      allowed-events:\n        \
                     - comment\n        - approve-with-suggestions\n\n\
                     Valid events: approve, approve-with-suggestions, request-changes, comment\n"
                );
            }
        } else {
            anyhow::bail!(
                "safe-outputs.submit-pr-review must be a configuration object with an \
                 'allowed-events' list. Example:\n\n  \
                 safe-outputs:\n    submit-pr-review:\n      allowed-events:\n        - comment\n"
            );
        }
    }
    Ok(())
}

/// Validate that update-pr has a required `allowed-votes` field when the `vote` operation
/// is enabled (i.e., `allowed-operations` is empty — meaning all ops — or explicitly contains
/// "vote").
///
/// An empty `allowed-votes` list when vote is enabled would always fail at Stage 3 with a
/// runtime error. Catching this at compile time is consistent with how
/// `validate_submit_pr_review_events` handles the analogous case.
pub fn validate_update_pr_votes(front_matter: &FrontMatter) -> Result<()> {
    if let Some(config_value) = front_matter.safe_outputs.get("update-pr")
        && let Some(obj) = config_value.as_object()
    {
        // Determine whether the vote operation is reachable:
        // - allowed-operations absent or empty → all operations allowed (includes vote)
        // - allowed-operations non-empty → vote is allowed only if explicitly listed
        let vote_reachable = match obj.get("allowed-operations") {
            None => true,
            Some(v) => v
                .as_array()
                .is_none_or(|a| a.is_empty() || a.iter().any(|x| x == "vote")),
        };

        if vote_reachable {
            let allowed_votes_empty = match obj.get("allowed-votes") {
                None => true,
                Some(v) => v.as_array().is_none_or(|a| a.is_empty()),
            };
            if allowed_votes_empty {
                anyhow::bail!(
                    "safe-outputs.update-pr enables the 'vote' operation but has no \
                     'allowed-votes' list. This would reject all votes at Stage 3. \
                     Either restrict 'allowed-operations' to exclude 'vote', or add an \
                     explicit 'allowed-votes' list:\n\n  \
                     safe-outputs:\n    update-pr:\n      allowed-votes:\n        \
                     - approve-with-suggestions\n        - wait-for-author\n\n\
                     Valid votes: approve, approve-with-suggestions, reject, \
                     wait-for-author, reset\n"
                );
            }
        }
        // If the value is a scalar (e.g. `update-pr: true`) we don't error here —
        // the config will default to empty allowed-votes, which is safe (vote always rejected).
    }
    Ok(())
}

/// Validate that resolve-pr-thread has a required `allowed-statuses` field when configured.
///
/// An empty or missing `allowed-statuses` list would let agents set any thread status,
/// including "fixed" or "wontFix" on security-critical review threads. Operators must
/// explicitly opt in to each allowed status transition.
pub fn validate_resolve_pr_thread_statuses(front_matter: &FrontMatter) -> Result<()> {
    if let Some(config_value) = front_matter.safe_outputs.get("resolve-pr-thread") {
        if let Some(obj) = config_value.as_object() {
            let allowed_statuses = obj.get("allowed-statuses");
            let is_empty = match allowed_statuses {
                None => true,
                Some(v) => v.as_array().is_none_or(|a| a.is_empty()),
            };
            if is_empty {
                anyhow::bail!(
                    "safe-outputs.resolve-pr-thread requires a non-empty \
                     'allowed-statuses' list to prevent agents from manipulating thread \
                     statuses without explicit operator consent. Example:\n\n  \
                     safe-outputs:\n    resolve-pr-thread:\n      allowed-statuses:\n\
                     \x20       - fixed\n\n\
                     Valid statuses: active, fixed, wont-fix, closed, by-design\n"
                );
            }
        } else {
            anyhow::bail!(
                "safe-outputs.resolve-pr-thread must be a configuration object \
                 with an 'allowed-statuses' list. Example:\n\n  \
                 safe-outputs:\n    resolve-pr-thread:\n      allowed-statuses:\n\
                 \x20       - fixed\n"
            );
        }
    }
    Ok(())
}

/// Validate that every key under `safe-outputs:` is a known tool name.
///
/// Unknown keys (typos, stale renamed tools, debug-only tools used in the
/// regular safe-output surface) used to be silently dropped with a warning
/// in `generate_enabled_tools_args`, which made user-visible failures hide
/// as "the tool just didn't run" at Stage 1. This validator promotes the
/// warning to a hard compile error and points at candidates that share the
/// typo's first hyphen-segment so users can spot the rename.
///
/// Special-cases preserved as warnings (with `continue` in
/// `generate_enabled_tools_args`):
///
/// * `memory` — migrated to `tools: cache-memory:`. Surfaces as a warning
///   in `generate_enabled_tools_args` for back-compat; this validator
///   skips it so the migration path stays soft.
///
/// `DEBUG_ONLY_TOOLS` keys are independently rejected by
/// `validate_ado_aw_debug_config` with a more specific error message;
/// this validator skips them so the operator gets that better message.
pub fn validate_safe_outputs_keys(front_matter: &FrontMatter) -> Result<()> {
    use crate::safeoutputs::{ALL_KNOWN_SAFE_OUTPUTS, DEBUG_ONLY_TOOLS, NON_MCP_SAFE_OUTPUT_KEYS};

    let mut unknown: Vec<(String, Vec<&'static str>)> = Vec::new();
    let mut invalid_names: Vec<String> = Vec::new();

    for key in front_matter.safe_outputs.keys() {
        if !validate::is_safe_tool_name(key) {
            invalid_names.push(key.clone());
            continue;
        }
        if NON_MCP_SAFE_OUTPUT_KEYS.contains(&key.as_str()) {
            continue;
        }
        // `memory` is a known migration path — left as a warning in
        // generate_enabled_tools_args. Don't promote it to an error.
        if key == "memory" {
            continue;
        }
        // Debug-only tools get a more specific error from
        // validate_ado_aw_debug_config, so skip them here.
        if DEBUG_ONLY_TOOLS.contains(&key.as_str()) {
            continue;
        }
        if !ALL_KNOWN_SAFE_OUTPUTS.contains(&key.as_str()) {
            let related = related_safe_output_names(key);
            unknown.push((key.clone(), related));
        }
    }

    if !invalid_names.is_empty() {
        invalid_names.sort();
        let list = invalid_names
            .iter()
            .map(|n| format!("  - {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        anyhow::bail!(
            "safe-outputs contains tool name(s) with invalid characters:\n{list}\n\n\
             Tool names must contain only ASCII letters, digits, and hyphens. Example:\n\n  \
             safe-outputs:\n    create-work-item: {{}}\n",
        );
    }

    if !unknown.is_empty() {
        // Stable order for deterministic error messages.
        unknown.sort_by(|a, b| a.0.cmp(&b.0));
        let mut msg = String::from("safe-outputs contains unrecognised tool name(s):\n");
        for (name, related) in &unknown {
            if related.is_empty() {
                msg.push_str(&format!("  - {name}\n"));
            } else {
                msg.push_str(&format!(
                    "  - {name} (similar known tools: {})\n",
                    related.join(", ")
                ));
            }
        }
        msg.push_str(
            "\nValid safe-output keys are listed in docs/safe-outputs.md. \
             Each key must match exactly the kebab-case name registered by a \
             tool in src/safeoutputs/ (e.g. `create-pull-request`, not \
             `create-pr`).",
        );
        anyhow::bail!("{}", msg);
    }

    Ok(())
}

/// Return all known safe-output names that share `key`'s first
/// hyphen-separated segment (e.g. `create-pr` → all `create-*` tools).
/// If no candidate shares the head, returns an empty vec — better to give
/// no suggestion than a misleading one (`update-pr` for `create-pr`).
fn related_safe_output_names(key: &str) -> Vec<&'static str> {
    use crate::safeoutputs::ALL_KNOWN_SAFE_OUTPUTS;

    let head = key.split('-').next().unwrap_or_default();
    if head.is_empty() {
        return Vec::new();
    }
    let mut matches: Vec<&'static str> = ALL_KNOWN_SAFE_OUTPUTS
        .iter()
        .copied()
        .filter(|name| name.split('-').next() == Some(head))
        .collect();
    matches.sort();
    matches
}

fn nonempty_vec<T: Clone>(v: &[T]) -> Option<Vec<T>> {
    if v.is_empty() { None } else { Some(v.to_vec()) }
}

/// Returns `Some(BTreeMap from m)` when `m` is non-empty, otherwise `None`.
///
/// Converts a `HashMap` source to a `BTreeMap` so JSON serialization is
/// deterministic (keys are emitted in sorted order).
fn nonempty_map<K, V>(m: &HashMap<K, V>) -> Option<std::collections::BTreeMap<K, V>>
where
    K: Clone + Eq + std::hash::Hash + Ord,
    V: Clone,
{
    if m.is_empty() {
        None
    } else {
        Some(m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
    }
}

/// Validate a container-based MCP entry and emit any warnings.
fn validate_stdio_mcp(name: &str, container: &str, opts: &crate::compile::types::McpOptions) {
    for w in validate::validate_container_image(container, name) {
        eprintln!("{}", w);
    }
    for mount in &opts.mounts {
        for w in validate::validate_mount_source(mount, name) {
            eprintln!("{}", w);
        }
    }
    for w in validate::validate_docker_args(&opts.args, name) {
        eprintln!("{}", w);
    }
    for w in validate::warn_potential_secrets(name, &opts.env, &opts.headers) {
        eprintln!("{}", w);
    }
}

/// Build a stdio `McpgServerConfig` from a container-based MCP options block.
fn build_stdio_mcpg_server(
    container: &str,
    opts: &crate::compile::types::McpOptions,
) -> McpgServerConfig {
    McpgServerConfig {
        server_type: "stdio".to_string(),
        container: Some(container.to_string()),
        entrypoint: opts.entrypoint.clone(),
        entrypoint_args: nonempty_vec(&opts.entrypoint_args),
        mounts: nonempty_vec(&opts.mounts),
        args: nonempty_vec(&opts.args),
        url: None,
        headers: None,
        env: nonempty_map(&opts.env),
        tools: nonempty_vec(&opts.allowed),
    }
}

/// Build an HTTP `McpgServerConfig` from a URL-based MCP options block.
fn build_http_mcpg_server(url: &str, opts: &crate::compile::types::McpOptions) -> McpgServerConfig {
    McpgServerConfig {
        server_type: "http".to_string(),
        container: None,
        entrypoint: None,
        entrypoint_args: None,
        mounts: None,
        args: None,
        url: Some(url.to_string()),
        headers: nonempty_map(&opts.headers),
        env: None,
        tools: nonempty_vec(&opts.allowed),
    }
}

/// Validate and insert a single user-defined MCP server into `servers`.
///
/// Returns `Ok(())` on success. Returns `Err` for invalid server names.
/// Silently skips reserved names, disabled entries, and unconfigured entries.
fn try_add_user_mcp(
    name: &str,
    config: &McpConfig,
    servers: &mut std::collections::BTreeMap<String, McpgServerConfig>,
) -> Result<()> {
    // Prevent user-defined MCPs from overwriting the reserved safeoutputs backend
    if name.eq_ignore_ascii_case("safeoutputs") {
        log::warn!(
            "MCP name 'safeoutputs' is reserved for the safe outputs HTTP backend — skipping"
        );
        return Ok(());
    }

    // Validate server name for URL safety — names are embedded in MCPG routed
    // endpoints (/mcp/{name}) and must be safe URL path segments.
    // Leading dots are rejected to prevent path normalization issues (e.g., ".." → parent).
    if name.is_empty()
        || name.starts_with('.')
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
    {
        anyhow::bail!(
            "MCP server name '{}' is invalid — must be non-empty, not start with '.', and contain only ASCII alphanumerics, hyphens, underscores, and dots",
            name
        );
    }

    // Skip if already auto-configured by an extension (e.g., tools.azure-devops)
    if servers.contains_key(name) {
        return Ok(());
    }

    let opts = match config {
        McpConfig::Enabled(false) => return Ok(()),
        McpConfig::Enabled(true) => {
            log::warn!("MCP '{}' has no container or url — skipping", name);
            return Ok(());
        }
        McpConfig::WithOptions(opts) => {
            if !opts.enabled.unwrap_or(true) {
                return Ok(());
            }
            opts
        }
    };

    if opts.container.is_some() && opts.url.is_some() {
        log::warn!(
            "MCP '{}': both 'container' and 'url' are set — using 'container' (stdio). \
            Remove 'url' to silence this warning.",
            name
        );
    }

    if let Some(container) = &opts.container {
        validate_stdio_mcp(name, container, opts);
        servers.insert(name.to_string(), build_stdio_mcpg_server(container, opts));
    } else if let Some(url) = &opts.url {
        // HTTP-based MCP (remote server)
        for w in validate::validate_mcp_url(url, name) {
            eprintln!("{}", w);
        }
        for w in validate::warn_potential_secrets(name, &HashMap::new(), &opts.headers) {
            eprintln!("{}", w);
        }
        if !opts.env.is_empty() {
            eprintln!(
                "Warning: MCP '{}': env vars are not supported for HTTP MCPs — they will be ignored. \
                Use headers for authentication instead.",
                name
            );
        }
        servers.insert(name.to_string(), build_http_mcpg_server(url, opts));
    } else {
        log::warn!("MCP '{}' has no container or url — skipping", name);
    }

    Ok(())
}

/// Generate MCPG configuration from front matter.
///
/// Converts the front matter `mcp-servers` definitions into MCPG-compatible JSON.
/// SafeOutputs is always included as an HTTP backend. Extension-contributed MCPG
/// entries (e.g., azure-devops) are included via the `extensions` parameter.
pub fn generate_mcpg_config(
    front_matter: &FrontMatter,
    extension_declarations: &[Declarations],
) -> Result<McpgConfig> {
    let mut mcp_servers = std::collections::BTreeMap::new();

    // Add extension-contributed MCPG server entries (safeoutputs, azure-devops, etc.)
    for decl in extension_declarations {
        for (name, config) in &decl.mcpg_servers {
            mcp_servers.insert(name.clone(), config.clone());
        }
    }

    for (name, config) in &front_matter.mcp_servers {
        try_add_user_mcp(name, config, &mut mcp_servers)?;
    }

    Ok(McpgConfig {
        mcp_servers,
        gateway: McpgGatewayConfig {
            port: MCPG_PORT,
            domain: MCPG_DOMAIN.to_string(),
            api_key: "${MCP_GATEWAY_API_KEY}".to_string(),
            payload_dir: "/tmp/gh-aw/mcp-payloads".to_string(),
        },
    })
}

/// Generate additional `-e` flags for the MCPG Docker run command.
///
/// Two sources of env flags:
/// 1. **Extension pipeline var mappings** — extensions declare `required_pipeline_vars()`
///    which map container env vars to pipeline variables (typically secrets).
///    These become `-e CONTAINER_VAR="$PIPELINE_VAR"` flags referencing bash vars
///    (the companion `generate_mcpg_step_env` provides the ADO `env:` mapping).
/// 2. **User-configured MCP passthrough** — front matter `mcp-servers:` entries with
///    `env: { VAR: "" }` become bare `-e VAR` flags (MCPG passthrough from host env).
///
/// Returns flags formatted for inline insertion in the `docker run` command.
pub fn generate_mcpg_docker_env(
    front_matter: &FrontMatter,
    extension_declarations: &[Declarations],
) -> String {
    let mut env_flags: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    // 1. Extension pipeline var mappings (e.g., AZURE_DEVOPS_EXT_PAT -> SC_READ_TOKEN)
    for decl in extension_declarations {
        for mapping in &decl.pipeline_env {
            if seen.contains(&mapping.container_var) {
                continue;
            }
            env_flags.push(format!(
                "-e {}=\"${}\"",
                mapping.container_var, mapping.pipeline_var
            ));
            seen.insert(mapping.container_var.clone());
        }
    }

    // 2. User-configured MCP passthrough env vars (empty value = passthrough from host)
    for (mcp_name, config) in &front_matter.mcp_servers {
        let opts = match config {
            McpConfig::WithOptions(opts) if opts.enabled.unwrap_or(true) => opts,
            _ => continue,
        };

        if opts.container.is_none() {
            continue;
        }

        for (var_name, var_value) in &opts.env {
            if !validate::is_valid_env_var_name(var_name) {
                log::warn!(
                    "MCP '{}': skipping invalid env var name '{}' — must match [A-Za-z_][A-Za-z0-9_]*",
                    mcp_name,
                    var_name
                );
                continue;
            }
            if seen.contains(var_name) {
                continue;
            }
            if var_value.is_empty() {
                env_flags.push(format!("-e {}", var_name));
                seen.insert(var_name.clone());
            }
        }
    }

    env_flags.sort();
    if env_flags.is_empty() {
        "\\".to_string()
    } else {
        let flags = env_flags.join(" \\\n");
        format!("{} \\", flags)
    }
}

/// Generate the ADO step-level `env:` block for the MCPG start step.
///
/// ADO secret variables (set via `##vso[task.setvariable;issecret=true]`) must
/// be explicitly mapped via the step's `env:` block to be available as bash
/// environment variables. This function collects all pipeline variable mappings
/// from extensions and generates the corresponding `env:` entries.
///
/// Returns YAML `env:` entries (e.g., `SC_READ_TOKEN: $(SC_READ_TOKEN)`),
/// or an empty string if no mappings are needed.
pub fn generate_mcpg_step_env(extension_declarations: &[Declarations]) -> String {
    let mut entries: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    for decl in extension_declarations {
        for mapping in &decl.pipeline_env {
            if seen.contains(&mapping.pipeline_var) {
                continue;
            }
            entries.push(format!(
                "{}: $({})",
                mapping.pipeline_var, mapping.pipeline_var
            ));
            seen.insert(mapping.pipeline_var.clone());
        }
    }

    if entries.is_empty() {
        return String::new();
    }

    // Return full `env:` block so the template marker can be cleanly omitted when empty
    let indented = entries
        .iter()
        .map(|e| format!("  {}", e))
        .collect::<Vec<_>>()
        .join("\n");
    format!("env:\n{}", indented)
}

// ==================== Domain allowlist ====================

/// Generate the allowed domains list for AWF network isolation.
///
/// This generates a comma-separated list of domain patterns for AWF's
/// `--allow-domains` flag. The list includes:
/// 1. Core Azure DevOps/GitHub endpoints
/// 2. MCP-specific endpoints for each enabled MCP
/// 3. User-specified additional hosts from network.allowed
pub fn generate_allowed_domains(
    front_matter: &FrontMatter,
    extensions: &[super::extensions::Extension],
    extension_declarations: &[Declarations],
) -> Result<String> {
    // Collect enabled MCP names (user-defined MCPs, not first-party tools)
    let enabled_mcps: Vec<String> = front_matter
        .mcp_servers
        .iter()
        .filter_map(|(name, config)| {
            let is_enabled = match config {
                McpConfig::Enabled(enabled) => *enabled,
                McpConfig::WithOptions(_) => true,
            };
            if is_enabled { Some(name.clone()) } else { None }
        })
        .collect();

    // Get user-specified hosts
    let user_hosts: Vec<String> = front_matter
        .network
        .as_ref()
        .map(|n| n.allowed.clone())
        .unwrap_or_default();

    // Generate the allowlist by combining core + MCP + extension + user hosts
    let mut hosts: HashSet<String> = HashSet::new();

    // Add core hosts
    for host in CORE_ALLOWED_HOSTS {
        hosts.insert((*host).to_string());
    }

    // Add host.docker.internal — required for the AWF container to reach
    // MCPG and SafeOutputs on the host.
    hosts.insert("host.docker.internal".to_string());

    // Add MCP-specific hosts (user-defined MCPs via mcp_required_hosts lookup)
    for mcp in &enabled_mcps {
        for host in mcp_required_hosts(mcp) {
            hosts.insert((*host).to_string());
        }
    }

    // Add extension-declared hosts (runtimes + first-party tools).
    // Extensions may return ecosystem identifiers (e.g., "lean") which are
    // expanded to their domain lists, or raw domain names.
    for (ext, decl) in extensions.iter().zip(extension_declarations.iter()) {
        for host in &decl.network_hosts {
            if is_ecosystem_identifier(host) {
                let domains = get_ecosystem_domains(host);
                if domains.is_empty() {
                    eprintln!(
                        "warning: extension '{}' requires unknown ecosystem '{}'; \
                         no domains added",
                        ext.name(),
                        host
                    );
                }
                for domain in domains {
                    hosts.insert(domain);
                }
            } else {
                hosts.insert(host.clone());
            }
        }
    }

    // Add engine-required hosts (e.g., GHES/GHEC api-target hostname).
    // The engine resolves its config and returns additional hosts that AWF must allow.
    let engine = crate::engine::get_engine(front_matter.engine.engine_id())?;
    for host in engine.required_hosts(&front_matter.engine) {
        hosts.insert(host);
    }

    // Add user-specified hosts (validated against DNS-safe characters)
    // Entries may be ecosystem identifiers (e.g., "python", "rust") which
    // expand to their domain lists, or raw domain names.
    for host in &user_hosts {
        if is_ecosystem_identifier(host) {
            let domains = get_ecosystem_domains(host);
            if domains.is_empty() && !is_known_ecosystem(host) {
                eprintln!(
                    "warning: network.allowed contains unknown ecosystem identifier '{}'. \
                     Known ecosystems: python, rust, node, go, java, etc. \
                     If this is a domain name, it should contain a dot.",
                    host
                );
            }
            for domain in domains {
                hosts.insert(domain);
            }
        } else {
            validate::validate_dns_domain(host)?;
            hosts.insert(host.clone());
        }
    }

    // Remove blocked hosts (supports both ecosystem identifiers and raw domains)
    let blocked_hosts: Vec<String> = front_matter
        .network
        .as_ref()
        .map(|n| n.blocked.clone())
        .unwrap_or_default();
    for blocked in &blocked_hosts {
        if is_ecosystem_identifier(blocked) {
            for domain in get_ecosystem_domains(blocked) {
                hosts.remove(&domain);
            }
        } else {
            hosts.remove(blocked);
        }
    }

    // Sort for deterministic output
    let mut allowlist: Vec<String> = hosts.into_iter().collect();
    allowlist.sort();

    // Format as comma-separated list for AWF --allow-domains
    Ok(allowlist.join(","))
}

/// Generate AWF `--mount` flags from extension-declared volume mounts.
///
/// Collects `required_awf_mounts()` from all extensions and formats them
/// as `--mount "spec"` CLI flags for the AWF invocation.
///
/// Each mount spec is rendered using its [`Display`][std::fmt::Display] impl
/// (Docker bind-mount format: `host_path:container_path[:mode]`).
///
/// When no extensions require mounts, returns `\` (a bare bash continuation
/// marker) so the surrounding `\`-continuation chain is preserved. When
/// mounts are present, each flag occupies its own line
/// (`--mount "spec" \`).
pub fn generate_awf_mounts(
    extensions: &[super::extensions::Extension],
    extension_declarations: &[Declarations],
) -> String {
    let mounts: Vec<super::extensions::AwfMount> = extensions
        .iter()
        .zip(extension_declarations.iter())
        .flat_map(|(_ext, decl)| decl.awf_mounts.clone())
        .collect();

    // When the always-on AzureCli extension is enabled, append a
    // pipeline-variable reference that expands at pipeline time to
    // either `--mount /opt/az:/opt/az:ro --mount /usr/bin/az:/usr/bin/az:ro`
    // (when the runner has azure-cli installed) or to nothing (when it
    // doesn't). The detection + setvariable happens in
    // `AzureCliExtension::declarations`. This avoids static bind-mounts
    // that would crash `docker run` on 1ES self-hosted runners without
    // azure-cli pre-installed.
    let inject_az_var = extensions
        .iter()
        .any(|ext| matches!(ext, super::extensions::Extension::AzureCli(_)));

    if mounts.is_empty() && !inject_az_var {
        return "\\".to_string();
    }

    let mut lines: Vec<String> = mounts
        .iter()
        .map(|m| format!("--mount \"{}\" \\", m))
        .collect();
    if inject_az_var {
        // Unquoted on purpose: bash word-splits the pipeline-var value
        // into separate `--mount <spec>` tokens. The value contains only
        // path chars + `:` + spaces, no shell metachars.
        lines.push("$(AW_AZ_MOUNTS) \\".to_string());
    }
    lines.join("\n")
}

/// Generates a dedicated pipeline step that writes a `GITHUB_PATH` file
/// containing directories collected from extension declarations.
///
/// AWF reads the `$GITHUB_PATH` environment variable (a path to a file) at
/// startup and merges its entries into the chroot PATH. This mechanism was
/// designed for GitHub Actions `setup-*` actions but works identically on
/// ADO when we compose the file ourselves.
///
/// The generated step uses `##vso[task.setvariable]` to set `GITHUB_PATH`
/// as a pipeline variable visible to subsequent steps (including the AWF
/// invocation step that runs under `sudo`). This bypasses the `sudo`
/// `secure_path` reset that strips custom PATH entries.
///
/// When no extensions declare path prepends, returns an empty string and
/// the step is omitted from the pipeline.
pub fn generate_awf_path_step(awf_paths: &[String]) -> String {
    if awf_paths.is_empty() {
        return String::new();
    }

    let path_lines = awf_paths
        .iter()
        .map(|p| format!("    {p}"))
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        "\
- bash: |
    AWF_PATH_FILE=\"/tmp/awf-tools/ado-path-entries\"
    cat > \"$AWF_PATH_FILE\" << AWF_PATH_EOF
{path_lines}
    AWF_PATH_EOF
    echo \"##vso[task.setvariable variable=GITHUB_PATH]$AWF_PATH_FILE\"
  displayName: \"Generate GITHUB_PATH file\""
    )
}

/// Generates the `env:` block entry that passes `GITHUB_PATH` to the AWF
/// invocation step.
///
/// ADO pipeline variables set via `##vso[task.setvariable]` are auto-mapped
/// as environment variables in subsequent steps, but we explicitly pass
/// `GITHUB_PATH` via the `env:` block for clarity and robustness.
///
/// When no path prepends exist, returns an empty string.
pub fn generate_awf_path_env(has_awf_paths: bool) -> String {
    if !has_awf_paths {
        return String::new();
    }

    "GITHUB_PATH: $(GITHUB_PATH)".to_string()
}

/// Collects path prepends from all extension declarations into a single `Vec`.
pub fn collect_awf_path_prepends(extension_declarations: &[Declarations]) -> Vec<String> {
    extension_declarations
        .iter()
        .flat_map(|decl| decl.awf_path_prepends.clone())
        .collect()
}

/// Collects agent env vars from all extension declarations, validates keys against
/// `BLOCKED_ENV_KEYS`, deduplicates (bails on collision), and formats them
/// as YAML `KEY: "value"` lines for injection into the `{{ engine_env }}` block.
///
/// Returns an empty string if no extensions declare env vars.
pub fn collect_agent_env_vars(
    extensions: &[super::extensions::Extension],
    extension_declarations: &[Declarations],
) -> anyhow::Result<String> {
    use crate::engine::BLOCKED_ENV_KEYS;
    use crate::validate;
    use std::collections::HashSet;

    let mut lines = Vec::new();
    let mut seen_keys = HashSet::new();

    for (ext, decl) in extensions.iter().zip(extension_declarations.iter()) {
        for (key, value) in &decl.agent_env_vars {
            // Deduplicate: bail on collision
            if !seen_keys.insert(key.clone()) {
                anyhow::bail!(
                    "Extension '{}' declares agent env var '{}' which was already declared \
                     by a previous extension. Each env var key must be unique.",
                    ext.name(),
                    key,
                );
            }

            // Validate key is not blocked
            if BLOCKED_ENV_KEYS
                .iter()
                .any(|blocked| key.eq_ignore_ascii_case(blocked))
            {
                anyhow::bail!(
                    "Extension '{}' declares agent env var '{}' which conflicts with a \
                     compiler-controlled environment variable.",
                    ext.name(),
                    key,
                );
            }

            // Validate key format
            if !validate::is_valid_env_var_name(key) {
                anyhow::bail!(
                    "Extension '{}' declares agent env var '{}' with invalid key format. \
                     Keys must contain only ASCII alphanumerics and underscores.",
                    ext.name(),
                    key,
                );
            }

            // Validate value for injection (defence in depth — covers ADO expressions,
            // pipeline commands, template markers, and newlines)
            validate::reject_pipeline_injection(value, &format!("agent env var '{key}'"))?;

            if value.contains('"') || value.contains('\'') {
                anyhow::bail!(
                    "Extension '{}' agent env var '{}' value contains a quote character \
                     which would produce malformed YAML or bash syntax.",
                    ext.name(),
                    key,
                );
            }

            lines.push(format!("{key}: \"{value}\""));
        }
    }

    Ok(lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::extensions::{
        CompileContext, CompilerExtension, Declarations, Extension, collect_extensions,
    };
    use crate::compile::types::{McpConfig, McpOptions, OnConfig, Repository};
    use std::collections::HashMap;

    /// Helper: create a minimal FrontMatter by parsing YAML
    fn minimal_front_matter() -> FrontMatter {
        let (fm, _) = parse_markdown("---\nname: test-agent\ndescription: test\n---\n").unwrap();
        fm
    }

    fn extension_declarations(extensions: &[Extension], fm: &FrontMatter) -> Vec<Declarations> {
        let ctx = CompileContext::for_test(fm);
        extension_declarations_with_ctx(extensions, &ctx)
    }

    fn extension_declarations_with_ctx(
        extensions: &[Extension],
        ctx: &CompileContext,
    ) -> Vec<Declarations> {
        try_extension_declarations_with_ctx(extensions, ctx).unwrap()
    }

    fn try_extension_declarations_with_ctx(
        extensions: &[Extension],
        ctx: &CompileContext,
    ) -> Result<Vec<Declarations>> {
        extensions.iter().map(|ext| ext.declarations(ctx)).collect()
    }

    fn collect_exts_and_decls(fm: &FrontMatter) -> (Vec<Extension>, Vec<Declarations>) {
        let extensions = collect_extensions(fm);
        let declarations = extension_declarations(&extensions, fm);
        (extensions, declarations)
    }

    fn collect_exts_and_decls_with_org(
        fm: &FrontMatter,
        org: &str,
    ) -> (Vec<Extension>, Vec<Declarations>) {
        let extensions = collect_extensions(fm);
        let ctx = CompileContext::for_test_with_org(fm, org);
        let declarations = extension_declarations_with_ctx(&extensions, &ctx);
        (extensions, declarations)
    }

    fn engine_args_for(fm: &FrontMatter) -> Result<String> {
        let (_extensions, declarations) = collect_exts_and_decls(fm);
        CompileContext::for_test(fm).engine.args(fm, &declarations)
    }

    // ─── generate_agent_job_variables ─────────────────────────────────

    // ─── normalize_yaml ───────────────────────────────────────────────────────

    #[test]
    fn normalize_yaml_round_trips_a_simple_mapping() {
        let input = "name: foo\nvalue: bar\n";
        let out = normalize_yaml(input).unwrap();
        // The output is whatever serde_yaml chooses to emit; the contract is
        // that re-parsing produces a structurally equal Value.
        let v1: serde_yaml::Value = serde_yaml::from_str(input).unwrap();
        let v2: serde_yaml::Value = serde_yaml::from_str(&out).unwrap();
        assert_eq!(v1, v2);
    }

    #[test]
    fn normalize_yaml_preserves_leading_comment_header() {
        let input = "# Auto-generated header\n# @ado-aw source=foo\n\nname: bar\n";
        let out = normalize_yaml(input).unwrap();
        assert!(
            out.starts_with("# Auto-generated header\n# @ado-aw source=foo\n\n"),
            "leading comment header must round-trip verbatim, got: {out:?}"
        );
        // Parse-equivalent on the body
        let body_start = out.find("name:").unwrap();
        let parsed: serde_yaml::Value = serde_yaml::from_str(&out[body_start..]).unwrap();
        let expected: serde_yaml::Value = serde_yaml::from_str("name: bar\n").unwrap();
        assert_eq!(parsed, expected);
    }

    #[test]
    fn normalize_yaml_drops_inline_yaml_comments_by_design() {
        // Inline comments between mapping keys are dropped by serde_yaml on
        // round-trip. This is the documented contract — the prep PR exists
        // precisely so this loss happens now (cosmetic) rather than during
        // the IR PR (mixed with structural changes).
        let input = "# leading\nname: foo\n# inline comment\nvalue: bar\n";
        let out = normalize_yaml(input).unwrap();
        assert!(out.starts_with("# leading\n"), "leader preserved");
        assert!(
            !out.contains("# inline comment"),
            "inline YAML comments are dropped by design, got: {out:?}"
        );
    }

    #[test]
    fn normalize_yaml_preserves_bash_comments_inside_literal_blocks() {
        // A `#` line inside a `|` literal block is string content from
        // the YAML parser's perspective, so it survives round-trip.
        let input = "name: foo\nscript: |\n  # bash comment, not YAML\n  echo hi\n";
        let out = normalize_yaml(input).unwrap();
        assert!(
            out.contains("# bash comment, not YAML"),
            "bash comments inside literal scalars must survive, got: {out:?}"
        );
    }

    #[test]
    fn normalize_yaml_passes_through_empty_body() {
        // Header-only input (no body) is returned verbatim — there is
        // nothing to round-trip.
        let input = "# only a comment\n\n";
        let out = normalize_yaml(input).unwrap();
        assert_eq!(out, input);
    }

    #[test]
    fn normalize_yaml_fails_on_invalid_yaml() {
        let input = "name: [unterminated\n";
        let err = normalize_yaml(input).unwrap_err();
        assert!(
            format!("{err:#}").contains("normalize_yaml"),
            "error must be wrapped with the helper's context, got: {err:#}"
        );
    }

    // ─── atomic_write ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn atomic_write_creates_new_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        atomic_write(&path, "hello\n").await.unwrap();
        let read = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(read, "hello\n");
    }

    #[tokio::test]
    async fn atomic_write_overwrites_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        tokio::fs::write(&path, "old contents").await.unwrap();
        atomic_write(&path, "new contents").await.unwrap();
        let read = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(read, "new contents");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn atomic_write_preserves_unix_mode() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        tokio::fs::write(&path, "old").await.unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
        atomic_write(&path, "new").await.unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(
            mode & 0o777,
            0o640,
            "expected mode 0o640, got {:o}",
            mode & 0o777
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn atomic_write_replaces_symlink_with_regular_file() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.txt");
        let link = dir.path().join("link.txt");
        tokio::fs::write(&target, "target").await.unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();

        atomic_write(&link, "via-link").await.unwrap();

        // Link is now a regular file; target is unchanged.
        let link_meta = std::fs::symlink_metadata(&link).unwrap();
        assert!(
            !link_meta.file_type().is_symlink(),
            "symlink should have been replaced"
        );
        assert_eq!(tokio::fs::read_to_string(&link).await.unwrap(), "via-link");
        assert_eq!(tokio::fs::read_to_string(&target).await.unwrap(), "target");
    }

    #[test]
    fn atomic_write_parent_derivation_handles_bare_filename() {
        // Regression test for the EXDEV bug: a bare filename (no
        // directory component) used to fall through to
        // `NamedTempFile::new()` which creates the tempfile in the
        // system temp dir, breaking persist() across filesystem
        // boundaries (e.g. tmpfs `/tmp` on Linux). Verify the
        // derivation logic now picks ".". Pure-logic test — no
        // filesystem I/O so it's parallel-safe.
        let bare = Path::new("agent.md");
        let parent = bare.parent().filter(|p| !p.as_os_str().is_empty());
        let parent_dir: &Path = parent.unwrap_or_else(|| Path::new("."));
        assert_eq!(parent_dir, Path::new("."));

        let with_dir = Path::new("subdir/agent.md");
        let parent = with_dir.parent().filter(|p| !p.as_os_str().is_empty());
        let parent_dir: &Path = parent.unwrap_or_else(|| Path::new("."));
        assert_eq!(parent_dir, Path::new("subdir"));

        let absolute = Path::new("/tmp/agent.md");
        let parent = absolute.parent().filter(|p| !p.as_os_str().is_empty());
        let parent_dir: &Path = parent.unwrap_or_else(|| Path::new("."));
        assert_eq!(parent_dir, Path::new("/tmp"));
    }

    // ─── parse_markdown_detailed ──────────────────────────────────────────────

    fn reconstruct(parsed: &ParsedSource) -> String {
        reconstruct_source(
            &parsed.leading_whitespace,
            &parsed.front_matter_mapping,
            &parsed.body_raw,
        )
        .unwrap()
    }

    #[test]
    fn parse_markdown_detailed_preserves_body_byte_for_byte() {
        // Case 1: trailing newline.
        let original = "---\nname: x\ndescription: y\n---\nbody line\n";
        let parsed = parse_markdown_detailed(original).unwrap();
        assert_eq!(parsed.body_raw, "\nbody line\n");
        let reconstructed = reconstruct(&parsed);
        // No migrations ran, so the YAML round-trip is the only
        // structural change. The body region is byte-faithful.
        assert!(reconstructed.ends_with("---\nbody line\n"));

        // Case 2: no trailing newline at all.
        let original = "---\nname: x\ndescription: y\n---\nbody";
        let parsed = parse_markdown_detailed(original).unwrap();
        assert_eq!(parsed.body_raw, "\nbody");
        let reconstructed = reconstruct(&parsed);
        assert!(reconstructed.ends_with("---\nbody"));

        // Case 3: empty body, but trailing newline.
        let original = "---\nname: x\ndescription: y\n---\n";
        let parsed = parse_markdown_detailed(original).unwrap();
        assert_eq!(parsed.body_raw, "\n");
        let reconstructed = reconstruct(&parsed);
        assert!(reconstructed.ends_with("---\n"));

        // Case 4: blank lines between closing fence and content are
        // preserved as-is in body_raw.
        let original = "---\nname: x\ndescription: y\n---\n\n\n## heading\n\nbody.\n";
        let parsed = parse_markdown_detailed(original).unwrap();
        assert_eq!(parsed.body_raw, "\n\n\n## heading\n\nbody.\n");
    }

    #[test]
    fn parse_markdown_detailed_byte_faithful_when_no_codemod_runs() {
        // With the registry empty, parsing a healthy source and
        // reconstructing it should produce a byte-identical document
        // apart from serde_yaml's canonical formatting of the YAML
        // mapping. We assert the body region matches exactly.
        let original = "---\nname: x\ndescription: y\n---\n## body\n";
        let parsed = parse_markdown_detailed(original).unwrap();
        assert!(!parsed.codemods.changed());
        let reconstructed = reconstruct(&parsed);
        // Find the closing fence in both and compare the suffix.
        let orig_suffix = &original[original.find("\n---\n").unwrap()..];
        let recon_suffix = &reconstructed[reconstructed.find("\n---\n").unwrap()..];
        assert_eq!(
            orig_suffix, recon_suffix,
            "body region must be byte-identical"
        );
    }

    #[test]
    fn parse_markdown_detailed_preserves_leading_whitespace() {
        // Leading blank lines / spaces (e.g. from editor BOM-strippers
        // adding a blank line) must round-trip through reconstruction
        // so migration rewrites don't silently normalize them away.
        let original = "\n  \n---\nname: x\ndescription: y\n---\nbody\n";
        let parsed = parse_markdown_detailed(original).unwrap();
        assert_eq!(parsed.leading_whitespace, "\n  \n");
        let reconstructed = reconstruct(&parsed);
        assert!(
            reconstructed.starts_with("\n  \n---\n"),
            "expected leading whitespace preserved, got: {:?}",
            &reconstructed[..20.min(reconstructed.len())]
        );
        assert!(reconstructed.ends_with("---\nbody\n"));
    }

    #[test]
    fn parse_markdown_detailed_rejects_missing_open_fence() {
        let original = "name: x\ndescription: y\nbody\n";
        let err = parse_markdown_detailed(original).unwrap_err();
        assert!(
            format!("{}", err).contains("must start with YAML front matter"),
            "got: {}",
            err
        );
    }

    #[test]
    fn parse_markdown_detailed_rejects_non_mapping_top_level() {
        let original = "---\n- a\n- b\n---\nbody\n";
        let err = parse_markdown_detailed(original).unwrap_err();
        assert!(
            format!("{}", err).contains("must be a mapping"),
            "got: {}",
            err
        );
    }

    #[test]
    fn parse_markdown_detailed_rejects_missing_close_fence() {
        let original = "---\nname: x\nno-fence";
        let err = parse_markdown_detailed(original).unwrap_err();
        assert!(
            format!("{}", err).contains("Could not find closing"),
            "got: {}",
            err
        );
    }

    #[test]
    fn parse_markdown_detailed_records_source_hash() {
        let a = "---\nname: x\ndescription: y\n---\n";
        let b = "---\nname: x\ndescription: y\n---\nextra\n";
        let pa = parse_markdown_detailed(a).unwrap();
        let pb = parse_markdown_detailed(b).unwrap();
        assert_ne!(pa.source_sha256, pb.source_sha256);
        // Hashing is stable over re-parses of the same input.
        let pa2 = parse_markdown_detailed(a).unwrap();
        assert_eq!(pa.source_sha256, pa2.source_sha256);
    }

    // ─── compute_effective_workspace ─────────────────────────────────────────

    #[test]
    fn test_workspace_explicit_root() {
        let ws = compute_effective_workspace(&Some("root".to_string()), &[], "agent").unwrap();
        assert_eq!(ws, "root");
        assert_eq!(generate_working_directory(&ws), "$(Build.SourcesDirectory)");
    }

    #[test]
    fn test_workspace_explicit_repo_with_checkouts() {
        let checkouts = vec!["other-repo".to_string()];
        let ws =
            compute_effective_workspace(&Some("repo".to_string()), &checkouts, "agent").unwrap();
        assert_eq!(ws, "repo");
        assert_eq!(
            generate_working_directory(&ws),
            "$(Build.SourcesDirectory)/$(Build.Repository.Name)"
        );
    }

    #[test]
    fn test_workspace_explicit_self_alias_for_repo() {
        let checkouts = vec!["other-repo".to_string()];
        let ws =
            compute_effective_workspace(&Some("self".to_string()), &checkouts, "agent").unwrap();
        // 'self' is a synonym for 'repo' (the trigger repository).
        assert_eq!(ws, "repo");
        assert_eq!(
            generate_working_directory(&ws),
            "$(Build.SourcesDirectory)/$(Build.Repository.Name)"
        );
    }

    #[test]
    fn test_workspace_implicit_root_no_checkouts() {
        let ws = compute_effective_workspace(&None, &[], "agent").unwrap();
        assert_eq!(ws, "root");
    }

    #[test]
    fn test_workspace_implicit_repo_with_checkouts() {
        let checkouts = vec!["other-repo".to_string()];
        let ws = compute_effective_workspace(&None, &checkouts, "agent").unwrap();
        assert_eq!(ws, "repo");
    }

    #[test]
    fn test_workspace_explicit_repo_no_checkouts_still_returns_repo() {
        // Emits a warning but still returns "repo"
        let ws = compute_effective_workspace(&Some("repo".to_string()), &[], "agent").unwrap();
        assert_eq!(ws, "repo");
    }

    #[test]
    fn test_workspace_explicit_self_no_checkouts_still_returns_repo() {
        // 'self' takes the same code path as 'repo'; it should also warn
        // and still resolve to the repo subfolder.
        let ws = compute_effective_workspace(&Some("self".to_string()), &[], "agent").unwrap();
        assert_eq!(ws, "repo");
    }

    #[test]
    fn test_workspace_explicit_alias_with_traversal_fails() {
        let checkouts = vec!["../sibling".to_string()];
        let err = compute_effective_workspace(&Some("../sibling".to_string()), &checkouts, "agent")
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("not a safe path"), "msg: {msg}");
    }

    #[test]
    fn test_workspace_explicit_alias_with_slash_fails() {
        let checkouts = vec!["foo/bar".to_string()];
        let err = compute_effective_workspace(&Some("foo/bar".to_string()), &checkouts, "agent")
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("not a safe path"), "msg: {msg}");
    }

    #[test]
    fn test_workspace_explicit_alias_with_shell_metacharacters_fails() {
        let checkouts = vec!["evil`env|base64>creds`".to_string()];
        let err = compute_effective_workspace(
            &Some("evil`env|base64>creds`".to_string()),
            &checkouts,
            "agent",
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("not a safe path"), "msg: {msg}");
    }

    #[test]
    fn test_workspace_explicit_alias_resolves_to_repo_subdir() {
        let checkouts = vec!["exp23-a7-nw".to_string(), "another-repo".to_string()];
        let ws = compute_effective_workspace(&Some("exp23-a7-nw".to_string()), &checkouts, "agent")
            .unwrap();
        assert_eq!(
            generate_working_directory(&ws),
            "$(Build.SourcesDirectory)/exp23-a7-nw"
        );
    }

    #[test]
    fn test_workspace_explicit_alias_not_in_checkout_fails() {
        let checkouts = vec!["other-repo".to_string()];
        let err =
            compute_effective_workspace(&Some("exp23-a7-nw".to_string()), &checkouts, "agent")
                .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("exp23-a7-nw"), "msg: {msg}");
        assert!(msg.contains("does not match"), "msg: {msg}");
    }

    #[test]
    fn test_workspace_explicit_alias_no_checkouts_fails() {
        let err = compute_effective_workspace(&Some("exp23-a7-nw".to_string()), &[], "agent")
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("exp23-a7-nw"), "msg: {msg}");
        assert!(
            msg.contains("no additional repositories are checked out"),
            "msg: {msg}"
        );
    }

    // ─── validate_checkout_list ───────────────────────────────────────────────

    #[test]
    fn test_validate_checkout_list_empty_is_ok() {
        let result = validate_checkout_list(&[], &[]);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_checkout_list_valid_alias_passes() {
        let repos = vec![Repository {
            repository: "my-repo".to_string(),
            repo_type: "git".to_string(),
            name: "org/my-repo".to_string(),
            repo_ref: "refs/heads/main".to_string(),
        }];
        let checkout = vec!["my-repo".to_string()];
        let result = validate_checkout_list(&repos, &checkout);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_checkout_list_unknown_alias_fails() {
        let repos = vec![Repository {
            repository: "my-repo".to_string(),
            repo_type: "git".to_string(),
            name: "org/my-repo".to_string(),
            repo_ref: "refs/heads/main".to_string(),
        }];
        let checkout = vec!["unknown-alias".to_string()];
        let result = validate_checkout_list(&repos, &checkout);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("unknown-alias"));
    }

    #[test]
    fn test_validate_checkout_list_empty_checkout_of_nonempty_repos_ok() {
        let repos = vec![Repository {
            repository: "my-repo".to_string(),
            repo_type: "git".to_string(),
            name: "org/my-repo".to_string(),
            repo_ref: "refs/heads/main".to_string(),
        }];
        let result = validate_checkout_list(&repos, &[]);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_checkout_list_reserved_name_fails() {
        // A repo aliased "repo" would silently shadow `workspace: repo`, so
        // reject it at compile time.
        let repos = vec![Repository {
            repository: "repo".to_string(),
            repo_type: "git".to_string(),
            name: "org/repo".to_string(),
            repo_ref: "refs/heads/main".to_string(),
        }];
        let checkout = vec!["repo".to_string()];
        let err = validate_checkout_list(&repos, &checkout).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("reserved"), "msg: {msg}");
        assert!(msg.contains("'repo'"), "msg: {msg}");
    }

    // ─── validate_checkout_self_collision ────────────────────────────────────

    #[test]
    fn test_validate_self_collision_detects_match() {
        // Workspace repo's name last segment matches the self repo's name,
        // so both `checkout: self` and `checkout: my-repo` would land in
        // `s/my-repo`. Must error.
        let repos = vec![Repository {
            repository: "my-repo".to_string(),
            repo_type: "git".to_string(),
            name: "some-org/my-repo".to_string(),
            repo_ref: "refs/heads/main".to_string(),
        }];
        let checkout = vec!["my-repo".to_string()];
        let err = validate_checkout_self_collision(&repos, &checkout, Some("my-repo")).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("'my-repo'"), "msg: {msg}");
        assert!(msg.contains("same"), "msg: {msg}");
        assert!(msg.contains("'self'"), "msg: {msg}");
    }

    #[test]
    fn test_validate_self_collision_no_collision_passes() {
        // Different repo name → different `s/<name>` directory, no collision.
        let repos = vec![Repository {
            repository: "other".to_string(),
            repo_type: "git".to_string(),
            name: "some-org/other".to_string(),
            repo_ref: "refs/heads/main".to_string(),
        }];
        let checkout = vec!["other".to_string()];
        let result = validate_checkout_self_collision(&repos, &checkout, Some("my-repo"));
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_self_collision_case_insensitive() {
        // ADO is case-insensitive on Windows; treat differing-only-by-case
        // names as a collision so the pipeline doesn't break on one OS.
        let repos = vec![Repository {
            repository: "my-repo".to_string(),
            repo_type: "git".to_string(),
            name: "Some-Org/My-Repo".to_string(),
            repo_ref: "refs/heads/main".to_string(),
        }];
        let checkout = vec!["my-repo".to_string()];
        let err = validate_checkout_self_collision(&repos, &checkout, Some("my-repo")).unwrap_err();
        assert!(err.to_string().contains("same"));
    }

    #[test]
    fn test_validate_self_collision_no_self_name_skipped() {
        // No git remote / no inferred self name → can't detect, skip.
        let repos = vec![Repository {
            repository: "my-repo".to_string(),
            repo_type: "git".to_string(),
            name: "org/my-repo".to_string(),
            repo_ref: "refs/heads/main".to_string(),
        }];
        let checkout = vec!["my-repo".to_string()];
        let result = validate_checkout_self_collision(&repos, &checkout, None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_self_collision_empty_checkout_passes() {
        let repos = vec![Repository {
            repository: "my-repo".to_string(),
            repo_type: "git".to_string(),
            name: "org/my-repo".to_string(),
            repo_ref: "refs/heads/main".to_string(),
        }];
        let result = validate_checkout_self_collision(&repos, &[], Some("my-repo"));
        assert!(result.is_ok());
    }

    // ─── Engine::args (copilot params) ──────────────────────────────────────

    #[test]
    fn test_engine_args_bash_wildcard() {
        let mut fm = minimal_front_matter();
        fm.tools = Some(crate::compile::types::ToolsConfig {
            bash: Some(vec![":*".to_string()]),
            edit: None,
            cache_memory: None,
            azure_devops: None,
        });
        let params = engine_args_for(&fm).unwrap();
        assert!(
            params.contains("--allow-all-tools"),
            "wildcard bash should emit --allow-all-tools"
        );
        assert!(
            !params.contains("--allow-tool"),
            "no individual --allow-tool flags with --allow-all-tools"
        );
    }

    #[test]
    fn test_engine_args_bash_star_wildcard() {
        let mut fm = minimal_front_matter();
        fm.tools = Some(crate::compile::types::ToolsConfig {
            bash: Some(vec!["*".to_string()]),
            edit: None,
            cache_memory: None,
            azure_devops: None,
        });
        let params = engine_args_for(&fm).unwrap();
        assert!(
            params.contains("--allow-all-tools"),
            "\"*\" should behave same as \":*\""
        );
        assert!(
            !params.contains("--allow-tool"),
            "no individual --allow-tool flags with --allow-all-tools"
        );
    }

    #[test]
    fn test_engine_args_bash_disabled() {
        let mut fm = minimal_front_matter();
        fm.tools = Some(crate::compile::types::ToolsConfig {
            bash: Some(vec![]),
            edit: None,
            cache_memory: None,
            azure_devops: None,
        });
        let params = engine_args_for(&fm).unwrap();
        // User-disabled bash must not produce a general bash allow-tool
        // (shell(:*) / shell(*) / shell(bash)). Always-on extensions
        // (e.g. Azure CLI) legitimately inject their own narrow
        // shell(<cmd>) entries via `required_bash_commands()`; those are
        // expected and should not regress this test.
        assert!(!params.contains("shell(:*)"));
        assert!(!params.contains("shell(*)"));
        assert!(!params.contains("shell(bash)"));
        // Sanity-check: the always-on Azure CLI extension still injects
        // its bash requirement even when user bash is disabled — agents
        // must be able to call `az` regardless of the user's `bash:`
        // narrowing decisions.
        assert!(
            params.contains("shell(az)"),
            "always-on Azure CLI extension should still inject shell(az): {params}"
        );
    }

    #[test]
    fn test_engine_args_allow_all_paths_when_edit_enabled() {
        let fm = minimal_front_matter(); // edit defaults to true, bash defaults to wildcard
        let params = engine_args_for(&fm).unwrap();
        assert!(
            params.contains("--allow-all-paths"),
            "edit enabled (default) should emit --allow-all-paths"
        );
        assert!(
            params.contains("--allow-all-tools"),
            "default (no bash) should emit --allow-all-tools"
        );
        assert!(
            !params.contains("--allow-tool"),
            "no individual --allow-tool flags with --allow-all-tools"
        );
    }

    #[test]
    fn test_engine_args_no_allow_all_paths_when_edit_disabled() {
        let mut fm = minimal_front_matter();
        fm.tools = Some(crate::compile::types::ToolsConfig {
            bash: None,
            edit: Some(false),
            cache_memory: None,
            azure_devops: None,
        });
        let params = engine_args_for(&fm).unwrap();
        assert!(
            !params.contains("--allow-all-paths"),
            "edit disabled should NOT emit --allow-all-paths"
        );
        assert!(
            !params.contains("--allow-tool write"),
            "edit disabled should NOT emit --allow-tool write"
        );
    }

    #[test]
    fn test_engine_args_allow_all_tools_with_allow_all_paths() {
        let mut fm = minimal_front_matter();
        fm.tools = Some(crate::compile::types::ToolsConfig {
            bash: Some(vec![":*".to_string()]),
            edit: Some(true),
            cache_memory: None,
            azure_devops: None,
        });
        let params = engine_args_for(&fm).unwrap();
        assert!(
            params.contains("--allow-all-tools"),
            "wildcard bash should emit --allow-all-tools"
        );
        assert!(
            params.contains("--allow-all-paths"),
            "edit enabled should still emit --allow-all-paths"
        );
        assert!(
            !params.contains("--allow-tool"),
            "no individual --allow-tool flags"
        );
    }

    #[test]
    fn test_engine_args_lean_adds_bash_commands() {
        let mut fm = minimal_front_matter();
        fm.tools = Some(crate::compile::types::ToolsConfig {
            bash: Some(vec!["cat".to_string()]),
            edit: None,
            cache_memory: None,
            azure_devops: None,
        });
        fm.runtimes = Some(crate::compile::types::RuntimesConfig {
            lean: Some(crate::runtimes::lean::LeanRuntimeConfig::Enabled(true)),
            python: None,
            node: None,
            dotnet: None,
        });
        let params = engine_args_for(&fm).unwrap();
        assert!(
            params.contains("shell(lean)"),
            "lean command should be allowed"
        );
        assert!(
            params.contains("shell(lake)"),
            "lake command should be allowed"
        );
        assert!(
            params.contains("shell(elan)"),
            "elan command should be allowed"
        );
        // Explicit bash commands should still be present
        assert!(
            params.contains("shell(cat)"),
            "explicit commands should remain"
        );
    }

    #[test]
    fn test_engine_args_lean_with_unrestricted_bash() {
        let mut fm = minimal_front_matter();
        fm.tools = Some(crate::compile::types::ToolsConfig {
            bash: Some(vec![":*".to_string()]),
            edit: None,
            cache_memory: None,
            azure_devops: None,
        });
        fm.runtimes = Some(crate::compile::types::RuntimesConfig {
            lean: Some(crate::runtimes::lean::LeanRuntimeConfig::Enabled(true)),
            python: None,
            node: None,
            dotnet: None,
        });
        let params = engine_args_for(&fm).unwrap();
        assert!(
            params.contains("--allow-all-tools"),
            "wildcard should use --allow-all-tools"
        );
        // Should NOT add individual tool flags when --allow-all-tools is active
        assert!(
            !params.contains("--allow-tool"),
            "no individual tool flags with --allow-all-tools"
        );
    }

    #[test]
    fn test_engine_args_custom_mcp_no_mcp_flag() {
        let mut fm = minimal_front_matter();
        fm.mcp_servers.insert(
            "my-tool".to_string(),
            McpConfig::WithOptions(Box::new(McpOptions {
                container: Some("node:20-slim".to_string()),
                ..Default::default()
            })),
        );
        let params = engine_args_for(&fm).unwrap();
        assert!(
            !params.contains("--allow-tool my-tool"),
            "default (all-tools) mode should not emit individual --allow-tool for MCPs"
        );
    }

    #[test]
    fn test_engine_args_allow_tool_for_container_mcp() {
        let mut fm = minimal_front_matter();
        fm.tools = Some(crate::compile::types::ToolsConfig {
            bash: Some(vec!["cat".to_string()]),
            edit: None,
            cache_memory: None,
            azure_devops: None,
        });
        fm.mcp_servers.insert(
            "my-tool".to_string(),
            McpConfig::WithOptions(Box::new(McpOptions {
                container: Some("node:20-slim".to_string()),
                ..Default::default()
            })),
        );
        let params = engine_args_for(&fm).unwrap();
        assert!(
            params.contains("--allow-tool my-tool"),
            "container MCP should get --allow-tool"
        );
    }

    #[test]
    fn test_engine_args_allow_tool_for_url_mcp() {
        let mut fm = minimal_front_matter();
        fm.tools = Some(crate::compile::types::ToolsConfig {
            bash: Some(vec!["cat".to_string()]),
            edit: None,
            cache_memory: None,
            azure_devops: None,
        });
        fm.mcp_servers.insert(
            "remote-ado".to_string(),
            McpConfig::WithOptions(Box::new(McpOptions {
                url: Some("https://mcp.dev.azure.com/myorg".to_string()),
                ..Default::default()
            })),
        );
        let params = engine_args_for(&fm).unwrap();
        assert!(
            params.contains("--allow-tool remote-ado"),
            "URL MCP should get --allow-tool"
        );
    }

    #[test]
    fn test_engine_args_no_allow_tool_for_enabled_only_mcp() {
        let mut fm = minimal_front_matter();
        fm.mcp_servers
            .insert("my-tool".to_string(), McpConfig::Enabled(true));
        let params = engine_args_for(&fm).unwrap();
        assert!(
            !params.contains("--allow-tool my-tool"),
            "Enabled(true) with no container/url should not get --allow-tool"
        );
    }

    #[test]
    fn test_engine_args_allow_tool_mcps_sorted() {
        let mut fm = minimal_front_matter();
        fm.tools = Some(crate::compile::types::ToolsConfig {
            bash: Some(vec!["cat".to_string()]),
            edit: None,
            cache_memory: None,
            azure_devops: None,
        });
        fm.mcp_servers.insert(
            "z-tool".to_string(),
            McpConfig::WithOptions(Box::new(McpOptions {
                container: Some("alpine".to_string()),
                ..Default::default()
            })),
        );
        fm.mcp_servers.insert(
            "a-tool".to_string(),
            McpConfig::WithOptions(Box::new(McpOptions {
                container: Some("alpine".to_string()),
                ..Default::default()
            })),
        );
        let params = engine_args_for(&fm).unwrap();
        let a_pos = params
            .find("--allow-tool a-tool")
            .expect("a-tool should be present");
        let z_pos = params
            .find("--allow-tool z-tool")
            .expect("z-tool should be present");
        assert!(
            a_pos < z_pos,
            "MCPs should be sorted alphabetically: a-tool before z-tool"
        );
    }

    #[test]
    fn test_engine_args_builtin_mcp_no_mcp_flag() {
        let mut fm = minimal_front_matter();
        fm.mcp_servers
            .insert("ado".to_string(), McpConfig::Enabled(true));
        let params = engine_args_for(&fm).unwrap();
        // Copilot CLI has no built-in MCPs — all MCPs are handled via the MCP firewall
        assert!(!params.contains("--mcp ado"));
    }

    #[test]
    fn test_engine_args_no_max_timeout() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  model: claude-opus-4.5\n  timeout-minutes: 30\n---\n",
        )
        .unwrap();
        let params = engine_args_for(&fm).unwrap();
        assert!(
            !params.contains("--max-timeout"),
            "timeout-minutes should not be emitted as a CLI arg"
        );
    }

    #[test]
    fn test_engine_args_no_max_timeout_when_simple_engine() {
        let fm = minimal_front_matter();
        let params = engine_args_for(&fm).unwrap();
        assert!(!params.contains("--max-timeout"));
    }

    #[test]
    fn test_engine_args_max_timeout_zero_not_emitted() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nengine:\n  model: claude-opus-4.5\n  timeout-minutes: 0\n---\n",
        )
        .unwrap();
        let params = engine_args_for(&fm).unwrap();
        assert!(
            !params.contains("--max-timeout"),
            "timeout-minutes should not be emitted as a CLI arg"
        );
    }

    // ─── sanitize_filename ────────────────────────────────────────────────────

    // ─── sanitize_pipeline_agent_name ───────────────────────────────────────

    #[test]
    fn test_sanitize_pipeline_agent_name_removes_invalid_build_number_chars() {
        assert_eq!(
            sanitize_pipeline_agent_name(r#"Daily safe-output smoke: "noop" @nightly"#),
            "Daily safe-output smoke noop nightly"
        );
    }

    #[test]
    fn test_sanitize_pipeline_agent_name_trims_trailing_dot() {
        assert_eq!(sanitize_pipeline_agent_name("Agent name."), "Agent name");
    }

    #[test]
    fn test_sanitize_pipeline_agent_name_enforces_length_budget() {
        let input = "x".repeat(ADO_BUILD_NUMBER_MAX_LEN);
        let sanitized = sanitize_pipeline_agent_name(&input);
        assert_eq!(
            sanitized.chars().count(),
            ADO_BUILD_NUMBER_MAX_LEN - ADO_BUILD_ID_SUFFIX.len()
        );
    }

    #[test]
    fn test_sanitize_pipeline_agent_name_fallback_when_empty_after_sanitize() {
        assert_eq!(sanitize_pipeline_agent_name(":@?*"), "pipeline");
    }

    // ─── yaml_double_quoted ──────────────────────────────────────────────────

    // ─── generate_pr_trigger ─────────────────────────────────────────────────

    // ─── generate_ci_trigger ─────────────────────────────────────────────────

    // ─── generate_ci_trigger: on.pr.mode behaviour (issue #916) ──────────────
    //
    // The synth path (default, `mode: synthetic`) leaves the CI trigger at
    // ADO default ("trigger on every branch") and relies on the synthPr
    // Setup step to promote / skip per build. Policy path (`mode: policy`)
    // emits `trigger: none` so the operator-installed Build Validation
    // policy is the sole source of pipeline runs — no duplicate builds.

    // ─── generate_pipeline_resources ─────────────────────────────────────────

    // ─── generate_header_comment ────────────────────────────────────────────

    #[test]
    fn test_generate_header_comment_escapes_quotes() {
        let path = std::path::Path::new("agents/my \"agent\".md");
        let header = generate_header_comment(path);
        assert!(
            header.contains(r#"source="agents/my \"agent\".md""#),
            "Quotes in path should be escaped: {}",
            header
        );
    }

    #[test]
    fn test_generate_header_comment_round_trip_with_quotes() {
        let path = std::path::Path::new("agents/my \"agent\".md");
        let header = generate_header_comment(path);
        let marker_line = header.lines().nth(1).expect("Should have second line");
        let meta = crate::detect::parse_header_line(marker_line)
            .expect("Should parse header with escaped quotes");
        assert_eq!(meta.source, r#"agents/my "agent".md"#);
    }

    #[test]
    fn test_generate_header_comment_strips_dot_slash_prefixes() {
        let path = std::path::Path::new("././././agents/release-readiness.md");
        let header = generate_header_comment(path);
        assert!(
            header.contains(r#"source="agents/release-readiness.md""#),
            "Redundant ./ prefixes should be stripped: {}",
            header
        );
    }

    #[test]
    fn test_generate_header_comment_strips_single_dot_slash() {
        let path = std::path::Path::new("./agents/my-agent.md");
        let header = generate_header_comment(path);
        assert!(
            header.contains(r#"source="agents/my-agent.md""#),
            "Single ./ prefix should be stripped: {}",
            header
        );
    }

    #[test]
    fn test_generate_header_comment_absolute_path_under_cwd() {
        // Build an absolute path by joining CWD with a relative agent path.
        // generate_header_comment should strip the CWD prefix so the stored
        // source remains relative (matching what --source filters expect).
        let cwd = std::env::current_dir().expect("current dir");
        let abs_path = cwd.join("agents/my-agent.md");
        let header = generate_header_comment(&abs_path);
        assert!(
            header.contains(r#"source="agents/my-agent.md""#),
            "Absolute path under CWD should be stored as relative: {}",
            header
        );
    }

    #[test]
    fn test_generate_header_comment_absolute_path_subdir() {
        // Absolute path that is nested several directories deep under CWD.
        let cwd = std::env::current_dir().expect("current dir");
        let abs_path = cwd.join(".azdo/pipelines/review.md");
        let header = generate_header_comment(&abs_path);
        assert!(
            header.contains(r#"source=".azdo/pipelines/review.md""#),
            "Nested absolute path should be stored as relative: {}",
            header
        );
    }

    // ─── generate_source_path ────────────────────────────────────────────────

    #[test]
    fn test_generate_source_path_preserves_directory() {
        // Compiling agents/ctf.md should produce the trigger-repo-anchored
        // path so the integrity check / Stage 3 executor find the file in the
        // self repo regardless of the user's workspace setting.
        let path = std::path::Path::new("agents/ctf.md");
        let result = generate_source_path(path);
        assert_eq!(result, "{{ trigger_repo_directory }}/agents/ctf.md");
    }

    #[test]
    fn test_generate_source_path_nested_directory() {
        let path = std::path::Path::new("pipelines/production/review.md");
        let result = generate_source_path(path);
        assert_eq!(
            result,
            "{{ trigger_repo_directory }}/pipelines/production/review.md"
        );
    }

    #[test]
    fn test_generate_source_path_strips_dot_slash() {
        let path = std::path::Path::new("./agents/my-agent.md");
        let result = generate_source_path(path);
        assert_eq!(result, "{{ trigger_repo_directory }}/agents/my-agent.md");
    }

    #[test]
    fn test_generate_source_path_filename_only() {
        let path = std::path::Path::new("my-agent.md");
        let result = generate_source_path(path);
        assert_eq!(result, "{{ trigger_repo_directory }}/my-agent.md");
    }

    // ─── generate_pipeline_path ──────────────────────────────────────────────

    #[test]
    fn test_generate_pipeline_path_preserves_directory() {
        // The original bug: compiling agents/ctf.md produced agents/ctf.yml as
        // output, but the embedded path was only ctf.yml (missing agents/).
        // Pipeline path is relative to the integrity check's workingDirectory
        // ({{ trigger_repo_directory }}), so no prefix is embedded here.
        let path = std::path::Path::new("agents/ctf.yml");
        let result = generate_pipeline_path(path);
        assert_eq!(result, "agents/ctf.yml");
    }

    #[test]
    fn test_generate_pipeline_path_nested_directory() {
        let path = std::path::Path::new("pipelines/production/review.yml");
        let result = generate_pipeline_path(path);
        assert_eq!(result, "pipelines/production/review.yml");
    }

    #[test]
    fn test_generate_pipeline_path_strips_dot_slash() {
        let path = std::path::Path::new("./agents/my-agent.yml");
        let result = generate_pipeline_path(path);
        assert_eq!(result, "agents/my-agent.yml");
    }

    #[test]
    fn test_generate_pipeline_path_filename_only() {
        let path = std::path::Path::new("pipeline.yml");
        let result = generate_pipeline_path(path);
        assert_eq!(result, "pipeline.yml");
    }

    #[test]
    fn test_generate_source_path_absolute_falls_back_to_filename() {
        // An absolute path that is NOT inside a git repo should fall back
        // to filename-only to avoid embedding a machine-specific absolute path.
        // Use a real temp dir so the path is genuinely absolute on any OS.
        let tmp = tempfile::TempDir::new().unwrap();
        let abs_path = tmp.path().join("agents").join("ctf.md");
        // No .git marker — find_git_root will walk up and find nothing
        // (temp dirs are outside any repo).
        let result = generate_source_path(&abs_path);
        assert_eq!(result, "{{ trigger_repo_directory }}/ctf.md");
    }

    #[test]
    fn test_generate_pipeline_path_absolute_falls_back_to_filename() {
        let tmp = tempfile::TempDir::new().unwrap();
        let abs_path = tmp.path().join("agents").join("ctf.yml");
        let result = generate_pipeline_path(&abs_path);
        assert_eq!(result, "ctf.yml");
    }

    #[test]
    fn test_generate_source_path_absolute_with_git_root_preserves_directory() {
        // When the absolute path is inside a git repo, the directory structure
        // relative to the repo root must be preserved.
        use std::fs;
        let tmp = tempfile::TempDir::new().unwrap();
        let agents_dir = tmp.path().join("agents");
        fs::create_dir_all(&agents_dir).unwrap();
        // A `.git` file (as used in worktrees) satisfies `.exists()` just like
        // a `.git` directory, so either form is a valid marker.
        fs::write(tmp.path().join(".git"), "gitdir: fake").unwrap();
        let abs_path = agents_dir.join("ctf.md");
        let result = generate_source_path(&abs_path);
        assert_eq!(result, "{{ trigger_repo_directory }}/agents/ctf.md");
    }

    #[test]
    fn test_generate_pipeline_path_absolute_with_git_root_preserves_directory() {
        use std::fs;
        let tmp = tempfile::TempDir::new().unwrap();
        let agents_dir = tmp.path().join("agents");
        fs::create_dir_all(&agents_dir).unwrap();
        fs::write(tmp.path().join(".git"), "gitdir: fake").unwrap();
        let abs_path = agents_dir.join("ctf.yml");
        let result = generate_pipeline_path(&abs_path);
        assert_eq!(result, "agents/ctf.yml");
    }

    // ─── generate_trigger_repo_directory ─────────────────────────────────────

    #[test]
    fn test_generate_trigger_repo_directory_no_additional_checkouts() {
        // With only `self` checked out, ADO places the repository content
        // directly into $(Build.SourcesDirectory).
        let result = generate_trigger_repo_directory(&[]);
        assert_eq!(result, "$(Build.SourcesDirectory)");
    }

    #[test]
    fn test_generate_trigger_repo_directory_with_additional_checkouts() {
        // As soon as any additional repo is checked out, ADO places every
        // checked-out repo (including `self`) into a subdirectory named
        // after the repository.
        let result = generate_trigger_repo_directory(&["exp23-a7-nw".to_string()]);
        assert_eq!(result, "$(Build.SourcesDirectory)/$(Build.Repository.Name)");
    }

    #[test]
    fn test_trigger_repo_directory_independent_of_workspace_alias() {
        // Regression: when workspace points at a checked-out alias, the
        // trigger-repo directory must still anchor at the self repo, NOT at
        // the alias subfolder. This is what makes the integrity check
        // (and Stage 3 --source) find the pipeline yaml / agent markdown.
        let checkout = vec!["exp23-a7-nw".to_string()];
        let trigger = generate_trigger_repo_directory(&checkout);
        let workspace =
            compute_effective_workspace(&Some("exp23-a7-nw".to_string()), &checkout, "ctf")
                .unwrap();
        let working_dir = generate_working_directory(&workspace);

        assert_eq!(
            trigger,
            "$(Build.SourcesDirectory)/$(Build.Repository.Name)"
        );
        assert_eq!(working_dir, "$(Build.SourcesDirectory)/exp23-a7-nw");
        assert_ne!(
            trigger, working_dir,
            "trigger repo dir must differ from working dir when workspace points at an alias"
        );
    }

    // ─── generate_integrity_check ────────────────────────────────────────────

    #[test]
    fn test_generate_integrity_check_default_produces_step() {
        let result = generate_integrity_check(false);
        assert!(
            result.contains("Verify pipeline integrity"),
            "Should contain the displayName"
        );
        assert!(
            result.contains("ado-aw"),
            "Should reference the ado-aw binary"
        );
        assert!(
            result.contains("{{ pipeline_path }}"),
            "Should contain the pipeline_path placeholder for later resolution"
        );
        assert!(
            result.contains("workingDirectory: {{ trigger_repo_directory }}"),
            "Should set workingDirectory to the trigger repo so `ado-aw check` \
             can recompile from a directory that contains .git (needed for \
             ADO org inference when tools.azure-devops is enabled)"
        );
    }

    #[test]
    fn test_generate_integrity_check_skip_produces_empty() {
        let result = generate_integrity_check(true);
        assert!(
            result.is_empty(),
            "Should produce empty string when skipping"
        );
    }

    #[test]
    fn test_debug_create_issue_enabled_helper() {
        let yaml_off = "---\nname: test\ndescription: test\n---\n";
        let (fm_off, _) = parse_markdown(yaml_off).unwrap();
        assert!(!debug_create_issue_enabled(&fm_off));

        let yaml_on = r#"---
name: test
description: test
ado-aw-debug:
  create-issue:
    target-repo: githubnext/ado-aw
---
"#;
        let (fm_on, _) = parse_markdown(yaml_on).unwrap();
        assert!(debug_create_issue_enabled(&fm_on));

        let yaml_section_only = r#"---
name: test
description: test
ado-aw-debug:
  skip-integrity: true
---
"#;
        let (fm_section, _) = parse_markdown(yaml_section_only).unwrap();
        assert!(
            !debug_create_issue_enabled(&fm_section),
            "ado-aw-debug.skip-integrity alone must NOT enable create-issue"
        );
    }

    // ─── generate_debug_pipeline_replacements ────────────────────────────────

    // ─── validate_submit_pr_review_events ────────────────────────────────────

    #[test]
    fn test_submit_pr_review_events_passes_when_not_configured() {
        let fm = minimal_front_matter();
        assert!(validate_submit_pr_review_events(&fm).is_ok());
    }

    #[test]
    fn test_submit_pr_review_events_fails_when_allowed_events_missing() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nsafe-outputs:\n  submit-pr-review:\n    allowed-repositories:\n      - self\n---\n"
        ).unwrap();
        let result = validate_submit_pr_review_events(&fm);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("allowed-events"), "message: {msg}");
    }

    #[test]
    fn test_submit_pr_review_events_fails_when_allowed_events_empty() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nsafe-outputs:\n  submit-pr-review:\n    allowed-events: []\n---\n"
        ).unwrap();
        let result = validate_submit_pr_review_events(&fm);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("allowed-events"), "message: {msg}");
    }

    #[test]
    fn test_submit_pr_review_events_fails_when_value_is_scalar() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nsafe-outputs:\n  submit-pr-review: true\n---\n",
        )
        .unwrap();
        let result = validate_submit_pr_review_events(&fm);
        assert!(result.is_err());
    }

    #[test]
    fn test_submit_pr_review_events_passes_when_events_provided() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nsafe-outputs:\n  submit-pr-review:\n    allowed-events:\n      - comment\n      - approve\n---\n"
        ).unwrap();
        assert!(validate_submit_pr_review_events(&fm).is_ok());
    }

    // ─── validate_update_pr_votes ─────────────────────────────────────────────

    #[test]
    fn test_update_pr_votes_passes_when_not_configured() {
        let fm = minimal_front_matter();
        assert!(validate_update_pr_votes(&fm).is_ok());
    }

    #[test]
    fn test_update_pr_votes_fails_when_vote_reachable_and_no_allowed_votes() {
        // allowed-operations absent → vote is reachable; no allowed-votes → should fail
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nsafe-outputs:\n  update-pr:\n    allowed-repositories:\n      - self\n---\n"
        ).unwrap();
        let result = validate_update_pr_votes(&fm);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("allowed-votes"), "message: {msg}");
    }

    #[test]
    fn test_update_pr_votes_fails_when_vote_explicit_and_no_allowed_votes() {
        // allowed-operations contains "vote"; no allowed-votes → should fail
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nsafe-outputs:\n  update-pr:\n    allowed-operations:\n      - vote\n---\n"
        ).unwrap();
        let result = validate_update_pr_votes(&fm);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("allowed-votes"), "message: {msg}");
    }

    #[test]
    fn test_update_pr_votes_fails_when_allowed_votes_empty() {
        // allowed-operations absent; allowed-votes is empty list → should fail
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nsafe-outputs:\n  update-pr:\n    allowed-votes: []\n---\n"
        ).unwrap();
        let result = validate_update_pr_votes(&fm);
        assert!(result.is_err());
    }

    #[test]
    fn test_update_pr_votes_passes_when_vote_excluded_from_allowed_operations() {
        // allowed-operations is non-empty and does not contain "vote" → safe, no error
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nsafe-outputs:\n  update-pr:\n    allowed-operations:\n      - add-reviewers\n      - set-auto-complete\n---\n"
        ).unwrap();
        assert!(validate_update_pr_votes(&fm).is_ok());
    }

    #[test]
    fn test_update_pr_votes_passes_when_vote_reachable_and_allowed_votes_set() {
        // allowed-operations absent; allowed-votes non-empty → OK
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nsafe-outputs:\n  update-pr:\n    allowed-votes:\n      - approve-with-suggestions\n---\n"
        ).unwrap();
        assert!(validate_update_pr_votes(&fm).is_ok());
    }

    #[test]
    fn test_update_pr_votes_passes_when_vote_explicit_and_allowed_votes_set() {
        // allowed-operations contains "vote"; allowed-votes non-empty → OK
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nsafe-outputs:\n  update-pr:\n    allowed-operations:\n      - vote\n    allowed-votes:\n      - wait-for-author\n---\n"
        ).unwrap();
        assert!(validate_update_pr_votes(&fm).is_ok());
    }

    // ─── validate_resolve_pr_thread_statuses ──────────────────────────────────

    #[test]
    fn test_resolve_pr_thread_passes_when_not_configured() {
        let fm = minimal_front_matter();
        assert!(validate_resolve_pr_thread_statuses(&fm).is_ok());
    }

    #[test]
    fn test_resolve_pr_thread_fails_when_allowed_statuses_missing() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nsafe-outputs:\n  resolve-pr-thread:\n    allowed-repositories:\n      - self\n---\n"
        ).unwrap();
        let result = validate_resolve_pr_thread_statuses(&fm);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("allowed-statuses"), "message: {msg}");
    }

    #[test]
    fn test_resolve_pr_thread_fails_when_allowed_statuses_empty() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nsafe-outputs:\n  resolve-pr-thread:\n    allowed-statuses: []\n---\n"
        ).unwrap();
        let result = validate_resolve_pr_thread_statuses(&fm);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("allowed-statuses"), "message: {msg}");
    }

    #[test]
    fn test_resolve_pr_thread_fails_when_value_is_scalar() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nsafe-outputs:\n  resolve-pr-thread: true\n---\n",
        )
        .unwrap();
        let result = validate_resolve_pr_thread_statuses(&fm);
        assert!(result.is_err());
    }

    #[test]
    fn test_resolve_pr_thread_passes_when_statuses_provided() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nsafe-outputs:\n  resolve-pr-thread:\n    allowed-statuses:\n      - fixed\n      - wont-fix\n---\n"
        ).unwrap();
        assert!(validate_resolve_pr_thread_statuses(&fm).is_ok());
    }

    // ─── Enabled tools args generation ──────────────────────────────────

    #[test]
    fn test_generate_enabled_tools_args_empty_safe_outputs() {
        let (fm, _) = parse_markdown("---\nname: test\ndescription: test\n---\n").unwrap();
        let args = generate_enabled_tools_args(&fm);
        assert!(args.is_empty(), "Empty safe-outputs should produce no args");
    }

    #[test]
    fn test_generate_enabled_tools_args_with_configured_tools() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nsafe-outputs:\n  create-pull-request:\n    target-branch: main\n  create-work-item:\n    work-item-type: Task\n---\n"
        ).unwrap();
        let args = generate_enabled_tools_args(&fm);
        assert!(args.contains("--enabled-tools create-pull-request"));
        assert!(args.contains("--enabled-tools create-work-item"));
        // Always-on tools should also be included
        assert!(args.contains("--enabled-tools noop"));
        assert!(args.contains("--enabled-tools missing-data"));
        assert!(args.contains("--enabled-tools missing-tool"));
        assert!(args.contains("--enabled-tools report-incomplete"));
    }

    #[test]
    fn test_generate_enabled_tools_args_no_duplicates() {
        // If a diagnostic tool is also in safe-outputs, it shouldn't appear twice
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nsafe-outputs:\n  noop:\n    max: 5\n---\n",
        )
        .unwrap();
        let args = generate_enabled_tools_args(&fm);
        let noop_count = args.matches("--enabled-tools noop").count();
        assert_eq!(noop_count, 1, "noop should appear exactly once");
    }

    #[test]
    fn test_is_safe_tool_name() {
        assert!(validate::is_safe_tool_name("create-pull-request"));
        assert!(validate::is_safe_tool_name("noop"));
        assert!(validate::is_safe_tool_name("my-tool-123"));
        assert!(!validate::is_safe_tool_name(""));
        assert!(!validate::is_safe_tool_name("$(curl evil.com)"));
        assert!(!validate::is_safe_tool_name("foo; rm -rf /"));
        assert!(!validate::is_safe_tool_name("tool name"));
        assert!(!validate::is_safe_tool_name("tool\ttab"));
    }

    #[test]
    fn test_generate_enabled_tools_args_skips_unknown_tool() {
        // An unrecognized (but safe-formatted) tool name should be skipped.
        // When no valid MCP tools remain, return empty (all tools available).
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nsafe-outputs:\n  crate-pull-request:\n    target-branch: main\n---\n"
        ).unwrap();
        let args = generate_enabled_tools_args(&fm);
        assert!(
            !args.contains("crate-pull-request"),
            "Unrecognized tool should be skipped"
        );
        assert!(
            args.is_empty(),
            "All-unrecognized safe-outputs should produce no args (all tools available)"
        );
    }

    #[test]
    fn test_generate_enabled_tools_args_memory_no_longer_safe_output() {
        // `memory` is no longer a safe-output key — it moved to `tools: cache-memory:`.
        // If someone still puts it in safe-outputs, it should be treated as unrecognized
        // and the real MCP tool should still be present.
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nsafe-outputs:\n  create-pull-request:\n    target-branch: main\n---\n"
        ).unwrap();
        let args = generate_enabled_tools_args(&fm);
        assert!(
            args.contains("--enabled-tools create-pull-request"),
            "Real MCP tool should be present"
        );
    }

    #[test]
    fn test_generate_enabled_tools_args_empty_safe_outputs_no_filter() {
        // When safe-outputs is empty, no --enabled-tools args should be generated
        // so all tools remain available.
        let (fm, _) = parse_markdown("---\nname: test\ndescription: test\n---\n").unwrap();
        let args = generate_enabled_tools_args(&fm);
        assert!(
            args.is_empty(),
            "empty safe-outputs should produce no args (all tools available)"
        );
    }

    // ─── ado-aw-debug wiring ────────────────────────────────────────────────

    #[test]
    fn test_generate_enabled_tools_args_debug_create_issue_alone() {
        let yaml = r#"---
name: test
description: test
ado-aw-debug:
  create-issue:
    target-repo: githubnext/ado-aw
---
"#;
        let (fm, _) = parse_markdown(yaml).unwrap();
        let args = generate_enabled_tools_args(&fm);
        assert!(
            args.contains("--enabled-tools create-issue"),
            "ado-aw-debug.create-issue should add create-issue to --enabled-tools, got: {}",
            args
        );
        // Always-on tools should also be present so the filter activates.
        assert!(args.contains("--enabled-tools noop"));
    }

    #[test]
    fn test_generate_enabled_tools_args_debug_plus_safe_outputs() {
        let yaml = r#"---
name: test
description: test
safe-outputs:
  create-pull-request:
    target-branch: main
ado-aw-debug:
  create-issue:
    target-repo: githubnext/ado-aw
---
"#;
        let (fm, _) = parse_markdown(yaml).unwrap();
        let args = generate_enabled_tools_args(&fm);
        assert!(args.contains("--enabled-tools create-pull-request"));
        assert!(args.contains("--enabled-tools create-issue"));
        // No duplicate
        assert_eq!(args.matches("--enabled-tools create-issue").count(), 1);
    }

    #[test]
    fn test_generate_enabled_tools_args_no_debug_does_not_emit_create_issue() {
        let yaml = r#"---
name: test
description: test
safe-outputs:
  create-pull-request:
    target-branch: main
---
"#;
        let (fm, _) = parse_markdown(yaml).unwrap();
        let args = generate_enabled_tools_args(&fm);
        assert!(
            !args.contains("create-issue"),
            "create-issue must not appear without ado-aw-debug.create-issue"
        );
    }

    #[test]
    fn test_validate_ado_aw_debug_config_accepts_valid_config() {
        let yaml = r#"---
name: test
description: test
ado-aw-debug:
  create-issue:
    target-repo: githubnext/ado-aw
    title-prefix: "[bug] "
    labels: [pipeline-failure]
    allowed-labels: ["agent-*"]
    assignees: [jamesdevine]
---
"#;
        let (fm, _) = parse_markdown(yaml).unwrap();
        assert!(validate_ado_aw_debug_config(&fm).is_ok());
    }

    #[test]
    fn test_validate_ado_aw_debug_config_passes_when_section_absent() {
        let (fm, _) = parse_markdown("---\nname: test\ndescription: test\n---\n").unwrap();
        assert!(validate_ado_aw_debug_config(&fm).is_ok());
    }

    #[test]
    fn test_validate_ado_aw_debug_config_rejects_missing_target_repo() {
        let yaml = r#"---
name: test
description: test
ado-aw-debug:
  create-issue:
    target-repo: ""
---
"#;
        let (fm, _) = parse_markdown(yaml).unwrap();
        let result = validate_ado_aw_debug_config(&fm);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("target-repo"), "msg: {}", msg);
    }

    #[test]
    fn test_validate_ado_aw_debug_config_rejects_invalid_target_repo() {
        let yaml = r#"---
name: test
description: test
ado-aw-debug:
  create-issue:
    target-repo: not-a-valid-shape
---
"#;
        let (fm, _) = parse_markdown(yaml).unwrap();
        let result = validate_ado_aw_debug_config(&fm);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("owner/repo"), "msg: {}", msg);
    }

    #[test]
    fn test_validate_ado_aw_debug_config_rejects_pipeline_injection_in_label() {
        let yaml = r###"---
name: test
description: test
ado-aw-debug:
  create-issue:
    target-repo: githubnext/ado-aw
    labels:
      - "##vso[task.complete]"
---
"###;
        let (fm, _) = parse_markdown(yaml).unwrap();
        let result = validate_ado_aw_debug_config(&fm);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_ado_aw_debug_config_rejects_pipeline_injection_in_title_prefix() {
        let yaml = r###"---
name: test
description: test
ado-aw-debug:
  create-issue:
    target-repo: githubnext/ado-aw
    title-prefix: "##vso[task.complete]"
---
"###;
        let (fm, _) = parse_markdown(yaml).unwrap();
        let result = validate_ado_aw_debug_config(&fm);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_safe_outputs_keys_accepts_known_keys() {
        let yaml = r#"---
name: test
description: test
safe-outputs:
  noop: {}
  create-pull-request:
    target-branch: main
  create-work-item: {}
---
"#;
        let (fm, _) = parse_markdown(yaml).unwrap();
        assert!(validate_safe_outputs_keys(&fm).is_ok());
    }

    #[test]
    fn test_validate_safe_outputs_keys_accepts_empty_section() {
        let (fm, _) = parse_markdown("---\nname: test\ndescription: test\n---\n").unwrap();
        assert!(validate_safe_outputs_keys(&fm).is_ok());
    }

    #[test]
    fn test_validate_safe_outputs_keys_rejects_unknown_typo_with_suggestion() {
        // Common past-and-current bug: tool was renamed from `create-pr` to
        // `create-pull-request` but a user (or our own smoke fixtures, before
        // the rename) still references the old name. The compiler used to
        // silently drop the key with only a warning.
        let yaml = r#"---
name: test
description: test
safe-outputs:
  create-pr:
    target-branch: main
---
"#;
        let (fm, _) = parse_markdown(yaml).unwrap();
        let result = validate_safe_outputs_keys(&fm);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("create-pr"), "msg: {}", msg);
        // The validator lists all `create-*` tools as hints, so the actual
        // renamed-from match must appear among them.
        assert!(
            msg.contains("create-pull-request"),
            "expected create-pull-request to appear in similar-tools list, got: {}",
            msg
        );
    }

    #[test]
    fn test_validate_safe_outputs_keys_rejects_unknown_no_close_match() {
        let yaml = r#"---
name: test
description: test
safe-outputs:
  fabricated-tool-name: {}
---
"#;
        let (fm, _) = parse_markdown(yaml).unwrap();
        let result = validate_safe_outputs_keys(&fm);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("fabricated-tool-name"), "msg: {}", msg);
        // No similar-tools line for keys that don't share a prefix with anything real.
        assert!(!msg.contains("similar known tools"), "msg: {}", msg);
    }

    #[test]
    fn test_validate_safe_outputs_keys_does_not_double_report_debug_only_tool() {
        // create-issue is in DEBUG_ONLY_TOOLS — validate_ado_aw_debug_config
        // gives a better error for it. This validator should skip rather
        // than redundantly flag it as "unknown".
        let yaml = r#"---
name: test
description: test
safe-outputs:
  create-issue:
    target-repo: githubnext/ado-aw
---
"#;
        let (fm, _) = parse_markdown(yaml).unwrap();
        assert!(validate_safe_outputs_keys(&fm).is_ok());
    }

    #[test]
    fn test_validate_safe_outputs_keys_allows_memory_migration_key() {
        // `memory` was migrated to `tools: cache-memory:`. The compiler
        // emits a soft warning for it during enabled-tools generation,
        // so this strict validator must not promote it to an error.
        let yaml = r#"---
name: test
description: test
safe-outputs:
  memory: {}
---
"#;
        let (fm, _) = parse_markdown(yaml).unwrap();
        assert!(validate_safe_outputs_keys(&fm).is_ok());
    }

    #[test]
    fn test_validate_safe_outputs_keys_rejects_invalid_characters() {
        let yaml = r#"---
name: test
description: test
safe-outputs:
  bad name with spaces: {}
---
"#;
        let (fm, _) = parse_markdown(yaml).unwrap();
        let result = validate_safe_outputs_keys(&fm);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("ASCII letters, digits, and hyphens"),
            "msg: {}",
            msg
        );
    }

    #[test]
    fn test_validate_safe_outputs_keys_reports_all_invalid_characters() {
        // Both invalid keys should appear in the single error (not just the first).
        let yaml = "---\nname: test\ndescription: test\nsafe-outputs:\n  bad key: {}\n  also bad!: {}\n---\n";
        let (fm, _) = parse_markdown(yaml).unwrap();
        let result = validate_safe_outputs_keys(&fm);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("bad key") && msg.contains("also bad!"),
            "expected both keys in error, got: {}",
            msg
        );
    }

    #[test]
    fn test_related_safe_output_names_returns_all_create_tools_for_create_pr() {
        let related = related_safe_output_names("create-pr");
        assert!(related.contains(&"create-pull-request"));
        assert!(related.contains(&"create-branch"));
        assert!(related.contains(&"create-work-item"));
        // Sanity check: nothing from a different first segment should leak in.
        assert!(!related.contains(&"update-pr"));
    }

    #[test]
    fn test_related_safe_output_names_returns_empty_for_distant_string() {
        let related = related_safe_output_names("fabricated-tool-name");
        assert!(related.is_empty());
    }

    #[test]
    fn test_validate_rejects_create_issue_under_safe_outputs() {
        // Defence-in-depth: `create-issue` MUST NOT appear under
        // `safe-outputs:` even when `ado-aw-debug:` isn't set. Allowing it
        // there would let a forged config flow into ctx.tool_configs and
        // sidestep the executor-side gate.
        let yaml = r#"---
name: test
description: test
safe-outputs:
  create-issue:
    target-repo: githubnext/ado-aw
---
"#;
        let (fm, _) = parse_markdown(yaml).unwrap();
        let result = validate_ado_aw_debug_config(&fm);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("debug-only") && msg.contains("ado-aw-debug"),
            "expected debug-only redirection error, got: {}",
            msg
        );
    }

    // ─── parameter name validation ──────────────────────────────────────────

    #[test]
    fn test_is_valid_parameter_name() {
        assert!(validate::is_valid_parameter_name("clearMemory"));
        assert!(validate::is_valid_parameter_name("myParam"));
        assert!(validate::is_valid_parameter_name("_private"));
        assert!(validate::is_valid_parameter_name("param123"));
        assert!(!validate::is_valid_parameter_name(""));
        assert!(!validate::is_valid_parameter_name("has space"));
        assert!(!validate::is_valid_parameter_name("has-dash"));
        assert!(!validate::is_valid_parameter_name("${{inject}}"));
        assert!(!validate::is_valid_parameter_name("123startsWithDigit"));
    }

    #[test]
    fn test_build_parameters_auto_injects_clear_memory() {
        let params = build_parameters(&[], true, false).unwrap();
        assert_eq!(params.len(), 1);
        assert_eq!(params[0].name, "clearMemory");
    }

    #[test]
    fn test_build_parameters_no_inject_without_memory() {
        let params = build_parameters(&[], false, false).unwrap();
        assert!(params.is_empty());
    }

    #[test]
    fn test_build_parameters_no_duplicate_clear_memory() {
        let user = vec![PipelineParameter {
            name: "clearMemory".to_string(),
            display_name: Some("Custom".to_string()),
            param_type: Some("boolean".to_string()),
            default: Some(serde_yaml::Value::Bool(true)),
            values: None,
        }];
        let params = build_parameters(&user, true, false).unwrap();
        assert_eq!(params.len(), 1, "Should not duplicate clearMemory");
        assert_eq!(
            params[0].display_name.as_deref(),
            Some("Custom"),
            "Should keep user's definition"
        );
    }

    #[test]
    fn test_build_parameters_template_target_injects_depends_on_and_condition() {
        let params = build_parameters(&[], false, true).unwrap();
        assert_eq!(params.len(), 2);
        assert_eq!(params[0].name, "dependsOn");
        assert_eq!(params[0].param_type.as_deref(), Some("object"));
        assert!(matches!(
            params[0].default,
            Some(serde_yaml::Value::Sequence(ref s)) if s.is_empty()
        ));
        assert_eq!(params[1].name, "condition");
        assert_eq!(params[1].param_type.as_deref(), Some("string"));
        assert!(matches!(
            params[1].default,
            Some(serde_yaml::Value::String(ref s)) if s.is_empty()
        ));
    }

    #[test]
    fn test_build_parameters_template_target_ordering() {
        let user = vec![PipelineParameter {
            name: "myParam".to_string(),
            display_name: None,
            param_type: Some("string".to_string()),
            default: Some(serde_yaml::Value::String("hi".to_string())),
            values: None,
        }];
        let params = build_parameters(&user, true, true).unwrap();
        // Order: dependsOn, condition, clearMemory, then user params
        assert_eq!(params.len(), 4);
        assert_eq!(params[0].name, "dependsOn");
        assert_eq!(params[1].name, "condition");
        assert_eq!(params[2].name, "clearMemory");
        assert_eq!(params[3].name, "myParam");
    }

    #[test]
    fn test_build_parameters_template_target_rejects_reserved_depends_on() {
        let user = vec![PipelineParameter {
            name: "dependsOn".to_string(),
            display_name: None,
            param_type: Some("string".to_string()),
            default: None,
            values: None,
        }];
        let err = build_parameters(&user, false, true).unwrap_err();
        let msg = format!("{}", err);
        assert!(
            msg.contains("dependsOn") && msg.contains("reserved"),
            "Expected reserved-name error mentioning dependsOn, got: {}",
            msg
        );
    }

    #[test]
    fn test_build_parameters_template_target_rejects_reserved_condition() {
        let user = vec![PipelineParameter {
            name: "condition".to_string(),
            display_name: None,
            param_type: Some("string".to_string()),
            default: None,
            values: None,
        }];
        let err = build_parameters(&user, false, true).unwrap_err();
        let msg = format!("{}", err);
        assert!(
            msg.contains("condition") && msg.contains("reserved"),
            "Expected reserved-name error mentioning condition, got: {}",
            msg
        );
    }

    #[test]
    fn test_build_parameters_non_template_target_allows_depends_on_name() {
        // For standalone/1es, dependsOn and condition are just regular UI param names.
        let user = vec![
            PipelineParameter {
                name: "dependsOn".to_string(),
                display_name: None,
                param_type: Some("string".to_string()),
                default: None,
                values: None,
            },
            PipelineParameter {
                name: "condition".to_string(),
                display_name: None,
                param_type: Some("string".to_string()),
                default: None,
                values: None,
            },
        ];
        let params = build_parameters(&user, false, false).unwrap();
        assert_eq!(params.len(), 2);
    }

    // ─── replace_with_indent ─────────────────────────────────────────────────

    // ─── format_step_yaml / format_step_yaml_indented ────────────────────────

    // ─── generate_acquire_ado_token ──────────────────────────────────────────

    #[test]
    fn test_generate_acquire_ado_token_with_sc() {
        let result = generate_acquire_ado_token(Some("my-arm-sc"), "SC_READ_TOKEN");
        assert!(result.contains("AzureCLI@2"), "Should use AzureCLI@2 task");
        assert!(
            result.contains("azureSubscription: 'my-arm-sc'"),
            "Should embed service connection name"
        );
        assert!(
            result.contains("variable=SC_READ_TOKEN;issecret=true"),
            "Should set correct pipeline variable as secret"
        );
        assert!(
            result.contains("az account get-access-token"),
            "Should call az CLI to get access token"
        );
    }

    #[test]
    fn test_generate_acquire_ado_token_none_returns_empty() {
        let result = generate_acquire_ado_token(None, "SC_READ_TOKEN");
        assert!(
            result.is_empty(),
            "None service connection should return empty string"
        );
    }

    #[test]
    fn test_generate_acquire_ado_token_write_token_variable() {
        let result = generate_acquire_ado_token(Some("write-sc"), "SC_WRITE_TOKEN");
        assert!(result.contains("variable=SC_WRITE_TOKEN;issecret=true"));
        assert!(!result.contains("SC_READ_TOKEN"));
    }

    // ─── engine env / generate_executor_ado_env ────────────────────────────

    #[test]
    fn test_engine_env() {
        let fm = minimal_front_matter();
        let ctx = CompileContext::for_test(&fm);
        let result = ctx.engine.env(&fm.engine).unwrap();
        assert!(
            result.contains("GITHUB_TOKEN: $(GITHUB_TOKEN)"),
            "Should include GITHUB_TOKEN"
        );
        assert!(
            !result.contains("AZURE_DEVOPS_EXT_PAT"),
            "ADO token is handled by MCPG, not engine env"
        );
    }

    #[test]
    fn test_generate_executor_ado_env_with_connection() {
        let result = generate_executor_ado_env(Some("my-sc"), false);
        assert!(
            result.contains("env:"),
            "Executor env block should include the 'env:' key"
        );
        assert!(
            result.contains("SYSTEM_ACCESSTOKEN: $(SC_WRITE_TOKEN)"),
            "Executor should use SC_WRITE_TOKEN when write SC is configured"
        );
        // Must NOT expose the read token in the executor env
        assert!(
            !result.contains("SC_READ_TOKEN"),
            "Executor env must not contain SC_READ_TOKEN"
        );
        assert!(
            !result.contains("$(System.AccessToken)"),
            "When write SC is configured, fall back to ARM-minted token, not System.AccessToken"
        );
        assert!(
            !result.contains("ADO_AW_DEBUG_GITHUB_TOKEN"),
            "Without debug flag, GitHub token must not be exposed to executor"
        );
    }

    #[test]
    fn test_generate_executor_ado_env_none_uses_system_access_token() {
        let result = generate_executor_ado_env(None, false);
        assert!(
            result.starts_with("env:\n"),
            "Should always emit env: block (executor needs SYSTEM_ACCESSTOKEN)"
        );
        assert!(
            result.contains("SYSTEM_ACCESSTOKEN: $(System.AccessToken)"),
            "Default executor token is $(System.AccessToken)"
        );
        assert!(
            !result.contains("SC_WRITE_TOKEN"),
            "Without write SC, must not reference SC_WRITE_TOKEN"
        );
        assert!(
            !result.contains("ADO_AW_DEBUG_GITHUB_TOKEN"),
            "Without debug flag, GitHub token must not appear"
        );
    }

    #[test]
    fn test_generate_executor_ado_env_with_create_issue_only() {
        let result = generate_executor_ado_env(None, true);
        assert!(result.starts_with("env:\n"), "Should emit env: block");
        assert!(
            result.contains("SYSTEM_ACCESSTOKEN: $(System.AccessToken)"),
            "Default executor token is $(System.AccessToken) even with debug enabled"
        );
        assert!(
            result.contains("ADO_AW_DEBUG_GITHUB_TOKEN: $(ADO_AW_DEBUG_GITHUB_TOKEN)"),
            "Debug flag should expose the GitHub PAT pipeline variable"
        );
        assert!(
            !result.contains("SC_WRITE_TOKEN"),
            "No write SC means no SC_WRITE_TOKEN"
        );
    }

    #[test]
    fn test_generate_executor_ado_env_with_both_tokens() {
        let result = generate_executor_ado_env(Some("write-sc"), true);
        assert!(result.contains("SYSTEM_ACCESSTOKEN: $(SC_WRITE_TOKEN)"));
        assert!(result.contains("ADO_AW_DEBUG_GITHUB_TOKEN: $(ADO_AW_DEBUG_GITHUB_TOKEN)"));
        assert!(
            !result.contains("$(System.AccessToken)"),
            "Write SC overrides System.AccessToken default"
        );
    }

    // ─── Security validation tests ────────────────────────────────────────────

    #[test]
    fn test_model_name_rejects_single_quote() {
        let mut fm = minimal_front_matter();
        fm.engine =
            crate::compile::types::EngineConfig::Full(crate::compile::types::EngineOptions {
                id: Some("copilot".to_string()),
                model: Some("model' && echo pwned".to_string()),
                version: None,
                agent: None,
                api_target: None,
                args: vec![],
                env: None,
                command: None,
                timeout_minutes: None,
            });
        let result = engine_args_for(&fm);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("invalid characters")
        );
    }

    #[test]
    fn test_model_name_rejects_space() {
        let mut fm = minimal_front_matter();
        fm.engine =
            crate::compile::types::EngineConfig::Full(crate::compile::types::EngineOptions {
                id: Some("copilot".to_string()),
                model: Some("model && curl evil.com".to_string()),
                version: None,
                agent: None,
                api_target: None,
                args: vec![],
                env: None,
                command: None,
                timeout_minutes: None,
            });
        let result = engine_args_for(&fm);
        assert!(result.is_err());
    }

    #[test]
    fn test_model_name_allows_valid_names() {
        for name in &[
            "claude-opus-4.5",
            "gpt-5.2-codex",
            "gemini-3-pro-preview",
            "my_model:latest",
        ] {
            let mut fm = minimal_front_matter();
            fm.engine =
                crate::compile::types::EngineConfig::Full(crate::compile::types::EngineOptions {
                    id: Some("copilot".to_string()),
                    model: Some(name.to_string()),
                    version: None,
                    agent: None,
                    api_target: None,
                    args: vec![],
                    env: None,
                    command: None,
                    timeout_minutes: None,
                });
            let result = engine_args_for(&fm);
            assert!(result.is_ok(), "Model name '{}' should be valid", name);
        }
    }

    #[test]
    fn test_bash_command_rejects_single_quote() {
        let mut fm = minimal_front_matter();
        fm.tools = Some(crate::compile::types::ToolsConfig {
            bash: Some(vec!["cat'".to_string()]),
            edit: None,
            cache_memory: None,
            azure_devops: None,
        });
        let result = engine_args_for(&fm);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("single quote"));
    }

    #[test]
    fn test_validate_front_matter_identity_rejects_ado_expression_in_name() {
        let mut fm = minimal_front_matter();
        fm.name = "My Agent ${{ variables['System.AccessToken'] }}".to_string();
        let result = validate_front_matter_identity(&fm);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("ADO expression"));
    }

    #[test]
    fn test_validate_front_matter_identity_rejects_macro_in_description() {
        let mut fm = minimal_front_matter();
        fm.description = "Agent $(System.AccessToken)".to_string();
        let result = validate_front_matter_identity(&fm);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("ADO expression"));
    }

    #[test]
    fn test_validate_front_matter_identity_rejects_newline_in_name() {
        let mut fm = minimal_front_matter();
        fm.name = "My Agent\ninjected: true".to_string();
        let result = validate_front_matter_identity(&fm);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("single line"));
    }

    #[test]
    fn test_validate_front_matter_identity_rejects_template_marker_in_name() {
        let mut fm = minimal_front_matter();
        fm.name = "{{ agent_content }}".to_string();
        let result = validate_front_matter_identity(&fm);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("template marker"));
    }

    #[test]
    fn test_validate_front_matter_identity_rejects_template_marker_in_description() {
        let mut fm = minimal_front_matter();
        fm.description = "{{ copilot_params }}".to_string();
        let result = validate_front_matter_identity(&fm);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("template marker"));
    }

    #[test]
    fn test_validate_front_matter_identity_rejects_newline_in_trigger_pipeline_name() {
        let mut fm = minimal_front_matter();
        fm.on_config = Some(OnConfig {
            pipeline: Some(crate::compile::types::PipelineTrigger {
                name: "Build\ninjected: true".to_string(),
                project: None,
                branches: vec![],
                filters: None,
            }),
            pr: None,
            schedule: None,
        });
        let result = validate_front_matter_identity(&fm);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("on.pipeline.name"));
    }

    #[test]
    fn test_validate_front_matter_identity_rejects_newline_in_trigger_pipeline_project() {
        let mut fm = minimal_front_matter();
        fm.on_config = Some(OnConfig {
            pipeline: Some(crate::compile::types::PipelineTrigger {
                name: "Build Pipeline".to_string(),
                project: Some("OtherProject\ninjected: true".to_string()),
                branches: vec![],
                filters: None,
            }),
            pr: None,
            schedule: None,
        });
        let result = validate_front_matter_identity(&fm);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("on.pipeline.project")
        );
    }

    #[test]
    fn test_validate_front_matter_identity_rejects_newline_in_trigger_pipeline_branch() {
        let mut fm = minimal_front_matter();
        fm.on_config = Some(OnConfig {
            pipeline: Some(crate::compile::types::PipelineTrigger {
                name: "Build Pipeline".to_string(),
                project: None,
                branches: vec!["main\ninjected: true".to_string()],
                filters: None,
            }),
            pr: None,
            schedule: None,
        });
        let result = validate_front_matter_identity(&fm);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("on.pipeline.branches")
        );
    }

    #[test]
    fn test_validate_front_matter_identity_rejects_newline_in_pr_branch_include() {
        let mut fm = minimal_front_matter();
        fm.on_config = Some(OnConfig {
            pipeline: None,
            pr: Some(crate::compile::types::PrTriggerConfig {
                branches: Some(crate::compile::types::BranchFilter {
                    include: vec!["main\ninjected: true".to_string()],
                    exclude: vec![],
                }),
                paths: None,
                filters: None,
                ..Default::default()
            }),
            schedule: None,
        });
        let result = validate_front_matter_identity(&fm);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("on.pr.branches.include")
        );
    }

    #[test]
    fn test_validate_front_matter_identity_rejects_newline_in_pr_branch_exclude() {
        let mut fm = minimal_front_matter();
        fm.on_config = Some(OnConfig {
            pipeline: None,
            pr: Some(crate::compile::types::PrTriggerConfig {
                branches: Some(crate::compile::types::BranchFilter {
                    include: vec![],
                    exclude: vec!["feature\ninjected: true".to_string()],
                }),
                paths: None,
                filters: None,
                ..Default::default()
            }),
            schedule: None,
        });
        let result = validate_front_matter_identity(&fm);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("on.pr.branches.exclude")
        );
    }

    #[test]
    fn test_validate_front_matter_identity_rejects_newline_in_pr_path_include() {
        let mut fm = minimal_front_matter();
        fm.on_config = Some(OnConfig {
            pipeline: None,
            pr: Some(crate::compile::types::PrTriggerConfig {
                branches: None,
                paths: Some(crate::compile::types::PathFilter {
                    include: vec!["src/\ninjected: true".to_string()],
                    exclude: vec![],
                }),
                filters: None,
                ..Default::default()
            }),
            schedule: None,
        });
        let result = validate_front_matter_identity(&fm);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("on.pr.paths.include")
        );
    }

    #[test]
    fn test_validate_front_matter_identity_rejects_newline_in_pr_path_exclude() {
        let mut fm = minimal_front_matter();
        fm.on_config = Some(OnConfig {
            pipeline: None,
            pr: Some(crate::compile::types::PrTriggerConfig {
                branches: None,
                paths: Some(crate::compile::types::PathFilter {
                    include: vec![],
                    exclude: vec!["tests/\ninjected: true".to_string()],
                }),
                filters: None,
                ..Default::default()
            }),
            schedule: None,
        });
        let result = validate_front_matter_identity(&fm);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("on.pr.paths.exclude")
        );
    }

    #[test]
    fn test_validate_front_matter_identity_rejects_ado_expression_in_pr_branch_include() {
        let mut fm = minimal_front_matter();
        fm.on_config = Some(OnConfig {
            pipeline: None,
            pr: Some(crate::compile::types::PrTriggerConfig {
                branches: Some(crate::compile::types::BranchFilter {
                    include: vec!["$(System.AccessToken)".to_string()],
                    exclude: vec![],
                }),
                paths: None,
                filters: None,
                ..Default::default()
            }),
            schedule: None,
        });
        let result = validate_front_matter_identity(&fm);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("ADO expression"));
    }

    #[test]
    fn test_validate_front_matter_identity_allows_valid_pr_branches_and_paths() {
        let mut fm = minimal_front_matter();
        fm.on_config = Some(OnConfig {
            pipeline: None,
            pr: Some(crate::compile::types::PrTriggerConfig {
                branches: Some(crate::compile::types::BranchFilter {
                    include: vec!["main".to_string(), "release/*".to_string()],
                    exclude: vec!["feature/*".to_string()],
                }),
                paths: Some(crate::compile::types::PathFilter {
                    include: vec!["src/**".to_string()],
                    exclude: vec!["tests/**".to_string()],
                }),
                filters: None,
                ..Default::default()
            }),
            schedule: None,
        });
        let result = validate_front_matter_identity(&fm);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_front_matter_identity_allows_valid_name_and_description() {
        let mut fm = minimal_front_matter();
        fm.name = "Daily Code Review Agent".to_string();
        fm.description = "Reviews code daily for quality issues".to_string();
        let result = validate_front_matter_identity(&fm);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_front_matter_identity_allows_valid_trigger_pipeline_fields() {
        let mut fm = minimal_front_matter();
        fm.on_config = Some(OnConfig {
            pipeline: Some(crate::compile::types::PipelineTrigger {
                name: "Build Pipeline".to_string(),
                project: Some("OtherProject".to_string()),
                branches: vec!["main".to_string(), "release/*".to_string()],
                filters: None,
            }),
            pr: None,
            schedule: None,
        });
        let result = validate_front_matter_identity(&fm);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_front_matter_identity_rejects_runtime_expression() {
        let mut fm = minimal_front_matter();
        fm.name = "Agent $[variables['System.AccessToken']]".to_string();
        let result = validate_front_matter_identity(&fm);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("ADO expression"));
    }

    #[test]
    fn test_validate_front_matter_identity_rejects_ado_expression_in_workspace() {
        let mut fm = minimal_front_matter();
        fm.workspace = Some("$(System.AccessToken)".to_string());
        let result = validate_front_matter_identity(&fm);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("workspace"));
    }

    #[test]
    fn test_validate_front_matter_identity_rejects_ado_expression_in_trigger_pipeline_name() {
        let mut fm = minimal_front_matter();
        fm.on_config = Some(OnConfig {
            pipeline: Some(crate::compile::types::PipelineTrigger {
                name: "Build $(System.AccessToken)".to_string(),
                project: None,
                branches: vec![],
                filters: None,
            }),
            pr: None,
            schedule: None,
        });
        let result = validate_front_matter_identity(&fm);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("ADO expression"));
    }

    #[test]
    fn test_validate_front_matter_identity_rejects_ado_expression_in_trigger_pipeline_project() {
        let mut fm = minimal_front_matter();
        fm.on_config = Some(OnConfig {
            pipeline: Some(crate::compile::types::PipelineTrigger {
                name: "Build Pipeline".to_string(),
                project: Some("$(System.AccessToken)".to_string()),
                branches: vec![],
                filters: None,
            }),
            pr: None,
            schedule: None,
        });
        let result = validate_front_matter_identity(&fm);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("ADO expression"));
    }

    #[test]
    fn test_validate_front_matter_identity_rejects_ado_expression_in_trigger_pipeline_branch() {
        let mut fm = minimal_front_matter();
        fm.on_config = Some(OnConfig {
            pipeline: Some(crate::compile::types::PipelineTrigger {
                name: "Build Pipeline".to_string(),
                project: None,
                branches: vec!["$[variables['token']]".to_string()],
                filters: None,
            }),
            pr: None,
            schedule: None,
        });
        let result = validate_front_matter_identity(&fm);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("ADO expression"));
    }

    // ─── generate_prepare_steps ──────────────────────────────────────────────

    // ─── generate_awf_mounts ──────────────────────────────────────────────

    #[test]
    fn test_generate_awf_mounts_no_extensions() {
        // Even with a minimal front matter, the always-on Azure CLI
        // extension contributes a `$(AW_AZ_MOUNTS) \` injection line
        // (no static mounts — those are runtime-detected by the
        // AzureCli prepare step which sets the pipeline variable).
        // The "no mounts" name is historical; this test now verifies
        // the always-on baseline.
        let fm = minimal_front_matter();
        let exts = crate::compile::extensions::collect_extensions(&fm);
        let _ctx = crate::compile::extensions::CompileContext::for_test(&fm);
        let declarations = extension_declarations(&exts, &fm);
        let result = generate_awf_mounts(&exts, &declarations);
        assert!(
            result.contains("$(AW_AZ_MOUNTS) \\"),
            "always-on Azure CLI injection line $(AW_AZ_MOUNTS) \\ should be present \
             (so the AzureCli prepare step's pipeline variable expands into runtime mounts): {result}"
        );
        assert!(
            !result.contains(r#"--mount "/opt/az:/opt/az:ro""#),
            "must NOT emit a static /opt/az --mount — that would crash docker run on \
             runners without azure-cli. The mount is contributed via $(AW_AZ_MOUNTS) instead: {result}"
        );
        assert!(
            result.ends_with(" \\"),
            "result should end with a backslash continuation: {result}"
        );
    }

    // ─── generate_awf_path_step ──────────────────────────────────────────────

    #[test]
    fn test_generate_awf_path_step_no_paths() {
        let result = generate_awf_path_step(&[]);
        assert!(
            result.is_empty(),
            "no path prepends should produce empty string"
        );
    }

    #[test]
    fn test_generate_awf_path_step_with_lean() {
        let (fm, _) =
            parse_markdown("---\nname: test\ndescription: test\nruntimes:\n  lean: true\n---\n")
                .unwrap();
        let (_extensions, declarations) = collect_exts_and_decls_with_org(&fm, "myorg");
        let paths = collect_awf_path_prepends(&declarations);
        let result = generate_awf_path_step(&paths);
        assert!(
            result.contains("ado-path-entries"),
            "should reference path entries file"
        );
        assert!(result.contains(".elan/bin"), "should include elan bin path");
        assert!(
            result.contains("GITHUB_PATH"),
            "should set GITHUB_PATH variable"
        );
        assert!(
            result.contains("displayName"),
            "should be a complete pipeline step"
        );
        assert!(
            result.contains("AWF_PATH_EOF"),
            "should use heredoc markers"
        );
    }

    #[test]
    fn test_generate_awf_path_step_multi_path_indentation() {
        let paths = vec![
            "$HOME/.elan/bin".to_string(),
            "$HOME/.other-tool/bin".to_string(),
        ];
        let result = generate_awf_path_step(&paths);
        // Every path line must have consistent 4-space indentation
        for path in &paths {
            assert!(
                result.contains(&format!("    {path}")),
                "path '{path}' should have 4-space indentation"
            );
        }
    }

    // ─── generate_awf_path_env ──────────────────────────────────────────────

    #[test]
    fn test_generate_awf_path_env_no_paths() {
        let result = generate_awf_path_env(false);
        assert!(
            result.is_empty(),
            "no path prepends should produce empty string"
        );
    }

    #[test]
    fn test_generate_awf_path_env_with_paths() {
        let result = generate_awf_path_env(true);
        assert_eq!(result, "GITHUB_PATH: $(GITHUB_PATH)");
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Tests moved from standalone.rs — MCPG config, docker env, validation
    // ═══════════════════════════════════════════════════════════════════════

    #[test]
    fn test_generate_firewall_config_custom_mcp() {
        let mut fm = minimal_front_matter();
        fm.mcp_servers.insert(
            "my-tool".to_string(),
            McpConfig::WithOptions(Box::new(McpOptions {
                container: Some("node:20-slim".to_string()),
                entrypoint: Some("node".to_string()),
                entrypoint_args: vec!["server.js".to_string()],
                allowed: vec!["do_thing".to_string()],
                ..Default::default()
            })),
        );
        let config = generate_mcpg_config(&fm, &collect_exts_and_decls(&fm).1).unwrap();
        let server = config.mcp_servers.get("my-tool").unwrap();
        assert_eq!(server.server_type, "stdio");
        assert_eq!(server.container.as_ref().unwrap(), "node:20-slim");
        assert_eq!(server.entrypoint.as_ref().unwrap(), "node");
        assert_eq!(server.entrypoint_args.as_ref().unwrap(), &vec!["server.js"]);
        assert_eq!(
            server.tools.as_ref().unwrap(),
            &vec!["do_thing".to_string()]
        );
    }

    #[test]
    fn test_generate_mcpg_config_mcp_without_transport_skipped() {
        let mut fm = minimal_front_matter();
        // An MCP with no container or url should be skipped
        fm.mcp_servers
            .insert("phantom".to_string(), McpConfig::Enabled(true));
        let config = generate_mcpg_config(&fm, &collect_exts_and_decls(&fm).1).unwrap();
        assert!(!config.mcp_servers.contains_key("phantom"));
        // safeoutputs is always present
        assert!(config.mcp_servers.contains_key("safeoutputs"));
    }

    #[test]
    fn test_generate_mcpg_config_disabled_mcp_skipped() {
        let mut fm = minimal_front_matter();
        fm.mcp_servers
            .insert("my-tool".to_string(), McpConfig::Enabled(false));
        let config = generate_mcpg_config(&fm, &collect_exts_and_decls(&fm).1).unwrap();
        assert!(!config.mcp_servers.contains_key("my-tool"));
    }

    #[test]
    fn test_generate_mcpg_config_empty_mcp_servers() {
        let fm = minimal_front_matter();
        let config = generate_mcpg_config(&fm, &collect_exts_and_decls(&fm).1).unwrap();
        // Only safeoutputs should be present
        assert_eq!(config.mcp_servers.len(), 1);
        assert!(config.mcp_servers.contains_key("safeoutputs"));
    }

    #[test]
    fn test_generate_mcpg_config_gateway_defaults() {
        let fm = minimal_front_matter();
        let config = generate_mcpg_config(&fm, &collect_exts_and_decls(&fm).1).unwrap();
        assert_eq!(config.gateway.port, 80);
        assert_eq!(config.gateway.domain, "host.docker.internal");
        assert_eq!(config.gateway.api_key, "${MCP_GATEWAY_API_KEY}");
        assert_eq!(config.gateway.payload_dir, "/tmp/gh-aw/mcp-payloads");
    }

    #[test]
    fn test_generate_mcpg_config_json_roundtrip() {
        let mut fm = minimal_front_matter();
        fm.mcp_servers.insert(
            "my-tool".to_string(),
            McpConfig::WithOptions(Box::new(McpOptions {
                container: Some("python:3.12-slim".to_string()),
                entrypoint: Some("python".to_string()),
                entrypoint_args: vec!["-m".to_string(), "server".to_string()],
                allowed: vec!["query".to_string()],
                ..Default::default()
            })),
        );
        let config = generate_mcpg_config(&fm, &collect_exts_and_decls(&fm).1).unwrap();
        let json = serde_json::to_string_pretty(&config).expect("Config should serialize to JSON");
        let parsed: serde_json::Value =
            serde_json::from_str(&json).expect("Serialized JSON should parse back");

        // Verify top-level structure matches MCPG expectation
        assert!(
            parsed.get("mcpServers").is_some(),
            "Should have mcpServers key"
        );
        assert!(parsed.get("gateway").is_some(), "Should have gateway key");

        let gw = parsed.get("gateway").unwrap();
        assert!(gw.get("port").is_some(), "Gateway should have port");
        assert!(gw.get("domain").is_some(), "Gateway should have domain");
        assert!(gw.get("apiKey").is_some(), "Gateway should have apiKey");
        assert!(
            gw.get("payloadDir").is_some(),
            "Gateway should have payloadDir"
        );
    }

    #[test]
    fn test_generate_mcpg_config_safeoutputs_variable_placeholders() {
        let fm = minimal_front_matter();
        let config = generate_mcpg_config(&fm, &collect_exts_and_decls(&fm).1).unwrap();
        let so = config.mcp_servers.get("safeoutputs").unwrap();

        // URL should reference the runtime-substituted port
        let url = so.url.as_ref().unwrap();
        assert!(
            url.contains("${SAFE_OUTPUTS_PORT}"),
            "SafeOutputs URL should use ${{SAFE_OUTPUTS_PORT}} placeholder, got: {url}"
        );

        // Auth header should reference the runtime-substituted API key
        let headers = so.headers.as_ref().unwrap();
        let auth = headers.get("Authorization").unwrap();
        assert!(
            auth.contains("${SAFE_OUTPUTS_API_KEY}"),
            "SafeOutputs auth header should use ${{SAFE_OUTPUTS_API_KEY}} placeholder, got: {auth}"
        );
    }

    #[test]
    fn test_generate_mcpg_config_safeoutputs_is_http_type() {
        let fm = minimal_front_matter();
        let config = generate_mcpg_config(&fm, &collect_exts_and_decls(&fm).1).unwrap();
        let so = config.mcp_servers.get("safeoutputs").unwrap();
        assert_eq!(so.server_type, "http");
        assert!(
            so.container.is_none(),
            "HTTP backend should have no container"
        );
        assert!(so.args.is_none(), "HTTP backend should have no args");
        assert!(so.url.is_some(), "HTTP backend must have a URL");
    }

    #[test]
    fn test_generate_mcpg_config_container_mcp_is_stdio_type() {
        let mut fm = minimal_front_matter();
        fm.mcp_servers.insert(
            "runner".to_string(),
            McpConfig::WithOptions(Box::new(McpOptions {
                container: Some("node:20-slim".to_string()),
                entrypoint: Some("node".to_string()),
                entrypoint_args: vec!["srv.js".to_string()],
                allowed: vec!["run".to_string()],
                ..Default::default()
            })),
        );
        let config = generate_mcpg_config(&fm, &collect_exts_and_decls(&fm).1).unwrap();
        let srv = config.mcp_servers.get("runner").unwrap();
        assert_eq!(srv.server_type, "stdio");
        assert!(
            srv.container.is_some(),
            "stdio server must have a container"
        );
        assert!(srv.url.is_none(), "stdio server should have no URL");
    }

    #[test]
    fn test_generate_mcpg_config_container_with_env() {
        let mut fm = minimal_front_matter();
        let mut env = HashMap::new();
        env.insert("TOKEN".to_string(), "secret".to_string());
        fm.mcp_servers.insert(
            "with-env".to_string(),
            McpConfig::WithOptions(Box::new(McpOptions {
                container: Some("node:20-slim".to_string()),
                env,
                ..Default::default()
            })),
        );
        let config = generate_mcpg_config(&fm, &collect_exts_and_decls(&fm).1).unwrap();
        let srv = config.mcp_servers.get("with-env").unwrap();
        let e = srv.env.as_ref().unwrap();
        assert_eq!(e.get("TOKEN").unwrap(), "secret");
    }

    #[test]
    fn test_generate_mcpg_config_reserved_safeoutputs_name_rejected() {
        let mut fm = minimal_front_matter();
        fm.mcp_servers.insert(
            "safeoutputs".to_string(),
            McpConfig::WithOptions(Box::new(McpOptions {
                container: Some("evil:latest".to_string()),
                ..Default::default()
            })),
        );
        let config = generate_mcpg_config(&fm, &collect_exts_and_decls(&fm).1).unwrap();
        // The reserved entry should still be the HTTP backend, not the user's container
        let so = config.mcp_servers.get("safeoutputs").unwrap();
        assert_eq!(
            so.server_type, "http",
            "safeoutputs should remain HTTP backend"
        );
        assert!(
            so.container.is_none(),
            "User container should not overwrite safeoutputs"
        );
    }

    #[test]
    fn test_generate_mcpg_config_safeoutputs_reserved_name_skipped() {
        let mut fm = minimal_front_matter();
        fm.mcp_servers.insert(
            "SafeOutputs".to_string(),
            McpConfig::WithOptions(Box::new(McpOptions {
                container: Some("node:20-slim".to_string()),
                entrypoint: Some("node".to_string()),
                entrypoint_args: vec!["evil.js".to_string()],
                allowed: vec!["hijack".to_string()],
                ..Default::default()
            })),
        );
        let config = generate_mcpg_config(&fm, &collect_exts_and_decls(&fm).1).unwrap();
        // The user-defined "SafeOutputs" must not overwrite the built-in entry
        let so = config.mcp_servers.get("safeoutputs").unwrap();
        assert_eq!(so.server_type, "http");
        assert!(so.url.as_ref().unwrap().contains("localhost"));
        // No stdio entry should have been added under any casing
        assert_eq!(config.mcp_servers.len(), 1);
    }

    #[test]
    fn test_generate_mcpg_config_http_mcp() {
        let mut fm = minimal_front_matter();
        fm.mcp_servers.insert(
            "remote".to_string(),
            McpConfig::WithOptions(Box::new(McpOptions {
                url: Some("https://mcp.example.com/api".to_string()),
                headers: {
                    let mut h = HashMap::new();
                    h.insert("X-Custom".to_string(), "value".to_string());
                    h
                },
                allowed: vec!["query".to_string()],
                ..Default::default()
            })),
        );
        let config = generate_mcpg_config(&fm, &collect_exts_and_decls(&fm).1).unwrap();
        let srv = config.mcp_servers.get("remote").unwrap();
        assert_eq!(srv.server_type, "http");
        assert_eq!(srv.url.as_ref().unwrap(), "https://mcp.example.com/api");
        assert_eq!(
            srv.headers.as_ref().unwrap().get("X-Custom").unwrap(),
            "value"
        );
        assert!(
            srv.container.is_none(),
            "HTTP server should have no container"
        );
    }

    #[test]
    fn test_generate_mcpg_config_container_with_entrypoint() {
        let mut fm = minimal_front_matter();
        fm.mcp_servers.insert(
            "ado".to_string(),
            McpConfig::WithOptions(Box::new(McpOptions {
                container: Some("node:20-slim".to_string()),
                entrypoint: Some("npx".to_string()),
                entrypoint_args: vec!["-y".to_string(), "@azure-devops/mcp".to_string()],
                ..Default::default()
            })),
        );
        let config = generate_mcpg_config(&fm, &collect_exts_and_decls(&fm).1).unwrap();
        let srv = config.mcp_servers.get("ado").unwrap();
        assert_eq!(srv.server_type, "stdio");
        assert_eq!(srv.container.as_ref().unwrap(), "node:20-slim");
        assert_eq!(srv.entrypoint.as_ref().unwrap(), "npx");
        assert_eq!(
            srv.entrypoint_args.as_ref().unwrap(),
            &vec!["-y", "@azure-devops/mcp"]
        );
    }

    #[test]
    fn test_generate_mcpg_config_container_with_mounts() {
        let mut fm = minimal_front_matter();
        fm.mcp_servers.insert(
            "data-tool".to_string(),
            McpConfig::WithOptions(Box::new(McpOptions {
                container: Some("data-tool:latest".to_string()),
                mounts: vec!["/host/data:/app/data:ro".to_string()],
                ..Default::default()
            })),
        );
        let config = generate_mcpg_config(&fm, &collect_exts_and_decls(&fm).1).unwrap();
        let srv = config.mcp_servers.get("data-tool").unwrap();
        assert_eq!(
            srv.mounts.as_ref().unwrap(),
            &vec!["/host/data:/app/data:ro"]
        );
    }

    #[test]
    fn test_generate_mcpg_config_no_transport_skipped() {
        let mut fm = minimal_front_matter();
        // MCP with options but no container or url should be skipped
        fm.mcp_servers.insert(
            "no-transport".to_string(),
            McpConfig::WithOptions(Box::new(McpOptions {
                allowed: vec!["tool".to_string()],
                ..Default::default()
            })),
        );
        let config = generate_mcpg_config(&fm, &collect_exts_and_decls(&fm).1).unwrap();
        assert!(!config.mcp_servers.contains_key("no-transport"));
    }

    #[test]
    fn test_generate_mcpg_docker_env_with_permissions_read() {
        // When ADO tool is enabled with permissions.read, the extension's
        // required_pipeline_vars should produce the -e flag
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\ntools:\n  azure-devops: true\npermissions:\n  read: my-read-sc\n---\n",
        ).unwrap();
        let (_extensions, declarations) = collect_exts_and_decls_with_org(&fm, "myorg");
        let env = generate_mcpg_docker_env(&fm, &declarations);
        assert!(
            env.contains("-e ADO_MCP_AUTH_TOKEN=\"$SC_READ_TOKEN\""),
            "Should map ADO token via extension pipeline var"
        );
    }

    #[test]
    fn test_generate_mcpg_docker_env_no_extensions() {
        // No tools enabled — no extension pipeline vars — only user MCP passthrough
        let fm = minimal_front_matter();
        let (_extensions, declarations) = collect_exts_and_decls_with_org(&fm, "myorg");
        let env = generate_mcpg_docker_env(&fm, &declarations);
        assert!(
            !env.contains("ADO_MCP_AUTH_TOKEN"),
            "Should not have ADO token when no extension needs it"
        );
    }

    #[test]
    fn test_generate_mcpg_docker_env_dedup_extension_and_user_passthrough() {
        // Extension provides ADO_MCP_AUTH_TOKEN mapping, user MCP also has it as passthrough.
        // Extension mapping should win (deduplicated).
        let (mut fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\ntools:\n  azure-devops: true\npermissions:\n  read: my-read-sc\n---\n",
        ).unwrap();
        fm.mcp_servers.insert(
            "ado-tool".to_string(),
            McpConfig::WithOptions(Box::new(McpOptions {
                container: Some("node:20-slim".to_string()),
                env: {
                    let mut e = HashMap::new();
                    e.insert("ADO_MCP_AUTH_TOKEN".to_string(), "".to_string());
                    e
                },
                ..Default::default()
            })),
        );
        let (_extensions, declarations) = collect_exts_and_decls_with_org(&fm, "myorg");
        let env = generate_mcpg_docker_env(&fm, &declarations);
        let count = env.matches("ADO_MCP_AUTH_TOKEN").count();
        assert_eq!(
            count, 1,
            "ADO_MCP_AUTH_TOKEN should appear exactly once, got {}",
            count
        );
    }

    #[test]
    fn test_generate_mcpg_docker_env_passthrough_vars() {
        let mut fm = minimal_front_matter();
        fm.mcp_servers.insert(
            "tool".to_string(),
            McpConfig::WithOptions(Box::new(McpOptions {
                container: Some("img:latest".to_string()),
                env: {
                    let mut e = HashMap::new();
                    e.insert("PASS_THROUGH".to_string(), "".to_string());
                    e.insert("STATIC".to_string(), "value".to_string());
                    e
                },
                ..Default::default()
            })),
        );
        let (_extensions, declarations) = collect_exts_and_decls_with_org(&fm, "myorg");
        let env = generate_mcpg_docker_env(&fm, &declarations);
        assert!(
            env.contains("-e PASS_THROUGH"),
            "Should include passthrough var"
        );
        assert!(!env.contains("-e STATIC"), "Should NOT include static var");
    }

    #[test]
    fn test_generate_mcpg_docker_env_rejects_invalid_names() {
        let mut fm = minimal_front_matter();
        fm.mcp_servers.insert(
            "evil".to_string(),
            McpConfig::WithOptions(Box::new(McpOptions {
                container: Some("img:latest".to_string()),
                env: {
                    let mut e = HashMap::new();
                    e.insert("MY_VAR --privileged".to_string(), "".to_string());
                    e.insert("GOOD_VAR".to_string(), "".to_string());
                    e
                },
                ..Default::default()
            })),
        );
        let (_extensions, declarations) = collect_exts_and_decls_with_org(&fm, "myorg");
        let env = generate_mcpg_docker_env(&fm, &declarations);
        assert!(
            !env.contains("--privileged"),
            "Should reject invalid env var name with Docker flag injection"
        );
        assert!(env.contains("-e GOOD_VAR"), "Should include valid env var");
    }

    // ─── generate_mcpg_step_env ──────────────────────────────────────────────

    #[test]
    fn test_generate_mcpg_step_env_with_ado_extension() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\ntools:\n  azure-devops: true\n---\n",
        )
        .unwrap();
        let (_extensions, declarations) = collect_exts_and_decls_with_org(&fm, "myorg");
        let env = generate_mcpg_step_env(&declarations);
        assert!(
            env.starts_with("env:\n"),
            "Should emit full env: block header"
        );
        assert!(
            env.contains("SC_READ_TOKEN: $(SC_READ_TOKEN)"),
            "Should map SC_READ_TOKEN for ADO extension"
        );
    }

    #[test]
    fn test_generate_mcpg_step_env_no_extensions() {
        let fm = minimal_front_matter();
        let (_extensions, declarations) = collect_exts_and_decls(&fm);
        let env = generate_mcpg_step_env(&declarations);
        assert!(
            env.is_empty(),
            "Should be empty when no extensions need pipeline vars"
        );
    }

    #[test]
    fn test_is_valid_env_var_name() {
        assert!(validate::is_valid_env_var_name("MY_VAR"));
        assert!(validate::is_valid_env_var_name("_PRIVATE"));
        assert!(validate::is_valid_env_var_name("A"));
        assert!(validate::is_valid_env_var_name("VAR123"));
        assert!(!validate::is_valid_env_var_name(""));
        assert!(!validate::is_valid_env_var_name("123ABC"));
        assert!(!validate::is_valid_env_var_name("MY-VAR"));
        assert!(!validate::is_valid_env_var_name("MY VAR"));
        assert!(!validate::is_valid_env_var_name("X --privileged"));
        assert!(!validate::is_valid_env_var_name("X -v /etc:/etc:rw"));
    }

    #[test]
    fn test_generate_mcpg_config_rejects_invalid_server_name() {
        let yaml = "---\nname: test-agent\ndescription: test\nmcp-servers:\n  bad/name:\n    container: python:3\n    entrypoint: python\n---\n";
        let (fm, _) = parse_markdown(yaml).unwrap();
        let result = generate_mcpg_config(&fm, &collect_exts_and_decls(&fm).1);
        assert!(result.is_err(), "Should reject server name with /");
    }

    #[test]
    fn test_generate_mcpg_config_rejects_dot_leading_server_name() {
        // ".." would resolve to /mcp via path normalization, bypassing routing
        let yaml = "---\nname: test-agent\ndescription: test\nmcp-servers:\n  ..:\n    container: python:3\n    entrypoint: python\n---\n";
        let (fm, _) = parse_markdown(yaml).unwrap();
        let result = generate_mcpg_config(&fm, &collect_exts_and_decls(&fm).1);
        assert!(
            result.is_err(),
            "Should reject server name starting with dot"
        );

        // ".hidden" would produce /mcp/.hidden
        let yaml2 = "---\nname: test-agent\ndescription: test\nmcp-servers:\n  .hidden:\n    container: python:3\n    entrypoint: python\n---\n";
        let (fm2, _) = parse_markdown(yaml2).unwrap();
        let result2 = generate_mcpg_config(&fm2, &collect_exts_and_decls(&fm2).1);
        assert!(
            result2.is_err(),
            "Should reject server name starting with dot"
        );
    }

    // ─── tools.azure-devops MCPG integration ────────────────────────────────

    #[test]
    fn test_ado_tool_generates_mcpg_entry() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\ntools:\n  azure-devops: true\n---\n",
        )
        .unwrap();
        // Pass inferred org since no explicit org is set
        let extensions = collect_extensions(&fm);
        let ctx = CompileContext::for_test_with_org(&fm, "inferred-org");
        let declarations = extension_declarations_with_ctx(&extensions, &ctx);
        let config = generate_mcpg_config(&fm, &declarations).unwrap();
        let ado = config.mcp_servers.get("azure-devops").unwrap();
        assert_eq!(ado.server_type, "stdio");
        assert_eq!(ado.container.as_deref(), Some(ADO_MCP_IMAGE));
        assert_eq!(ado.entrypoint.as_deref(), Some(ADO_MCP_ENTRYPOINT));
        let args = ado.entrypoint_args.as_ref().unwrap();
        assert!(args.contains(&"-y".to_string()));
        assert!(args.contains(&ADO_MCP_PACKAGE.to_string()));
        assert!(args.contains(&"inferred-org".to_string()));
        // Should have ADO_MCP_AUTH_TOKEN in env (for bearer token via envvar auth)
        let env = ado.env.as_ref().unwrap();
        assert!(env.contains_key("ADO_MCP_AUTH_TOKEN"));
    }

    #[test]
    fn test_ado_tool_with_toolsets() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\ntools:\n  azure-devops:\n    toolsets: [repos, wit, core]\n---\n",
        )
        .unwrap();
        let extensions = collect_extensions(&fm);
        let ctx = CompileContext::for_test_with_org(&fm, "myorg");
        let declarations = extension_declarations_with_ctx(&extensions, &ctx);
        let config = generate_mcpg_config(&fm, &declarations).unwrap();
        let ado = config.mcp_servers.get("azure-devops").unwrap();
        let args = ado.entrypoint_args.as_ref().unwrap();
        assert!(args.contains(&"-d".to_string()));
        assert!(args.contains(&"repos".to_string()));
        assert!(args.contains(&"wit".to_string()));
        assert!(args.contains(&"core".to_string()));
    }

    #[test]
    fn test_ado_tool_with_org_override() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\ntools:\n  azure-devops:\n    org: myorg\n---\n",
        )
        .unwrap();
        // Explicit org should be used even when inferred_org is None
        let config = generate_mcpg_config(&fm, &collect_exts_and_decls(&fm).1).unwrap();
        let ado = config.mcp_servers.get("azure-devops").unwrap();
        let args = ado.entrypoint_args.as_ref().unwrap();
        assert!(args.contains(&"myorg".to_string()));
    }

    #[test]
    fn test_ado_tool_explicit_org_overrides_inferred() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\ntools:\n  azure-devops:\n    org: explicit-org\n---\n",
        )
        .unwrap();
        let extensions = collect_extensions(&fm);
        let ctx = CompileContext::for_test_with_org(&fm, "inferred-org");
        let declarations = extension_declarations_with_ctx(&extensions, &ctx);
        let config = generate_mcpg_config(&fm, &declarations).unwrap();
        let ado = config.mcp_servers.get("azure-devops").unwrap();
        let args = ado.entrypoint_args.as_ref().unwrap();
        assert!(args.contains(&"explicit-org".to_string()));
        assert!(!args.contains(&"inferred-org".to_string()));
    }

    #[test]
    fn test_ado_tool_no_org_fails() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\ntools:\n  azure-devops: true\n---\n",
        )
        .unwrap();
        // No explicit org and no inferred org — should fail
        let extensions = collect_extensions(&fm);
        let ctx = CompileContext::for_test(&fm);
        let result = try_extension_declarations_with_ctx(&extensions, &ctx);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("no ADO organization"),
            "Error should mention missing org"
        );
    }

    #[test]
    fn test_ado_tool_invalid_org_fails() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\ntools:\n  azure-devops:\n    org: \"my org/bad\"\n---\n",
        )
        .unwrap();
        let extensions = collect_extensions(&fm);
        let ctx = CompileContext::for_test(&fm);
        let result = try_extension_declarations_with_ctx(&extensions, &ctx);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Invalid ADO org name"),
            "Error should mention invalid org"
        );
    }

    #[test]
    fn test_ado_tool_invalid_toolset_fails() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\ntools:\n  azure-devops:\n    org: myorg\n    toolsets: [\"repos\", \"bad toolset\"]\n---\n",
        )
        .unwrap();
        let extensions = collect_extensions(&fm);
        let ctx = CompileContext::for_test(&fm);
        let result = try_extension_declarations_with_ctx(&extensions, &ctx);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Invalid ADO toolset name"),
            "Error should mention invalid toolset"
        );
    }

    #[test]
    fn test_ado_tool_with_allowed_tools() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\ntools:\n  azure-devops:\n    org: myorg\n    allowed:\n      - wit_get_work_item\n      - core_list_projects\n---\n",
        )
        .unwrap();
        let config = generate_mcpg_config(&fm, &collect_exts_and_decls(&fm).1).unwrap();
        let ado = config.mcp_servers.get("azure-devops").unwrap();
        let tools = ado.tools.as_ref().unwrap();
        assert_eq!(tools, &["wit_get_work_item", "core_list_projects"]);
    }

    #[test]
    fn test_ado_tool_disabled_not_generated() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\ntools:\n  azure-devops: false\n---\n",
        )
        .unwrap();
        let config = generate_mcpg_config(&fm, &collect_exts_and_decls(&fm).1).unwrap();
        assert!(!config.mcp_servers.contains_key("azure-devops"));
    }

    #[test]
    fn test_ado_tool_not_set_not_generated() {
        let fm = minimal_front_matter();
        let config = generate_mcpg_config(&fm, &collect_exts_and_decls(&fm).1).unwrap();
        assert!(!config.mcp_servers.contains_key("azure-devops"));
    }

    #[test]
    fn test_ado_tool_skips_manual_mcp_entry() {
        // When tools.azure-devops is enabled AND mcp-servers also has azure-devops,
        // the tools config takes precedence and the manual entry is skipped.
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\ntools:\n  azure-devops:\n    org: auto-org\nmcp-servers:\n  azure-devops:\n    container: \"node:20-slim\"\n    entrypoint: \"npx\"\n    entrypoint-args: [\"-y\", \"@azure-devops/mcp\", \"manual-org\"]\n---\n",
        )
        .unwrap();
        let config = generate_mcpg_config(&fm, &collect_exts_and_decls(&fm).1).unwrap();
        let ado = config.mcp_servers.get("azure-devops").unwrap();
        // Should use the auto-configured org, not the manual one
        let args = ado.entrypoint_args.as_ref().unwrap();
        assert!(args.contains(&"auto-org".to_string()));
        assert!(!args.contains(&"manual-org".to_string()));
    }

    #[test]
    fn test_ado_tool_docker_env_passthrough() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\ntools:\n  azure-devops: true\npermissions:\n  read: my-read-sc\n---\n",
        )
        .unwrap();
        let (_extensions, declarations) = collect_exts_and_decls_with_org(&fm, "myorg");
        let env = generate_mcpg_docker_env(&fm, &declarations);
        assert!(
            env.contains("ADO_MCP_AUTH_TOKEN"),
            "Should include ADO token passthrough when permissions.read is set"
        );
    }

    // ─── validate_docker_args ────────────────────────────────────────────────

    #[test]
    fn test_validate_docker_args_privileged_flag() {
        let warnings = validate::validate_docker_args(&["--privileged".to_string()], "my-mcp");
        assert_eq!(warnings.len(), 1);
        assert!(
            warnings[0].contains("--privileged"),
            "should warn about --privileged"
        );
    }

    #[test]
    fn test_validate_docker_args_entrypoint_in_args_warns() {
        let warnings = validate::validate_docker_args(
            &["--entrypoint".to_string(), "/bin/sh".to_string()],
            "my-mcp",
        );
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("--entrypoint") && w.contains("entrypoint:")),
            "should warn about --entrypoint with hint to use entrypoint: field"
        );
    }

    #[test]
    fn test_validate_docker_args_volume_flag_calls_mount_validation() {
        // -v docker.sock in args bypasses `mounts:` validation; should produce warnings
        let warnings = validate::validate_docker_args(
            &[
                "-v".to_string(),
                "/var/run/docker.sock:/var/run/docker.sock".to_string(),
            ],
            "my-mcp",
        );
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("bypasses mounts validation")),
            "should warn about volume mount in args"
        );
        assert!(
            warnings.iter().any(|w| w.contains("Docker socket")),
            "should propagate mount source warning for docker.sock"
        );
    }

    #[test]
    fn test_validate_docker_args_volume_equals_form() {
        // --volume=source:dest form should also be detected
        let warnings = validate::validate_docker_args(
            &["--volume=/var/run/docker.sock:/var/run/docker.sock".to_string()],
            "my-mcp",
        );
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("bypasses mounts validation")),
            "should warn about --volume= form"
        );
    }

    #[test]
    fn test_validate_docker_args_safe_args_no_warnings() {
        // A legitimate arg like --read-only should produce no warnings
        let warnings = validate::validate_docker_args(&["--read-only".to_string()], "my-mcp");
        assert!(warnings.is_empty(), "safe args should not produce warnings");
    }

    #[test]
    fn test_validate_docker_args_empty_no_warnings() {
        let warnings = validate::validate_docker_args(&[], "my-mcp");
        assert!(
            warnings.is_empty(),
            "empty args should not produce warnings"
        );
    }

    #[test]
    fn test_validate_docker_args_volume_flag_trailing_warns() {
        // -v as the last arg with no mount spec is malformed
        let warnings = validate::validate_docker_args(&["-v".to_string()], "my-mcp");
        assert_eq!(warnings.len(), 1);
        assert!(
            warnings[0].contains("malformed"),
            "trailing -v with no mount spec should warn"
        );
    }

    #[test]
    fn test_validate_docker_args_long_volume_flag_trailing_warns() {
        // --volume as the last arg with no mount spec is malformed
        let warnings = validate::validate_docker_args(&["--volume".to_string()], "my-mcp");
        assert_eq!(warnings.len(), 1);
        assert!(
            warnings[0].contains("malformed"),
            "trailing --volume with no mount spec should warn"
        );
    }

    // ─── validate_mcp_url ────────────────────────────────────────────────────

    #[test]
    fn test_validate_mcp_url_https_no_warnings() {
        let warnings = validate::validate_mcp_url("https://mcp.dev.azure.com/myorg", "my-mcp");
        assert!(warnings.is_empty(), "https URL should not produce warnings");
    }

    #[test]
    fn test_validate_mcp_url_http_no_warnings() {
        let warnings = validate::validate_mcp_url("http://localhost:8100/mcp", "my-mcp");
        assert!(warnings.is_empty(), "http URL should not produce warnings");
    }

    #[test]
    fn test_validate_mcp_url_bad_scheme_warns() {
        let warnings = validate::validate_mcp_url("ftp://files.example.com", "my-mcp");
        assert_eq!(warnings.len(), 1);
        assert!(
            warnings[0].contains("does not use http://"),
            "non-HTTP scheme should warn"
        );
    }

    #[test]
    fn test_validate_mcp_url_no_scheme_warns() {
        let warnings = validate::validate_mcp_url("mcp.dev.azure.com/myorg", "my-mcp");
        assert_eq!(warnings.len(), 1);
        assert!(
            warnings[0].contains("does not use http://"),
            "URL without scheme should warn"
        );
    }

    // ─── validate_mount_source ───────────────────────────────────────────────

    #[test]
    fn test_validate_mount_source_docker_sock() {
        let warnings = validate::validate_mount_source(
            "/var/run/docker.sock:/var/run/docker.sock:rw",
            "my-mcp",
        );
        assert_eq!(warnings.len(), 1);
        assert!(
            warnings[0].contains("Docker socket"),
            "should warn about Docker socket exposure"
        );
    }

    #[test]
    fn test_validate_mount_source_sensitive_path_etc() {
        let warnings = validate::validate_mount_source("/etc/passwd:/data/passwd:ro", "my-mcp");
        assert_eq!(warnings.len(), 1);
        assert!(
            warnings[0].contains("sensitive host path"),
            "should warn about /etc mount"
        );
    }

    #[test]
    fn test_validate_mount_source_sensitive_path_proc() {
        let warnings = validate::validate_mount_source("/proc:/host/proc:ro", "my-mcp");
        assert_eq!(warnings.len(), 1);
        assert!(
            warnings[0].contains("sensitive host path"),
            "should warn about /proc mount"
        );
    }

    #[test]
    fn test_validate_mount_source_case_insensitive() {
        // /ETC/shadow should match sensitive /etc prefix (lowercased comparison)
        let warnings = validate::validate_mount_source("/ETC/shadow:/data/shadow:ro", "my-mcp");
        assert_eq!(warnings.len(), 1);
        assert!(
            warnings[0].contains("sensitive host path"),
            "case-insensitive match should trigger warning"
        );
    }

    #[test]
    fn test_validate_mount_source_no_false_positive_on_etc_configs() {
        // /etc-configs should NOT match the /etc prefix (path boundary check requires trailing /)
        let warnings = validate::validate_mount_source("/etc-configs:/app/config:ro", "my-mcp");
        assert!(
            warnings.is_empty(),
            "/etc-configs must not match /etc prefix due to path boundary check"
        );
    }

    #[test]
    fn test_validate_mount_source_safe_path_no_warnings() {
        // /app/data is not a sensitive path; should produce no warnings
        let warnings = validate::validate_mount_source("/app/data:/app/data:ro", "my-mcp");
        assert!(warnings.is_empty(), "safe path should not produce warnings");
    }

    // ─── validate_container_image ────────────────────────────────────────────

    #[test]
    fn test_validate_container_image_empty_string() {
        let warnings = validate::validate_container_image("", "my-mcp");
        assert_eq!(warnings.len(), 1);
        assert!(
            warnings[0].contains("empty"),
            "should warn about empty image name"
        );
    }

    #[test]
    fn test_validate_container_image_shell_metacharacters() {
        let warnings = validate::validate_container_image("node:20-slim; rm -rf /", "my-mcp");
        assert_eq!(warnings.len(), 1);
        assert!(
            warnings[0].contains("unexpected characters"),
            "should warn about shell metacharacters"
        );
    }

    #[test]
    fn test_validate_container_image_valid_name_no_warnings() {
        // Standard image references should produce no warnings
        assert!(validate::validate_container_image("node:20-slim", "my-mcp").is_empty());
        assert!(
            validate::validate_container_image("ghcr.io/org/image:latest", "my-mcp").is_empty()
        );
        assert!(validate::validate_container_image("python:3.12-slim", "my-mcp").is_empty());
    }

    // ─── warn_potential_secrets ──────────────────────────────────────────────

    #[test]
    fn test_warn_potential_secrets_token_env_var_triggers() {
        let env = HashMap::from([("API_TOKEN".to_string(), "secret123".to_string())]);
        let headers = HashMap::new();
        let warnings = validate::warn_potential_secrets("my-mcp", &env, &headers);
        assert_eq!(warnings.len(), 1);
        assert!(
            warnings[0].contains("API_TOKEN"),
            "should warn about secret-looking env var"
        );
    }

    #[test]
    fn test_warn_potential_secrets_empty_passthrough_no_warnings() {
        // Empty string = passthrough; should NOT trigger a warning
        let env = HashMap::from([("API_TOKEN".to_string(), "".to_string())]);
        let headers = HashMap::new();
        let warnings = validate::warn_potential_secrets("my-mcp", &env, &headers);
        assert!(
            warnings.is_empty(),
            "empty passthrough value must not trigger a warning"
        );
    }

    #[test]
    fn test_warn_potential_secrets_authorization_header_triggers() {
        let env = HashMap::new();
        let headers = HashMap::from([("Authorization".to_string(), "Bearer abc".to_string())]);
        let warnings = validate::warn_potential_secrets("my-mcp", &env, &headers);
        assert_eq!(warnings.len(), 1);
        assert!(
            warnings[0].contains("Authorization"),
            "should warn about Authorization header"
        );
    }

    #[test]
    fn test_warn_potential_secrets_bearer_value_triggers() {
        // A header whose value starts with "Bearer " should also warn
        let env = HashMap::new();
        let headers = HashMap::from([("X-Custom-Auth".to_string(), "Bearer token123".to_string())]);
        let warnings = validate::warn_potential_secrets("my-mcp", &env, &headers);
        assert_eq!(warnings.len(), 1);
        assert!(
            warnings[0].contains("X-Custom-Auth"),
            "should warn about header with Bearer value"
        );
    }

    #[test]
    fn test_warn_potential_secrets_safe_env_no_warnings() {
        // Env keys with non-secret names and non-empty values should produce no warnings
        let env = HashMap::from([("MY_CONFIG".to_string(), "value".to_string())]);
        let headers = HashMap::new();
        let warnings = validate::warn_potential_secrets("my-mcp", &env, &headers);
        assert!(
            warnings.is_empty(),
            "non-secret env var should not produce warnings"
        );
    }

    // ─── standalone setup/teardown/finalize/checkout/repositories generators ───

    // ──────────────────────────────────────────────────────────────────────
    // Tests for compact `repos:` lowering
    // ──────────────────────────────────────────────────────────────────────

    use super::{derive_alias, lower_repos, parse_shorthand, resolve_repos};
    use crate::compile::types::{RepoEntry, ReposItem};

    #[test]
    fn test_repos_shorthand_simple() {
        let items = vec![ReposItem::Shorthand("my-org/my-repo".to_string())];
        let (repos, checkout) = lower_repos(&items).unwrap();
        assert_eq!(repos.len(), 1);
        assert_eq!(repos[0].repository, "my-repo");
        assert_eq!(repos[0].name, "my-org/my-repo");
        assert_eq!(repos[0].repo_type, "git");
        assert_eq!(repos[0].repo_ref, "refs/heads/main");
        assert_eq!(checkout, vec!["my-repo"]);
    }

    #[test]
    fn test_repos_shorthand_with_alias() {
        let items = vec![ReposItem::Shorthand(
            "schemas=my-org/internal-schemas".to_string(),
        )];
        let (repos, checkout) = lower_repos(&items).unwrap();
        assert_eq!(repos.len(), 1);
        assert_eq!(repos[0].repository, "schemas");
        assert_eq!(repos[0].name, "my-org/internal-schemas");
        assert_eq!(checkout, vec!["schemas"]);
    }

    #[test]
    fn test_repos_object_form_defaults() {
        let items = vec![ReposItem::Full(RepoEntry {
            name: "my-org/docs".to_string(),
            alias: None,
            repo_type: "git".to_string(),
            repo_ref: "refs/heads/main".to_string(),
            checkout: true,
        })];
        let (repos, checkout) = lower_repos(&items).unwrap();
        assert_eq!(repos[0].repository, "docs");
        assert_eq!(repos[0].name, "my-org/docs");
        assert_eq!(checkout, vec!["docs"]);
    }

    #[test]
    fn test_repos_object_form_no_checkout() {
        let items = vec![ReposItem::Full(RepoEntry {
            name: "my-org/big-monorepo".to_string(),
            alias: None,
            repo_type: "git".to_string(),
            repo_ref: "refs/heads/main".to_string(),
            checkout: false,
        })];
        let (repos, checkout) = lower_repos(&items).unwrap();
        assert_eq!(repos.len(), 1);
        assert_eq!(repos[0].repository, "big-monorepo");
        assert!(checkout.is_empty());
    }

    #[test]
    fn test_repos_object_form_custom_ref() {
        let items = vec![ReposItem::Full(RepoEntry {
            name: "my-org/docs".to_string(),
            alias: Some("docs-v2".to_string()),
            repo_type: "git".to_string(),
            repo_ref: "refs/heads/release/2.x".to_string(),
            checkout: true,
        })];
        let (repos, checkout) = lower_repos(&items).unwrap();
        assert_eq!(repos[0].repository, "docs-v2");
        assert_eq!(repos[0].repo_ref, "refs/heads/release/2.x");
        assert_eq!(checkout, vec!["docs-v2"]);
    }

    #[test]
    fn test_repos_rejects_duplicate_aliases() {
        let items = vec![
            ReposItem::Shorthand("org/tools".to_string()),
            ReposItem::Shorthand("other-org/tools".to_string()),
        ];
        let err = lower_repos(&items).unwrap_err();
        assert!(
            err.to_string()
                .contains("Duplicate repository alias 'tools'"),
            "{err}"
        );
    }

    #[test]
    fn test_repos_rejects_reserved_alias() {
        let items = vec![ReposItem::Shorthand("org/self".to_string())];
        let err = lower_repos(&items).unwrap_err();
        assert!(err.to_string().contains("reserved"), "{err}");

        let items = vec![ReposItem::Shorthand("org/repo".to_string())];
        let err = lower_repos(&items).unwrap_err();
        assert!(err.to_string().contains("reserved"), "{err}");

        // Reserved via explicit alias in shorthand form
        let items = vec![ReposItem::Shorthand("self=org/some-repo".to_string())];
        let err = lower_repos(&items).unwrap_err();
        assert!(err.to_string().contains("reserved"), "{err}");

        // Reserved via explicit alias in object form
        let items = vec![ReposItem::Full(RepoEntry {
            name: "org/fine-repo".to_string(),
            alias: Some("root".to_string()),
            repo_type: "git".to_string(),
            repo_ref: "refs/heads/main".to_string(),
            checkout: true,
        })];
        let err = lower_repos(&items).unwrap_err();
        assert!(err.to_string().contains("reserved"), "{err}");
    }

    #[test]
    fn test_repos_multiple_mixed() {
        let items = vec![
            ReposItem::Shorthand("my-org/tools".to_string()),
            ReposItem::Shorthand("schemas=my-org/internal-schemas".to_string()),
            ReposItem::Full(RepoEntry {
                name: "my-org/templates".to_string(),
                alias: None,
                repo_type: "git".to_string(),
                repo_ref: "refs/heads/main".to_string(),
                checkout: false,
            }),
        ];
        let (repos, checkout) = lower_repos(&items).unwrap();
        assert_eq!(repos.len(), 3);
        assert_eq!(checkout, vec!["tools", "schemas"]);
        assert_eq!(repos[2].repository, "templates");
    }

    #[test]
    fn test_parse_shorthand_simple() {
        let (alias, name) = parse_shorthand("org/my-repo").unwrap();
        assert_eq!(alias, "my-repo");
        assert_eq!(name, "org/my-repo");
    }

    #[test]
    fn test_parse_shorthand_with_equals() {
        let (alias, name) = parse_shorthand("custom=org/my-repo").unwrap();
        assert_eq!(alias, "custom");
        assert_eq!(name, "org/my-repo");
    }

    #[test]
    fn test_parse_shorthand_empty_alias_rejected() {
        let err = parse_shorthand("=org/repo").unwrap_err();
        assert!(err.to_string().contains("empty alias"), "{err}");
    }

    #[test]
    fn test_parse_shorthand_empty_name_rejected() {
        let err = parse_shorthand("alias=").unwrap_err();
        assert!(err.to_string().contains("empty name"), "{err}");
    }

    #[test]
    fn test_derive_alias_basic() {
        assert_eq!(derive_alias("org/my-repo").unwrap(), "my-repo");
        assert_eq!(derive_alias("my-repo").unwrap(), "my-repo");
        assert_eq!(derive_alias("a/b/c").unwrap(), "c");
    }

    #[test]
    fn test_derive_alias_trailing_slash() {
        // Trailing slash should be trimmed gracefully
        assert_eq!(derive_alias("org/repo/").unwrap(), "repo");
    }

    #[test]
    fn test_resolve_repos_empty() {
        use crate::compile::types::FrontMatter;
        let yaml = r#"
name: "test"
description: "test"
"#;
        let fm: FrontMatter = serde_yaml::from_str(yaml).unwrap();
        let (repos, checkout) = resolve_repos(&fm).unwrap();
        assert!(repos.is_empty());
        assert!(checkout.is_empty());
    }

    #[test]
    fn test_resolve_repos_compact_syntax() {
        use crate::compile::types::FrontMatter;
        let yaml = r#"
name: "test"
description: "test"
repos:
  - my-org/tools
  - schemas=my-org/internal-schemas
  - name: my-org/docs
    ref: refs/heads/v2
    checkout: false
"#;
        let fm: FrontMatter = serde_yaml::from_str(yaml).unwrap();
        let (repos, checkout) = resolve_repos(&fm).unwrap();
        assert_eq!(repos.len(), 3);
        assert_eq!(repos[0].repository, "tools");
        assert_eq!(repos[0].name, "my-org/tools");
        assert_eq!(repos[1].repository, "schemas");
        assert_eq!(repos[1].name, "my-org/internal-schemas");
        assert_eq!(repos[2].repository, "docs");
        assert_eq!(repos[2].repo_ref, "refs/heads/v2");
        assert_eq!(checkout, vec!["tools", "schemas"]);
    }

    #[test]
    fn test_resolve_repos_legacy_via_codemod() {
        // Legacy `repositories:` + `checkout:` now flow through the
        // `repos_unified` codemod and arrive at typed deserialization
        // already in the unified `repos:` shape.
        use crate::compile::parse_markdown;
        let source = "---\n\
                      name: test\n\
                      description: test\n\
                      repositories:\n  - repository: tools\n    type: git\n    name: my-org/tools\n\
                      checkout:\n  - tools\n\
                      ---\nbody\n";
        let (fm, _) = parse_markdown(source).unwrap();
        let (repos, checkout) = resolve_repos(&fm).unwrap();
        assert_eq!(repos.len(), 1);
        assert_eq!(repos[0].repository, "tools");
        assert_eq!(checkout, vec!["tools"]);
    }
}
