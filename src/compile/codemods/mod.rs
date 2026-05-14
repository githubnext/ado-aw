//! Front-matter codemod framework.
//!
//! A flat, append-only registry of detection-based transformations
//! that rewrite deprecated front-matter shapes to current ones
//! before typed deserialization. Modeled on gh-aw's codemod registry
//! (`pkg/cli/fix_codemods.go`).
//!
//! See `docs/codemods.md` for the full contributor reference. In
//! short:
//!
//! - Each codemod is a pure function that **detects** an old shape
//!   and rewrites it. When the input does not match, it returns
//!   `Ok(false)` and leaves the mapping untouched.
//! - Codemods live in `src/compile/codemods/<NNNN>_<id>.rs` and are
//!   appended to [`CODEMODS`] in registration order.
//! - Codemods operate on the **untyped** `serde_yaml::Mapping`
//!   representation so they can rewrite shapes that no longer match
//!   the typed `FrontMatter` (renamed/removed fields,
//!   `deny_unknown_fields`).
//! - Codemods MUST be **idempotent**: running twice produces the
//!   same final state as running once.
//! - When an old key and a new key coexist in the same source, the
//!   codemod errors with a "manual migration required" message
//!   rather than guess. The [`helpers::rename_key`] +
//!   [`helpers::ConflictPolicy::Error`] pattern gives this for free.
//!
//! Unlike a version-stamped chain, codemods leave no per-source
//! version footprint — there is no `schema-version` field in user
//! front matter. Each codemod is idempotent so re-running the whole
//! registry on every compile is cheap when nothing matches.

use anyhow::{Context, Result};
use serde_yaml::Mapping;

mod helpers;
#[path = "0001_repos_unified.rs"]
mod m0001_repos_unified;
#[path = "0002_pool_object_form.rs"]
mod m0002_pool_object_form;

#[allow(unused_imports)] // Re-exported for future codemods; only `take_key` is in-tree use.
pub use helpers::{insert_no_overwrite, rename_key, take_key, ConflictPolicy};

/// Forward-compatible context passed to every codemod.
///
/// Carries ambient information (e.g. compiler version) so codemods
/// can condition their behaviour without hard-coding assumptions.
#[non_exhaustive]
pub struct CodemodContext {
    /// Semantic version of the running `ado-aw` binary
    /// (e.g. `"0.30.0"`). Codemods can compare this against their
    /// `introduced_in` to decide when a default has changed.
    pub compiler_version: &'static str,
}

impl CodemodContext {
    /// Build a context using the compile-time package version.
    pub fn current() -> Self {
        Self {
            compiler_version: env!("CARGO_PKG_VERSION"),
        }
    }
}

/// A single front-matter codemod.
///
/// Codemods are pure functions that detect a specific deprecated
/// shape in the front-matter mapping and rewrite it to the current
/// shape. They must satisfy the following invariants (see
/// `docs/codemods.md`):
///
/// - **Idempotent.** Running twice produces the same result as
///   running once.
/// - **Detection-based.** Returns `Ok(false)` and leaves the mapping
///   untouched when the input does not contain the targeted shape.
/// - **Conflict-aware.** Errors with an actionable "manual migration
///   required" message when both old and new shapes coexist.
/// - **Pure.** No I/O, no env, no time/randomness.
/// - **Mapping-only.** Cannot inspect the markdown body, file path,
///   lock file, or git state.
pub struct Codemod {
    /// Stable snake_case identifier used in logs, errors, and tests.
    pub id: &'static str,
    /// Human-facing one-line summary, surfaced in the compile warning.
    pub summary: &'static str,
    /// Compiler version that introduced this codemod (e.g. "0.27.0").
    /// Provenance only; not consumed by the runner.
    #[allow(dead_code)]
    pub introduced_in: &'static str,
    /// The transformation. Returns `Ok(true)` when the codemod
    /// modified the mapping; `Ok(false)` when the mapping does not
    /// match this codemod's detection predicate. Errors propagate up
    /// and abort the whole compile.
    pub apply: fn(&mut Mapping, &CodemodContext) -> Result<bool>,
}

/// The fixed registry of codemods. Append-only — new codemods land
/// at the end. Order matters when one codemod depends on another
/// having run first (e.g. A renames `foo` → `bar`, B operates on
/// `bar`); idempotency means any codemod can re-run on any source
/// without harm.
pub static CODEMODS: &[&'static Codemod] = &[
    &m0001_repos_unified::CODEMOD,
    &m0002_pool_object_form::CODEMOD,
];

/// Result of running the codemod registry on a single front-matter
/// mapping.
#[derive(Debug, Clone)]
pub struct CodemodReport {
    /// Ordered list of codemods that ran (returned `Ok(true)`).
    pub applied: Vec<AppliedCodemod>,
}

/// Record of a single codemod that fired. Carries enough info for
/// warning emission and tests without re-querying the registry.
#[derive(Debug, Clone)]
pub struct AppliedCodemod {
    pub id: &'static str,
    pub summary: &'static str,
}

impl CodemodReport {
    /// An empty report (no codemod fired).
    #[allow(dead_code)]
    pub fn empty() -> Self {
        Self {
            applied: Vec::new(),
        }
    }

    /// Returns true when at least one codemod ran.
    pub fn changed(&self) -> bool {
        !self.applied.is_empty()
    }

    /// IDs of codemods that ran, in order. Helpful for tests.
    pub fn applied_ids(&self) -> Vec<&'static str> {
        self.applied.iter().map(|a| a.id).collect()
    }
}

/// Apply the registered codemods to `fm`.
///
/// Equivalent to [`apply_codemods_with`] called with the global
/// [`CODEMODS`] registry.
#[allow(dead_code)]
pub fn apply_codemods(fm: &mut Mapping) -> Result<CodemodReport> {
    apply_codemods_with(fm, CODEMODS)
}

/// Apply an explicit codemod registry. Used by tests with a stub
/// registry; production code calls [`apply_codemods`].
pub(crate) fn apply_codemods_with(
    fm: &mut Mapping,
    registry: &[&'static Codemod],
) -> Result<CodemodReport> {
    let ctx = CodemodContext::current();
    let mut applied: Vec<AppliedCodemod> = Vec::new();
    for c in registry {
        let changed =
            (c.apply)(fm, &ctx).with_context(|| format!("codemod {} failed", c.id))?;
        if changed {
            applied.push(AppliedCodemod {
                id: c.id,
                summary: c.summary,
            });
        }
    }
    Ok(CodemodReport { applied })
}

#[cfg(test)]
mod tests {
    use super::helpers::ConflictPolicy;
    use super::*;
    use anyhow::bail;
    use serde_yaml::Value;

    // ─── Registry health (operates on the real CODEMODS slice) ────────────

    #[test]
    fn registry_ids_are_unique() {
        // Vacuously true while CODEMODS is empty; the assertion
        // machinery still compiles so this test guards against
        // duplicate ids the moment a real codemod ships.
        let mut seen = std::collections::BTreeSet::new();
        for c in CODEMODS {
            assert!(
                seen.insert(c.id),
                "duplicate codemod id `{}` in CODEMODS",
                c.id
            );
        }
    }

    #[test]
    fn codemod_filenames_match_registry_count() {
        // Vacuously true while CODEMODS is empty (the directory
        // contains only `mod.rs` and `helpers.rs`, which are
        // skipped). Once a numeric `<NNNN>_<id>.rs` file lands, this
        // test asserts the registry was updated to match.
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/compile/codemods");
        let mut numeric_files: Vec<String> = Vec::new();
        for entry in std::fs::read_dir(&dir).expect("read codemods dir") {
            let entry = entry.expect("dir entry");
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .expect("utf8 stem")
                .to_string();
            if stem == "mod" || stem == "helpers" {
                continue;
            }
            // Expect `<NNNN>_<id>` so files sort naturally in
            // directory listings. The registry order is the canonical
            // application order; the prefix is purely cosmetic.
            let (prefix, _rest) = stem.split_once('_').unwrap_or_else(|| {
                panic!(
                    "codemod file {:?} does not match `<NNNN>_<id>.rs`",
                    path
                )
            });
            let _: u32 = prefix.parse().unwrap_or_else(|_| {
                panic!(
                    "codemod file {:?} has non-numeric prefix `{}`",
                    path, prefix
                )
            });
            numeric_files.push(stem);
        }
        assert_eq!(
            numeric_files.len(),
            CODEMODS.len(),
            "number of `<NNNN>_<id>.rs` files ({}) does not match \
             CODEMODS.len() ({}); files: {:?}",
            numeric_files.len(),
            CODEMODS.len(),
            numeric_files
        );
    }

    // ─── apply_codemods_with ──────────────────────────────────────────────

    fn noop_no_change(_fm: &mut Mapping, _ctx: &CodemodContext) -> Result<bool> {
        Ok(false)
    }

    fn rename_a_to_b(fm: &mut Mapping, _ctx: &CodemodContext) -> Result<bool> {
        rename_key(fm, "a", "b", ConflictPolicy::Error)
    }

    fn always_fail(_fm: &mut Mapping, _ctx: &CodemodContext) -> Result<bool> {
        bail!("intentional test failure");
    }

    static TEST_CODEMOD_NOOP: Codemod = Codemod {
        id: "test_noop",
        summary: "no-op codemod for tests",
        introduced_in: "test",
        apply: noop_no_change,
    };

    static TEST_CODEMOD_RENAME: Codemod = Codemod {
        id: "test_rename_a_to_b",
        summary: "rename a -> b",
        introduced_in: "test",
        apply: rename_a_to_b,
    };

    static TEST_CODEMOD_FAIL: Codemod = Codemod {
        id: "test_fail",
        summary: "always fails",
        introduced_in: "test",
        apply: always_fail,
    };

    #[test]
    fn empty_registry_produces_empty_report() {
        let mut m: Mapping = serde_yaml::from_str("name: x\n").unwrap();
        let report = apply_codemods_with(&mut m, &[]).unwrap();
        assert!(!report.changed());
        assert!(report.applied.is_empty());
    }

    #[test]
    fn all_no_op_codemods_produce_empty_report() {
        let mut m: Mapping = serde_yaml::from_str("name: x\n").unwrap();
        let registry: &[&'static Codemod] = &[&TEST_CODEMOD_NOOP];
        let report = apply_codemods_with(&mut m, registry).unwrap();
        assert!(!report.changed());
    }

    #[test]
    fn matching_codemod_appears_in_report() {
        let mut m: Mapping = serde_yaml::from_str("a: 1\n").unwrap();
        let registry: &[&'static Codemod] = &[&TEST_CODEMOD_RENAME];
        let report = apply_codemods_with(&mut m, registry).unwrap();
        assert!(report.changed());
        assert_eq!(report.applied_ids(), vec!["test_rename_a_to_b"]);
        assert!(!m.contains_key(Value::String("a".into())));
        assert!(m.contains_key(Value::String("b".into())));
    }

    #[test]
    fn non_matching_codemod_omitted_from_report() {
        // Source has no `a` key, so the rename is a no-op and is
        // not listed in the report.
        let mut m: Mapping = serde_yaml::from_str("name: x\n").unwrap();
        let registry: &[&'static Codemod] = &[&TEST_CODEMOD_RENAME];
        let report = apply_codemods_with(&mut m, registry).unwrap();
        assert!(!report.changed());
    }

    #[test]
    fn mixed_registry_lists_only_changed_codemods() {
        let mut m: Mapping = serde_yaml::from_str("a: 1\n").unwrap();
        let registry: &[&'static Codemod] =
            &[&TEST_CODEMOD_NOOP, &TEST_CODEMOD_RENAME, &TEST_CODEMOD_NOOP];
        let report = apply_codemods_with(&mut m, registry).unwrap();
        assert_eq!(report.applied_ids(), vec!["test_rename_a_to_b"]);
    }

    #[test]
    fn codemod_failure_carries_id_in_context() {
        let mut m: Mapping = serde_yaml::from_str("a: 1\n").unwrap();
        let registry: &[&'static Codemod] = &[&TEST_CODEMOD_FAIL];
        let err = apply_codemods_with(&mut m, registry).unwrap_err();
        let chain = format!("{:#}", err);
        assert!(
            chain.contains("codemod test_fail failed"),
            "expected codemod context, got: {}",
            chain
        );
        assert!(
            chain.contains("intentional test failure"),
            "expected inner error, got: {}",
            chain
        );
    }

    #[test]
    fn idempotent_codemod_runs_safely_twice() {
        let registry: &[&'static Codemod] = &[&TEST_CODEMOD_RENAME];
        let mut m: Mapping = serde_yaml::from_str("a: 1\n").unwrap();
        let report1 = apply_codemods_with(&mut m, registry).unwrap();
        assert!(report1.changed());
        let report2 = apply_codemods_with(&mut m, registry).unwrap();
        assert!(
            !report2.changed(),
            "second run on already-migrated mapping must be a no-op"
        );
    }
}
