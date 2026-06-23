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
        content.contains("Azure DevOps Agentic Workflows Agent"),
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
        content.contains("Azure DevOps Agentic Workflows Agent"),
        "Default init should restore the template content"
    );
    assert!(
        !content.contains("tampered"),
        "Tampered content should be overwritten"
    );
}

/// Test that `init --agency` is additive: it produces the standard agent file
/// AND the Agency / Claude Code plugin under the `agency/plugins/ado-aw` directory.
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

    // Claude Code plugin manifest + MCP wiring under `agency/plugins/ado-aw`.
    let plugin_root = temp_dir.path().join("agency/plugins/ado-aw");
    let plugin_json = plugin_root.join(".claude-plugin/plugin.json");
    assert!(plugin_json.exists(), "plugin.json should be created");
    assert!(
        plugin_root.join(".mcp.json").exists(),
        "MCP server wiring (.mcp.json) should be created"
    );
    assert!(
        plugin_root.join("agency.json").exists(),
        "agency.json governance metadata should be created"
    );
    assert!(
        plugin_root.join("README.md").exists(),
        "Plugin README.md should be created"
    );

    // Root marketplace catalogs make the repo registerable via
    // `/plugin marketplace add <repo>` — they live at the repo root, not in the
    // plugin dir, and must list the ado-aw plugin via a relative `source`.
    for catalog in [
        ".claude-plugin/marketplace.json",
        ".github/plugin/marketplace.json",
    ] {
        let cat_path = temp_dir.path().join(catalog);
        assert!(
            cat_path.exists(),
            "Root marketplace catalog {catalog} should be created"
        );
        let cat: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&cat_path).expect("catalog readable"))
                .unwrap_or_else(|e| panic!("{catalog} should be valid JSON: {e}"));
        assert_eq!(
            cat["plugins"][0]["name"], "ado-aw",
            "{catalog} should list the ado-aw plugin"
        );
        assert_eq!(
            cat["plugins"][0]["source"], "./agency/plugins/ado-aw",
            "{catalog} plugin source should point at the plugin dir"
        );
    }

    // Dispatcher subagent.
    assert!(
        plugin_root.join("agents/ado-aw.md").exists(),
        "Agency subagent should be created"
    );

    // All six skills.
    for skill in [
        "create-workflow",
        "update-workflow",
        "debug-workflow",
        "compile-and-validate",
        "manage-lifecycle",
        "audit-build",
    ] {
        assert!(
            plugin_root
                .join("skills")
                .join(skill)
                .join("SKILL.md")
                .exists(),
            "Skill {skill}/SKILL.md should be created"
        );
    }

    // Doctor prerequisite scripts (both platforms).
    for script in ["doctor.sh", "doctor.ps1"] {
        assert!(
            plugin_root.join("scripts").join(script).exists(),
            "scripts/{script} should be created"
        );
    }

    // On Unix, the scaffolded doctor.sh must be executable so the documented
    // `./scripts/doctor.sh` invocation works (not just `bash doctor.sh`).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(plugin_root.join("scripts/doctor.sh"))
            .expect("doctor.sh metadata")
            .permissions()
            .mode();
        assert!(
            mode & 0o111 != 0,
            "scaffolded doctor.sh should have the executable bit set, got mode {mode:o}"
        );
    }

    // The manifest is a verbatim copy of the canonical plugin: it must carry a
    // concrete version (no unresolved placeholder) and be valid JSON.
    let manifest = fs::read_to_string(&plugin_json).expect("Should be able to read plugin.json");
    assert!(
        !manifest.contains("{{ compiler_version }}"),
        "Version placeholder should not appear in plugin.json"
    );
    // plugin.json must be valid JSON with the expected plugin name.
    let parsed: serde_json::Value =
        serde_json::from_str(&manifest).expect("plugin.json should be valid JSON");
    assert_eq!(parsed["name"], "ado-aw", "plugin.json name should be ado-aw");
}

/// Test that `init --agency` scaffolds a byte-for-byte copy of the canonical
/// in-repo plugin (`agency/plugins/ado-aw/`). This guards the single-source-of-
/// truth invariant: the embedded files and the checked-in files must not drift.
#[test]
fn test_init_agency_matches_canonical_source() {
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
    assert!(output.status.success(), "init --agency should succeed");

    let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let canonical = repo_root.join("agency/plugins/ado-aw");
    let scaffolded = temp_dir.path().join("agency/plugins/ado-aw");

    for rel in [
        ".claude-plugin/plugin.json",
        ".mcp.json",
        "agency.json",
        "README.md",
        "agents/ado-aw.md",
        "skills/create-workflow/SKILL.md",
        "skills/update-workflow/SKILL.md",
        "skills/debug-workflow/SKILL.md",
        "skills/compile-and-validate/SKILL.md",
        "skills/manage-lifecycle/SKILL.md",
        "skills/audit-build/SKILL.md",
        "scripts/doctor.sh",
        "scripts/doctor.ps1",
    ] {
        let want = fs::read_to_string(canonical.join(rel))
            .unwrap_or_else(|e| panic!("canonical {rel} should be readable: {e}"));
        let got = fs::read_to_string(scaffolded.join(rel))
            .unwrap_or_else(|e| panic!("scaffolded {rel} should be readable: {e}"));
        assert_eq!(
            got, want,
            "scaffolded {rel} must match the canonical agency/plugins/ado-aw source"
        );
    }

    // Root marketplace catalogs must also match the canonical repo-root copies.
    for rel in [
        ".claude-plugin/marketplace.json",
        ".github/plugin/marketplace.json",
    ] {
        let want = fs::read_to_string(repo_root.join(rel))
            .unwrap_or_else(|e| panic!("canonical {rel} should be readable: {e}"));
        let got = fs::read_to_string(temp_dir.path().join(rel))
            .unwrap_or_else(|e| panic!("scaffolded {rel} should be readable: {e}"));
        assert_eq!(got, want, "scaffolded {rel} must match the canonical repo-root catalog");
    }
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
        !temp_dir.path().join("agency/plugins/ado-aw").exists(),
        "Plugin directory should not be created without --agency"
    );
    assert!(
        !temp_dir.path().join(".claude-plugin/marketplace.json").exists(),
        "Root Claude catalog should not be created without --agency"
    );
    assert!(
        !temp_dir.path().join(".github/plugin/marketplace.json").exists(),
        "Root Copilot catalog should not be created without --agency"
    );
}
/// Test that `--force` is advertised in `init --help` and describes its
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
