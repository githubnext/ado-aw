//! End-to-end tests for the `inspect` and `graph` subcommands.
//!
//! These verify the full path: agent `.md` → `compile::build_pipeline_ir`
//! → `PipelineSummary::from_pipeline` → CLI rendering. The fixtures
//! are copied into a temp dir to avoid the lost-update guard racing
//! parallel tests, matching the convention used in
//! `tests/bash_lint_tests.rs`.

use std::path::PathBuf;
use std::process::Command;

fn binary_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_ado-aw"))
}

fn fixture_copy(fixture_name: &str) -> (tempfile::TempDir, PathBuf) {
    let workspace = tempfile::tempdir().expect("create temp dir");
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("safe-outputs")
        .join(fixture_name);
    let dst = workspace.path().join(fixture_name);
    std::fs::copy(&src, &dst)
        .unwrap_or_else(|e| panic!("copy {} into temp dir: {e}", src.display()));
    (workspace, dst)
}

#[test]
fn inspect_emits_pipeline_summary_text() {
    let (_workspace, src) = fixture_copy("create-pull-request.md");
    let out = Command::new(binary_path())
        .arg("inspect")
        .arg(&src)
        .output()
        .expect("run ado-aw inspect");
    assert!(
        out.status.success(),
        "inspect exited non-zero. stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("Target shape:"));
    assert!(
        stdout.contains("Jobs ("),
        "expected jobs section, got:\n{stdout}"
    );
    assert!(stdout.contains("Graph:"));
}

#[test]
fn inspect_json_emits_schema_version_one() {
    let (_workspace, src) = fixture_copy("create-pull-request.md");
    let out = Command::new(binary_path())
        .arg("inspect")
        .arg(&src)
        .arg("--json")
        .output()
        .expect("run ado-aw inspect --json");
    assert!(
        out.status.success(),
        "inspect --json exited non-zero. stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    // schema_version is the public stability contract.
    assert!(
        stdout.contains("\"schema_version\": 1"),
        "expected schema_version: 1 in JSON output, got:\n{stdout}"
    );
    assert!(stdout.contains("\"shape\":"));
    assert!(stdout.contains("\"graph\":"));
}

#[test]
fn graph_dot_emits_digraph_with_known_edges() {
    let (_workspace, src) = fixture_copy("create-pull-request.md");
    let out = Command::new(binary_path())
        .arg("graph")
        .arg("dump")
        .arg(&src)
        .arg("--format")
        .arg("dot")
        .output()
        .expect("run ado-aw graph dump --format dot");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.starts_with("digraph ado_aw_pipeline {"));
    // Canonical 3-job graph has Detection→Agent and SafeOutputs→Detection.
    assert!(
        stdout.contains("\"Detection\" -> \"Agent\""),
        "expected Detection→Agent edge, got:\n{stdout}"
    );
    assert!(
        stdout.contains("\"SafeOutputs\" -> \"Detection\""),
        "expected SafeOutputs→Detection edge, got:\n{stdout}"
    );
}

#[test]
fn graph_rejects_unknown_format() {
    let (_workspace, src) = fixture_copy("create-pull-request.md");
    let out = Command::new(binary_path())
        .arg("graph")
        .arg("dump")
        .arg(&src)
        .arg("--format")
        .arg("yaml")
        .output()
        .expect("run ado-aw graph dump --format yaml");
    assert!(!out.status.success(), "unknown format should fail");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("unknown --format"),
        "expected unknown-format error, got:\n{stderr}"
    );
}
