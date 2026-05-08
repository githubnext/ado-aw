//! Front-matter migration framework.
//!
//! A version-stamped, append-only chain of transformations that rewrites
//! older source front-matter shapes into the current one before typed
//! deserialization. Modeled on Django/Rails-style schema migrations.
//!
//! See `docs/migrations.md` for the full contributor reference. In
//! short:
//!
//! - Each migration goes from `from_version: N` to `to_version: N + 1`.
//! - Migrations live in `src/compile/migrations/<NNNN>_<id>.rs` and are
//!   appended to [`MIGRATIONS`] in strict ascending order.
//! - [`CURRENT_SCHEMA_VERSION`] is `1 + MIGRATIONS.len()`.
//! - Each source `.md` carries a `schema-version: <u32>` field (missing
//!   = 1). The compiler bumps it in place when migrations apply.
//! - Migrations operate on the **untyped** `serde_yaml::Mapping`
//!   representation so they can rewrite shapes that no longer match the
//!   typed `FrontMatter` (renamed/removed fields,
//!   `deny_unknown_fields`).
//!
//! The runner is intentionally simple: read current version, validate
//! the registry is contiguous at runtime, and walk the chain.

use anyhow::{bail, ensure, Context, Result};
use serde_yaml::{Mapping, Value};

mod helpers;
#[allow(unused_imports)]
pub use helpers::{insert_no_overwrite, rename_key, take_key, ConflictPolicy};

#[path = "0001_repos_unified.rs"]
mod m0001_repos_unified;

/// Front-matter key that holds the schema version. Kebab-case to match
/// the rest of the front-matter grammar.
#[allow(dead_code)]
pub const SCHEMA_VERSION_KEY: &str = "schema-version";

/// Forward-compatible context passed to every migration. Currently
/// empty; we keep it in the signature so future migrations can be given
/// (e.g.) the source path without breaking the function pointer type.
#[non_exhaustive]
pub struct MigrationContext {}

/// A single front-matter migration step.
///
/// Migrations are pure functions that mutate the front-matter mapping
/// in place. They must NOT bump [`SCHEMA_VERSION_KEY`] themselves — the
/// runner does that after each successful step. They must be
/// deterministic and side-effect-free (documentation convention; not
/// enforced by the type system).
pub struct Migration {
    /// Source schema version this migration consumes.
    pub from_version: u32,
    /// Target schema version produced (always `from_version + 1`).
    pub to_version: u32,
    /// Stable snake_case identifier used in logs, errors, and tests.
    pub id: &'static str,
    /// Human-facing one-line summary, surfaced in the compile warning.
    pub summary: &'static str,
    /// Compiler version that introduced this migration (e.g. "0.27.0").
    /// Currently used only for human-readable provenance; not consumed
    /// by the runner.
    #[allow(dead_code)]
    pub introduced_in: &'static str,
    /// The transformation. See type-level docs for invariants.
    pub apply: fn(&mut Mapping, &MigrationContext) -> Result<()>,
}

/// The fixed registry of migrations, in strict ascending `from_version`
/// order. Append-only — never reorder, never delete entries.
///
/// Adding a migration is two edits:
///
/// 1. Create `src/compile/migrations/<NNNN>_<id>.rs` with a
///    `pub static MIGRATION: Migration` and register the module here.
/// 2. Append `&<module>::MIGRATION` to this slice.
pub static MIGRATIONS: &[&'static Migration] = &[
    &m0001_repos_unified::MIGRATION,
];

/// Total number of schema versions known to this compiler.
///
/// Computed from the registry length so it can never drift.
#[allow(dead_code)]
pub const CURRENT_SCHEMA_VERSION: u32 = 1 + MIGRATIONS.len() as u32;

/// Result of running the migration chain on a single front-matter
/// mapping.
#[derive(Debug, Clone)]
pub struct MigrationReport {
    /// The schema version present in the source before migration.
    pub from_version: u32,
    /// The schema version after migration (always
    /// [`CURRENT_SCHEMA_VERSION`] when the runner returns Ok).
    pub to_version: u32,
    /// Ordered list of migrations that ran, with id + summary so
    /// callers (warnings, logs) don't need to re-query the registry.
    pub applied: Vec<AppliedMigration>,
}

/// Record of a single migration that ran. Carries enough info for
/// warning emission and tests without re-looking up the registry.
#[derive(Debug, Clone)]
pub struct AppliedMigration {
    pub id: &'static str,
    pub summary: &'static str,
}

impl MigrationReport {
    /// Build a no-migration report for a source already at `version`.
    pub fn unchanged(version: u32) -> Self {
        Self {
            from_version: version,
            to_version: version,
            applied: Vec::new(),
        }
    }

    /// Returns true when at least one migration ran.
    pub fn changed(&self) -> bool {
        !self.applied.is_empty()
    }

    /// IDs of migrations that ran, in order.
    #[allow(dead_code)]
    pub fn applied_ids(&self) -> Vec<&'static str> {
        self.applied.iter().map(|a| a.id).collect()
    }
}

/// Read [`SCHEMA_VERSION_KEY`] from the mapping. Missing field → 1.
/// Returns Err on any non-positive-integer value (zero, negative,
/// float, string, sequence, null).
pub fn read_schema_version(fm: &Mapping) -> Result<u32> {
    let Some(value) = fm.get(Value::String(SCHEMA_VERSION_KEY.to_string())) else {
        return Ok(1);
    };
    let n = value.as_u64().ok_or_else(|| {
        anyhow::anyhow!(
            "front matter `schema-version` must be a positive integer (>= 1), got: {}",
            describe_yaml_value(value)
        )
    })?;
    if n == 0 {
        bail!(
            "front matter `schema-version` must be a positive integer (>= 1), got: 0"
        );
    }
    if n > u32::MAX as u64 {
        bail!(
            "front matter `schema-version` must be a positive integer (>= 1), got: {} (exceeds u32::MAX)",
            n
        );
    }
    Ok(n as u32)
}

/// Set [`SCHEMA_VERSION_KEY`] on the mapping. Inserts at the end if
/// absent, updates in place otherwise.
pub fn set_schema_version(fm: &mut Mapping, version: u32) {
    fm.insert(
        Value::String(SCHEMA_VERSION_KEY.to_string()),
        Value::Number(serde_yaml::Number::from(version as u64)),
    );
}

/// Apply the registered migration chain to `fm`.
///
/// Equivalent to [`migrate_front_matter_with`] called with the global
/// [`MIGRATIONS`] registry.
#[allow(dead_code)]
pub fn migrate_front_matter(fm: &mut Mapping) -> Result<MigrationReport> {
    migrate_front_matter_with(fm, MIGRATIONS)
}

/// Apply an explicit migration chain. Used by tests with a stub
/// registry; production code calls [`migrate_front_matter`].
pub fn migrate_front_matter_with(
    fm: &mut Mapping,
    registry: &[&'static Migration],
) -> Result<MigrationReport> {
    // Use checked arithmetic so we surface a clear error rather than
    // panic-or-wrap if a registry ever grows past u32::MAX. Realistic
    // registries are tiny — this is a "no panic in library code" guard.
    let registry_len: u32 = registry
        .len()
        .try_into()
        .ok()
        .context("migration registry has more than u32::MAX entries")?;
    let target_version = 1u32
        .checked_add(registry_len)
        .context("migration registry too large: target_version would overflow u32")?;
    let mut current = read_schema_version(fm)?;
    let from_version = current;

    if current > target_version {
        bail!(
            "source `schema-version` is {}, but this compiler only supports up to {}. Upgrade ado-aw.",
            current,
            target_version
        );
    }

    if current == target_version {
        return Ok(MigrationReport::unchanged(current));
    }

    let mut applied: Vec<AppliedMigration> = Vec::new();

    while current < target_version {
        let idx = (current - 1) as usize;
        let m = registry.get(idx).ok_or_else(|| {
            anyhow::anyhow!(
                "migration registry corrupt: expected entry at index {} for from_version={}",
                idx,
                current
            )
        })?;
        let next_version = current
            .checked_add(1)
            .context("migration version overflow: current + 1 exceeds u32::MAX")?;
        ensure!(
            m.from_version == current && m.to_version == next_version,
            "migration registry corrupt: expected from_version={} at index {}, found from_version={} to_version={}",
            current,
            idx,
            m.from_version,
            m.to_version
        );

        let ctx = MigrationContext {};
        (m.apply)(fm, &ctx).with_context(|| {
            format!(
                "migration {} ({} -> {}) failed",
                m.id, m.from_version, m.to_version
            )
        })?;

        set_schema_version(fm, m.to_version);
        applied.push(AppliedMigration {
            id: m.id,
            summary: m.summary,
        });
        current = m.to_version;
    }

    Ok(MigrationReport {
        from_version,
        to_version: current,
        applied,
    })
}

fn describe_yaml_value(v: &Value) -> String {
    match v {
        Value::Null => "null".to_string(),
        Value::Bool(b) => format!("bool ({})", b),
        Value::Number(n) => format!("number ({})", n),
        Value::String(s) => format!("string ({:?})", s),
        Value::Sequence(_) => "sequence".to_string(),
        Value::Mapping(_) => "mapping".to_string(),
        Value::Tagged(_) => "tagged".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── Registry health (operates on the real MIGRATIONS slice) ──────────

    #[test]
    fn registry_is_contiguous_and_starts_at_one() {
        // Vacuously true with an empty registry; the assertions still
        // exercise the loop machinery so this test guards against future
        // regressions when migrations are added.
        for (i, m) in MIGRATIONS.iter().enumerate() {
            assert_eq!(
                m.from_version,
                (i + 1) as u32,
                "migration at index {} has from_version={}; expected {}",
                i,
                m.from_version,
                i + 1
            );
            assert_eq!(
                m.to_version,
                (i + 2) as u32,
                "migration at index {} has to_version={}; expected {}",
                i,
                m.to_version,
                i + 2
            );
        }
    }

    #[test]
    fn registry_ids_are_unique() {
        let mut seen = std::collections::BTreeSet::new();
        for m in MIGRATIONS {
            assert!(
                seen.insert(m.id),
                "duplicate migration id `{}` in MIGRATIONS",
                m.id
            );
        }
    }

    #[test]
    fn migration_filenames_match_from_version() {
        // Scan src/compile/migrations/*.rs and check that each numeric
        // migration file's prefix matches its from_version.
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/compile/migrations");
        let mut numeric_files: Vec<(u32, String)> = Vec::new();
        for entry in std::fs::read_dir(&dir).expect("read migrations dir") {
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
            // Expect `<NNNN>_<id>` where NNNN is the zero-padded
            // from_version. Files that don't match this shape are tests
            // or fixtures and should not exist in this directory.
            let (prefix, _rest) = stem.split_once('_').unwrap_or_else(|| {
                panic!(
                    "migration file {:?} does not match `<NNNN>_<id>.rs`",
                    path
                )
            });
            let n: u32 = prefix.parse().unwrap_or_else(|_| {
                panic!(
                    "migration file {:?} has non-numeric prefix `{}`",
                    path, prefix
                )
            });
            numeric_files.push((n, stem));
        }
        numeric_files.sort();
        assert_eq!(
            numeric_files.len(),
            MIGRATIONS.len(),
            "number of `<NNNN>_<id>.rs` files ({}) does not match \
             MIGRATIONS.len() ({}); files: {:?}",
            numeric_files.len(),
            MIGRATIONS.len(),
            numeric_files
        );
        for (i, (n, stem)) in numeric_files.iter().enumerate() {
            assert_eq!(
                *n,
                (i + 1) as u32,
                "migration file `{}` has from_version prefix {} but is at index {}; expected {}",
                stem,
                n,
                i,
                i + 1
            );
        }
    }

    // ─── read_schema_version ──────────────────────────────────────────────

    fn map_with_version(version: Option<&str>) -> Mapping {
        let mut m = Mapping::new();
        if let Some(v) = version {
            // Insert as raw YAML so we can test integer/string/etc.
            let parsed: Value = serde_yaml::from_str(v).unwrap();
            m.insert(Value::String(SCHEMA_VERSION_KEY.to_string()), parsed);
        }
        m
    }

    #[test]
    fn read_schema_version_missing_returns_one() {
        let m = Mapping::new();
        assert_eq!(read_schema_version(&m).unwrap(), 1);
    }

    #[test]
    fn read_schema_version_integer_is_returned() {
        let m = map_with_version(Some("5"));
        assert_eq!(read_schema_version(&m).unwrap(), 5);
    }

    #[test]
    fn read_schema_version_zero_rejected() {
        let m = map_with_version(Some("0"));
        let err = read_schema_version(&m).unwrap_err();
        assert!(
            format!("{}", err).contains("must be a positive integer"),
            "got: {}",
            err
        );
    }

    #[test]
    fn read_schema_version_negative_rejected() {
        let m = map_with_version(Some("-1"));
        let err = read_schema_version(&m).unwrap_err();
        assert!(
            format!("{}", err).contains("must be a positive integer"),
            "got: {}",
            err
        );
    }

    #[test]
    fn read_schema_version_string_rejected() {
        let m = map_with_version(Some("\"five\""));
        let err = read_schema_version(&m).unwrap_err();
        assert!(
            format!("{}", err).contains("must be a positive integer"),
            "got: {}",
            err
        );
    }

    #[test]
    fn read_schema_version_float_rejected() {
        let m = map_with_version(Some("2.5"));
        let err = read_schema_version(&m).unwrap_err();
        assert!(
            format!("{}", err).contains("must be a positive integer"),
            "got: {}",
            err
        );
    }

    #[test]
    fn read_schema_version_null_rejected() {
        let m = map_with_version(Some("null"));
        let err = read_schema_version(&m).unwrap_err();
        assert!(
            format!("{}", err).contains("must be a positive integer"),
            "got: {}",
            err
        );
    }

    // ─── set_schema_version ───────────────────────────────────────────────

    #[test]
    fn set_schema_version_inserts_when_absent() {
        let mut m = Mapping::new();
        set_schema_version(&mut m, 3);
        assert_eq!(read_schema_version(&m).unwrap(), 3);
    }

    #[test]
    fn set_schema_version_updates_in_place() {
        let mut m = map_with_version(Some("1"));
        set_schema_version(&mut m, 4);
        assert_eq!(read_schema_version(&m).unwrap(), 4);
    }

    // ─── migrate_front_matter_with ────────────────────────────────────────

    #[test]
    fn migrate_empty_registry_is_noop_for_v1() {
        let mut m: Mapping = serde_yaml::from_str("name: x\n").unwrap();
        let report = migrate_front_matter_with(&mut m, &[]).unwrap();
        assert!(!report.changed());
        assert_eq!(report.from_version, 1);
        assert_eq!(report.to_version, 1);
    }

    #[test]
    fn migrate_already_current_is_noop() {
        // Empty registry → CURRENT == 1. Source explicitly stamped 1.
        let mut m: Mapping =
            serde_yaml::from_str("schema-version: 1\nname: x\n").unwrap();
        let report = migrate_front_matter_with(&mut m, &[]).unwrap();
        assert!(!report.changed());
    }

    #[test]
    fn migrate_future_version_rejected() {
        // Empty registry → CURRENT == 1. Source claims version 2.
        let mut m: Mapping =
            serde_yaml::from_str("schema-version: 2\nname: x\n").unwrap();
        let err = migrate_front_matter_with(&mut m, &[]).unwrap_err();
        assert!(
            format!("{}", err).contains("only supports up to"),
            "got: {}",
            err
        );
    }

    fn noop_apply(_fm: &mut Mapping, _ctx: &MigrationContext) -> Result<()> {
        Ok(())
    }

    static TEST_MIG_1_TO_2: Migration = Migration {
        from_version: 1,
        to_version: 2,
        id: "test_v1_to_v2",
        summary: "test migration",
        introduced_in: "test",
        apply: noop_apply,
    };

    static TEST_MIG_2_TO_3: Migration = Migration {
        from_version: 2,
        to_version: 3,
        id: "test_v2_to_v3",
        summary: "test migration",
        introduced_in: "test",
        apply: rename_a_to_b,
    };

    fn rename_a_to_b(fm: &mut Mapping, _ctx: &MigrationContext) -> Result<()> {
        rename_key(fm, "a", "b", ConflictPolicy::Error)?;
        Ok(())
    }

    #[test]
    fn migrate_with_test_registry_chains_migrations() {
        let registry: &[&'static Migration] =
            &[&TEST_MIG_1_TO_2, &TEST_MIG_2_TO_3];
        let mut m: Mapping = serde_yaml::from_str("a: 1\n").unwrap();
        let report = migrate_front_matter_with(&mut m, registry).unwrap();
        assert!(report.changed());
        assert_eq!(report.from_version, 1);
        assert_eq!(report.to_version, 3);
        assert_eq!(report.applied_ids(), vec!["test_v1_to_v2", "test_v2_to_v3"]);
        assert_eq!(read_schema_version(&m).unwrap(), 3);
        // The rename in the second migration moved a → b.
        assert!(!m.contains_key(Value::String("a".into())));
        assert!(m.contains_key(Value::String("b".into())));
    }

    #[test]
    fn migrate_starting_partway_through_chain() {
        let registry: &[&'static Migration] =
            &[&TEST_MIG_1_TO_2, &TEST_MIG_2_TO_3];
        // Source is already at v2, only the second migration should run.
        let mut m: Mapping =
            serde_yaml::from_str("schema-version: 2\na: 1\n").unwrap();
        let report = migrate_front_matter_with(&mut m, registry).unwrap();
        assert_eq!(report.from_version, 2);
        assert_eq!(report.to_version, 3);
        assert_eq!(report.applied_ids(), vec!["test_v2_to_v3"]);
    }

    fn failing_apply(_fm: &mut Mapping, _ctx: &MigrationContext) -> Result<()> {
        bail!("intentional test failure");
    }

    static TEST_MIG_FAIL: Migration = Migration {
        from_version: 1,
        to_version: 2,
        id: "test_fail",
        summary: "always fails",
        introduced_in: "test",
        apply: failing_apply,
    };

    #[test]
    fn migrate_aborts_on_step_failure_with_context() {
        let registry: &[&'static Migration] = &[&TEST_MIG_FAIL];
        let mut m: Mapping = serde_yaml::from_str("a: 1\n").unwrap();
        let err = migrate_front_matter_with(&mut m, registry).unwrap_err();
        let chain = format!("{:#}", err);
        assert!(
            chain.contains("test_fail (1 -> 2) failed"),
            "expected migration context, got: {}",
            chain
        );
        assert!(
            chain.contains("intentional test failure"),
            "expected inner error, got: {}",
            chain
        );
    }
}
