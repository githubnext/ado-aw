//! Integration tests for the front-matter codemod framework.
//!
//! These tests spawn the compiled `ado-aw` binary as a subprocess
//! (matching the pattern used in `tests/compiler_tests.rs`) and
//! assert on the user-visible behavior of `compile` and `check` for
//! sources with various front-matter shapes.
//!
//! White-box rewrite mechanics are exercised in
//! `src/compile/codemod_integration_test.rs`, which can inject a
//! stub registry. These tests cover shipped codemods and user-facing
//! CLI behavior:
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

#[test]
fn compile_migrates_pr_tools_and_keeps_prompt_warning_until_fixed() {
    let dir = fresh_git_temp_dir();
    let original = "---\r\nname: pr-migration\r\ndescription: d\r\nsafe-outputs:\r\n  update-pr:\r\n    allowed-operations: [add-reviewers, update-description]\r\n    allowed-reviewers: [owner@example.test]\r\n    max: 1\r\n---\r\nCall `update-pr` to add reviewers.\r\n";
    let source = write_source(dir.path(), original);
    let first = run_compile(&source);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let after = fs::read_to_string(&source).unwrap();
    assert!(after.ends_with("\r\nCall `update-pr` to add reviewers.\r\n"));
    let fm: serde_yaml::Value = serde_yaml::from_str(after.split("---").nth(1).unwrap()).unwrap();
    assert!(fm["safe-outputs"]["update-pr"].is_null());
    assert_eq!(fm["safe-outputs"]["budget-groups"]["update-pr"]["max"], 1);
    assert!(String::from_utf8_lossy(&first.stderr).contains("deprecated-tool-reference"));
    assert!(String::from_utf8_lossy(&first.stderr).contains("add-pull-request-reviewers"));
    let second = run_compile(&source);
    assert!(second.status.success());
    assert_eq!(fs::read_to_string(&source).unwrap(), after);
    assert!(String::from_utf8_lossy(&second.stderr).contains("deprecated-tool-reference"));
    let lint = Command::new(ado_aw_binary())
        .args(["lint", source.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert!(
        lint.status.success(),
        "{}",
        String::from_utf8_lossy(&lint.stderr)
    );
    assert!(String::from_utf8_lossy(&lint.stdout).contains("deprecated-tool-reference"));
}

#[test]
fn conflicting_pr_migration_does_not_rewrite_source_or_lock() {
    let dir = fresh_git_temp_dir();
    let original = "---\nname: conflict\ndescription: d\nsafe-outputs:\n  update-pr:\n    allowed-operations: [update-description]\n  update-pull-request: {}\n---\nBody\n";
    let source = write_source(dir.path(), original);
    let output = run_compile(&source);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("manual migration required"));
    assert_eq!(fs::read_to_string(&source).unwrap(), original);
    assert!(!source.with_extension("lock.yml").exists());
}

#[test]
fn compile_expands_all_pr_tool_names_and_budget_members_without_body_edits() {
    let dir = fresh_git_temp_dir();
    let original = "---\nname: full-names\ndescription: d\nsafe-outputs:\n  require-approval: true\n  staged: true\n  add-pr-comment: {max: 2}\n  reply-to-pr-comment: {max: 2}\n  resolve-pr-thread: {allowed-statuses: [fixed], max: 2}\n  submit-pr-review: {allowed-events: [comment], max: 2}\n  add-pr-reviewers: {allowed-reviewers: [owner@example.test], max-reviewers: 1, max: 2}\n  add-pr-labels: {max: 2}\n  set-pr-auto-complete: {max: 2}\n  budget-groups:\n    shared: {max: 1, tools: [add-pr-reviewers, add-pr-labels]}\n---\nCall `add-pr-comment` and `submit-pr-review`.\n";
    let source = write_source(dir.path(), original);
    let output = run_compile(&source);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let rewritten = fs::read_to_string(&source).unwrap();
    assert!(rewritten.ends_with("\nCall `add-pr-comment` and `submit-pr-review`.\n"));
    let fm: serde_yaml::Value =
        serde_yaml::from_str(rewritten.split("---").nth(1).unwrap()).unwrap();
    for (old, new) in [
        ("add-pr-comment", "add-pull-request-comment"),
        ("reply-to-pr-comment", "reply-to-pull-request-comment"),
        ("resolve-pr-thread", "resolve-pull-request-thread"),
        ("submit-pr-review", "submit-pull-request-review"),
        ("add-pr-reviewers", "add-pull-request-reviewers"),
        ("add-pr-labels", "add-pull-request-labels"),
        ("set-pr-auto-complete", "set-pull-request-auto-complete"),
    ] {
        assert!(fm["safe-outputs"][old].is_null());
        assert_eq!(fm["safe-outputs"][new]["max"], 2);
    }
    assert_eq!(fm["safe-outputs"]["budget-groups"]["shared"]["max"], 1);
    assert_eq!(
        fm["safe-outputs"]["budget-groups"]["shared"]["tools"][0],
        "add-pull-request-reviewers"
    );
    assert_eq!(
        fm["safe-outputs"]["budget-groups"]["shared"]["tools"][1],
        "add-pull-request-labels"
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("pull_request_tool_names"));
    assert!(String::from_utf8_lossy(&output.stderr).contains("deprecated-tool-reference"));
    assert!(run_compile(&source).status.success());
    assert_eq!(fs::read_to_string(&source).unwrap(), rewritten);
}

#[test]
fn abbreviated_and_full_pr_keys_conflict_without_rewriting() {
    let dir = fresh_git_temp_dir();
    let original = "---\nname: conflict\ndescription: d\nsafe-outputs:\n  add-pr-comment: {max: 1}\n  add-pull-request-comment: {max: 3}\n---\nbody\n";
    let source = write_source(dir.path(), original);
    let output = run_compile(&source);
    assert!(!output.status.success());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains("manual migration required"));
    assert!(error.contains("add-pr-comment"));
    assert!(error.contains("add-pull-request-comment"));
    assert_eq!(fs::read_to_string(&source).unwrap(), original);
    assert!(!source.with_extension("lock.yml").exists());
}

#[test]
fn bare_update_pr_migrates_without_voting_and_stays_stable() {
    for spelling in ["", "null", "true"] {
        let dir = fresh_git_temp_dir();
        let original = format!("---\nname: bare-pr\ndescription: d\nsafe-outputs:\n  update-pr: {spelling}\n---\nbody\n");
        let source = write_source(dir.path(), &original);
        let output = run_compile(&source);
        assert!(output.status.success(), "{spelling}: {}", String::from_utf8_lossy(&output.stderr));
        let after = fs::read_to_string(&source).unwrap();
        let fm: serde_yaml::Value = serde_yaml::from_str(after.split("---").nth(1).unwrap()).unwrap();
        assert!(fm["safe-outputs"]["submit-pull-request-review"].is_null());
        assert_eq!(fm["safe-outputs"]["budget-groups"]["update-pr"]["max"], 1);
        assert_eq!(fm["safe-outputs"]["budget-groups"]["update-pr"]["tools"].as_sequence().unwrap().len(), 4);
        assert!(run_compile(&source).status.success());
        assert_eq!(fs::read_to_string(&source).unwrap(), after);
    }
}

#[test]
fn invalid_legacy_votes_fail_before_rewrite_but_empty_object_is_not_bare() {
    for config in [
        "{allowed-operations: [vote], allowed-votes: [comment]}",
        "{allowed-operations: [vote], allowed-votes: [request-changes]}",
        "{allowed-operations: [vote], allowed-votes: [unknown]}",
        "{allowed-operations: [vote], allowed-votes: comment}",
        "{}",
        "{allowed-operations: [vote]}",
    ] {
        let dir = fresh_git_temp_dir();
        let original = format!("---\nname: invalid-vote\ndescription: d\nsafe-outputs:\n  update-pr: {config}\n---\nbody\n");
        let source = write_source(dir.path(), &original);
        let output = run_compile(&source);
        assert!(!output.status.success(), "accepted {config}");
        assert!(String::from_utf8_lossy(&output.stderr).contains("allowed-votes"));
        assert_eq!(fs::read_to_string(&source).unwrap(), original);
        assert!(!source.with_extension("lock.yml").exists());
    }

}

#[test]
fn execute_source_uses_imported_migrated_policy_without_rewriting_files() {
    let dir = fresh_git_temp_dir();
    let component = "---\nsafe-outputs:\n  add-pr-labels:\n    max: 0\n---\nImported instructions.\n";
    let component_path = dir.path().join("shared.md");
    fs::write(&component_path, component).unwrap();
    let original = "---\nname: imported-execution\ndescription: d\nimports: [./shared.md]\n---\nRoot instructions.\n";
    let source = write_source(dir.path(), original);
    fs::write(
        dir.path().join("safe_outputs.ndjson"),
        "{\"name\":\"add-pull-request-labels\",\"pull_request_id\":42,\"labels\":[\"test\"]}\n",
    ).unwrap();
    let output = Command::new(ado_aw_binary())
        .arg("execute")
        .arg("--source").arg(&source)
        .arg("--safe-output-dir").arg(dir.path())
        .arg("--log-output-dir").arg(dir.path().join("logs"))
        .arg("--dry-run")
        .output().unwrap();
    assert_eq!(output.status.code(), Some(1), "{}", String::from_utf8_lossy(&output.stderr));
    let records = fs::read_to_string(dir.path().join("safe-outputs-executed.ndjson")).unwrap();
    let record: serde_json::Value = serde_json::from_str(records.trim()).unwrap();
    assert_eq!(record["status"], "budget_exhausted");
    assert!(record["error"].as_str().unwrap().contains("(0)"));
    assert_eq!(fs::read_to_string(source).unwrap(), original);
    assert_eq!(fs::read_to_string(component_path).unwrap(), component);
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

#[test]
fn compile_migrates_debug_create_issue_to_public_safe_output() {
    let dir = fresh_temp_dir();
    let original = "---\nname: issue-migration\ndescription: d\nado-aw-debug:\n  skip-integrity: true\n  create-issue:\n    target-repo: octo/repo\n---\nbody\n";
    let source = write_source(dir.path(), original);
    let output = run_compile(&source);
    assert!(
        output.status.success(),
        "compile should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let after = fs::read_to_string(&source).expect("re-read source");
    assert!(after.contains("ado-aw-debug:\n  skip-integrity: true"));
    assert!(after.contains("safe-outputs:"));
    assert!(after.contains("github-token: $(ADO_AW_DEBUG_GITHUB_TOKEN)"));
    assert!(after.contains("create-github-issue:"));
    assert_eq!(after.matches("create-issue:").count(), 0);
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("promote_debug_create_github_issue")
    );
}

#[test]
fn compile_migrates_empty_mcp_env_to_explicit_pipeline_variable() {
    let dir = fresh_temp_dir();
    let original = "---\nname: mcp-env-migration\ndescription: d\nmcp-servers:\n  custom:\n    container: node:20-slim\n    env:\n      TOKEN: \"\"\n      STATIC: value\n---\nbody\n";
    let source = write_source(dir.path(), original);
    let output = run_compile(&source);
    assert!(
        output.status.success(),
        "compile should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let after = fs::read_to_string(&source).expect("re-read source");
    assert!(after.contains("TOKEN:"));
    assert!(after.contains("pipeline-variable: TOKEN"));
    assert!(after.contains("STATIC: value"));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("explicit_mcp_pipeline_env")
    );

    let lock = source.with_extension("lock.yml");
    let compiled = fs::read_to_string(lock).expect("read lock");
    assert!(compiled.contains("TOKEN: $(TOKEN)"));
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
