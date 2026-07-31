//! Integration tests for the front-matter codemod framework.
//!
//! These tests spawn the compiled `ado-aw` binary as a subprocess
//! (matching the pattern used in `tests/compiler_tests.rs`) and
//! assert on the user-visible behavior of `compile` and `check` for
//! sources with various front-matter shapes.
//!
//! The codemod registry shipped with this binary is intentionally
//! empty; the rewrite path is exercised by the white-box tests in
//! `src/compile/codemod_integration_test.rs`, which can inject a
//! stub registry. These tests cover the user-facing CLI behaviors
//! that don't require codemod registration:
//!
//! - Healthy current sources compile and `check` cleanly without
//!   rewriting the source.
//! - Non-mapping front matter is rejected with a clear message.
//! - The full `compile` -> `check` round-trip succeeds.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

/// Set up a unique temp directory for each test run. Returned as a
/// `TempDir` so RAII cleans the directory up even if a test panics.
fn fresh_temp_dir() -> TempDir {
    tempfile::Builder::new()
        .prefix("ado-aw-codemod-tests-")
        .tempdir()
        .expect("create temp dir")
}

/// Same as [`fresh_temp_dir`] but also creates an empty `.git/`
/// directory at the root so `ado-aw check` (which walks up to the
/// repo root) can resolve a source path from the compiled lock
/// file's `@ado-aw` header.
fn fresh_git_temp_dir() -> TempDir {
    let dir = fresh_temp_dir();
    fs::create_dir(dir.path().join(".git")).expect("create .git dir");
    dir
}

fn ado_aw_binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_ado-aw"))
}

/// Run `ado-aw compile <source>`, returning the captured output.
fn run_compile(source: &Path) -> std::process::Output {
    Command::new(ado_aw_binary())
        .args(["compile", source.to_str().unwrap()])
        .output()
        .expect("Failed to run ado-aw compile")
}

/// Run `ado-aw check <pipeline>`, returning the captured output.
fn run_check(pipeline: &Path) -> std::process::Output {
    Command::new(ado_aw_binary())
        .args(["check", pipeline.to_str().unwrap()])
        .output()
        .expect("Failed to run ado-aw check")
}

/// Write a source file to `dir/agent.md` and return its path.
fn write_source(dir: &Path, content: &str) -> PathBuf {
    let path = dir.join("agent.md");
    fs::write(&path, content).expect("write source");
    path
}

/// Copy a fixture into the test workspace before compiling so any source
/// rewrites stay isolated to that workspace.
fn copy_fixture(dir: &Path, fixture_name: &str) -> PathBuf {
    let source = fixture_path(fixture_name);
    let dest = dir.join(fixture_name);
    fs::copy(&source, &dest)
        .unwrap_or_else(|e| panic!("copy fixture {} into test workspace: {e}", fixture_name));
    dest
}

// ─── Legacy directory marker migration (codemod 0004) ──────────────────────

#[test]
fn compile_migrates_legacy_workspace_marker_in_steps() {
    let dir = fresh_temp_dir();
    let original = "---\nname: ws-marker\ndescription: d\nsteps:\n  - script: cd {{ workspace }} && ls\n---\n## Body\n\nHello.\n";
    let source = write_source(dir.path(), original);

    let output = run_compile(&source);
    assert!(
        output.status.success(),
        "compile should succeed: stdout={:?} stderr={:?}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    // The source is rewritten in place: the marker is replaced with the
    // explicit ADO path it resolved to (single-checkout → sources root).
    let after = fs::read_to_string(&source).expect("re-read source");
    assert!(
        after.contains("cd $(Build.SourcesDirectory) && ls"),
        "source should be migrated, got:\n{after}"
    );
    assert!(
        !after.contains("{{ workspace }}"),
        "legacy marker must be gone from source, got:\n{after}"
    );

    // The codemod warning is surfaced.
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("applied codemods"),
        "expected codemod warning, got stderr: {stderr}"
    );

    // The compiled lock file carries the resolved path, not the marker.
    let lock = source.with_extension("lock.yml");
    let lock_str = fs::read_to_string(&lock).expect("read lock");
    assert!(
        lock_str.contains("cd $(Build.SourcesDirectory) && ls"),
        "lock should contain resolved path, got:\n{lock_str}"
    );
    assert!(
        !lock_str.contains("{{ workspace }}"),
        "lock must not contain the legacy marker"
    );
}

// ─── Healthy compile (no codemods needed) ──────────────────────────────────

#[test]
fn compile_succeeds_on_current_source() {
    let dir = fresh_temp_dir();
    let original =
        "---\nname: smoketest\ndescription: smoketest description\n---\n## Body\n\nHello.\n";
    let source = write_source(dir.path(), original);

    let output = run_compile(&source);

    assert!(
        output.status.success(),
        "compile should succeed on healthy source: stdout={:?} stderr={:?}",
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

    // Empty registry + healthy source must NOT rewrite — verify
    // byte-identity.
    let after = fs::read_to_string(&source).expect("re-read source");
    assert_eq!(
        after, original,
        "source must be byte-identical after compile when no codemods apply"
    );

    // Stderr should NOT contain a codemod warning.
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("warning: applied codemods"),
        "no codemod warning expected, got stderr: {}",
        stderr
    );
}

#[test]
fn compile_then_check_round_trip_passes() {
    let dir = fresh_git_temp_dir();
    let source = write_source(
        dir.path(),
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

    let check_output = run_check(&lock);
    assert!(
        check_output.status.success(),
        "check should succeed: stdout={:?} stderr={:?}",
        String::from_utf8_lossy(&check_output.stdout),
        String::from_utf8_lossy(&check_output.stderr)
    );
}

// ─── Integrity check semantics ─────────────────────────────────────────────

#[test]
fn test_integrity_check_inlined_imports_false_passes_on_body_edit() {
    let dir = fresh_git_temp_dir();
    let source = copy_fixture(dir.path(), "integrity-check-default.md");

    let compile_output = run_compile(&source);
    assert!(
        compile_output.status.success(),
        "compile should succeed: {}",
        String::from_utf8_lossy(&compile_output.stderr)
    );

    let lock = source.with_extension("lock.yml");
    assert!(lock.exists(), "expected lock file at {}", lock.display());

    let original = fs::read_to_string(&source).expect("read source after compile");
    fs::write(&source, format!("{original}\n\nAdditional body content.\n"))
        .expect("append body-only edit");

    let check_output = run_check(&lock);
    assert!(
        check_output.status.success(),
        "body-only edits should not trip integrity check when imports are not inlined: stdout={:?} stderr={:?}",
        String::from_utf8_lossy(&check_output.stdout),
        String::from_utf8_lossy(&check_output.stderr)
    );
}

#[test]
fn test_integrity_check_inlined_imports_false_fails_on_frontmatter_edit() {
    let dir = fresh_git_temp_dir();
    let source = copy_fixture(dir.path(), "integrity-check-default.md");

    let compile_output = run_compile(&source);
    assert!(
        compile_output.status.success(),
        "compile should succeed: {}",
        String::from_utf8_lossy(&compile_output.stderr)
    );

    let lock = source.with_extension("lock.yml");
    assert!(lock.exists(), "expected lock file at {}", lock.display());

    let original = fs::read_to_string(&source).expect("read source after compile");
    let edited = original.replace(
        "name: integrity-default-agent",
        "name: integrity-default-agent-renamed",
    );
    assert_ne!(edited, original, "front matter edit should change fixture");
    fs::write(&source, edited).expect("write front matter edit");

    let check_output = run_check(&lock);
    assert!(
        !check_output.status.success(),
        "front-matter edits must fail integrity check: stdout={:?} stderr={:?}",
        String::from_utf8_lossy(&check_output.stdout),
        String::from_utf8_lossy(&check_output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&check_output.stderr).contains("Integrity check failed"),
        "front-matter edits should fail with the integrity-check error"
    );
}

#[test]
fn test_integrity_check_inlined_imports_true_fails_on_body_edit() {
    let dir = fresh_git_temp_dir();
    let source = copy_fixture(dir.path(), "integrity-check-inlined.md");

    let compile_output = run_compile(&source);
    assert!(
        compile_output.status.success(),
        "compile should succeed: {}",
        String::from_utf8_lossy(&compile_output.stderr)
    );

    let lock = source.with_extension("lock.yml");
    assert!(lock.exists(), "expected lock file at {}", lock.display());

    let original = fs::read_to_string(&source).expect("read source after compile");
    fs::write(&source, format!("{original}\n\nAdditional body content.\n"))
        .expect("append body-only edit");

    let check_output = run_check(&lock);
    assert!(
        !check_output.status.success(),
        "body edits must fail integrity check when imports are inlined: stdout={:?} stderr={:?}",
        String::from_utf8_lossy(&check_output.stdout),
        String::from_utf8_lossy(&check_output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&check_output.stderr).contains("Integrity check failed"),
        "inlined body edits should fail with the integrity-check error"
    );
}

#[test]
fn test_integrity_check_resolves_imports_and_passes() {
    // Regression: `ado-aw check` must resolve `imports:` the same way `compile`
    // does. Before the shared resolve-and-merge helper, `check` skipped import
    // resolution entirely, so a freshly-compiled import-using workflow reported
    // false "drift" (missing imported tools + body). A local import needs no
    // cache/network, so this exercises the merge deterministically.
    let dir = fresh_git_temp_dir();
    fs::write(
        dir.path().join("component.md"),
        "---\ntools:\n  edit: true\n---\nImported guidance line.\n",
    )
    .expect("write component");
    let source = write_source(
        dir.path(),
        "---\nname: check-imports-agent\ndescription: check resolves imports\nimports:\n  - component.md\n---\nConsumer body.\n",
    );

    let compile_output = run_compile(&source);
    assert!(
        compile_output.status.success(),
        "compile should succeed: {}",
        String::from_utf8_lossy(&compile_output.stderr)
    );
    let lock = source.with_extension("lock.yml");
    assert!(lock.exists(), "expected lock file at {}", lock.display());

    // The imported tool + inlined imported body must be present in the lock.
    let lock_content = fs::read_to_string(&lock).expect("read lock");
    assert!(
        lock_content.contains("Imported guidance line."),
        "compiled lock should inline the imported body"
    );

    // check must PASS on the freshly compiled, unedited import-using workflow.
    let check_output = run_check(&lock);
    assert!(
        check_output.status.success(),
        "check must resolve imports and pass on a fresh compile: stdout={:?} stderr={:?}",
        String::from_utf8_lossy(&check_output.stdout),
        String::from_utf8_lossy(&check_output.stderr)
    );
}

// ─── Non-mapping front matter ──────────────────────────────────────────────

#[test]
fn compile_rejects_non_mapping_top_level_yaml() {
    let dir = fresh_temp_dir();
    let source = write_source(dir.path(), "---\n- a\n- b\n---\nbody\n");

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
}
