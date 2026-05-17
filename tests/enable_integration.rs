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
