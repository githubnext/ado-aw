//! Semantic version parsing shared across the CLI.
//!
//! Every version string `ado-aw` compares originates from a proper semver
//! triple — `env!("CARGO_PKG_VERSION")`, the `version=…` field of a compiled
//! pipeline's `# @ado-aw` header (itself written from `CARGO_PKG_VERSION`),
//! or a `githubnext/ado-aw` release tag. This module wraps the [`semver`]
//! crate so those comparisons are done in one place with real semver
//! ordering rather than repeated ad-hoc `split('.')` parsers.
//!
//! Using the crate rather than a hand-rolled triple matters for
//! pre-releases: semver orders `1.0.0-alpha` *before* `1.0.0`, which a
//! numeric-triple parser that discards the suffix gets wrong.

use semver::Version;

/// Parse a bare semver string such as `"0.31.0"`.
///
/// A leading `v` is accepted and stripped, so release tags (`v0.31.0`) and
/// bare versions parse identically. Returns `None` when `s` is not valid
/// semver — callers treat that as "unknown" and fall back to safe defaults
/// rather than guessing.
pub fn parse(s: &str) -> Option<Version> {
    Version::parse(s.trim().trim_start_matches('v')).ok()
}

/// Returns `true` when `version` is strictly older than `threshold`.
///
/// Returns `false` when either side fails to parse, so an unrecognisable
/// version can never be mistaken for an old one. Callers relying on this to
/// gate a migration therefore fail closed.
pub fn is_older_than(version: &str, threshold: &str) -> bool {
    match (parse(version), parse(threshold)) {
        (Some(v), Some(t)) => v < t,
        _ => false,
    }
}

/// Returns `true` when `version` is the same as, or newer than, `threshold`.
///
/// Returns `false` when either side fails to parse.
pub fn is_at_least(version: &str, threshold: &str) -> bool {
    match (parse(version), parse(threshold)) {
        (Some(v), Some(t)) => v >= t,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bare_and_v_prefixed_versions() {
        assert_eq!(parse("0.31.0"), Some(Version::new(0, 31, 0)));
        assert_eq!(parse("v0.31.0"), Some(Version::new(0, 31, 0)));
        assert_eq!(parse("  0.31.0  "), Some(Version::new(0, 31, 0)));
    }

    #[test]
    fn rejects_non_semver() {
        assert!(parse("not-a-version").is_none());
        assert!(parse("").is_none());
        // Bare `major.minor` is not valid semver; callers must fail closed
        // rather than silently assuming a patch of 0.
        assert!(parse("0.31").is_none());
    }

    #[test]
    fn orders_components_numerically_not_lexically() {
        // Lexically "0.9.0" > "0.48.0" because '9' > '4'; numerically it is
        // the older release. This is the case a naive string compare fails.
        assert!(is_older_than("0.9.0", "0.48.0"));
        assert!(!is_older_than("0.48.0", "0.9.0"));
    }

    #[test]
    fn is_older_than_is_strict() {
        assert!(is_older_than("0.47.0", "0.48.0"));
        assert!(is_older_than("0.47.99", "0.48.0"));
        assert!(!is_older_than("0.48.0", "0.48.0"));
        assert!(!is_older_than("0.48.1", "0.48.0"));
        assert!(!is_older_than("1.0.0", "0.48.0"));
    }

    #[test]
    fn is_at_least_is_inclusive() {
        assert!(is_at_least("0.30.0", "0.30.0"));
        assert!(is_at_least("0.31.0", "0.30.0"));
        assert!(is_at_least("1.0.0", "0.30.0"));
        assert!(!is_at_least("0.29.99", "0.30.0"));
    }

    #[test]
    fn prereleases_sort_before_their_release() {
        // The behaviour a numeric-triple parser cannot express: a
        // pre-release precedes its final release.
        assert!(is_older_than("0.48.0-beta.1", "0.48.0"));
        assert!(!is_at_least("0.48.0-beta.1", "0.48.0"));
        assert!(is_older_than("0.47.1-beta", "0.48.0"));
    }

    #[test]
    fn unparseable_input_fails_closed() {
        assert!(!is_older_than("not-a-version", "0.48.0"));
        assert!(!is_older_than("0.48.0", "not-a-version"));
        assert!(!is_at_least("not-a-version", "0.48.0"));
    }
}
