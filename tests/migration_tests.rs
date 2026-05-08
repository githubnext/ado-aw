//! Integration tests for the front-matter migration framework.
//!
//! These tests spawn the compiled `ado-aw` binary as a subprocess
//! (matching the pattern used in `tests/compiler_tests.rs`) and assert
//! on the user-visible behavior of `compile` and `check` for sources
//! with various `schema-version` shapes.
//!
//! The migration registry shipped with this binary is intentionally
//! empty (`CURRENT_SCHEMA_VERSION == 1`); the rewrite path is exercised
//! by the white-box tests in `src/compile/migrations/integration_test.rs`,
//! which can inject a stub registry. These tests cover the user-facing
//! CLI behaviors that don't require migration registration:
//!
//! - Future-version sources are rejected with a clear message.
//! - Non-integer / zero / negative `schema-version` is rejected.
//! - Healthy v1 sources (no `schema-version` field, or explicit `1`)
//!   compile and `check` cleanly without rewriting the source.
//! - The full `compile` -> `check` round-trip succeeds.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

/// Set up a unique temp directory for each test run.
fn fresh_temp_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ado-aw-migration-tests-{}-{}-{}",
        label,
        std::process::id(),
        rand_suffix(),
    ));
    fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

/// Same as [`fresh_temp_dir`] but also creates an empty `.git/`
/// directory at the root so `ado-aw check` (which walks up to the
/// repo root) can resolve a source path from the compiled lock file's
/// `@ado-aw` header.
fn fresh_git_temp_dir(label: &str) -> PathBuf {
    let dir = fresh_temp_dir(label);
    fs::create_dir(dir.join(".git")).expect("create .git dir");
    dir
}

/// Suffix that's unique within the process for the lifetime of a
/// single test binary run. Uses a wall-clock nanosecond timestamp
/// combined with a monotonic atomic counter so two parallel tests
/// scheduled in the same nanosecond still get distinct directories.
fn rand_suffix() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    format!("{:x}-{:x}", nanos, seq)
}

fn ado_aw_binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_ado-aw"))
}

/// Run `ado-aw compile <source>`, returning the captured output.
fn run_compile(source: &PathBuf) -> std::process::Output {
    Command::new(ado_aw_binary())
        .args(["compile", source.to_str().unwrap()])
        .output()
        .expect("Failed to run ado-aw compile")
}

/// Run `ado-aw check <pipeline>`, returning the captured output.
fn run_check(pipeline: &PathBuf) -> std::process::Output {
    Command::new(ado_aw_binary())
        .args(["check", pipeline.to_str().unwrap()])
        .output()
        .expect("Failed to run ado-aw check")
}

/// Write a source file to `dir/agent.md` and return its path.
fn write_source(dir: &PathBuf, content: &str) -> PathBuf {
    let path = dir.join("agent.md");
    fs::write(&path, content).expect("write source");
    path
}

// ─── Future-version rejection ──────────────────────────────────────────────

#[test]
fn compile_rejects_future_schema_version() {
    let dir = fresh_temp_dir("future-version");
    let source = write_source(
        &dir,
        "---\nschema-version: 99\nname: x\ndescription: y\n---\nbody\n",
    );

    let output = run_compile(&source);

    assert!(
        !output.status.success(),
        "compile should fail on future schema-version: stdout={:?} stderr={:?}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("only supports up to"),
        "stderr should mention compiler's supported schema-version range, got: {}",
        stderr
    );
    assert!(
        stderr.contains("99"),
        "stderr should mention the offending version 99, got: {}",
        stderr
    );

    let _ = fs::remove_dir_all(&dir);
}

// ─── Invalid schema-version values ─────────────────────────────────────────

#[test]
fn compile_rejects_zero_schema_version() {
    let dir = fresh_temp_dir("zero-version");
    let source = write_source(
        &dir,
        "---\nschema-version: 0\nname: x\ndescription: y\n---\nbody\n",
    );

    let output = run_compile(&source);

    assert!(
        !output.status.success(),
        "compile should fail on zero schema-version"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("must be a positive integer"),
        "stderr should reject zero with `must be a positive integer`, got: {}",
        stderr
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn compile_rejects_negative_schema_version() {
    let dir = fresh_temp_dir("negative-version");
    let source = write_source(
        &dir,
        "---\nschema-version: -1\nname: x\ndescription: y\n---\nbody\n",
    );

    let output = run_compile(&source);

    assert!(
        !output.status.success(),
        "compile should fail on negative schema-version"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("must be a positive integer"),
        "stderr should reject negative with `must be a positive integer`, got: {}",
        stderr
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn compile_rejects_non_integer_schema_version() {
    let dir = fresh_temp_dir("non-integer-version");
    let source = write_source(
        &dir,
        "---\nschema-version: \"not-a-number\"\nname: x\ndescription: y\n---\nbody\n",
    );

    let output = run_compile(&source);

    assert!(
        !output.status.success(),
        "compile should fail on non-integer schema-version"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("must be a positive integer"),
        "stderr should reject non-integer with `must be a positive integer`, got: {}",
        stderr
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn compile_rejects_float_schema_version() {
    let dir = fresh_temp_dir("float-version");
    let source = write_source(
        &dir,
        "---\nschema-version: 1.5\nname: x\ndescription: y\n---\nbody\n",
    );

    let output = run_compile(&source);

    assert!(
        !output.status.success(),
        "compile should fail on float schema-version"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("must be a positive integer"),
        "stderr should reject float with `must be a positive integer`, got: {}",
        stderr
    );

    let _ = fs::remove_dir_all(&dir);
}

// ─── Healthy v1 round-trip ─────────────────────────────────────────────────

#[test]
fn compile_succeeds_on_unstamped_v1_source() {
    let dir = fresh_temp_dir("unstamped-v1");
    let original = "---\nname: smoketest\ndescription: smoketest description\n---\n## Body\n\nHello.\n";
    let source = write_source(&dir, original);

    let output = run_compile(&source);

    assert!(
        output.status.success(),
        "compile should succeed on healthy unstamped source: stdout={:?} stderr={:?}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    // Lock file must be generated.
    let lock = source.with_extension("lock.yml");
    assert!(
        lock.exists(),
        "expected compiled YAML at {}",
        lock.display()
    );

    // With at least one registered migration (the repos_unified
    // migration), every unstamped v1 source is rewritten to add a
    // `schema-version` stamp at the latest known version. The body
    // must still be preserved.
    let after = fs::read_to_string(&source).expect("re-read source");
    assert!(
        after.contains("schema-version:"),
        "rewritten source should carry a `schema-version` stamp, got: {after}"
    );
    assert!(
        after.contains("## Body"),
        "body must be preserved across migration, got: {after}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn compile_succeeds_on_explicitly_stamped_v1_source() {
    let dir = fresh_temp_dir("stamped-v1");
    let original = "---\nschema-version: 1\nname: x\ndescription: y\n---\n## Body\n";
    let source = write_source(&dir, original);

    let output = run_compile(&source);

    assert!(
        output.status.success(),
        "compile should succeed on explicitly stamped v1 source: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Source is rewritten by the migration framework (version stamp
    // bumps to the current version), but the body must be preserved.
    let after = fs::read_to_string(&source).expect("re-read source");
    assert!(
        !after.contains("schema-version: 1\n"),
        "schema-version should have been bumped past 1, got: {after}"
    );
    assert!(
        after.contains("## Body"),
        "body must be preserved across migration, got: {after}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn compile_then_check_round_trip_passes() {
    let dir = fresh_git_temp_dir("round-trip");
    let source = write_source(
        &dir,
        "---\nname: round-trip-agent\ndescription: round-trip\n---\n## Body\n",
    );

    let compile_output = run_compile(&source);
    assert!(
        compile_output.status.success(),
        "compile should succeed: {}",
        String::from_utf8_lossy(&compile_output.stderr)
    );

    let lock = source.with_extension("lock.yml");
    assert!(lock.exists(), "expected lock file at {}", lock.display());

    // `check` reads the source path from the lock file's @ado-aw header.
    // The header records an absolute or relative path from the compile
    // invocation; since we passed an absolute path, that's what we get.
    let check_output = run_check(&lock);
    assert!(
        check_output.status.success(),
        "check should succeed: stdout={:?} stderr={:?}",
        String::from_utf8_lossy(&check_output.stdout),
        String::from_utf8_lossy(&check_output.stderr)
    );

    let _ = fs::remove_dir_all(&dir);
}

// ─── check rejects future-version sources ──────────────────────────────────

#[test]
fn check_rejects_future_schema_version() {
    // Setup: compile a healthy source so we have a valid lock file with
    // a header pointing at our source path. Then mutate the source to
    // claim a future schema version, and confirm `check` fails.
    let dir = fresh_git_temp_dir("check-future");
    let source = write_source(
        &dir,
        "---\nname: x\ndescription: y\n---\n## Body\n",
    );

    let compile_output = run_compile(&source);
    assert!(
        compile_output.status.success(),
        "initial compile should succeed: {}",
        String::from_utf8_lossy(&compile_output.stderr)
    );

    // Mutate the source to claim a future version. Note: this mutation
    // would also fail the lock-file integrity check, but the migration
    // runner runs first so we observe the schema-version error.
    fs::write(
        &source,
        "---\nschema-version: 99\nname: x\ndescription: y\n---\n## Body\n",
    )
    .expect("rewrite source");

    let lock = source.with_extension("lock.yml");
    let check_output = run_check(&lock);
    assert!(
        !check_output.status.success(),
        "check should fail when source is at a future schema-version"
    );
    let stderr = String::from_utf8_lossy(&check_output.stderr);
    assert!(
        stderr.contains("only supports up to") || stderr.contains("Failed to migrate"),
        "stderr should report the future-version error, got: {}",
        stderr
    );

    let _ = fs::remove_dir_all(&dir);
}

// ─── Non-mapping front matter ──────────────────────────────────────────────

#[test]
fn compile_rejects_non_mapping_top_level_yaml() {
    let dir = fresh_temp_dir("non-mapping");
    let source = write_source(&dir, "---\n- a\n- b\n---\nbody\n");

    let output = run_compile(&source);

    assert!(
        !output.status.success(),
        "compile should fail when front matter is a sequence not a mapping"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("must be a mapping"),
        "stderr should report non-mapping error, got: {}",
        stderr
    );

    let _ = fs::remove_dir_all(&dir);
}
