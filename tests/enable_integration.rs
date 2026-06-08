//! Integration tests for `ado-aw enable`.
//!
//! These tests run the compiled binary in `--dry-run` mode against a
//! fake org/project so no real HTTP traffic is generated. We assert
//! that:
//!
//! - The help text advertises the documented surface.
//! - `--token` without `--also-set-token` is a clap-level error.
//!
//! The decision logic (`decide_action`, `sanitize_ado_display_name`,
//! `build_create_body`) is covered by `#[cfg(test)] mod tests` inside
//! `src/enable.rs`, since wire-stubbing the full ADO REST surface from
//! an integration test would add more infrastructure than it pays off
//! for in Phase 1.

use std::path::PathBuf;

fn binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_ado-aw"))
}

#[test]
fn enable_help_describes_command() {
    let output = std::process::Command::new(binary())
        .args(["enable", "--help"])
        .output()
        .expect("Failed to run ado-aw enable --help");
    assert!(output.status.success(), "--help should exit 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Register an ADO build definition"),
        "Help text should describe the enable command, got:\n{stdout}"
    );
    for flag in [
        "--org",
        "--project",
        "--pat",
        "--folder",
        "--default-branch",
        "--dry-run",
        "--also-set-token",
        "--token",
        "--service-connection",
        "--repository-name",
    ] {
        assert!(
            stdout.contains(flag),
            "Expected --help to advertise {flag}, got:\n{stdout}"
        );
    }
}

#[test]
fn enable_rejects_token_without_also_set_token() {
    // clap should reject this at parse time via `requires = "also_set_token"`.
    let output = std::process::Command::new(binary())
        .args(["enable", "--token", "secret", "--dry-run"])
        .output()
        .expect("Failed to run ado-aw enable");
    assert!(
        !output.status.success(),
        "Expected non-zero exit when --token used without --also-set-token"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--also-set-token") || stderr.contains("also_set_token"),
        "stderr should reference the requires-constraint, got:\n{stderr}"
    );
}

#[test]
fn enable_help_describes_github_source_support() {
    // The new --service-connection flag should have a help string
    // that mentions GitHub so operators discover the feature.
    let output = std::process::Command::new(binary())
        .args(["enable", "--help"])
        .output()
        .expect("Failed to run ado-aw enable --help");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("GitHub"),
        "Help text should mention GitHub in the --service-connection / --repository-name docs, got:\n{stdout}"
    );
}

#[test]
fn enable_dry_run_against_subdirectory_uses_repo_root_relative_yaml_path() {
    // Regression: previously `enable PATH` joined `pipeline.source`
    // against the scan root rather than the repo root, producing
    // doubled paths like
    //   C:\repo\tests\safe-outputs\tests\safe-outputs\noop.md
    // for every fixture, and posted a yamlFilename of
    // `/noop.lock.yml` (relative to scan root) instead of the
    // real repo-relative `/tests/safe-outputs/noop.lock.yml`.
    //
    // This test exercises the subdirectory PATH form via --dry-run
    // (so no network calls are made) and asserts:
    //   1) at least one fixture was found,
    //   2) the printed yamlFilename starts with `/tests/safe-outputs/`,
    //   3) no "Failed to read source" errors appear.
    let output = std::process::Command::new(binary())
        .args([
            "enable",
            "--service-connection",
            "00000000-0000-0000-0000-000000000000",
            "--project",
            "AgentPlayground",
            "--org",
            "msazuresphere",
            "--dry-run",
            "tests/safe-outputs",
        ])
        .output()
        .expect("Failed to run ado-aw enable");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "expected --dry-run exit 0; stdout=\n{stdout}\nstderr=\n{stderr}"
    );
    assert!(
        stdout.contains("Found ") && stdout.contains(" agentic pipeline(s)."),
        "expected pipeline-discovery line, got:\n{stdout}"
    );
    assert!(
        stdout.contains("\"yamlFilename\": \"/tests/safe-outputs/"),
        "yamlFilename must be repo-root-relative, got:\n{stdout}"
    );
    assert!(
        !stdout.contains("Failed to read source"),
        "no fixture should fail to read; got:\n{stdout}"
    );
}
