//! Integration tests for `ado-aw remove`.

use std::path::PathBuf;

fn binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_ado-aw"))
}

#[test]
fn remove_help_describes_command() {
    let output = std::process::Command::new(binary())
        .args(["remove", "--help"])
        .output()
        .expect("Failed to run ado-aw remove --help");
    assert!(output.status.success(), "--help should exit 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Delete every ADO build definition"),
        "Help text should describe the remove command, got:\n{stdout}"
    );
    for flag in ["--org", "--project", "--pat", "--yes", "--dry-run"] {
        assert!(
            stdout.contains(flag),
            "Expected --help to advertise {flag}, got:\n{stdout}"
        );
    }
}

#[test]
fn remove_is_listed_in_top_level_help() {
    let output = std::process::Command::new(binary())
        .arg("--help")
        .output()
        .expect("Failed to run ado-aw --help");
    assert!(output.status.success(), "--help should exit 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("remove"),
        "Top-level --help should mention the remove subcommand, got:\n{stdout}"
    );
}
