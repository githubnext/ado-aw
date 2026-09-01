use std::path::{Path, PathBuf};

/// Shared audit data types for `ado-aw audit`.
///
/// This module defines the public report model that analyzers populate and renderers
/// consume for single-build Azure DevOps audit output.
pub mod analyzers;
pub mod cache;
pub mod cli;
pub mod findings;
pub mod model;
pub mod pipeline_graph;
pub mod render;
pub mod url;

pub use cli::{AuditOptions, default_cache_root, dispatch, fetch_audit_data};
#[allow(unused_imports)]
pub use model::*;

pub(crate) fn malformed_aw_info_warning() -> model::ErrorInfo {
    model::ErrorInfo {
        source: String::from("audit::aw_info"),
        message: String::from(
            "aw_info.json could not be read or parsed; optional engine metadata and pipeline graph correlation are unavailable",
        ),
        timestamp: None,
    }
}

pub(crate) fn push_warning_once(audit: &mut model::AuditData, warning: model::ErrorInfo) {
    if !audit.warnings.contains(&warning) {
        audit.warnings.push(warning);
    }
}

/// Compare two `<prefix>_<id>` directory names by their trailing
/// integer suffix, falling back to a full lexicographic comparison
/// when the suffix isn't a u64.
///
/// Plain string sort treats `"agent_outputs_9"` as greater than
/// `"agent_outputs_10"` because `'9' > '1'`. When ADO produces
/// multi-digit build IDs (which happens after the very first builds),
/// the lexicographic "last" is the wrong directory — usually older.
/// This comparator parses the trailing token after the final `_` and
/// compares numerically so the highest-numbered build wins.
pub(crate) fn cmp_numeric_suffix(a: &str, b: &str) -> std::cmp::Ordering {
    fn suffix(s: &str) -> u64 {
        s.rsplit('_')
            .next()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0)
    }
    suffix(a).cmp(&suffix(b)).then_with(|| a.cmp(b))
}

/// Resolve the newest local artifact directory for `prefix`.
///
/// ADO PipelineArtifact zip downloads may contain a top-level directory whose
/// name repeats the artifact name. The downloader already creates an outer
/// `<run>/<artifact-name>` extraction directory, producing
/// `<run>/<artifact-name>/<artifact-name>/...`. When that repeated directory is
/// the outer directory's only entry, return the inner content root so every
/// analyzer sees the same layout as a non-wrapped artifact or manual download.
pub(crate) async fn find_artifact_dir(run_dir: &Path, prefix: &str) -> Option<PathBuf> {
    let mut entries = tokio::fs::read_dir(run_dir).await.ok()?;
    let mut hits: Vec<(String, PathBuf)> = Vec::new();
    while let Ok(Some(entry)) = entries.next_entry().await {
        if entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false)
            && let Some(name) = entry.file_name().to_str()
            && (name == prefix || name.starts_with(&format!("{prefix}_")))
        {
            hits.push((name.to_string(), entry.path()));
        }
    }
    hits.sort_by(|(a, _), (b, _)| cmp_numeric_suffix(a, b));
    let (name, outer) = hits.pop()?;

    let mut outer_entries = match tokio::fs::read_dir(&outer).await {
        Ok(entries) => entries,
        Err(_) => return Some(outer),
    };
    let first = match outer_entries.next_entry().await {
        Ok(Some(entry)) => entry,
        Ok(None) | Err(_) => return Some(outer),
    };
    let has_no_sibling = matches!(outer_entries.next_entry().await, Ok(None));
    let first_is_dir = first
        .file_type()
        .await
        .map(|file_type| file_type.is_dir())
        .unwrap_or(false);
    if has_no_sibling && first_is_dir && first.file_name().to_str() == Some(name.as_str()) {
        return Some(first.path());
    }
    Some(outer)
}

#[cfg(test)]
mod numeric_suffix_tests {
    use super::{cmp_numeric_suffix, find_artifact_dir};
    use std::cmp::Ordering;

    #[test]
    fn double_digit_outranks_single_digit() {
        assert_eq!(
            cmp_numeric_suffix("agent_outputs_10", "agent_outputs_9"),
            Ordering::Greater
        );
        assert_eq!(
            cmp_numeric_suffix("analyzed_outputs_42", "analyzed_outputs_41"),
            Ordering::Greater
        );
    }

    #[test]
    fn non_numeric_suffix_falls_back_to_lexicographic() {
        // Both suffixes parse to 0; tie-break is lexicographic on the
        // full name.
        assert_eq!(
            cmp_numeric_suffix("agent_outputs_alpha", "agent_outputs_beta"),
            Ordering::Less
        );
    }

    #[test]
    fn no_suffix_compares_as_zero() {
        // "agent_outputs" -> last token "outputs" -> parse fails -> 0.
        // "agent_outputs_5" -> 5. So the numeric one wins.
        assert_eq!(
            cmp_numeric_suffix("agent_outputs", "agent_outputs_5"),
            Ordering::Less
        );
    }

    #[tokio::test]
    async fn artifact_dir_unwraps_a_single_redundant_named_root() {
        let temp = tempfile::tempdir().unwrap();
        let outer = temp.path().join("agent_outputs_42");
        let inner = outer.join("agent_outputs_42");
        tokio::fs::create_dir_all(&inner).await.unwrap();

        assert_eq!(
            find_artifact_dir(temp.path(), "agent_outputs").await,
            Some(inner)
        );
    }

    #[tokio::test]
    async fn artifact_dir_keeps_outer_root_when_it_has_other_entries() {
        let temp = tempfile::tempdir().unwrap();
        let outer = temp.path().join("agent_outputs_42");
        tokio::fs::create_dir_all(outer.join("agent_outputs_42"))
            .await
            .unwrap();
        tokio::fs::write(outer.join("aw_info.json"), "{}")
            .await
            .unwrap();

        assert_eq!(
            find_artifact_dir(temp.path(), "agent_outputs").await,
            Some(outer)
        );
    }
}
