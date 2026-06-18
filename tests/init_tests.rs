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

    assert!(
        output.status.success(),
        "init should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

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

/// Test that `init` always overwrites an existing agent file (no --force needed)
#[test]
fn test_init_overwrites_by_default() {
    let temp_dir = tempfile::tempdir().expect("Failed to create temp directory");

    // First run should succeed
    let output = ado_aw_bin()
        .args(["init", "--path", temp_dir.path().to_str().unwrap()])
        .output()
        .expect("Failed to run ado-aw init");
    assert!(output.status.success(), "First init should succeed");

    let agent_path = temp_dir.path().join(".github/agents/ado-aw.agent.md");

    // Tamper with the file
    fs::write(&agent_path, "tampered content").expect("Should write tampered content");

    // Second run without --force should still succeed and restore the template
    let output = ado_aw_bin()
        .args(["init", "--path", temp_dir.path().to_str().unwrap()])
        .output()
        .expect("Failed to run ado-aw init");
    assert!(
        output.status.success(),
        "Second init should succeed and overwrite: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let content = fs::read_to_string(&agent_path).expect("Should read agent file");
    assert!(
        content.contains("ADO Agentic Pipelines Agent"),
        "Default init should restore the template content"
    );
    assert!(
        !content.contains("tampered"),
        "Tampered content should be overwritten"
    );
}

/// Test that `init --agency` is additive: it produces the standard agent file
/// AND the Agency / Claude Code plugin under the `.github/ado-aw` directory.
#[test]
fn test_init_agency_generates_plugin() {
    let temp_dir = tempfile::tempdir().expect("Failed to create temp directory");

    let output = ado_aw_bin()
        .args([
            "init",
            "--agency",
            "--path",
            temp_dir.path().to_str().unwrap(),
        ])
        .output()
        .expect("Failed to run ado-aw init --agency");

    assert!(
        output.status.success(),
        "init --agency should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Standard agent file is still produced (additive behavior).
    let agent_path = temp_dir.path().join(".github/agents/ado-aw.agent.md");
    assert!(
        agent_path.exists(),
        "Standard agent file should still be created with --agency"
    );

    // Claude Code plugin manifest + marketplace under `.github/ado-aw`.
    let plugin_root = temp_dir.path().join(".github/ado-aw");
    let plugin_json = plugin_root.join(".claude-plugin/plugin.json");
    let marketplace_json = plugin_root.join(".claude-plugin/marketplace.json");
    assert!(plugin_json.exists(), "plugin.json should be created");
    assert!(
        marketplace_json.exists(),
        "marketplace.json should be created"
    );

    // Subagent + slash commands.
    assert!(
        plugin_root.join("agents/ado-aw.md").exists(),
        "Agency subagent should be created"
    );
    for cmd in [
        "create-ado-agentic-workflow.md",
        "update-ado-agentic-workflow.md",
        "debug-ado-agentic-workflow.md",
    ] {
        assert!(
            plugin_root.join("commands").join(cmd).exists(),
            "Command {cmd} should be created"
        );
    }

    // Version placeholder must be substituted in the manifest.
    let manifest =
        fs::read_to_string(&plugin_json).expect("Should be able to read plugin.json");
    assert!(
        !manifest.contains("{{ compiler_version }}"),
        "Version placeholder should be replaced in plugin.json"
    );
    // plugin.json must be valid JSON with the expected plugin name.
    let parsed: serde_json::Value =
        serde_json::from_str(&manifest).expect("plugin.json should be valid JSON");
    assert_eq!(
        parsed["name"], "ado-aw",
        "plugin.json name should be ado-aw"
    );
}

/// Test that `init` WITHOUT `--agency` does not create the plugin directory.
#[test]
fn test_init_without_agency_skips_plugin() {
    let temp_dir = tempfile::tempdir().expect("Failed to create temp directory");

    let output = ado_aw_bin()
        .args(["init", "--path", temp_dir.path().to_str().unwrap()])
        .output()
        .expect("Failed to run ado-aw init");
    assert!(output.status.success(), "init should succeed");

    assert!(
        !temp_dir.path().join(".github/ado-aw").exists(),
        "Plugin directory should not be created without --agency"
    );
}
/// actual purpose: bypassing the GitHub-remote guard so maintainers can run
/// `ado-aw init` inside a GitHub-hosted fork of `ado-aw` itself.
///
/// NOTE: `--force` has nothing to do with overwriting (init always overwrites).
/// It skips `ensure_non_github_remote_for_ado_aw`. We cannot trigger that
/// guard from within a `cargo test` run because `CARGO_BIN_EXE_ado-aw` being
/// set already bypasses it, so the meaningful check is the CLI surface test.
#[test]
fn test_init_force_flag_is_advertised_in_help() {
    let output = ado_aw_bin()
        .args(["init", "--help"])
        .output()
        .expect("Failed to run ado-aw init --help");
    assert!(output.status.success(), "init --help should exit 0");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("--force"),
        "init --help should document the --force flag, got:\n{stdout}"
    );
    // The help text must explain the flag's purpose (GitHub-remote guard bypass),
    // not merely say it exists.
    assert!(
        stdout.contains("GitHub") || stdout.contains("bypass"),
        "init --help should explain that --force bypasses the GitHub-remote guard, got:\n{stdout}"
    );
}
