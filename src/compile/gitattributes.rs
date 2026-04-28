//! `.gitattributes` management for compiled pipelines.
//!
//! Compiled pipeline files are generated artifacts: they should be marked as
//! linguist-generated (so GitHub UI hides them from PR reviews and language
//! statistics) and use the `merge=ours` strategy (so merge conflicts in the
//! generated YAML are resolved by keeping the local copy and re-running
//! `ado-aw compile`).
//!
//! The compiler manages a clearly delimited block in `<repo-root>/.gitattributes`.
//! User-managed entries outside the block are preserved.

use anyhow::{Context, Result};
use std::collections::BTreeSet;
use std::path::Path;

const BEGIN_MARKER: &str = "# BEGIN ado-aw managed (do not edit)";
const END_MARKER: &str = "# END ado-aw managed";
const ATTRIBUTES: &str = "linguist-generated=true merge=ours text eol=lf";

/// Update the managed block of `<repo_root>/.gitattributes` so that exactly
/// the supplied compiled-pipeline paths are marked as generated.
///
/// Each entry takes the form `<path> linguist-generated=true merge=ours`.
/// Paths are normalized to forward slashes and de-duplicated.
///
/// Existing user-managed lines outside the block are preserved verbatim.
/// If `pipelines` is empty, the managed block is removed entirely.
pub async fn update_gitattributes<P: AsRef<Path>>(
    repo_root: &Path,
    pipelines: impl IntoIterator<Item = P>,
) -> Result<()> {
    let path = repo_root.join(".gitattributes");

    let existing = match tokio::fs::read_to_string(&path).await {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => {
            return Err(e).with_context(|| {
                format!("Failed to read existing {}", path.display())
            })
        }
    };

    let entries: BTreeSet<String> = pipelines
        .into_iter()
        .map(|p| normalize_path(p.as_ref()))
        .collect();

    let new_content = render(&existing, &entries);

    if new_content == existing {
        return Ok(());
    }

    tokio::fs::write(&path, new_content)
        .await
        .with_context(|| format!("Failed to write {}", path.display()))?;
    Ok(())
}

/// Normalize a path to forward slashes and strip any leading `./`.
///
/// The `.gitattributes` format treats whitespace as a separator between the
/// pattern and the attributes, so any pattern containing a space, `"`, or `#`
/// must be wrapped in double quotes (with embedded `"` escaped) for git to
/// parse it as a single pattern. Paths without those characters are emitted
/// unquoted to keep the file readable.
fn normalize_path(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    let s = s.trim_start_matches("./");
    if s.contains(' ') || s.contains('"') || s.contains('#') {
        format!("\"{}\"", s.replace('"', "\\\""))
    } else {
        s.to_string()
    }
}

/// Compute the new file contents given the existing file and the desired
/// managed entries.
///
/// The managed block is always written at the end of the file. If a user has
/// previously placed the block elsewhere (e.g. between user-managed entries),
/// the first recompile will move it to EOF; user lines outside the block are
/// preserved verbatim either way.
fn render(existing: &str, entries: &BTreeSet<String>) -> String {
    let preserved = strip_managed_block(existing);

    if entries.is_empty() {
        // Nothing to manage — leave only the user-managed portion.
        return preserved;
    }

    let mut block = String::new();
    block.push_str(BEGIN_MARKER);
    block.push('\n');
    for entry in entries {
        block.push_str(entry);
        block.push(' ');
        block.push_str(ATTRIBUTES);
        block.push('\n');
    }
    block.push_str(END_MARKER);
    block.push('\n');

    if preserved.is_empty() {
        block
    } else if preserved.ends_with('\n') {
        format!("{}{}", preserved, block)
    } else {
        format!("{}\n{}", preserved, block)
    }
}

/// Remove any existing managed block (between BEGIN and END markers) from
/// `content`. Lines outside the markers are preserved verbatim. If the BEGIN
/// marker appears without a matching END, everything from BEGIN to EOF is
/// stripped (treated as a corrupted/truncated managed block).
fn strip_managed_block(content: &str) -> String {
    let mut out = String::new();
    let mut in_block = false;

    for line in content.split_inclusive('\n') {
        let trimmed = line.trim_end_matches(['\n', '\r']);
        if !in_block && trimmed == BEGIN_MARKER {
            in_block = true;
            continue;
        }
        if in_block {
            if trimmed == END_MARKER {
                in_block = false;
            }
            continue;
        }
        out.push_str(line);
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[tokio::test]
    async fn writes_new_gitattributes_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let pipelines = vec![
            PathBuf::from("agents/my-agent.lock.yml"),
            PathBuf::from(".azdo/pipelines/review.lock.yml"),
        ];
        update_gitattributes(dir.path(), pipelines).await.unwrap();

        let written = std::fs::read_to_string(dir.path().join(".gitattributes")).unwrap();
        assert!(written.contains(BEGIN_MARKER));
        assert!(written.contains(END_MARKER));
        assert!(written.contains(
            ".azdo/pipelines/review.lock.yml linguist-generated=true merge=ours text eol=lf"
        ));
        assert!(written.contains(
            "agents/my-agent.lock.yml linguist-generated=true merge=ours text eol=lf"
        ));
    }

    #[tokio::test]
    async fn preserves_user_managed_lines() {
        let dir = tempfile::tempdir().unwrap();
        let user = "*.png binary\n# my own comment\n";
        std::fs::write(dir.path().join(".gitattributes"), user).unwrap();

        update_gitattributes(
            dir.path(),
            vec![PathBuf::from("agents/x.lock.yml")],
        )
        .await
        .unwrap();

        let written = std::fs::read_to_string(dir.path().join(".gitattributes")).unwrap();
        assert!(written.starts_with("*.png binary\n# my own comment\n"));
        assert!(written.contains("agents/x.lock.yml linguist-generated=true merge=ours text eol=lf"));
    }

    #[tokio::test]
    async fn replaces_existing_managed_block() {
        let dir = tempfile::tempdir().unwrap();
        let initial = format!(
            "*.png binary\n{}\nstale/path.lock.yml linguist-generated=true merge=ours text eol=lf\n{}\n",
            BEGIN_MARKER, END_MARKER
        );
        std::fs::write(dir.path().join(".gitattributes"), initial).unwrap();

        update_gitattributes(
            dir.path(),
            vec![PathBuf::from("new/path.lock.yml")],
        )
        .await
        .unwrap();

        let written = std::fs::read_to_string(dir.path().join(".gitattributes")).unwrap();
        assert!(written.starts_with("*.png binary\n"));
        assert!(!written.contains("stale/path.lock.yml"));
        assert!(written.contains("new/path.lock.yml linguist-generated=true merge=ours text eol=lf"));
        // Block markers should appear exactly once
        assert_eq!(written.matches(BEGIN_MARKER).count(), 1);
        assert_eq!(written.matches(END_MARKER).count(), 1);
    }

    #[tokio::test]
    async fn removes_block_when_no_pipelines() {
        let dir = tempfile::tempdir().unwrap();
        let initial = format!(
            "*.png binary\n{}\nold/path.lock.yml linguist-generated=true merge=ours text eol=lf\n{}\n",
            BEGIN_MARKER, END_MARKER
        );
        std::fs::write(dir.path().join(".gitattributes"), initial).unwrap();

        update_gitattributes(dir.path(), Vec::<PathBuf>::new()).await.unwrap();

        let written = std::fs::read_to_string(dir.path().join(".gitattributes")).unwrap();
        assert_eq!(written, "*.png binary\n");
    }

    #[tokio::test]
    async fn entries_are_sorted_and_deduplicated() {
        let dir = tempfile::tempdir().unwrap();
        let pipelines = vec![
            PathBuf::from("./b/x.lock.yml"),
            PathBuf::from("a/y.lock.yml"),
            PathBuf::from("b/x.lock.yml"), // duplicate after normalization
        ];
        update_gitattributes(dir.path(), pipelines).await.unwrap();

        let written = std::fs::read_to_string(dir.path().join(".gitattributes")).unwrap();
        let body: Vec<&str> = written
            .lines()
            .filter(|l| l.contains("linguist-generated"))
            .collect();
        assert_eq!(body.len(), 2);
        assert!(body[0].starts_with("a/y.lock.yml "));
        assert!(body[1].starts_with("b/x.lock.yml "));
    }

    #[tokio::test]
    async fn idempotent_when_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let pipelines = vec![PathBuf::from("agents/x.lock.yml")];
        update_gitattributes(dir.path(), pipelines.clone()).await.unwrap();
        let first = std::fs::read_to_string(dir.path().join(".gitattributes")).unwrap();

        update_gitattributes(dir.path(), pipelines).await.unwrap();
        let second = std::fs::read_to_string(dir.path().join(".gitattributes")).unwrap();

        // Content equality is the contract; the writer additionally
        // short-circuits the on-disk write when contents already match (see
        // `update_gitattributes`), but we don't assert mtime here because
        // mtime granularity varies by filesystem (e.g. 1s on macOS HFS+).
        assert_eq!(first, second);
    }

    #[tokio::test]
    async fn quotes_paths_containing_spaces() {
        let dir = tempfile::tempdir().unwrap();
        let pipelines = vec![PathBuf::from("my agents/pipeline.lock.yml")];
        update_gitattributes(dir.path(), pipelines).await.unwrap();

        let written = std::fs::read_to_string(dir.path().join(".gitattributes")).unwrap();
        assert!(
            written.contains("\"my agents/pipeline.lock.yml\" linguist-generated=true merge=ours text eol=lf"),
            "expected quoted path entry, got:\n{}",
            written
        );
    }

    #[tokio::test]
    async fn entries_pin_lf_eol() {
        // Regression: managed entries must include `text eol=lf` so that Git
        // doesn't autoconvert LF→CRLF on Windows checkouts and emit warnings
        // each time the pipeline is recompiled.
        let dir = tempfile::tempdir().unwrap();
        update_gitattributes(
            dir.path(),
            vec![PathBuf::from("agents/x.lock.yml")],
        )
        .await
        .unwrap();

        let written = std::fs::read_to_string(dir.path().join(".gitattributes")).unwrap();
        assert!(
            written.contains("text eol=lf"),
            "managed entries must pin LF line endings, got:\n{}",
            written
        );
    }
}
