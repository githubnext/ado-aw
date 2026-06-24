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

/// Recursively collect file paths under `root`, returned relative to `root`
/// (with forward-slash separators for stable comparison across platforms).
fn collect_files_rel(root: &std::path::Path) -> Vec<String> {
    fn walk(dir: &std::path::Path, base: &std::path::Path, out: &mut Vec<String>) {
        let entries = fs::read_dir(dir)
            .unwrap_or_else(|e| panic!("read_dir {} failed: {e}", dir.display()));
        for entry in entries {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                walk(&path, base, out);
            } else {
                let rel = path
                    .strip_prefix(base)
                    .expect("strip_prefix")
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push(rel);
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out.sort();
    out
}

/// Test that `init --agency` scaffolds a byte-for-byte copy of the canonical
/// in-repo plugin (`agency/plugins/ado-aw/`). This guards the single-source-of-
/// truth invariant: the embedded files and the checked-in files must not drift.
///
/// It walks the canonical directory rather than a hardcoded list, so a NEW file
/// added to `agency/plugins/ado-aw/` that nobody wired into `init.rs`'s embed
/// list is caught here (it would exist canonically but never be scaffolded).
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

    // Every canonical plugin file must be scaffolded byte-for-byte. Walking the
    // directory (not a fixed list) is what makes a newly-added-but-unwired file
    // fail loudly instead of silently shipping un-scaffolded.
    let canonical_files = collect_files_rel(&canonical);
    assert!(
        !canonical_files.is_empty(),
        "canonical plugin dir should contain files"
    );
    for rel in &canonical_files {
        let want = fs::read_to_string(canonical.join(rel))
            .unwrap_or_else(|e| panic!("canonical {rel} should be readable: {e}"));
        let got = fs::read_to_string(scaffolded.join(rel)).unwrap_or_else(|e| {
            panic!("canonical file {rel} was NOT scaffolded by `init --agency` (add it to AGENCY_PLUGIN_FILES in src/init.rs): {e}")
        });
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

/// Guard the lock-step versioning invariant: the plugin manifest and both root
/// marketplace catalogs must carry the same version as the compiler crate.
///
/// release-please bumps all of these together via `extra-files`, but if a
/// release lands while a plugin-touching change is in flight (which has happened),
/// the literals can desync. This asserts that never reaches `main`.
#[test]
fn test_plugin_version_matches_crate_version() {
    let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let crate_version = env!("CARGO_PKG_VERSION");

    let read_json = |rel: &str| -> serde_json::Value {
        let s = fs::read_to_string(repo_root.join(rel))
            .unwrap_or_else(|e| panic!("{rel} should be readable: {e}"));
        serde_json::from_str(&s).unwrap_or_else(|e| panic!("{rel} should be valid JSON: {e}"))
    };

    let plugin = read_json("agency/plugins/ado-aw/.claude-plugin/plugin.json");
    assert_eq!(
        plugin["version"], crate_version,
        "plugin.json version must match Cargo.toml ({crate_version}); release-please \
         bumps both — resync if a release landed mid-change"
    );

    for catalog in [
        ".claude-plugin/marketplace.json",
        ".github/plugin/marketplace.json",
    ] {
        let cat = read_json(catalog);
        assert_eq!(
            cat["metadata"]["version"], crate_version,
            "{catalog} metadata.version must match Cargo.toml ({crate_version})"
        );
        assert_eq!(
            cat["plugins"][0]["version"], crate_version,
            "{catalog} plugins[0].version must match Cargo.toml ({crate_version})"
        );
    }
}

/// Guard that the committed `.github/agents/ado-aw.agent.md` stays in sync with
/// its template `src/data/init-agent.md`: running `init` must reproduce the
/// committed file byte-for-byte. This is what keeps the version-pinned URLs (and
/// their release-please markers) in the committed file correct, since that file
/// — not the placeholder-bearing template — is what release-please updates.
#[test]
fn test_committed_agent_file_matches_template_output() {
    let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let committed = fs::read_to_string(repo_root.join(".github/agents/ado-aw.agent.md"))
        .expect("committed agent file should be readable");

    let temp_dir = tempfile::tempdir().expect("Failed to create temp directory");
    let output = ado_aw_bin()
        .args(["init", "--path", temp_dir.path().to_str().unwrap()])
        .output()
        .expect("Failed to run ado-aw init");
    assert!(output.status.success(), "init should succeed");

    let generated = fs::read_to_string(temp_dir.path().join(".github/agents/ado-aw.agent.md"))
        .expect("generated agent file should be readable");

    assert_eq!(
        generated.replace("\r\n", "\n"),
        committed.replace("\r\n", "\n"),
        "committed .github/agents/ado-aw.agent.md is stale — regenerate it with \
         `ado-aw init --force` after editing src/data/init-agent.md"
    );
}

/// `init --agency` must NOT clobber a consumer's pre-existing, differing root
/// marketplace catalog. It should leave the existing file untouched and still
/// scaffold the plugin tree.
#[test]
fn test_init_agency_does_not_clobber_existing_catalog() {
    let temp_dir = tempfile::tempdir().expect("Failed to create temp directory");

    // Simulate a consumer that already maintains a Claude marketplace catalog.
    let catalog_dir = temp_dir.path().join(".claude-plugin");
    fs::create_dir_all(&catalog_dir).expect("create .claude-plugin");
    let catalog = catalog_dir.join("marketplace.json");
    let sentinel = "{\n  \"name\": \"consumer-owned\",\n  \"plugins\": []\n}\n";
    fs::write(&catalog, sentinel).expect("write pre-existing catalog");

    let output = ado_aw_bin()
        .args([
            "init",
            "--agency",
            "--path",
            temp_dir.path().to_str().unwrap(),
        ])
        .output()
        .expect("Failed to run ado-aw init --agency");
    assert!(output.status.success(), "init --agency should still succeed");

    // The consumer's catalog must be left exactly as it was.
    let after = fs::read_to_string(&catalog).expect("catalog should still be readable");
    assert_eq!(
        after, sentinel,
        "existing root catalog must not be clobbered by init --agency"
    );

    // The plugin tree is still scaffolded regardless.
    assert!(
        temp_dir
            .path()
            .join("agency/plugins/ado-aw/.claude-plugin/plugin.json")
            .exists(),
        "plugin tree should still be scaffolded even when a root catalog pre-exists"
    );

    // The other catalog (.github/plugin/) had no pre-existing file, so it IS
    // written — this is the mixed (wrote one, skipped one) case. The user-facing
    // output must reflect both: a skip warning on stderr and a summary line that
    // does NOT falsely claim both catalogs were written.
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Skipped .claude-plugin/marketplace.json"),
        "stderr should warn about the skipped pre-existing catalog, got:\n{stderr}"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("left a pre-existing one untouched"),
        "stdout summary must acknowledge the skipped catalog, not claim both were written, got:\n{stdout}"
    );
    assert!(
        temp_dir
            .path()
            .join(".github/plugin/marketplace.json")
            .exists(),
        "the non-pre-existing catalog should still be written"
    );
}
