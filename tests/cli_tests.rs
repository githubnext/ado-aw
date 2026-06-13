use std::path::PathBuf;

// The `run` subcommand previously had a description "Run agent locally" when
// it was an internal developer tool. It now queues ADO builds ("Queue a build
// for every ADO definition…"). This test guards against that old description
// being reinstated.
#[test]
fn test_run_agent_locally_description_absent() {
    let binary_path = PathBuf::from(env!("CARGO_BIN_EXE_ado-aw"));
    let output = std::process::Command::new(&binary_path)
        .arg("--help")
        .output()
        .expect("Failed to run ado-aw --help");

    assert!(output.status.success(), "--help should succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("Run agent locally"),
        "Help output should not contain the old 'Run agent locally' description; \
         the run subcommand now queues ADO builds. Got:\n{stdout}"
    );
}
