//! White-box integration tests for the front-matter codemod
//! framework.
//!
//! These tests exercise the rewrite path end-to-end (parse →
//! codemods → compile → atomic source rewrite → lock-file write)
//! using a stub codemod registry. They live inside `src/` because
//! they need access to crate-private hooks
//! (`compile_pipeline_with_registry`,
//! `perform_source_rewrite_if_needed`,
//! `common::parse_markdown_detailed_with_registry`) that the empty
//! production [`super::codemods::CODEMODS`] registry cannot
//! exercise.
//!
//! Black-box subprocess tests of the user-visible CLI behavior live
//! in [`tests/codemod_tests.rs`](../../tests/codemod_tests.rs).

#![cfg(test)]

use super::codemods::{Codemod, CodemodContext, ConflictPolicy};
use super::common;
use super::{compile_pipeline_with_registry, perform_source_rewrite_if_needed};
use anyhow::Result;
use serde_yaml::Mapping;

// ─── Stub registry ──────────────────────────────────────────────────────────

/// Stub codemod: renames the `legacy-name` top-level key to `name`.
/// Drives end-to-end rewrite tests without registering a real codemod.
static TEST_RENAME_LEGACY_NAME: Codemod = Codemod {
    id: "test_rename_legacy_name",
    summary: "rename `legacy-name` -> `name`",
    introduced_in: "test",
    apply: rename_legacy_name,
};

fn rename_legacy_name(fm: &mut Mapping, _ctx: &CodemodContext) -> Result<bool> {
    super::codemods::rename_key(fm, "legacy-name", "name", ConflictPolicy::Error)
}

/// Source whose typed deserialization would fail without a codemod:
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
async fn codemod_rewrites_stale_source_and_preserves_body() {
    let (dir, source_path) = write_temp_md("agent.md", stale_source());

    let registry: &[&'static Codemod] = &[&TEST_RENAME_LEGACY_NAME];
    let rewrote = compile_pipeline_with_registry(
        &source_path.to_string_lossy(),
        None,
        true,  // skip_integrity
        false, // debug_pipeline
        registry,
    )
    .await
    .expect("compile_pipeline_with_registry should succeed");

    assert!(rewrote, "expected compile to report a source rewrite");

    // Source file rewritten: contains `name: my-agent`, no
    // `legacy-name`.
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

    // Body region byte-identical to the original (everything after
    // the closing `---`).
    let orig = stale_source();
    let orig_body = &orig[orig.find("\n---\n").unwrap() + 5..];
    let new_body = &rewritten[rewritten.find("\n---\n").unwrap() + 5..];
    assert_eq!(
        orig_body, new_body,
        "body region not preserved byte-for-byte"
    );

    // Lock file generated.
    let lock = source_path.with_extension("lock.yml");
    assert!(lock.exists(), "expected {} to exist", lock.display());

    // Re-running parse with the same stub registry produces no
    // further codemod fires — the rewrite moved the source to its
    // current shape.
    let after = std::fs::read_to_string(&source_path).unwrap();
    let parsed_again = common::parse_markdown_detailed_with_registry(&after, registry, None).unwrap();
    assert!(
        !parsed_again.codemods.changed(),
        "expected post-rewrite source to require no further codemods, but \
         {} fired",
        parsed_again.codemods.applied.len()
    );

    drop(dir);
}

#[tokio::test]
async fn codemod_skip_when_no_stub_registry_runs() {
    // With the empty production registry, a healthy source must
    // compile without rewriting anything.
    let healthy = "---\nname: x\ndescription: y\n---\nbody\n";
    let (dir, source_path) = write_temp_md("agent.md", healthy);
    let registry: &[&'static Codemod] = &[];
    let rewrote =
        compile_pipeline_with_registry(&source_path.to_string_lossy(), None, true, false, registry)
            .await
            .expect("compile should succeed");
    assert!(
        !rewrote,
        "expected no rewrite for source that needs no codemods"
    );
    let after = std::fs::read_to_string(&source_path).unwrap();
    assert_eq!(after, healthy, "source must be byte-identical");
    drop(dir);
}

#[tokio::test]
async fn codemod_no_op_returns_false_does_not_rewrite() {
    // A codemod that always returns Ok(false) must not trigger a
    // source rewrite even though it ran.
    fn noop(_fm: &mut Mapping, _ctx: &CodemodContext) -> Result<bool> {
        Ok(false)
    }
    static NOOP_MIG: Codemod = Codemod {
        id: "test_noop_returns_false",
        summary: "no-op",
        introduced_in: "test",
        apply: noop,
    };

    let healthy = "---\nname: x\ndescription: y\n---\nbody\n";
    let (dir, source_path) = write_temp_md("agent.md", healthy);
    let registry: &[&'static Codemod] = &[&NOOP_MIG];
    let rewrote =
        compile_pipeline_with_registry(&source_path.to_string_lossy(), None, true, false, registry)
            .await
            .expect("compile should succeed");
    assert!(
        !rewrote,
        "no-op codemod that returns Ok(false) must not trigger a rewrite"
    );
    let after = std::fs::read_to_string(&source_path).unwrap();
    assert_eq!(after, healthy, "source must be byte-identical");
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
    let registry: &[&'static Codemod] = &[&TEST_RENAME_LEGACY_NAME];
    let parsed = common::parse_markdown_detailed_with_registry(&original, registry, None).unwrap();
    assert!(parsed.codemods.changed());

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
        &parsed.codemods,
    )
    .await;
    let err = result.expect_err("expected lost-update guard to fire");
    assert!(
        format!("{:#}", err).contains("changed during compilation"),
        "unexpected error: {:#}",
        err
    );

    // The source is left as the interloper wrote it (we did not
    // clobber); it is *not* the codemod-rewritten version.
    let after = std::fs::read_to_string(&source_path).unwrap();
    assert_eq!(after, "different bytes\n");

    drop(dir);
}

// ─── on.push migration (real registry) ──────────────────────────────────────

/// Write a `.lock.yml` next to `source_path` carrying the
/// `# @ado-aw … version=…` header the codemod reads for provenance.
fn write_lock_with_version(source_path: &std::path::Path, version: &str) {
    let lock = source_path.with_extension("lock.yml");
    let name = source_path.file_name().unwrap().to_string_lossy();
    std::fs::write(
        &lock,
        format!(
            "# This file is auto-generated by ado-aw.\n\
             # @ado-aw source=\"{name}\" version={version}\n\
             name: legacy\n"
        ),
    )
    .expect("write lock");
}

async fn compile_with_real_registry(source_path: &std::path::Path) -> bool {
    compile_pipeline_with_registry(
        &source_path.to_string_lossy(),
        None,
        true,
        false,
        super::codemods::CODEMODS,
    )
    .await
    .expect("compile should succeed")
}

/// A source with no `on:` compiles to a manual-only pipeline. The migration
/// codemod is gated on the *running* binary reaching its cutover release, so
/// while the crate version is below that threshold it must stay dormant and
/// leave the source untouched.
///
/// The fire path itself is covered by unit tests in
/// `codemods/0006_explicit_push_trigger.rs`, which can vary the running
/// version that this path takes from `CARGO_PKG_VERSION`.
#[tokio::test]
async fn on_push_codemod_is_dormant_before_its_cutover_release() {
    let src = "---\nname: legacy\ndescription: d\n---\n## Body\nHello.\n";
    let (dir, source_path) = write_temp_md("legacy-agent.md", src);
    write_lock_with_version(&source_path, "0.1.0");

    let running_is_pre_cutover = !crate::version::is_at_least(
        env!("CARGO_PKG_VERSION"),
        super::codemods::EXPLICIT_PUSH_TRIGGER_INTRODUCED_IN,
    );

    let rewrote = compile_with_real_registry(&source_path).await;
    let after = std::fs::read_to_string(&source_path).unwrap();

    if running_is_pre_cutover {
        assert!(!rewrote, "codemod must be dormant before its cutover");
        assert_eq!(after, src, "source must be byte-identical");
    } else {
        assert!(rewrote, "an ancient source must be migrated after cutover");
        assert!(
            after.contains("push:"),
            "migrated source must declare on.push:\n{after}"
        );
        let lock = std::fs::read_to_string(source_path.with_extension("lock.yml")).unwrap();
        let doc: serde_yaml::Value = serde_yaml::from_str(&lock).expect("valid YAML");
        let include = doc
            .get("trigger")
            .and_then(|t| t.get("branches"))
            .and_then(|b| b.get("include"))
            .and_then(|i| i.as_sequence())
            .expect("trigger.branches.include");
        assert_eq!(
            include.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>(),
            vec!["*"],
            "legacy CI-on-every-branch behaviour must be preserved:\n{lock}"
        );
    }

    drop(dir);
}

/// A source whose lock file was produced at or after the cutover is newly
/// authored, so an absent `on:` is the deliberate spelling of
/// "manual / API-queued only" and must be left alone.
#[tokio::test]
async fn on_push_codemod_leaves_a_post_cutover_source_manual_only() {
    let src = "---\nname: current\ndescription: d\n---\n## Body\nHello.\n";
    let (dir, source_path) = write_temp_md("current-agent.md", src);
    write_lock_with_version(
        &source_path,
        super::codemods::EXPLICIT_PUSH_TRIGGER_INTRODUCED_IN,
    );

    let rewrote = compile_with_real_registry(&source_path).await;
    assert!(!rewrote, "a post-cutover source must not be rewritten");
    assert_eq!(
        std::fs::read_to_string(&source_path).unwrap(),
        src,
        "source must be byte-identical"
    );

    let lock = std::fs::read_to_string(source_path.with_extension("lock.yml")).unwrap();
    assert!(
        lock.contains("trigger: none"),
        "no `on:` must compile to a manual-only pipeline:\n{lock}"
    );

    drop(dir);
}

/// Regression: `compile` writes a lock stamped with the running version, and
/// the next `check` reads that stamp back as the source's provenance. If the
/// migration gate ignored the running version, a just-compiled source would
/// immediately report a pending migration.
#[tokio::test]
async fn compile_then_reparse_reports_no_pending_migration() {
    let src = "---\nname: fresh\ndescription: d\n---\n## Body\nHello.\n";
    let (dir, source_path) = write_temp_md("fresh-agent.md", src);

    compile_with_real_registry(&source_path).await;

    // Re-parse exactly as `check` does: with the version the compile just
    // stamped into the lock file.
    let lock = std::fs::read_to_string(source_path.with_extension("lock.yml")).unwrap();
    let stamped = lock
        .lines()
        .find_map(crate::detect::parse_header_line)
        .map(|meta| meta.version)
        .filter(|v| !v.is_empty())
        .expect("compiled lock must record a version");

    let after = std::fs::read_to_string(&source_path).unwrap();
    let reparsed = common::parse_markdown_detailed_for_source(&after, Some(&stamped)).unwrap();
    assert!(
        !reparsed.codemods.changed(),
        "a just-compiled source must not report pending codemods, but {:?} fired",
        reparsed.codemods.applied_ids()
    );

    drop(dir);
}
