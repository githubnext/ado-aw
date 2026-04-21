use std::path::PathBuf;

#[cfg(debug_assertions)]
#[test]
fn test_run_subcommand_exposed_in_debug_builds() {
    let binary_path = PathBuf::from(env!("CARGO_BIN_EXE_ado-aw"));
    let output = std::process::Command::new(&binary_path)
        .arg("--help")
        .output()
        .expect("Failed to run ado-aw --help");

    assert!(output.status.success(), "--help should succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Run agent locally"),
        "Debug build help output should include the run subcommand, got:\n{stdout}"
    );
}

#[cfg(not(debug_assertions))]
#[test]
fn test_run_subcommand_not_exposed_in_release_builds() {
    let binary_path = PathBuf::from(env!("CARGO_BIN_EXE_ado-aw"));
    let output = std::process::Command::new(&binary_path)
        .arg("--help")
        .output()
        .expect("Failed to run ado-aw --help");

    assert!(output.status.success(), "--help should succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("Run agent locally"),
        "Release build help output should not include the run subcommand, got:\n{stdout}"
    );
}
