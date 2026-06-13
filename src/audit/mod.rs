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

pub use cli::{AuditOptions, dispatch, fetch_audit_data};
#[allow(unused_imports)]
pub use model::*;

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

#[cfg(test)]
mod numeric_suffix_tests {
    use super::cmp_numeric_suffix;
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
}
