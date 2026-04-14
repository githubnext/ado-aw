use std::fs;
use std::process::Command;

fn ado_aw_bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_ado-aw"))
}

/// Test that `init` creates the agent file in the expected location
#[test]
fn test_init_creates_agent_file() {
    let temp_dir = tempfile::tempdir().expect("Failed to create temp directory");

    let output = ado_aw_bin()
        .args(["init", "--path", temp_dir.path().to_str().unwrap()])
        .output()
        .expect("Failed to run ado-aw init");

    assert!(output.status.success(), "init should succeed: {}", String::from_utf8_lossy(&output.stderr));

    let agent_path = temp_dir.path().join(".github/agents/ado-aw.agent.md");
    assert!(agent_path.exists(), "Agent file should be created");

    let content = fs::read_to_string(&agent_path).expect("Should be able to read agent file");
    assert!(
        content.contains("ADO Agentic Pipelines Agent"),
        "Agent file should contain the expected title"
    );
    // Verify version placeholder was substituted
    assert!(
        !content.contains("{{ compiler_version }}"),
        "Version placeholder should be replaced with actual version"
    );
}

/// Test that `init` refuses to overwrite without --force
#[test]
fn test_init_refuses_overwrite_without_force() {
    let temp_dir = tempfile::tempdir().expect("Failed to create temp directory");

    // First run should succeed
    let output = ado_aw_bin()
        .args(["init", "--path", temp_dir.path().to_str().unwrap()])
        .output()
        .expect("Failed to run ado-aw init");
    assert!(output.status.success(), "First init should succeed");

    // Second run without --force should fail
    let output = ado_aw_bin()
        .args(["init", "--path", temp_dir.path().to_str().unwrap()])
        .output()
        .expect("Failed to run ado-aw init");
    assert!(!output.status.success(), "Second init without --force should fail");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("already exists"),
        "Error should mention file already exists: {stderr}"
    );
}

/// Test that `init --force` overwrites an existing agent file
#[test]
fn test_init_force_overwrites() {
    let temp_dir = tempfile::tempdir().expect("Failed to create temp directory");

    // First run
    let output = ado_aw_bin()
        .args(["init", "--path", temp_dir.path().to_str().unwrap()])
        .output()
        .expect("Failed to run ado-aw init");
    assert!(output.status.success(), "First init should succeed");

    let agent_path = temp_dir.path().join(".github/agents/ado-aw.agent.md");

    // Tamper with the file
    fs::write(&agent_path, "tampered content").expect("Should write tampered content");

    // Re-run with --force should succeed and restore the template
    let output = ado_aw_bin()
        .args(["init", "--path", temp_dir.path().to_str().unwrap(), "--force"])
        .output()
        .expect("Failed to run ado-aw init --force");
    assert!(output.status.success(), "Init with --force should succeed");

    let content = fs::read_to_string(&agent_path).expect("Should read agent file");
    assert!(
        content.contains("ADO Agentic Pipelines Agent"),
        "Force should restore the template content"
    );
    assert!(
        !content.contains("tampered"),
        "Tampered content should be overwritten"
    );
}
