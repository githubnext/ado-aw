//! Integration tests for `ado-aw disable`.
//!
//! These tests exercise the compiled binary at the CLI surface level —
//! `--help` output and clap-level validation — without driving real
//! HTTP traffic. The pure decision logic (`decide_action`, the full
//! enabled/disabled/paused transition matrix) is covered by
//! `#[cfg(test)] mod tests` inside `src/disable.rs`.

use std::path::PathBuf;

fn binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_ado-aw"))
}

#[test]
fn disable_help_describes_command() {
    let output = std::process::Command::new(binary())
        .args(["disable", "--help"])
        .output()
        .expect("Failed to run ado-aw disable --help");
    assert!(output.status.success(), "--help should exit 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Disable (or pause) every ADO build definition"),
        "Help text should describe the disable command, got:\n{stdout}"
    );
    for flag in ["--org", "--project", "--pat", "--paused", "--dry-run"] {
        assert!(
            stdout.contains(flag),
            "Expected --help to advertise {flag}, got:\n{stdout}"
        );
    }
}
