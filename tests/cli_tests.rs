use std::path::PathBuf;

#[test]
fn test_run_subcommand_not_present() {
    let binary_path = PathBuf::from(env!("CARGO_BIN_EXE_ado-aw"));
    let output = std::process::Command::new(&binary_path)
        .arg("--help")
        .output()
        .expect("Failed to run ado-aw --help");

    assert!(output.status.success(), "--help should succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("Run agent locally"),
        "Help output should not include a run subcommand, got:\n{stdout}"
    );
}
