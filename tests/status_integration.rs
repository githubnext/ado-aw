//! Integration tests for `ado-aw status`.

use std::path::PathBuf;

fn binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_ado-aw"))
}

#[test]
fn status_help_describes_command() {
    let output = std::process::Command::new(binary())
        .args(["status", "--help"])
        .output()
        .expect("Failed to run ado-aw status --help");
    assert!(output.status.success(), "--help should exit 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Per-pipeline status"),
        "Help text should describe the status command, got:\n{stdout}"
    );
    for flag in ["--org", "--project", "--pat", "--json"] {
        assert!(
            stdout.contains(flag),
            "Expected --help to advertise {flag}, got:\n{stdout}"
        );
    }
}

#[test]
fn status_is_in_top_level_help() {
    let output = std::process::Command::new(binary())
        .arg("--help")
        .output()
        .expect("Failed to run ado-aw --help");
    assert!(output.status.success(), "--help should exit 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("status"),
        "Top-level --help should mention the status subcommand, got:\n{stdout}"
    );
}
