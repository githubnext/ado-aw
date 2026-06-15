//! Validation for caller-supplied workflow source paths.
//!
//! Two entry points feed an untrusted string to
//! [`crate::compile::build_pipeline_ir`]:
//!
//! 1. `audit::pipeline_graph` — `aw_info.json::source` from an
//!    audited build's artifact payload (the build itself may have
//!    been prompt-injected).
//! 2. `mcp_author` — `source_path` MCP tool parameters supplied by
//!    an IDE/Copilot Chat agent that may be processing untrusted
//!    content (PR descriptions, issue comments, fetched pages).
//!
//! Both sites need the same defence: refuse non-markdown paths,
//! refuse parent-directory traversal, refuse `~`-prefixed
//! shell-style expansion, refuse `.md` symlinks that resolve to
//! non-`.md` targets. This module centralises the guard so the two
//! call sites cannot drift apart.
//!
//! See the function-level doc on [`validate_workflow_source_path`]
//! for the full security contract.
//!
//! **Do not weaken any of the listed guards** without simultaneously
//! adding a stronger containment check (e.g. canonicalize +
//! prefix-against-cwd). Every existing audit and MCP entry point
//! relies on this function as the primary gate against arbitrary
//! file reads.

use std::path::{Component, Path, PathBuf};

use anyhow::Result;

/// Outcome of validating a caller-supplied workflow source path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedSourcePath {
    /// The validated path. Absolute paths are returned as-is (after
    /// symlink target re-check); relative paths are returned joined
    /// to the canonicalized current working directory.
    pub path: PathBuf,
    /// The trimmed + separator-normalised form of the original
    /// input string. Suitable for embedding in user-facing error
    /// messages without leaking trailing whitespace.
    pub normalized: String,
}

/// Validate a caller-supplied workflow source path string.
///
/// **Security**: the input is untrusted. Mitigations applied (in
/// order):
///
/// 1. Trim whitespace and normalise platform path separators —
///    `\\` → `/` on Unix, `/` → `\\` on Windows. This step prevents
///    a Linux caller from smuggling `..\\workflow.md` past the
///    `ParentDir` check, since `PathBuf::from` would otherwise
///    treat the whole string as one `Normal` component on Unix.
/// 2. Require a `.md` extension — the only valid agentic workflow
///    source extension. Closes the arbitrary-file-read vector
///    against keys, `/etc/passwd`, etc.
/// 3. For absolute paths, canonicalise and **re-check the
///    extension on the resolved target** so a `foo.md → /etc/passwd`
///    symlink does not satisfy the lexical check. Canonicalisation
///    failures are tolerated (file may not exist locally — the
///    caller upstream surfaces a clean read error in that case).
/// 4. For relative paths, reject `..` components and a leading
///    `~` (no directory traversal, no shell-style expansion), then
///    join to the canonicalised current working directory.
pub async fn validate_workflow_source_path(source: &str) -> Result<ValidatedSourcePath> {
    let normalized = normalize_separators(source.trim());
    let path = PathBuf::from(&normalized);

    if !has_md_extension(&path) {
        anyhow::bail!(
            "refusing source path '{normalized}': only `.md` files are valid agentic workflow sources"
        );
    }

    if path.is_absolute() {
        if let Ok(canonical) = tokio::fs::canonicalize(&path).await
            && !has_md_extension(&canonical)
        {
            anyhow::bail!(
                "refusing source path '{normalized}': symlink resolves to non-`.md` target '{}'",
                canonical.display()
            );
        }
        return Ok(ValidatedSourcePath {
            path,
            normalized,
        });
    }

    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
        || normalized.starts_with('~')
    {
        anyhow::bail!("refusing suspicious relative source path '{normalized}'");
    }

    let cwd = tokio::fs::canonicalize(".")
        .await
        .map_err(|err| anyhow::anyhow!("could not resolve current directory: {err}"))?;
    Ok(ValidatedSourcePath {
        path: cwd.join(&path),
        normalized,
    })
}

/// Returns `true` when `path` carries a `.md` (case-insensitive)
/// extension.
fn has_md_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
}

/// Normalise path separators so the platform-native `PathBuf`
/// machinery treats `..` and similar components consistently.
///
/// `PathBuf::from("..\\foo.md")` on Unix produces a single
/// `Normal("..\\foo.md")` component, which would otherwise sneak
/// past the `ParentDir` check below. Mirrors the helper that used
/// to live inside `audit::pipeline_graph::normalize_source_path`.
fn normalize_separators(source: &str) -> String {
    if std::path::MAIN_SEPARATOR == '/' {
        source.replace('\\', "/")
    } else {
        source.replace('/', "\\")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn rejects_non_markdown_extension() {
        let err = validate_workflow_source_path("/etc/passwd")
            .await
            .expect_err("non-md path must be rejected");
        assert!(
            format!("{err}").contains("only `.md`"),
            "expected non-md rejection message, got: {err}"
        );
    }

    #[tokio::test]
    async fn rejects_parent_traversal_with_unix_separators() {
        let err = validate_workflow_source_path("../../../etc/passwd.md")
            .await
            .expect_err("`..` must be rejected");
        assert!(
            format!("{err}").contains("suspicious relative source path"),
            "expected traversal rejection message, got: {err}"
        );
    }

    #[tokio::test]
    async fn rejects_parent_traversal_with_backslash_separators() {
        // Regression for the linux-side `..\\workflow.md` bypass: on
        // Unix, `PathBuf::from("..\\workflow.md")` produces a single
        // Normal component without the separator normalisation, so
        // the `ParentDir` check would never fire.
        let err = validate_workflow_source_path("..\\..\\workflow.md")
            .await
            .expect_err("backslash-encoded `..` must be rejected");
        assert!(
            format!("{err}").contains("suspicious relative source path"),
            "expected traversal rejection message, got: {err}"
        );
    }

    #[tokio::test]
    async fn rejects_tilde_prefix() {
        let err = validate_workflow_source_path("~/secret.md")
            .await
            .expect_err("tilde prefix must be rejected");
        assert!(
            format!("{err}").contains("suspicious relative source path"),
            "expected tilde rejection message, got: {err}"
        );
    }

    #[tokio::test]
    async fn accepts_legitimate_relative_md() {
        let result = validate_workflow_source_path("workflows/foo.md")
            .await
            .expect("plain relative .md path must be accepted");
        assert!(result.path.is_absolute());
        assert!(result.normalized.ends_with("foo.md"));
    }

    #[tokio::test]
    async fn accepts_absolute_markdown_path() {
        let path = if cfg!(windows) {
            r"C:\workflows\foo.md"
        } else {
            "/repo/workflows/foo.md"
        };
        let result = validate_workflow_source_path(path)
            .await
            .expect("absolute `.md` paths must be accepted");
        assert!(result.path.is_absolute());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn rejects_md_symlink_to_non_md_target() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let target = temp_dir.path().join("binary.bin");
        tokio::fs::write(&target, b"x").await.unwrap();
        let link = temp_dir.path().join("evil.md");
        tokio::fs::symlink(&target, &link).await.unwrap();

        let err = validate_workflow_source_path(link.to_str().unwrap())
            .await
            .expect_err("symlink to non-md target must be rejected");
        assert!(
            format!("{err}").contains("symlink resolves to non-`.md` target"),
            "expected symlink rejection message, got: {err}"
        );
    }
}
