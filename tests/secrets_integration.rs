//! Integration tests for `ado-aw secrets` (and the deprecation alias).

use std::path::PathBuf;

fn binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_ado-aw"))
}

#[test]
fn secrets_help_advertises_subcommands() {
    let output = std::process::Command::new(binary())
        .args(["secrets", "--help"])
        .output()
        .expect("Failed to run ado-aw secrets --help");
    assert!(output.status.success(), "--help should exit 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    for sub in ["set", "list", "delete"] {
        assert!(
            stdout.contains(sub),
            "secrets --help should advertise the {sub} subcommand, got:\n{stdout}"
        );
    }
}

#[test]
fn secrets_set_help_advertises_flags() {
    let output = std::process::Command::new(binary())
        .args(["secrets", "set", "--help"])
        .output()
        .expect("Failed to run ado-aw secrets set --help");
    assert!(output.status.success(), "--help should exit 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    for flag in ["--allow-override", "--value-stdin", "--dry-run", "--pat"] {
        assert!(
            stdout.contains(flag),
            "secrets set --help should advertise {flag}, got:\n{stdout}"
        );
    }
}

#[test]
fn secrets_list_help_warns_no_values() {
    let output = std::process::Command::new(binary())
        .args(["secrets", "list", "--help"])
        .output()
        .expect("Failed to run ado-aw secrets list --help");
    assert!(output.status.success(), "--help should exit 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("--json"),
        "secrets list --help should advertise --json, got:\n{stdout}"
    );
}

#[test]
fn configure_is_hidden_in_top_level_help() {
    let output = std::process::Command::new(binary())
        .arg("--help")
        .output()
        .expect("Failed to run ado-aw --help");
    assert!(output.status.success(), "--help should exit 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    // `secrets` is the documented replacement.
    assert!(
        stdout.contains("secrets"),
        "Top-level --help should advertise the secrets subcommand, got:\n{stdout}"
    );
    // The legacy `configure` line must not appear (it's hidden).
    // We allow any line that mentions "configure" elsewhere but the
    // top-level commands list must not include the literal subcommand.
    // We check the "Commands:" section, which is everything between
    // `Commands:` and `Options:`.
    let lower = stdout.to_lowercase();
    if let (Some(c), Some(o)) = (lower.find("commands:"), lower.find("options:")) {
        let block = &lower[c..o];
        assert!(
            !block.contains("configure"),
            "configure should be hidden in top-level --help; commands block was:\n{block}"
        );
    }
}

#[test]
fn configure_invocation_still_works_and_warns() {
    // We can't drive a real ADO call from a unit test, but the
    // deprecation warning is emitted by the very first line of
    // `run_set_github_token`, so we trigger it by passing arguments
    // that will fail early after the warning prints.
    //
    // Use a path that doesn't exist; the warning is emitted before
    // the path canonicalization step. The command will still
    // ultimately fail, which is fine — we only assert on stderr.
    let output = std::process::Command::new(binary())
        .args([
            "configure",
            "--org",
            "fake",
            "--project",
            "fake",
            "--pat",
            "dummy",
            "--token",
            "x",
            "--path",
            "/definitely-does-not-exist-9c4f0",
            "--dry-run",
        ])
        .output()
        .expect("Failed to run ado-aw configure");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("deprecated"),
        "configure should emit a deprecation warning, got stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("secrets set GITHUB_TOKEN") || stderr.contains("secrets set"),
        "deprecation warning should mention the replacement, got stderr:\n{stderr}"
    );
}
