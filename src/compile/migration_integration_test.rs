//! White-box integration tests for the front-matter migration framework.
//!
//! These tests exercise the rewrite path end-to-end (parse → migrate →
//! compile → atomic source rewrite → lock-file write) using a stub
//! migration registry. They live inside `src/` because they need
//! access to crate-private hooks (`compile_pipeline_with_registry`,
//! `perform_source_rewrite_if_needed`,
//! `common::parse_markdown_detailed_with_registry`) that the empty
//! production [`super::migrations::MIGRATIONS`] registry cannot
//! exercise.
//!
//! Black-box subprocess tests of the user-visible CLI behavior live in
//! [`tests/migration_tests.rs`](../../tests/migration_tests.rs).

#![cfg(test)]

use super::common;
use super::migrations::{ConflictPolicy, Migration, MigrationContext};
use super::{compile_pipeline, compile_pipeline_with_registry, perform_source_rewrite_if_needed};
use anyhow::Result;
use serde_yaml::Mapping;

// ─── Stub registry ──────────────────────────────────────────────────────────

/// Stub migration: renames the `legacy-name` top-level key to `name`.
/// Drives end-to-end rewrite tests without registering a real migration.
static TEST_RENAME_LEGACY_NAME: Migration = Migration {
    from_version: 1,
    to_version: 2,
    id: "test_rename_legacy_name",
    summary: "rename `legacy-name` -> `name`",
    introduced_in: "test",
    apply: rename_legacy_name,
};

fn rename_legacy_name(fm: &mut Mapping, _ctx: &MigrationContext) -> Result<()> {
    super::migrations::rename_key(fm, "legacy-name", "name", ConflictPolicy::Error)?;
    Ok(())
}

/// Source whose typed deserialization would fail without a migration:
/// it has `legacy-name` instead of the required `name` field.
fn stale_source() -> &'static str {
    "---\nlegacy-name: my-agent\ndescription: test description\n---\n## Body\nHello.\n"
}

fn write_temp_md(name: &str, content: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join(name);
    std::fs::write(&path, content).expect("write temp md");
    (dir, path)
}

// ─── End-to-end rewrite path ────────────────────────────────────────────────

#[tokio::test]
async fn migration_rewrites_stale_source_and_preserves_body() {
    let (dir, source_path) = write_temp_md("agent.md", stale_source());

    let registry: &[&'static Migration] = &[&TEST_RENAME_LEGACY_NAME];
    let migrated = compile_pipeline_with_registry(
        &source_path.to_string_lossy(),
        None,
        true,  // skip_integrity
        false, // debug_pipeline
        registry,
    )
    .await
    .expect("compile_pipeline_with_registry should succeed");

    assert!(migrated, "expected compile to report a source migration");

    // Source file rewritten: contains `name: my-agent`, no
    // `legacy-name`, has `schema-version: 2`.
    let rewritten = std::fs::read_to_string(&source_path).expect("read rewritten");
    assert!(
        rewritten.contains("name: my-agent"),
        "rewritten source missing `name: my-agent`:\n{}",
        rewritten
    );
    assert!(
        !rewritten.contains("legacy-name"),
        "rewritten source still contains legacy-name:\n{}",
        rewritten
    );
    assert!(
        rewritten.contains("schema-version: 2"),
        "rewritten source missing `schema-version: 2`:\n{}",
        rewritten
    );

    // Body region byte-identical to the original (everything after
    // the closing `---`).
    let orig = stale_source();
    let orig_body = &orig[orig.find("\n---\n").unwrap() + 5..];
    let new_body = &rewritten[rewritten.find("\n---\n").unwrap() + 5..];
    assert_eq!(orig_body, new_body, "body region not preserved byte-for-byte");

    // Lock file generated.
    let lock = source_path.with_extension("lock.yml");
    assert!(lock.exists(), "expected {} to exist", lock.display());

    // Re-running parse with the same stub registry produces no
    // further migration — the rewrite moved the source to current.
    let after = std::fs::read_to_string(&source_path).unwrap();
    let parsed_again =
        common::parse_markdown_detailed_with_registry(&after, registry).unwrap();
    assert!(
        !parsed_again.migrations.changed(),
        "expected post-rewrite source to be at current schema-version, but \
         {} more migration(s) ran",
        parsed_again.migrations.applied.len()
    );

    drop(dir);
}

#[tokio::test]
async fn migration_skip_when_no_stub_registry_runs() {
    // With the empty production registry, a healthy v1 source must
    // compile without rewriting anything.
    let healthy = "---\nname: x\ndescription: y\n---\nbody\n";
    let (dir, source_path) = write_temp_md("agent.md", healthy);
    let registry: &[&'static Migration] = &[];
    let migrated = compile_pipeline_with_registry(
        &source_path.to_string_lossy(),
        None,
        true,
        false,
        registry,
    )
    .await
    .expect("compile should succeed");
    assert!(!migrated, "expected no rewrite for already-current source");
    let after = std::fs::read_to_string(&source_path).unwrap();
    assert_eq!(after, healthy, "source must be byte-identical");
    drop(dir);
}

#[tokio::test]
async fn migration_with_only_version_bump_still_writes() {
    fn noop(_fm: &mut Mapping, _ctx: &MigrationContext) -> Result<()> {
        Ok(())
    }
    static NOOP_MIG: Migration = Migration {
        from_version: 1,
        to_version: 2,
        id: "test_noop_v1_to_v2",
        summary: "no-op",
        introduced_in: "test",
        apply: noop,
    };

    // Even a no-op migration changes bytes on disk because the
    // schema-version field is added. Verify we DO rewrite.
    let healthy = "---\nname: x\ndescription: y\n---\nbody\n";
    let (dir, source_path) = write_temp_md("agent.md", healthy);
    let registry: &[&'static Migration] = &[&NOOP_MIG];
    let migrated = compile_pipeline_with_registry(
        &source_path.to_string_lossy(),
        None,
        true,
        false,
        registry,
    )
    .await
    .expect("compile should succeed");
    assert!(
        migrated,
        "expected rewrite because schema-version field was added"
    );
    let after = std::fs::read_to_string(&source_path).unwrap();
    assert!(after.contains("schema-version: 2"));
    drop(dir);
}

// ─── Lost-update guard ──────────────────────────────────────────────────────

#[tokio::test]
async fn perform_source_rewrite_lost_update_guard() {
    // Construct a parsed-source pointing at a file whose hash will
    // not match (we mutate it after computing the hash). This
    // exercises the lost-update guard directly without needing a
    // full compile.
    let (dir, source_path) = write_temp_md("agent.md", stale_source());
    let original = std::fs::read_to_string(&source_path).unwrap();
    let registry: &[&'static Migration] = &[&TEST_RENAME_LEGACY_NAME];
    let parsed =
        common::parse_markdown_detailed_with_registry(&original, registry).unwrap();
    assert!(parsed.migrations.changed());

    // Mutate the source after parse (simulates editor / concurrent
    // tool overwriting).
    std::fs::write(&source_path, "different bytes\n").unwrap();

    // Rewrite must refuse.
    let result = perform_source_rewrite_if_needed(
        &source_path,
        &original,
        &parsed.leading_whitespace,
        &parsed.front_matter_mapping,
        &parsed.body_raw,
        &parsed.source_sha256,
        &parsed.migrations,
    )
    .await;
    let err = result.expect_err("expected lost-update guard to fire");
    assert!(
        format!("{:#}", err).contains("changed during compilation"),
        "unexpected error: {:#}",
        err
    );

    // The source is left as the interloper wrote it (we did not
    // clobber); it is *not* the migrated version.
    let after = std::fs::read_to_string(&source_path).unwrap();
    assert_eq!(after, "different bytes\n");

    drop(dir);
}

// ─── Future-version safety net ──────────────────────────────────────────────

#[tokio::test]
async fn check_pipeline_fails_on_future_schema_version() {
    // The real registry is empty (CURRENT=1), so a source claiming
    // schema-version: 2 is a future version. compile should reject it
    // loudly through the public entry point.
    let future = "---\nschema-version: 2\nname: x\ndescription: y\n---\n";
    let (dir, source_path) = write_temp_md("agent.md", future);
    let result = compile_pipeline(&source_path.to_string_lossy(), None, true, false).await;
    let err = result.expect_err("future-version source should fail compile");
    assert!(
        format!("{:#}", err).contains("only supports up to"),
        "unexpected error: {:#}",
        err
    );
    drop(dir);
}
