//! Integration tests for `ado-aw list`.

use std::path::PathBuf;

fn binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_ado-aw"))
}

#[test]
fn list_help_describes_command() {
    let output = std::process::Command::new(binary())
        .args(["list", "--help"])
        .output()
        .expect("Failed to run ado-aw list --help");
    assert!(output.status.success(), "--help should exit 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("List ADO build definitions"),
        "Help text should describe the list command, got:\n{stdout}"
    );
    for flag in ["--org", "--project", "--pat", "--all", "--json"] {
        assert!(
            stdout.contains(flag),
            "Expected --help to advertise {flag}, got:\n{stdout}"
        );
    }
}
