//! Integration tests for `ado-aw run`.

use std::path::PathBuf;

fn binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_ado-aw"))
}

#[test]
fn run_help_describes_command() {
    let output = std::process::Command::new(binary())
        .args(["run", "--help"])
        .output()
        .expect("Failed to run ado-aw run --help");
    assert!(output.status.success(), "--help should exit 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Queue a build"),
        "Help text should describe the run command, got:\n{stdout}"
    );
    for flag in [
        "--org",
        "--project",
        "--pat",
        "--branch",
        "--parameters",
        "--wait",
        "--poll-interval",
        "--timeout",
        "--dry-run",
    ] {
        assert!(
            stdout.contains(flag),
            "Expected --help to advertise {flag}, got:\n{stdout}"
        );
    }
    // The comma-in-value constraint is surfaced in `--help` so users
    // can self-diagnose without consulting the module doc-comment.
    assert!(
        stdout.contains("VALUES MUST NOT CONTAIN COMMAS")
            || stdout.contains("must not contain commas"),
        "Expected --help to advertise the no-commas-in-values constraint, got:\n{stdout}"
    );
}

#[test]
fn run_rejects_poll_interval_without_wait() {
    // clap should reject `--poll-interval` (and `--timeout`) when `--wait` is absent.
    let output = std::process::Command::new(binary())
        .args(["run", "--poll-interval", "5"])
        .output()
        .expect("Failed to run ado-aw run");
    assert!(
        !output.status.success(),
        "Expected non-zero exit when --poll-interval used without --wait"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--wait") || stderr.contains("wait"),
        "stderr should reference the requires-constraint, got:\n{stderr}"
    );
}
