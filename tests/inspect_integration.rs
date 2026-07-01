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
    assert!(
        stdout.contains("Target shape:"),
        "expected 'Target shape:' line in inspect output, got:\n{stdout}"
    );
    assert!(
        stdout.contains("Jobs ("),
        "expected jobs section, got:\n{stdout}"
    );
    assert!(
        stdout.contains("Graph:"),
        "expected 'Graph:' section in inspect output, got:\n{stdout}"
    );
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
    assert!(
        stdout.contains("\"shape\":"),
        "expected 'shape' key in JSON output, got:\n{stdout}"
    );
    assert!(
        stdout.contains("\"graph\":"),
        "expected 'graph' key in JSON output, got:\n{stdout}"
    );
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
    assert!(
        out.status.success(),
        "graph dump --format dot exited non-zero. stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.starts_with("digraph ado_aw_pipeline {"));
    // The canonical 3-job pipeline produces three dependency edges:
    // Detection depends on Agent, SafeOutputs depends on both Agent and Detection.
    assert!(
        stdout.contains("\"Detection\" -> \"Agent\""),
        "expected Detection→Agent edge, got:\n{stdout}"
    );
    assert!(
        stdout.contains("\"SafeOutputs\" -> \"Agent\""),
        "expected SafeOutputs→Agent edge, got:\n{stdout}"
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
    // Clap value-enum validation emits "invalid value 'yaml' for
    // '--format <FORMAT>': ... [possible values: text, json, dot]".
    assert!(
        stderr.contains("invalid value 'yaml'") && stderr.contains("--format"),
        "expected clap value-enum rejection for --format, got:\n{stderr}"
    );
}

/// `ado-aw lint` surfaces invalid authored task inputs as a `task-input-invalid`
/// **warning** finding (not an error), so the agent self-optimization loop can
/// read structured feedback on the steps it synthesised. Warnings do not fail
/// the command (exit 0).
#[test]
fn lint_reports_invalid_task_input_as_warning_finding() {
    let workspace = tempfile::tempdir().expect("create temp dir");
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("invalid-task-input-agent.md");
    let dst = workspace.path().join("invalid-task-input-agent.md");
    std::fs::copy(&src, &dst).expect("copy fixture into temp dir");

    let out = Command::new(binary_path())
        .arg("lint")
        .arg(&dst)
        .arg("--json")
        .output()
        .expect("run ado-aw lint --json");

    // Warning-level findings must not fail the command.
    assert!(
        out.status.success(),
        "lint must exit 0 for warning-only findings. stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("\"code\": \"task-input-invalid\""),
        "expected a task-input-invalid finding, got:\n{stdout}"
    );
    assert!(
        stdout.contains("CopyFiles@2"),
        "the finding should name the offending task, got:\n{stdout}"
    );
    assert!(
        stdout.contains("\"severity\": \"warning\""),
        "task-input-invalid must be a warning, got:\n{stdout}"
    );
}
