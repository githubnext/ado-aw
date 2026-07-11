use std::fs;
use std::path::PathBuf;

// `assert_required_markers`, `assert_pool_config`, `assert_compiler_download`,
// `assert_awf_download`, `assert_mcpg_integration`, and `test_compiled_yaml_structure`
// validated the legacy `src/data/base.yml` template. The standalone target
// now builds its YAML programmatically via `src/compile/agentic_pipeline.rs`
// (see `feat(compile): standalone target builds Pipeline IR; delete base.yml`,
// then `refactor(compile): extract canonical agentic-pipeline shape into
// agentic_pipeline.rs`); the template is gone, so these template-shape
// assertions no longer apply. The shape tests in
// `src/compile/agentic_pipeline.rs` and the bash-lint suite take over coverage.

/// Test that the example file is valid and can be parsed
#[test]
fn test_example_file_structure() {
    let example_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join("sample-agent.md");

    assert!(example_path.exists(), "Example file should exist");

    let content = fs::read_to_string(&example_path).expect("Should be able to read example file");

    // Verify basic structure
    assert!(
        content.starts_with("---"),
        "Example should start with front matter"
    );
    assert!(content.contains("name:"), "Example should have name field");
    assert!(
        content.contains("description:"),
        "Example should have description field"
    );

    // Verify it has closing front matter
    let after_start = &content[3..];
    assert!(
        after_start.contains("\n---\n") || after_start.contains("\n---\r\n"),
        "Example should have closing front matter"
    );
}

/// Test that validates the presence of required dependencies
#[test]
fn test_project_dependencies() {
    let cargo_toml_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");

    assert!(cargo_toml_path.exists(), "Cargo.toml should exist");

    let cargo_content =
        fs::read_to_string(&cargo_toml_path).expect("Should be able to read Cargo.toml");

    // Verify required dependencies are present
    assert!(
        cargo_content.contains("clap"),
        "Should have clap dependency"
    );
    assert!(
        cargo_content.contains("anyhow"),
        "Should have anyhow dependency"
    );
    assert!(
        cargo_content.contains("serde"),
        "Should have serde dependency"
    );
    assert!(
        cargo_content.contains("serde_yaml"),
        "Should have serde_yaml dependency"
    );
}

/// Test that fixture files are valid markdown with front matter
#[test]
fn test_fixture_minimal_agent() {
    let fixture_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("minimal-agent.md");

    assert!(fixture_path.exists(), "Minimal agent fixture should exist");

    let content =
        fs::read_to_string(&fixture_path).expect("Should be able to read minimal agent fixture");

    // Verify it has proper structure
    assert!(
        content.starts_with("---"),
        "Fixture should start with front matter"
    );
    assert!(content.contains("name:"), "Fixture should have name field");
    assert!(
        content.contains("description:"),
        "Fixture should have description field"
    );
}

/// Test that complete fixture has all fields
#[test]
fn test_fixture_complete_agent() {
    let fixture_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("complete-agent.md");

    assert!(fixture_path.exists(), "Complete agent fixture should exist");

    let content =
        fs::read_to_string(&fixture_path).expect("Should be able to read complete agent fixture");

    // Verify all fields are present
    assert!(content.contains("name:"), "Should have name");
    assert!(content.contains("description:"), "Should have description");
    assert!(content.contains("schedule:"), "Should have schedule");
    assert!(content.contains("repos:"), "Should have repos");
    assert!(content.contains("mcp-servers:"), "Should have mcp-servers");

    // Verify it has MCP configuration and custom MCPs
    assert!(content.contains("container:"), "Should have custom MCP");

    // Verify permissions
    assert!(content.contains("permissions:"), "Should have permissions");
    assert!(
        content.contains("read: my-read-arm-connection"),
        "Should have read service connection"
    );
    assert!(
        content.contains("write: my-write-arm-connection"),
        "Should have write service connection"
    );
}

/// Test that compiled output has no unreplaced template markers
#[test]
fn test_compiled_output_no_unreplaced_markers() {
    let temp_dir =
        std::env::temp_dir().join(format!("agentic-pipeline-markers-{}", std::process::id()));
    fs::create_dir_all(&temp_dir).expect("Failed to create temp directory");

    let fixture_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("minimal-agent.md");

    let output_path = temp_dir.join("minimal-agent.yml");

    // Run the compiler binary
    let binary_path = PathBuf::from(env!("CARGO_BIN_EXE_ado-aw"));
    let output = std::process::Command::new(&binary_path)
        .args([
            "compile",
            fixture_path.to_str().unwrap(),
            "-o",
            output_path.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to run compiler");

    assert!(
        output.status.success(),
        "Compiler should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output_path.exists(), "Compiled YAML should exist");

    let compiled = fs::read_to_string(&output_path).expect("Should read compiled YAML");

    // Verify no unreplaced {{ markers }} remain (excluding ${{ }} which are ADO expressions)
    for line in compiled.lines() {
        let stripped = line.replace("${{", "");
        assert!(
            !stripped.contains("{{ "),
            "Compiled output should not contain unreplaced marker: {}",
            line.trim()
        );
    }

    // Verify the compiler version was correctly substituted
    let version = env!("CARGO_PKG_VERSION");
    assert!(
        compiled.contains(version),
        "Compiled output should contain compiler version {version}"
    );
    assert!(
        compiled.contains("github.com/githubnext/ado-aw/releases"),
        "Compiled output should reference GitHub Releases for the compiler"
    );

    // Verify the AWF firewall version was correctly substituted
    assert!(
        compiled.contains("github.com/github/gh-aw-firewall/releases"),
        "Compiled output should reference GitHub Releases for AWF"
    );

    // Verify MCPG references
    assert!(
        compiled.contains("ghcr.io/github/gh-aw-mcpg"),
        "Compiled output should reference MCPG Docker image"
    );
    assert!(
        compiled.contains("host.docker.internal"),
        "Compiled output should reference host.docker.internal for MCPG"
    );
    assert!(
        compiled.contains("--enable-host-access"),
        "Compiled output should include --enable-host-access for AWF"
    );

    // Verify no legacy MCP firewall references
    assert!(
        !compiled.contains("mcp-firewall"),
        "Compiled output should not reference legacy mcp-firewall"
    );
    assert!(
        !compiled.contains("mcp_firewall"),
        "Compiled output should not reference legacy mcp_firewall"
    );

    let _ = fs::remove_dir_all(&temp_dir);
}

/// Test that the pipeline-trigger-agent fixture compiles correctly
///
/// Verifies that the `triggers.pipeline` front matter field generates:
/// - `resources.pipelines` block with the correct source pipeline name
/// - `trigger: none` and `pr: none` to suppress CI/PR triggers
/// - No `schedules:` block (since only a pipeline trigger is configured)
#[test]
fn test_fixture_pipeline_trigger_agent_compiled_output() {
    let temp_dir = std::env::temp_dir().join(format!(
        "agentic-pipeline-trigger-test-{}",
        std::process::id()
    ));
    fs::create_dir_all(&temp_dir).expect("Failed to create temp directory");

    let fixture_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("pipeline-trigger-agent.md");

    assert!(
        fixture_path.exists(),
        "Pipeline trigger agent fixture should exist"
    );

    let output_path = temp_dir.join("pipeline-trigger-agent.yml");

    let binary_path = PathBuf::from(env!("CARGO_BIN_EXE_ado-aw"));
    let output = std::process::Command::new(&binary_path)
        .args([
            "compile",
            fixture_path.to_str().unwrap(),
            "-o",
            output_path.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to run compiler");

    assert!(
        output.status.success(),
        "Compiler should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output_path.exists(), "Compiled YAML should exist");

    let compiled = fs::read_to_string(&output_path).expect("Should read compiled YAML");

    // Should contain pipeline resource pointing to the upstream pipeline
    assert!(
        compiled.contains("Build Pipeline"),
        "Compiled output should contain source pipeline name 'Build Pipeline'"
    );
    assert!(
        compiled.contains("OtherProject"),
        "Compiled output should contain the project name 'OtherProject'"
    );
    assert!(
        compiled.contains("resources:"),
        "Compiled output should contain a resources block"
    );
    assert!(
        compiled.contains("pipelines:"),
        "Compiled output should contain a pipelines resource block"
    );

    // CI and PR triggers should be suppressed
    assert!(
        compiled.contains("trigger: none"),
        "Compiled output should disable CI trigger with 'trigger: none'"
    );
    assert!(
        compiled.contains("pr: none"),
        "Compiled output should disable PR trigger with 'pr: none'"
    );

    // cancel-previous-builds was removed — verify it's not present
    assert!(
        !compiled.contains("Cancel previous queued builds"),
        "Compiled output should not include cancel-previous-builds step"
    );

    // Should NOT contain a schedules block (no schedule configured)
    assert!(
        !compiled.contains("schedules:"),
        "Compiled output should not contain a schedules block"
    );

    // Verify no unreplaced markers remain
    for line in compiled.lines() {
        let stripped = line.replace("${{", "");
        assert!(
            !stripped.contains("{{ "),
            "Compiled output should not contain unreplaced marker: {}",
            line.trim()
        );
    }

    let _ = fs::remove_dir_all(&temp_dir);
}

/// Test that permissions with read+write service connections generates correct token steps
#[test]
fn test_permissions_read_write_compiled_output() {
    let temp_dir = std::env::temp_dir().join(format!(
        "agentic-pipeline-permissions-rw-{}",
        std::process::id()
    ));
    fs::create_dir_all(&temp_dir).expect("Failed to create temp directory");

    let test_input = temp_dir.join("perms-agent.md");
    let test_content = r#"---
name: "Permissions Test Agent"
description: "Agent with read and write permissions"
permissions:
  read: my-read-sc
  write: my-write-sc
safe-outputs:
  create-work-item:
    work-item-type: Task
---

## Test Agent

Do something.
"#;
    fs::write(&test_input, test_content).expect("Failed to write test input");

    let output_path = temp_dir.join("perms-agent.yml");
    let binary_path = PathBuf::from(env!("CARGO_BIN_EXE_ado-aw"));
    let output = std::process::Command::new(&binary_path)
        .args([
            "compile",
            test_input.to_str().unwrap(),
            "-o",
            output_path.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to run compiler");

    assert!(
        output.status.success(),
        "Compiler should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let compiled = fs::read_to_string(&output_path).expect("Should read compiled YAML");

    // Should contain read token acquisition (SC_READ_TOKEN)
    assert!(
        compiled.contains("SC_READ_TOKEN"),
        "Compiled output should contain SC_READ_TOKEN for read service connection"
    );
    assert!(
        compiled.contains("my-read-sc"),
        "Compiled output should contain the read service connection name"
    );

    // Should contain write token acquisition (SC_WRITE_TOKEN)
    assert!(
        compiled.contains("SC_WRITE_TOKEN"),
        "Compiled output should contain SC_WRITE_TOKEN for write service connection"
    );
    assert!(
        compiled.contains("my-write-sc"),
        "Compiled output should contain the write service connection name"
    );

    // Should NOT contain System.AccessToken in executor env
    assert!(
        !compiled.contains("SYSTEM_ACCESSTOKEN: $(System.AccessToken)"),
        "Compiled output should not pass System.AccessToken to executor"
    );

    // Verify no unreplaced markers remain
    for line in compiled.lines() {
        let stripped = line.replace("${{", "");
        assert!(
            !stripped.contains("{{ "),
            "Compiled output should not contain unreplaced marker: {}",
            line.trim()
        );
    }

    let _ = fs::remove_dir_all(&temp_dir);
}

/// Test that permissions omitted results in no token steps
#[test]
fn test_permissions_omitted_compiled_output() {
    let temp_dir = std::env::temp_dir().join(format!(
        "agentic-pipeline-permissions-none-{}",
        std::process::id()
    ));
    fs::create_dir_all(&temp_dir).expect("Failed to create temp directory");

    let test_input = temp_dir.join("no-perms-agent.md");
    let test_content = r#"---
name: "No Permissions Agent"
description: "Agent with no permissions configured"
---

## Test Agent

Do something.
"#;
    fs::write(&test_input, test_content).expect("Failed to write test input");

    let output_path = temp_dir.join("no-perms-agent.yml");
    let binary_path = PathBuf::from(env!("CARGO_BIN_EXE_ado-aw"));
    let output = std::process::Command::new(&binary_path)
        .args([
            "compile",
            test_input.to_str().unwrap(),
            "-o",
            output_path.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to run compiler");

    assert!(
        output.status.success(),
        "Compiler should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let compiled = fs::read_to_string(&output_path).expect("Should read compiled YAML");

    // Should NOT contain any SC token variables
    assert!(
        !compiled.contains("SC_READ_TOKEN"),
        "Compiled output should not contain SC_READ_TOKEN when permissions are omitted"
    );
    assert!(
        !compiled.contains("SC_WRITE_TOKEN"),
        "Compiled output should not contain SC_WRITE_TOKEN when permissions are omitted"
    );
    assert!(
        !compiled.contains("AzureCLI@2"),
        "Compiled output should not contain AzureCLI task when permissions are omitted"
    );

    // Verify no unreplaced markers remain
    for line in compiled.lines() {
        let stripped = line.replace("${{", "");
        assert!(
            !stripped.contains("{{ "),
            "Compiled output should not contain unreplaced marker: {}",
            line.trim()
        );
    }

    let _ = fs::remove_dir_all(&temp_dir);
}

/// Test that write-requiring safe-outputs compile successfully without an ARM write SC.
/// Default behavior: the executor uses `$(System.AccessToken)`; ARM write SC is optional.
#[test]
fn test_compile_succeeds_without_write_sc_for_write_requiring_safe_outputs() {
    let temp_dir = std::env::temp_dir().join(format!(
        "agentic-pipeline-default-token-{}",
        std::process::id()
    ));
    fs::create_dir_all(&temp_dir).expect("Failed to create temp directory");

    let test_input = temp_dir.join("default-token-agent.md");
    let test_content = r#"---
name: "Default Token Agent"
description: "Agent with create-work-item; relies on default System AccessToken"
safe-outputs:
  create-work-item:
    work-item-type: Task
---

## Test Agent

Do something.
"#;
    fs::write(&test_input, test_content).expect("Failed to write test input");

    let output_path = temp_dir.join("default-token-agent.yml");
    let binary_path = PathBuf::from(env!("CARGO_BIN_EXE_ado-aw"));
    let output = std::process::Command::new(&binary_path)
        .args([
            "compile",
            test_input.to_str().unwrap(),
            "-o",
            output_path.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to run compiler");

    assert!(
        output.status.success(),
        "Compiler should succeed for write-requiring safe-outputs without ARM write SC.\nstderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let compiled = fs::read_to_string(&output_path).expect("Compiled YAML should exist on success");
    assert!(
        compiled.contains("SYSTEM_ACCESSTOKEN: $(System.AccessToken)"),
        "Executor must map SYSTEM_ACCESSTOKEN from $(System.AccessToken) by default. \
         Compiled YAML did not contain it:\n{compiled}"
    );
    assert!(
        !compiled.contains("$(SC_WRITE_TOKEN)"),
        "Without permissions.write, executor must not reference SC_WRITE_TOKEN.\n{compiled}"
    );

    let _ = fs::remove_dir_all(&temp_dir);
}

/// Test that write-requiring safe-outputs succeed with write service connection
#[test]
fn test_permissions_validation_passes_with_write_sc() {
    let temp_dir = std::env::temp_dir().join(format!(
        "agentic-pipeline-permissions-pass-{}",
        std::process::id()
    ));
    fs::create_dir_all(&temp_dir).expect("Failed to create temp directory");

    let test_input = temp_dir.join("good-perms-agent.md");
    let test_content = r#"---
name: "Good Permissions Agent"
description: "Agent with create-work-item and write SC"
permissions:
  write: my-write-sc
safe-outputs:
  create-work-item:
    work-item-type: Task
  create-pull-request:
    target-branch: main
---

## Test Agent

Do something.
"#;
    fs::write(&test_input, test_content).expect("Failed to write test input");

    let output_path = temp_dir.join("good-perms-agent.yml");
    let binary_path = PathBuf::from(env!("CARGO_BIN_EXE_ado-aw"));
    let output = std::process::Command::new(&binary_path)
        .args([
            "compile",
            test_input.to_str().unwrap(),
            "-o",
            output_path.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to run compiler");

    assert!(
        output.status.success(),
        "Compiler should succeed when write SC is provided: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let compiled = fs::read_to_string(&output_path).expect("Should read compiled YAML");

    // Write-only path: SC_WRITE_TOKEN must be acquired; SC_READ_TOKEN must be absent
    assert!(
        compiled.contains("SC_WRITE_TOKEN"),
        "Compiled output should contain SC_WRITE_TOKEN for write service connection"
    );
    assert!(
        !compiled.contains("SC_READ_TOKEN"),
        "Compiled output should not contain SC_READ_TOKEN when only write SC is configured"
    );

    // Both configured safe-output tools must appear in the --enabled-tools list
    assert!(
        compiled.contains("--enabled-tools create-work-item"),
        "Compiled output should contain --enabled-tools create-work-item"
    );
    assert!(
        compiled.contains("--enabled-tools create-pull-request"),
        "Compiled output should contain --enabled-tools create-pull-request"
    );

    let _ = fs::remove_dir_all(&temp_dir);
}

/// Test that read-only permissions work (agent gets token, executor does not)
#[test]
fn test_permissions_read_only_compiled_output() {
    let temp_dir = std::env::temp_dir().join(format!(
        "agentic-pipeline-permissions-ro-{}",
        std::process::id()
    ));
    fs::create_dir_all(&temp_dir).expect("Failed to create temp directory");

    let test_input = temp_dir.join("read-only-agent.md");
    let test_content = r#"---
name: "Read Only Agent"
description: "Agent with read-only permissions"
permissions:
  read: my-read-sc
---

## Test Agent

Do something.
"#;
    fs::write(&test_input, test_content).expect("Failed to write test input");

    let output_path = temp_dir.join("read-only-agent.yml");
    let binary_path = PathBuf::from(env!("CARGO_BIN_EXE_ado-aw"));
    let output = std::process::Command::new(&binary_path)
        .args([
            "compile",
            test_input.to_str().unwrap(),
            "-o",
            output_path.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to run compiler");

    assert!(
        output.status.success(),
        "Compiler should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let compiled = fs::read_to_string(&output_path).expect("Should read compiled YAML");

    // Should contain read token but not write token
    assert!(
        compiled.contains("SC_READ_TOKEN"),
        "Compiled output should contain SC_READ_TOKEN"
    );
    assert!(
        !compiled.contains("SC_WRITE_TOKEN"),
        "Compiled output should not contain SC_WRITE_TOKEN when only read is configured"
    );

    let _ = fs::remove_dir_all(&temp_dir);
}

/// Test that the 1ES fixture compiles correctly with no unreplaced markers
/// and uses Copilot CLI (not Agency CLI) in custom jobs
#[test]
fn test_1es_compiled_output_no_unreplaced_markers() {
    let temp_dir = std::env::temp_dir().join(format!(
        "agentic-pipeline-1es-markers-{}",
        std::process::id()
    ));
    fs::create_dir_all(&temp_dir).expect("Failed to create temp directory");

    let fixture_src = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("1es-test-agent.md");

    // Copy the fixture into the temp dir before compiling so codemods
    // (e.g. pool_object_form) can never rewrite the source tree, and
    // parallel test runs cannot race on the same input file.
    let fixture_path = temp_dir.join("1es-test-agent.md");
    fs::copy(&fixture_src, &fixture_path).expect("Failed to copy 1ES fixture into temp dir");

    let output_path = temp_dir.join("1es-test-agent.yml");

    // Run the compiler binary
    let binary_path = PathBuf::from(env!("CARGO_BIN_EXE_ado-aw"));
    let output = std::process::Command::new(&binary_path)
        .args([
            "compile",
            fixture_path.to_str().unwrap(),
            "-o",
            output_path.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to run compiler");

    assert!(
        output.status.success(),
        "1ES compiler should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output_path.exists(), "Compiled 1ES YAML should exist");

    let compiled = fs::read_to_string(&output_path).expect("Should read compiled YAML");

    // Verify no unreplaced {{ markers }} remain (excluding ${{ }} which are ADO expressions)
    for line in compiled.lines() {
        let stripped = line.replace("${{", "");
        assert!(
            !stripped.contains("{{ "),
            "1ES compiled output should not contain unreplaced marker: {}",
            line.trim()
        );
    }

    // Verify the compiler version was correctly substituted
    let version = env!("CARGO_PKG_VERSION");
    assert!(
        compiled.contains(version),
        "1ES compiled output should contain compiler version {version}"
    );

    // Verify 1ES template uses Copilot CLI, not Agency CLI
    assert!(
        compiled.contains("Microsoft.Copilot.CLI.linux-x64"),
        "1ES template should install Copilot CLI"
    );
    assert!(
        !compiled.contains("install agency.linux-x64"),
        "1ES template should not install Agency CLI"
    );
    assert!(
        !compiled.contains("agency copilot"),
        "1ES template should not invoke 'agency copilot' command"
    );

    let _ = fs::remove_dir_all(&temp_dir);
}

/// Test that comment-on-work-item requires a target field
#[test]
fn test_comment_on_work_item_requires_target_field() {
    let temp_dir = std::env::temp_dir().join(format!(
        "agentic-pipeline-cwi-target-{}",
        std::process::id()
    ));
    fs::create_dir_all(&temp_dir).expect("Failed to create temp directory");

    let test_input = temp_dir.join("cwi-agent.md");
    let test_content = r#"---
name: "Comment Agent"
description: "Agent that comments on work items but has no target"
permissions:
  write: my-write-sc
safe-outputs:
  comment-on-work-item:
    max: 3
---

## Comment Agent

Comment on work items.
"#;
    fs::write(&test_input, test_content).expect("Failed to write test input");

    let output_path = temp_dir.join("cwi-agent.yml");
    let binary_path = PathBuf::from(env!("CARGO_BIN_EXE_ado-aw"));
    let output = std::process::Command::new(&binary_path)
        .args([
            "compile",
            test_input.to_str().unwrap(),
            "-o",
            output_path.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to run compiler");

    assert!(
        !output.status.success(),
        "Compiler should fail when comment-on-work-item lacks a target field"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("target"),
        "Error message should mention target: {stderr}"
    );

    let _ = fs::remove_dir_all(&temp_dir);
}

/// Test that comment-on-work-item fixture compiles and generates correct pipeline output
#[test]
fn test_fixture_comment_on_work_item_compiled_output() {
    let temp_dir = std::env::temp_dir().join(format!(
        "agentic-pipeline-cwi-fixture-{}",
        std::process::id()
    ));
    fs::create_dir_all(&temp_dir).expect("Failed to create temp directory");

    let fixture_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("comment-on-work-item-agent.md");

    assert!(
        fixture_path.exists(),
        "comment-on-work-item fixture should exist"
    );

    let output_path = temp_dir.join("comment-on-work-item-agent.yml");

    let binary_path = PathBuf::from(env!("CARGO_BIN_EXE_ado-aw"));
    let output = std::process::Command::new(&binary_path)
        .args([
            "compile",
            fixture_path.to_str().unwrap(),
            "-o",
            output_path.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to run compiler");

    assert!(
        output.status.success(),
        "Compiler should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output_path.exists(), "Compiled YAML should exist");

    let compiled = fs::read_to_string(&output_path).expect("Should read compiled YAML");

    // Should contain comment-on-work-item in the safe outputs tool config
    assert!(
        compiled.contains("comment-on-work-item"),
        "Compiled output should reference comment-on-work-item tool"
    );

    // Should have safeoutputs in the allowed tools (agent can call comment-on-work-item via MCP)
    assert!(
        compiled.contains("safeoutputs"),
        "Compiled output should allow the safeoutputs MCP tool"
    );

    // Should contain the write service connection for Stage 3
    assert!(
        compiled.contains("my-write-sc"),
        "Compiled output should contain the write service connection"
    );

    // Verify no unreplaced markers
    for line in compiled.lines() {
        let stripped = line.replace("${{", "");
        assert!(
            !stripped.contains("{{ "),
            "Compiled output should not contain unreplaced marker: {}",
            line.trim()
        );
    }

    let _ = fs::remove_dir_all(&temp_dir);
}

/// Test that update-work-item requires a target field
#[test]
fn test_update_work_item_requires_target_field() {
    let temp_dir = std::env::temp_dir().join(format!(
        "agentic-pipeline-uwi-target-{}",
        std::process::id()
    ));
    fs::create_dir_all(&temp_dir).expect("Failed to create temp directory");

    let test_input = temp_dir.join("uwi-agent.md");
    let test_content = r#"---
name: "Update Work Item Agent"
description: "Agent that updates work items but has no target"
permissions:
  write: my-write-sc
safe-outputs:
  update-work-item:
    title: true
    status: true
---

## Update Work Item Agent

Update existing work items.
"#;
    fs::write(&test_input, test_content).expect("Failed to write test input");

    let output_path = temp_dir.join("uwi-agent.yml");
    let binary_path = PathBuf::from(env!("CARGO_BIN_EXE_ado-aw"));
    let output = std::process::Command::new(&binary_path)
        .args([
            "compile",
            test_input.to_str().unwrap(),
            "-o",
            output_path.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to run compiler");

    assert!(
        !output.status.success(),
        "Compiler should fail when update-work-item lacks a target field"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("target"),
        "Error message should mention target: {stderr}"
    );

    let _ = fs::remove_dir_all(&temp_dir);
}

/// Test that compiled output starts with the `@ado-aw` header comment for pipeline detection
#[test]
fn test_compiled_output_has_header_comment() {
    let temp_dir =
        std::env::temp_dir().join(format!("agentic-pipeline-header-{}", std::process::id()));
    fs::create_dir_all(&temp_dir).expect("Failed to create temp directory");

    let fixture_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("minimal-agent.md");

    let output_path = temp_dir.join("minimal-agent.yml");

    let binary_path = PathBuf::from(env!("CARGO_BIN_EXE_ado-aw"));
    let output = std::process::Command::new(&binary_path)
        .args([
            "compile",
            fixture_path.to_str().unwrap(),
            "-o",
            output_path.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to run compiler");

    assert!(
        output.status.success(),
        "Compiler should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let compiled = fs::read_to_string(&output_path).expect("Should read compiled YAML");
    let lines: Vec<&str> = compiled.lines().collect();

    assert!(
        lines.len() >= 2,
        "Compiled output should have at least 2 lines for the header, got {}",
        lines.len()
    );

    // First line should be the "do not edit" warning
    assert!(
        lines[0].contains("auto-generated by ado-aw"),
        "First line should be auto-generated warning, got: {}",
        lines[0]
    );

    // Second line should be the @ado-aw marker with source and version
    assert!(
        lines[1].starts_with("# @ado-aw"),
        "Second line should start with '# @ado-aw', got: {}",
        lines[1]
    );
    // Source path should contain the fixture filename (quoted, path as passed to compiler)
    assert!(
        lines[1].contains("minimal-agent.md"),
        "Header should contain source filename, got: {}",
        lines[1]
    );
    assert!(
        lines[1].contains("source=\""),
        "Header should contain quoted source= key, got: {}",
        lines[1]
    );
    let version = env!("CARGO_PKG_VERSION");
    assert!(
        lines[1].contains(&format!("version={}", version)),
        "Header should contain compiler version, got: {}",
        lines[1]
    );

    let _ = fs::remove_dir_all(&temp_dir);
}

/// Test that `compile` with no path argument auto-discovers and recompiles all pipelines
#[test]
fn test_compile_auto_discover_recompiles_detected_pipelines() {
    let temp_dir = std::env::temp_dir().join(format!(
        "agentic-pipeline-autodiscover-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&temp_dir);
    fs::create_dir_all(&temp_dir).expect("Failed to create temp directory");

    // Create a source markdown file at agents/my-agent.md
    let agents_dir = temp_dir.join("agents");
    fs::create_dir_all(&agents_dir).expect("Failed to create agents directory");

    let source_content = r#"---
name: "Auto Discover Agent"
description: "An agent for testing auto-discovery"
---

## Auto Discover Agent

This agent tests the auto-discovery feature.
"#;
    fs::write(agents_dir.join("my-agent.md"), source_content)
        .expect("Failed to write source markdown");

    // Step 1: Compile the source file to create the initial YAML with header
    let binary_path = PathBuf::from(env!("CARGO_BIN_EXE_ado-aw"));
    let output = std::process::Command::new(&binary_path)
        .args(["compile", "agents/my-agent.md"])
        .current_dir(&temp_dir)
        .output()
        .expect("Failed to run initial compile");

    assert!(
        output.status.success(),
        "Initial compile should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Verify the YAML was created with the header
    let yaml_path = agents_dir.join("my-agent.lock.yml");
    assert!(yaml_path.exists(), "Compiled YAML should exist");
    let initial_yaml = fs::read_to_string(&yaml_path).expect("Should read initial YAML");
    assert!(
        initial_yaml.contains("# @ado-aw"),
        "Initial YAML should contain @ado-aw header"
    );

    // Step 2: Run compile with no arguments (auto-discover mode)
    let output = std::process::Command::new(&binary_path)
        .args(["compile"])
        .current_dir(&temp_dir)
        .output()
        .expect("Failed to run auto-discover compile");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "Auto-discover compile should succeed.\nstdout: {}\nstderr: {}",
        stdout,
        stderr
    );

    assert!(
        stdout.contains("Found 1 agentic pipeline"),
        "Should report finding 1 pipeline, got stdout: {}",
        stdout
    );

    assert!(
        stdout.contains("1 compiled"),
        "Should report 1 compiled, got stdout: {}",
        stdout
    );

    // The YAML should still exist and be valid
    let recompiled_yaml = fs::read_to_string(&yaml_path).expect("Should read recompiled YAML");
    assert!(
        recompiled_yaml.contains("# @ado-aw"),
        "Recompiled YAML should still contain @ado-aw header"
    );
    assert!(
        recompiled_yaml.contains("Auto Discover Agent"),
        "Recompiled YAML should contain agent name"
    );

    let _ = fs::remove_dir_all(&temp_dir);
}

/// Test that auto-discover resolves a bare source path relative to the lock file directory
#[test]
fn test_compile_auto_discover_resolves_source_relative_to_lock_file_dir() {
    let temp_dir = std::env::temp_dir().join(format!(
        "agentic-pipeline-autodiscover-relative-source-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&temp_dir);
    fs::create_dir_all(&temp_dir).expect("Failed to create temp directory");

    let templates_dir = temp_dir.join("azure-pipelines").join("templates");
    fs::create_dir_all(&templates_dir).expect("Failed to create templates directory");

    let source_content = r#"---
name: "Nested Agent"
description: "An agent in a nested directory"
---

## Nested Agent
"#;
    let source_path = templates_dir.join("nested-agent.md");
    fs::write(&source_path, source_content).expect("Failed to write source markdown");

    // Compile from the lock-file directory so the header stores a bare source filename.
    let binary_path = PathBuf::from(env!("CARGO_BIN_EXE_ado-aw"));
    let output = std::process::Command::new(&binary_path)
        .args(["compile", "nested-agent.md"])
        .current_dir(&templates_dir)
        .output()
        .expect("Failed to run initial compile");

    assert!(
        output.status.success(),
        "Initial compile should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let yaml_path = templates_dir.join("nested-agent.lock.yml");
    assert!(yaml_path.exists(), "Compiled YAML should exist");

    let initial_yaml = fs::read_to_string(&yaml_path).expect("Should read initial YAML");
    assert!(
        initial_yaml.contains(r#"source="nested-agent.md""#),
        "Expected bare source path in header, got:\n{}",
        initial_yaml
    );

    // Re-run auto-discover from repo root. This should find nested-agent.md next to the lock file.
    let output = std::process::Command::new(&binary_path)
        .args(["compile"])
        .current_dir(&temp_dir)
        .output()
        .expect("Failed to run auto-discover compile");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "Auto-discover compile should succeed.\nstdout: {}\nstderr: {}",
        stdout,
        stderr
    );
    assert!(
        stdout.contains("1 compiled"),
        "Should report 1 compiled, got stdout: {}",
        stdout
    );
    assert!(
        !stderr.contains("not found"),
        "Should not report missing source, got stderr: {}",
        stderr
    );

    let _ = fs::remove_dir_all(&temp_dir);
}

/// Test that auto-discover mode gracefully skips missing source files
#[test]
fn test_compile_auto_discover_skips_missing_source() {
    let temp_dir = std::env::temp_dir().join(format!(
        "agentic-pipeline-autodiscover-missing-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&temp_dir);
    fs::create_dir_all(&temp_dir).expect("Failed to create temp directory");

    // Create a YAML file with a @ado-aw header pointing to a source that doesn't exist
    let version = env!("CARGO_PKG_VERSION");
    let fake_yaml = format!(
        "# This file is auto-generated by ado-aw. Do not edit manually.\n\
         # @ado-aw source=\"agents/nonexistent.md\" version={}\n\
         name: fake-pipeline\n",
        version
    );
    fs::write(temp_dir.join("orphaned.yml"), fake_yaml).expect("Failed to write fake YAML");

    let binary_path = PathBuf::from(env!("CARGO_BIN_EXE_ado-aw"));
    let output = std::process::Command::new(&binary_path)
        .args(["compile"])
        .current_dir(&temp_dir)
        .output()
        .expect("Failed to run auto-discover compile");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    // Should succeed (skips, doesn't fail)
    assert!(
        output.status.success(),
        "Should succeed even with missing source.\nstdout: {}\nstderr: {}",
        stdout,
        stderr
    );

    assert!(
        stderr.contains("not found") || stdout.contains("skipped"),
        "Should warn about missing source.\nstdout: {}\nstderr: {}",
        stdout,
        stderr
    );

    let _ = fs::remove_dir_all(&temp_dir);
}

/// Test that submit-pr-review fails compilation when allowed-events is missing
#[test]
fn test_submit_pr_review_requires_allowed_events() {
    let temp_dir = std::env::temp_dir().join(format!(
        "agentic-pipeline-spr-events-{}",
        std::process::id()
    ));
    fs::create_dir_all(&temp_dir).expect("Failed to create temp directory");

    let test_input = temp_dir.join("spr-agent.md");
    let test_content = r#"---
name: "PR Review Agent"
description: "Agent that submits PR reviews but has no allowed-events"
permissions:
  write: my-write-sc
safe-outputs:
  submit-pr-review:
    allowed-repositories:
      - self
---

## PR Review Agent

Submit PR reviews.
"#;
    fs::write(&test_input, test_content).expect("Failed to write test input");

    let output_path = temp_dir.join("spr-agent.yml");
    let binary_path = PathBuf::from(env!("CARGO_BIN_EXE_ado-aw"));
    let output = std::process::Command::new(&binary_path)
        .args([
            "compile",
            test_input.to_str().unwrap(),
            "-o",
            output_path.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to run compiler");

    assert!(
        !output.status.success(),
        "Compiler should fail when submit-pr-review lacks allowed-events"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("allowed-events"),
        "Error message should mention allowed-events: {stderr}"
    );

    let _ = fs::remove_dir_all(&temp_dir);
}

/// Test that submit-pr-review fails compilation when allowed-events is an empty list
#[test]
fn test_submit_pr_review_requires_nonempty_allowed_events() {
    let temp_dir =
        std::env::temp_dir().join(format!("agentic-pipeline-spr-empty-{}", std::process::id()));
    fs::create_dir_all(&temp_dir).expect("Failed to create temp directory");

    let test_input = temp_dir.join("spr-agent.md");
    let test_content = r#"---
name: "PR Review Agent"
description: "Agent that submits PR reviews but has empty allowed-events"
permissions:
  write: my-write-sc
safe-outputs:
  submit-pr-review:
    allowed-events: []
---

## PR Review Agent

Submit PR reviews.
"#;
    fs::write(&test_input, test_content).expect("Failed to write test input");

    let output_path = temp_dir.join("spr-agent.yml");
    let binary_path = PathBuf::from(env!("CARGO_BIN_EXE_ado-aw"));
    let output = std::process::Command::new(&binary_path)
        .args([
            "compile",
            test_input.to_str().unwrap(),
            "-o",
            output_path.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to run compiler");

    assert!(
        !output.status.success(),
        "Compiler should fail when submit-pr-review has empty allowed-events"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("allowed-events"),
        "Error message should mention allowed-events: {stderr}"
    );

    let _ = fs::remove_dir_all(&temp_dir);
}

/// Test that submit-pr-review compiles successfully with proper config
#[test]
fn test_submit_pr_review_compiles_with_allowed_events() {
    let temp_dir =
        std::env::temp_dir().join(format!("agentic-pipeline-spr-pass-{}", std::process::id()));
    fs::create_dir_all(&temp_dir).expect("Failed to create temp directory");

    let test_input = temp_dir.join("spr-agent.md");
    let test_content = r#"---
name: "PR Review Agent"
description: "Agent that submits PR reviews with proper config"
permissions:
  write: my-write-sc
safe-outputs:
  submit-pr-review:
    allowed-events:
      - comment
      - approve-with-suggestions
---

## PR Review Agent

Submit PR reviews.
"#;
    fs::write(&test_input, test_content).expect("Failed to write test input");

    let output_path = temp_dir.join("spr-agent.yml");
    let binary_path = PathBuf::from(env!("CARGO_BIN_EXE_ado-aw"));
    let output = std::process::Command::new(&binary_path)
        .args([
            "compile",
            test_input.to_str().unwrap(),
            "-o",
            output_path.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to run compiler");

    assert!(
        output.status.success(),
        "Compiler should succeed with proper submit-pr-review config: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let compiled = fs::read_to_string(&output_path).expect("Should read compiled YAML");

    // The submit-pr-review tool must be listed as an enabled tool for the agent
    assert!(
        compiled.contains("--enabled-tools submit-pr-review"),
        "Compiled output should contain --enabled-tools submit-pr-review"
    );
    // Stage 3 write token must be acquired for the executor
    assert!(
        compiled.contains("SC_WRITE_TOKEN"),
        "Compiled output should contain SC_WRITE_TOKEN for write service connection"
    );

    let _ = fs::remove_dir_all(&temp_dir);
}

/// Test that update-pr fails compilation when vote is reachable but allowed-votes is missing
#[test]
fn test_update_pr_requires_allowed_votes_when_vote_reachable() {
    let temp_dir =
        std::env::temp_dir().join(format!("agentic-pipeline-uprvote-{}", std::process::id()));
    fs::create_dir_all(&temp_dir).expect("Failed to create temp directory");

    let test_input = temp_dir.join("upr-agent.md");
    // No allowed-operations → vote is reachable; no allowed-votes → should fail
    let test_content = r#"---
name: "Update PR Agent"
description: "Agent that votes on PRs but forgot to set allowed-votes"
permissions:
  write: my-write-sc
safe-outputs:
  update-pr:
    allowed-repositories:
      - self
---

## Update PR Agent

Vote on pull requests.
"#;
    fs::write(&test_input, test_content).expect("Failed to write test input");

    let output_path = temp_dir.join("upr-agent.yml");
    let binary_path = PathBuf::from(env!("CARGO_BIN_EXE_ado-aw"));
    let output = std::process::Command::new(&binary_path)
        .args([
            "compile",
            test_input.to_str().unwrap(),
            "-o",
            output_path.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to run compiler");

    assert!(
        !output.status.success(),
        "Compiler should fail when update-pr lacks allowed-votes with vote reachable"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("allowed-votes"),
        "Error message should mention allowed-votes: {stderr}"
    );

    let _ = fs::remove_dir_all(&temp_dir);
}

/// Test that update-pr compiles successfully when vote is restricted via allowed-operations
#[test]
fn test_update_pr_compiles_when_vote_excluded_from_allowed_operations() {
    let temp_dir = std::env::temp_dir().join(format!(
        "agentic-pipeline-uprnotvote-{}",
        std::process::id()
    ));
    fs::create_dir_all(&temp_dir).expect("Failed to create temp directory");

    let test_input = temp_dir.join("upr-agent.md");
    let test_content = r#"---
name: "Update PR Agent"
description: "Agent that sets reviewers but cannot vote"
permissions:
  write: my-write-sc
safe-outputs:
  update-pr:
    allowed-operations:
      - add-reviewers
      - set-auto-complete
---

## Update PR Agent

Manage pull requests.
"#;
    fs::write(&test_input, test_content).expect("Failed to write test input");

    let output_path = temp_dir.join("upr-agent.yml");
    let binary_path = PathBuf::from(env!("CARGO_BIN_EXE_ado-aw"));
    let output = std::process::Command::new(&binary_path)
        .args([
            "compile",
            test_input.to_str().unwrap(),
            "-o",
            output_path.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to run compiler");

    assert!(
        output.status.success(),
        "Compiler should succeed when vote is excluded from allowed-operations: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let compiled = fs::read_to_string(&output_path).expect("Should read compiled YAML");

    // update-pr must be listed as an enabled tool for the agent
    assert!(
        compiled.contains("--enabled-tools update-pr"),
        "Compiled output should contain --enabled-tools update-pr"
    );
    // Stage 3 must acquire a write token (permissions.write is set)
    assert!(
        compiled.contains("SC_WRITE_TOKEN"),
        "Compiled output should contain SC_WRITE_TOKEN for write service connection"
    );

    let _ = fs::remove_dir_all(&temp_dir);
}

/// Test that update-pr compiles successfully when allowed-votes is set
#[test]
fn test_update_pr_compiles_when_allowed_votes_set() {
    let temp_dir = std::env::temp_dir().join(format!(
        "agentic-pipeline-uprvoteset-{}",
        std::process::id()
    ));
    fs::create_dir_all(&temp_dir).expect("Failed to create temp directory");

    let test_input = temp_dir.join("upr-agent.md");
    let test_content = r#"---
name: "Update PR Agent"
description: "Agent that can vote on PRs with proper config"
permissions:
  write: my-write-sc
safe-outputs:
  update-pr:
    allowed-votes:
      - approve-with-suggestions
      - wait-for-author
---

## Update PR Agent

Vote on pull requests.
"#;
    fs::write(&test_input, test_content).expect("Failed to write test input");

    let output_path = temp_dir.join("upr-agent.yml");
    let binary_path = PathBuf::from(env!("CARGO_BIN_EXE_ado-aw"));
    let output = std::process::Command::new(&binary_path)
        .args([
            "compile",
            test_input.to_str().unwrap(),
            "-o",
            output_path.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to run compiler");

    assert!(
        output.status.success(),
        "Compiler should succeed with proper update-pr vote config: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let compiled = fs::read_to_string(&output_path).expect("Should read compiled YAML");

    // update-pr must be listed as an enabled tool for the agent
    assert!(
        compiled.contains("--enabled-tools update-pr"),
        "Compiled output should contain --enabled-tools update-pr"
    );
    // Stage 3 must acquire a write token (permissions.write is set)
    assert!(
        compiled.contains("SC_WRITE_TOKEN"),
        "Compiled output should contain SC_WRITE_TOKEN for write service connection"
    );

    let _ = fs::remove_dir_all(&temp_dir);
}

/// Integration test: compiling a pipeline with safe-outputs produces --enabled-tools flags
/// in the rendered YAML. This exercises standalone.rs wiring + generate_enabled_tools_args
/// + template substitution end-to-end.
#[test]
fn test_safe_outputs_enabled_tools_in_compiled_output() {
    let temp_dir = std::env::temp_dir().join(format!(
        "agentic-pipeline-enabled-tools-{}",
        std::process::id()
    ));
    fs::create_dir_all(&temp_dir).expect("Failed to create temp directory");

    let test_input = temp_dir.join("tools-agent.md");
    let test_content = r#"---
name: "Enabled Tools Agent"
description: "Agent with specific safe-outputs to verify enabled-tools flags"
permissions:
  write: my-write-sc
safe-outputs:
  create-pull-request:
    target-branch: main
  create-work-item:
    work-item-type: Task
---

## Agent

Do something.
"#;
    fs::write(&test_input, test_content).expect("Failed to write test input");

    let output_path = temp_dir.join("tools-agent.yml");
    let binary_path = PathBuf::from(env!("CARGO_BIN_EXE_ado-aw"));
    let output = std::process::Command::new(&binary_path)
        .args([
            "compile",
            test_input.to_str().unwrap(),
            "-o",
            output_path.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to run compiler");

    assert!(
        output.status.success(),
        "Compiler should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let compiled = fs::read_to_string(&output_path).expect("Should read compiled YAML");

    // Configured safe-output tools must appear as --enabled-tools flags
    assert!(
        compiled.contains("--enabled-tools create-pull-request"),
        "Compiled output should contain --enabled-tools create-pull-request"
    );
    assert!(
        compiled.contains("--enabled-tools create-work-item"),
        "Compiled output should contain --enabled-tools create-work-item"
    );

    // Always-on diagnostic tools must also appear
    assert!(
        compiled.contains("--enabled-tools noop"),
        "Compiled output should contain --enabled-tools noop"
    );
    assert!(
        compiled.contains("--enabled-tools missing-data"),
        "Compiled output should contain --enabled-tools missing-data"
    );

    // Tools NOT in safe-outputs should NOT appear (verifies filtering is selective)
    assert!(
        !compiled.contains("--enabled-tools update-wiki-page"),
        "Non-configured tools should not appear in --enabled-tools"
    );

    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_compiled_agent_job_copilot_invocation_uses_safeoutputs_mcp_config() {
    let compiled = compile_inline_agent(
        "copilot-cli-safeoutputs-default",
        r#"---
name: "Copilot CLI SafeOutputs"
description: "Compile-time contract for default Copilot SafeOutputs wiring"
engine:
  id: copilot
  model: gpt-5-mini
safe-outputs:
  noop: {}
---

## Agent

Call the noop tool exactly once.
"#,
    );

    let agent = extract_job_block(&compiled, "Agent").expect("Agent job should exist");
    let detection =
        extract_job_block(&compiled, "Detection").expect("Detection job should exist");

    assert!(
        agent.contains(
            "/tmp/awf-tools/copilot --prompt \"$(cat /tmp/awf-tools/agent-prompt.md)\" \
             --additional-mcp-config @/tmp/awf-tools/mcp-config.json"
        ),
        "agent job should pass compiler-emitted MCP config to Copilot CLI: {agent}"
    );
    assert!(
        agent.contains("--allow-all-tools"),
        "default unrestricted tools path should emit --allow-all-tools: {agent}"
    );
    assert!(
        !detection.contains("--additional-mcp-config"),
        "detection job should not receive the SafeOutputs MCP config: {detection}"
    );
    assert!(
        compiled.contains("\"safeoutputs\": {") && compiled.contains("\"type\": \"http\""),
        "compiled MCPG config should include the SafeOutputs HTTP backend: {compiled}"
    );
    assert!(
        compiled.contains("\"url\": \"http://localhost:${SAFE_OUTPUTS_PORT}/mcp\""),
        "compiled MCPG config should keep the runtime SafeOutputs port placeholder: {compiled}"
    );
    assert!(
        compiled.contains("\"Authorization\": \"Bearer ")
            && compiled.contains("SAFE_OUTPUTS_API_KEY"),
        "compiled MCPG config should keep the runtime SafeOutputs auth placeholder: {compiled}"
    );
}

#[test]
fn test_compiled_agent_job_copilot_invocation_supports_representative_cli_variants() {
    let compiled = compile_inline_agent(
        "copilot-cli-safeoutputs-matrix",
        r#"---
name: "Copilot CLI Matrix"
description: "Compile-time contract for representative Copilot CLI surface variants"
tools:
  bash:
    - echo
engine:
  id: copilot
  model: gpt-5-mini
  agent: my-custom-agent
  api-target: api.example.com
  args:
    - --reasoning-effort=high
safe-outputs:
  noop: {}
---

## Agent

Call the noop tool exactly once.
"#,
    );

    let agent = extract_job_block(&compiled, "Agent").expect("Agent job should exist");

    assert!(
        !agent.contains("--allow-all-tools"),
        "restricted bash path should not emit --allow-all-tools: {agent}"
    );
    assert!(
        agent.contains("--allow-tool safeoutputs"),
        "restricted bash path must explicitly allow the SafeOutputs MCP server: {agent}"
    );
    assert!(
        agent.contains("--allow-tool \"shell(echo)\""),
        "restricted bash path must emit the configured bash allowlist: {agent}"
    );
    assert!(
        agent.contains("--agent my-custom-agent"),
        "engine.agent should flow through the compiled Copilot CLI invocation: {agent}"
    );
    assert!(
        agent.contains("--api-target api.example.com"),
        "engine.api-target should flow through the compiled Copilot CLI invocation: {agent}"
    );
    assert!(
        agent.contains("--reasoning-effort=high"),
        "engine.args should append additive Copilot CLI arguments: {agent}"
    );
    assert!(
        agent.contains("--additional-mcp-config @/tmp/awf-tools/mcp-config.json"),
        "restricted tools path should still use the compiler-emitted MCP config: {agent}"
    );
}

// ==================== Azure DevOps MCP Integration Tests ====================

/// Test that the Azure DevOps MCP fixture compiles successfully with no unreplaced markers
#[test]
fn test_fixture_azure_devops_mcp_compiled_output() {
    let temp_dir =
        std::env::temp_dir().join(format!("agentic-pipeline-ado-mcp-{}", std::process::id()));
    fs::create_dir_all(&temp_dir).expect("Failed to create temp directory");

    let fixture_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("azure-devops-mcp-agent.md");

    let output_path = temp_dir.join("azure-devops-mcp-agent.yml");

    let binary_path = PathBuf::from(env!("CARGO_BIN_EXE_ado-aw"));
    let output = std::process::Command::new(&binary_path)
        .args([
            "compile",
            fixture_path.to_str().unwrap(),
            "-o",
            output_path.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to run compiler");

    assert!(
        output.status.success(),
        "Compiler should succeed for Azure DevOps MCP fixture: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let compiled = fs::read_to_string(&output_path).expect("Should read compiled output");

    // No unreplaced template markers (except ADO ${{ }} expressions)
    for line in compiled.lines() {
        let stripped = line.replace("${{", "");
        assert!(
            !stripped.contains("{{ "),
            "Compiled output should not contain unreplaced marker: {}",
            line.trim()
        );
    }

    // Should contain MCPG references
    assert!(
        compiled.contains("ghcr.io/github/gh-aw-mcpg"),
        "Should reference MCPG Docker image"
    );

    // Should contain the container-based MCP config (container field, not command)
    assert!(
        compiled.contains("\"container\""),
        "MCPG config should use container field"
    );
    assert!(
        compiled.contains("node:20-slim"),
        "MCPG config should contain the container image"
    );
    assert!(
        compiled.contains("\"entrypoint\""),
        "MCPG config should have entrypoint field"
    );
    assert!(
        compiled.contains("\"entrypointArgs\""),
        "MCPG config should have entrypointArgs field"
    );
    assert!(
        !compiled.contains("\"command\""),
        "MCPG config should NOT use command field"
    );

    // Should contain env for ADO_MCP_AUTH_TOKEN (envvar auth for @azure-devops/mcp)
    assert!(
        compiled.contains("ADO_MCP_AUTH_TOKEN"),
        "Should reference ADO_MCP_AUTH_TOKEN"
    );

    // Should contain SC_READ_TOKEN (from permissions.read)
    assert!(
        compiled.contains("SC_READ_TOKEN"),
        "Should contain SC_READ_TOKEN"
    );

    // Should contain the MCPG docker env passthrough (auto-mapped ADO token)
    assert!(
        compiled.contains("-e ADO_MCP_AUTH_TOKEN=\"$SC_READ_TOKEN\""),
        "Should auto-map SC_READ_TOKEN to ADO_MCP_AUTH_TOKEN on MCPG Docker run"
    );

    let _ = fs::remove_dir_all(&temp_dir);
}

/// Test that container-based MCPs generate correct MCPG config JSON structure
#[test]
fn test_mcpg_config_container_based_mcp() {
    let temp_dir = std::env::temp_dir().join(format!(
        "agentic-pipeline-mcpg-container-{}",
        std::process::id()
    ));
    fs::create_dir_all(&temp_dir).expect("Failed to create temp directory");

    let input = "---\nname: \"Container MCP Test\"\ndescription: \"Tests container-based MCP\"\nmcp-servers:\n  my-tool:\n    container: \"ghcr.io/example/my-tool:latest\"\n    entrypoint: \"my-tool\"\n    entrypoint-args: [\"--mode\", \"stdio\"]\n    mounts:\n      - \"/host/data:/app/data:ro\"\n    env:\n      API_KEY: \"test-key\"\n    allowed:\n      - tool_a\n      - tool_b\n---\n\n## Test\n";

    let input_path = temp_dir.join("container-mcp.md");
    let output_path = temp_dir.join("container-mcp.yml");
    fs::write(&input_path, input).unwrap();

    let binary_path = PathBuf::from(env!("CARGO_BIN_EXE_ado-aw"));
    let output = std::process::Command::new(&binary_path)
        .args([
            "compile",
            input_path.to_str().unwrap(),
            "-o",
            output_path.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to run compiler");

    assert!(
        output.status.success(),
        "Compiler should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let compiled = fs::read_to_string(&output_path).unwrap();

    assert!(compiled.contains("\"container\": \"ghcr.io/example/my-tool:latest\""));
    assert!(compiled.contains("\"entrypoint\": \"my-tool\""));
    assert!(compiled.contains("\"entrypointArgs\""));
    assert!(compiled.contains("\"mounts\""));
    assert!(compiled.contains("/host/data:/app/data:ro"));
    assert!(compiled.contains("\"API_KEY\": \"test-key\""));
    assert!(compiled.contains("\"tool_a\""));
    assert!(!compiled.contains("\"command\""));

    let _ = fs::remove_dir_all(&temp_dir);
}

/// Test that HTTP-based MCPs generate correct MCPG config JSON structure
#[test]
fn test_mcpg_config_http_based_mcp() {
    let temp_dir =
        std::env::temp_dir().join(format!("agentic-pipeline-mcpg-http-{}", std::process::id()));
    fs::create_dir_all(&temp_dir).expect("Failed to create temp directory");

    let input = "---\nname: \"HTTP MCP Test\"\ndescription: \"Tests HTTP MCP\"\nmcp-servers:\n  remote-ado:\n    url: \"https://mcp.dev.azure.com/myorg\"\n    headers:\n      X-MCP-Toolsets: \"repos,wit\"\n    allowed:\n      - wit_get_work_item\n---\n\n## Test\n";

    let input_path = temp_dir.join("http-mcp.md");
    let output_path = temp_dir.join("http-mcp.yml");
    fs::write(&input_path, input).unwrap();

    let binary_path = PathBuf::from(env!("CARGO_BIN_EXE_ado-aw"));
    let output = std::process::Command::new(&binary_path)
        .args([
            "compile",
            input_path.to_str().unwrap(),
            "-o",
            output_path.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to run compiler");

    assert!(
        output.status.success(),
        "Compiler should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let compiled = fs::read_to_string(&output_path).unwrap();

    assert!(compiled.contains("\"url\": \"https://mcp.dev.azure.com/myorg\""));
    assert!(compiled.contains("\"X-MCP-Toolsets\": \"repos,wit\""));
    assert!(compiled.contains("\"wit_get_work_item\""));
    assert!(!compiled.contains("\"command\""));

    let _ = fs::remove_dir_all(&temp_dir);
}

/// Test that env passthrough generates -e flags in MCPG Docker run
#[test]
fn test_mcpg_docker_env_passthrough() {
    let temp_dir =
        std::env::temp_dir().join(format!("agentic-pipeline-mcpg-env-{}", std::process::id()));
    fs::create_dir_all(&temp_dir).expect("Failed to create temp directory");

    let input = "---\nname: \"Env Test\"\ndescription: \"Tests env passthrough\"\npermissions:\n  read: my-read-sc\n  write: my-write-sc\nmcp-servers:\n  my-tool:\n    container: \"node:20-slim\"\n    env:\n      AZURE_DEVOPS_EXT_PAT: \"\"\n      MY_TOKEN: \"\"\n      STATIC_VAR: \"static-value\"\nsafe-outputs:\n  create-work-item:\n    work-item-type: Task\n---\n\n## Test\n";

    let input_path = temp_dir.join("env-passthrough.md");
    let output_path = temp_dir.join("env-passthrough.yml");
    fs::write(&input_path, input).unwrap();

    let binary_path = PathBuf::from(env!("CARGO_BIN_EXE_ado-aw"));
    let output = std::process::Command::new(&binary_path)
        .args([
            "compile",
            input_path.to_str().unwrap(),
            "-o",
            output_path.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to run compiler");

    assert!(
        output.status.success(),
        "Compiler should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let compiled = fs::read_to_string(&output_path).unwrap();

    // AZURE_DEVOPS_EXT_PAT with "" is bare passthrough for user-configured MCPs
    // (only tools.azure-devops extension provides SC_READ_TOKEN mapping)
    assert!(
        compiled.contains("-e AZURE_DEVOPS_EXT_PAT"),
        "Should forward AZURE_DEVOPS_EXT_PAT as passthrough"
    );

    // Should forward passthrough env var MY_TOKEN
    assert!(
        compiled.contains("-e MY_TOKEN"),
        "Should forward passthrough env var"
    );

    // Static var should be in config
    assert!(
        compiled.contains("\"STATIC_VAR\": \"static-value\""),
        "Static env var should be in config"
    );

    // Regression for issue #1034: the docker-env continuation lines must never
    // emit a stray `\ \` sequence. In bash `\ ` is an escaped space (a
    // one-character " " argument) followed by a line continuation, which
    // corrupts the `docker run` image reference and makes MCPG fail with
    // `docker: invalid reference format.`
    assert!(
        !compiled.contains("\\ \\"),
        "Compiled YAML must not contain the corrupt `\\ \\` continuation sequence"
    );

    let _ = fs::remove_dir_all(&temp_dir);
}

/// Test that user-defined parameters are emitted in the compiled pipeline YAML
#[test]
fn test_parameters_in_compiled_output() {
    let temp_dir =
        std::env::temp_dir().join(format!("agentic-pipeline-params-{}", std::process::id()));
    fs::create_dir_all(&temp_dir).expect("Failed to create temp directory");

    let input = r#"---
name: "Params Agent"
description: "Tests parameters feature"
parameters:
  - name: verbose
    displayName: "Verbose output"
    type: boolean
    default: false
  - name: region
    displayName: "Target region"
    type: string
    default: "us-east"
    values:
      - us-east
      - eu-west
---

## Test

Do the thing.
"#;

    let input_path = temp_dir.join("params-agent.md");
    let output_path = temp_dir.join("params-agent.yml");
    fs::write(&input_path, input).unwrap();

    let binary_path = PathBuf::from(env!("CARGO_BIN_EXE_ado-aw"));
    let output = std::process::Command::new(&binary_path)
        .args([
            "compile",
            input_path.to_str().unwrap(),
            "-o",
            output_path.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to run compiler");

    assert!(
        output.status.success(),
        "Compiler should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let compiled = fs::read_to_string(&output_path).unwrap();

    // Verify parameters block is present
    assert!(
        compiled.contains("parameters:"),
        "Should contain parameters: block"
    );
    assert!(
        compiled.contains("name: verbose"),
        "Should contain verbose parameter"
    );
    assert!(
        compiled.contains("name: region"),
        "Should contain region parameter"
    );
    assert!(
        compiled.contains("displayName: Verbose output"),
        "Should contain displayName"
    );
    assert!(
        compiled.contains("default: false"),
        "Should contain default for verbose"
    );
    assert!(
        compiled.contains("default: us-east"),
        "Should contain default for region"
    );
    assert!(
        compiled.contains("- us-east"),
        "Should contain values for region"
    );
    assert!(
        compiled.contains("- eu-west"),
        "Should contain values for region"
    );

    // No clearMemory should be injected (no memory configured)
    assert!(
        !compiled.contains("clearMemory"),
        "Should NOT contain clearMemory without memory"
    );

    let _ = fs::remove_dir_all(&temp_dir);
}

/// Test that clearMemory is auto-injected when memory is enabled
#[test]
fn test_parameters_clear_memory_auto_injected() {
    let temp_dir = std::env::temp_dir().join(format!(
        "agentic-pipeline-clear-memory-{}",
        std::process::id()
    ));
    fs::create_dir_all(&temp_dir).expect("Failed to create temp directory");

    let input = r#"---
name: "Memory Agent"
description: "Tests clearMemory auto-injection"
tools:
  cache-memory:
    allowed-extensions:
      - .md
---

## Test

Do the thing.
"#;

    let input_path = temp_dir.join("memory-agent.md");
    let output_path = temp_dir.join("memory-agent.yml");
    fs::write(&input_path, input).unwrap();

    let binary_path = PathBuf::from(env!("CARGO_BIN_EXE_ado-aw"));
    let output = std::process::Command::new(&binary_path)
        .args([
            "compile",
            input_path.to_str().unwrap(),
            "-o",
            output_path.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to run compiler");

    assert!(
        output.status.success(),
        "Compiler should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let compiled = fs::read_to_string(&output_path).unwrap();

    // Verify clearMemory parameter is auto-injected
    assert!(
        compiled.contains("name: clearMemory"),
        "Should auto-inject clearMemory parameter"
    );
    assert!(
        compiled.contains("displayName: Clear agent memory"),
        "Should have displayName"
    );
    assert!(compiled.contains("type: boolean"), "Should be boolean type");

    // Verify memory download has condition
    assert!(
        compiled.contains("condition: eq(${{ parameters.clearMemory }}, false)"),
        "Memory download should be conditional on clearMemory=false"
    );
    assert!(
        compiled.contains("condition: eq(${{ parameters.clearMemory }}, true)"),
        "Clear memory step should run when clearMemory=true"
    );
    assert!(
        compiled.contains("Initialize empty agent memory (clearMemory=true)"),
        "Should have the clear memory initialization step"
    );

    let _ = fs::remove_dir_all(&temp_dir);
}

/// Test that user-defined clearMemory is not duplicated
#[test]
fn test_parameters_user_defined_clear_memory_not_duplicated() {
    let temp_dir = std::env::temp_dir().join(format!(
        "agentic-pipeline-user-clear-memory-{}",
        std::process::id()
    ));
    fs::create_dir_all(&temp_dir).expect("Failed to create temp directory");

    let input = r#"---
name: "Custom Memory Agent"
description: "Tests user-defined clearMemory not duplicated"
parameters:
  - name: clearMemory
    displayName: "Reset memory"
    type: boolean
    default: true
tools:
  cache-memory:
    allowed-extensions:
      - .md
---

## Test

Do the thing.
"#;

    let input_path = temp_dir.join("custom-memory.md");
    let output_path = temp_dir.join("custom-memory.yml");
    fs::write(&input_path, input).unwrap();

    let binary_path = PathBuf::from(env!("CARGO_BIN_EXE_ado-aw"));
    let output = std::process::Command::new(&binary_path)
        .args([
            "compile",
            input_path.to_str().unwrap(),
            "-o",
            output_path.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to run compiler");

    assert!(
        output.status.success(),
        "Compiler should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let compiled = fs::read_to_string(&output_path).unwrap();

    // Verify user's clearMemory is present (with their custom displayName and default)
    assert!(
        compiled.contains("displayName: Reset memory"),
        "Should use user's displayName"
    );
    assert!(
        compiled.contains("default: true"),
        "Should use user's default value"
    );

    // Verify clearMemory only appears once (not duplicated)
    let count = compiled.matches("name: clearMemory").count();
    assert_eq!(
        count, 1,
        "clearMemory should appear exactly once, not duplicated"
    );

    let _ = fs::remove_dir_all(&temp_dir);
}

/// Test that parameters block has no unreplaced markers
#[test]
fn test_parameters_no_unreplaced_markers() {
    let temp_dir = std::env::temp_dir().join(format!(
        "agentic-pipeline-params-markers-{}",
        std::process::id()
    ));
    fs::create_dir_all(&temp_dir).expect("Failed to create temp directory");

    let input = r#"---
name: "Markers Agent"
description: "Tests no unreplaced markers with parameters"
parameters:
  - name: myParam
    type: string
    default: "hello"
tools:
  cache-memory:
    allowed-extensions:
      - .md
---

## Test
"#;

    let input_path = temp_dir.join("markers-agent.md");
    let output_path = temp_dir.join("markers-agent.yml");
    fs::write(&input_path, input).unwrap();

    let binary_path = PathBuf::from(env!("CARGO_BIN_EXE_ado-aw"));
    let output = std::process::Command::new(&binary_path)
        .args([
            "compile",
            input_path.to_str().unwrap(),
            "-o",
            output_path.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to run compiler");

    assert!(
        output.status.success(),
        "Compiler should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let compiled = fs::read_to_string(&output_path).unwrap();

    // Verify no unreplaced {{ markers }} remain (excluding ${{ }} which are ADO expressions)
    for line in compiled.lines() {
        let stripped = line.replace("${{", "");
        assert!(
            !stripped.contains("{{ "),
            "Should not contain unreplaced marker: {}",
            line.trim()
        );
    }

    let _ = fs::remove_dir_all(&temp_dir);
}

/// Test that network.allowed with a valid leading wildcard (*.example.com) compiles successfully
#[test]
fn test_network_allow_valid_wildcard_compiles() {
    let temp_dir = std::env::temp_dir().join(format!(
        "agentic-pipeline-network-valid-wildcard-{}",
        std::process::id()
    ));
    fs::create_dir_all(&temp_dir).expect("Failed to create temp directory");

    let input = r#"---
name: "Network Wildcard Agent"
description: "Agent with valid leading wildcard in network.allowed"
network:
  allowed:
    - "*.mycompany.com"
    - "api.external-service.com"
---

## Test
"#;

    let input_path = temp_dir.join("network-valid-wildcard.md");
    let output_path = temp_dir.join("network-valid-wildcard.yml");
    fs::write(&input_path, input).unwrap();

    let binary_path = PathBuf::from(env!("CARGO_BIN_EXE_ado-aw"));
    let output = std::process::Command::new(&binary_path)
        .args([
            "compile",
            input_path.to_str().unwrap(),
            "-o",
            output_path.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to run compiler");

    assert!(
        output.status.success(),
        "Compiler should succeed for valid wildcard '*.mycompany.com': {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let compiled = fs::read_to_string(&output_path).expect("Should read compiled YAML");
    assert!(
        compiled.contains("*.mycompany.com"),
        "Compiled output should include the wildcard domain '*.mycompany.com' in the allow list"
    );
    assert!(
        compiled.contains("api.external-service.com"),
        "Compiled output should include 'api.external-service.com' in the allow list"
    );

    let _ = fs::remove_dir_all(&temp_dir);
}

/// Test that network.allowed with a trailing wildcard (example.*) fails compilation
#[test]
fn test_network_allow_trailing_wildcard_fails() {
    let temp_dir = std::env::temp_dir().join(format!(
        "agentic-pipeline-network-trailing-wildcard-{}",
        std::process::id()
    ));
    fs::create_dir_all(&temp_dir).expect("Failed to create temp directory");

    let input = r#"---
name: "Network Trailing Wildcard Agent"
description: "Agent with trailing wildcard in network.allowed"
network:
  allowed:
    - "example.*"
---

## Test
"#;

    let input_path = temp_dir.join("network-trailing-wildcard.md");
    let output_path = temp_dir.join("network-trailing-wildcard.yml");
    fs::write(&input_path, input).unwrap();

    let binary_path = PathBuf::from(env!("CARGO_BIN_EXE_ado-aw"));
    let output = std::process::Command::new(&binary_path)
        .args([
            "compile",
            input_path.to_str().unwrap(),
            "-o",
            output_path.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to run compiler");

    assert!(
        !output.status.success(),
        "Compiler should fail for trailing wildcard 'example.*'"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unsupported position"),
        "Error message should mention unsupported position: {stderr}"
    );

    let _ = fs::remove_dir_all(&temp_dir);
}

/// Test that network.allowed with a mid-string wildcard (ex*ample.com) fails compilation
#[test]
fn test_network_allow_mid_wildcard_fails() {
    let temp_dir = std::env::temp_dir().join(format!(
        "agentic-pipeline-network-mid-wildcard-{}",
        std::process::id()
    ));
    fs::create_dir_all(&temp_dir).expect("Failed to create temp directory");

    let input = r#"---
name: "Network Mid Wildcard Agent"
description: "Agent with mid-string wildcard in network.allowed"
network:
  allowed:
    - "ex*ample.com"
---

## Test
"#;

    let input_path = temp_dir.join("network-mid-wildcard.md");
    let output_path = temp_dir.join("network-mid-wildcard.yml");
    fs::write(&input_path, input).unwrap();

    let binary_path = PathBuf::from(env!("CARGO_BIN_EXE_ado-aw"));
    let output = std::process::Command::new(&binary_path)
        .args([
            "compile",
            input_path.to_str().unwrap(),
            "-o",
            output_path.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to run compiler");

    assert!(
        !output.status.success(),
        "Compiler should fail for mid-string wildcard 'ex*ample.com'"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unsupported position"),
        "Error message should mention unsupported position: {stderr}"
    );

    let _ = fs::remove_dir_all(&temp_dir);
}

/// Test that network.allowed with a double wildcard (*.*.com) fails compilation
#[test]
fn test_network_allow_double_wildcard_fails() {
    let temp_dir = std::env::temp_dir().join(format!(
        "agentic-pipeline-network-double-wildcard-{}",
        std::process::id()
    ));
    fs::create_dir_all(&temp_dir).expect("Failed to create temp directory");

    let input = r#"---
name: "Network Double Wildcard Agent"
description: "Agent with double wildcard in network.allowed"
network:
  allowed:
    - "*.*.com"
---

## Test
"#;

    let input_path = temp_dir.join("network-double-wildcard.md");
    let output_path = temp_dir.join("network-double-wildcard.yml");
    fs::write(&input_path, input).unwrap();

    let binary_path = PathBuf::from(env!("CARGO_BIN_EXE_ado-aw"));
    let output = std::process::Command::new(&binary_path)
        .args([
            "compile",
            input_path.to_str().unwrap(),
            "-o",
            output_path.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to run compiler");

    assert!(
        !output.status.success(),
        "Compiler should fail for double wildcard '*.*.com'"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unsupported position"),
        "Error message should mention unsupported position: {stderr}"
    );

    let _ = fs::remove_dir_all(&temp_dir);
}

/// Integration test: `runtimes: lean: true` end-to-end compilation
///
/// Verifies that a pipeline compiled with `runtimes: lean: true` contains:
/// - The elan installer step (`elan-init.sh`)
/// - Lean ecosystem domains in the network allow-list (`elan.lean-lang.org`)
/// - `--allow-all-tools` (default when bash is not explicitly configured)
/// - No unreplaced `{{ }}` template markers
#[test]
fn test_lean_runtime_compiled_output() {
    let temp_dir =
        std::env::temp_dir().join(format!("agentic-pipeline-lean-{}", std::process::id()));
    fs::create_dir_all(&temp_dir).expect("Failed to create temp directory");

    let input = r#"---
name: "Lean Agent"
description: "Agent with Lean 4 runtime"
runtimes:
  lean: true
---

## Lean Agent

Prove theorems and build Lean 4 projects.
"#;

    let input_path = temp_dir.join("lean-agent.md");
    let output_path = temp_dir.join("lean-agent.yml");
    fs::write(&input_path, input).expect("Failed to write test input");

    let binary_path = PathBuf::from(env!("CARGO_BIN_EXE_ado-aw"));
    let output = std::process::Command::new(&binary_path)
        .args([
            "compile",
            input_path.to_str().unwrap(),
            "-o",
            output_path.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to run compiler");

    assert!(
        output.status.success(),
        "Compiler should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output_path.exists(), "Compiled YAML should exist");

    let compiled = fs::read_to_string(&output_path).expect("Should read compiled YAML");

    // Lean runtime installs elan via the elan-init.sh script
    assert!(
        compiled.contains("elan-init.sh"),
        "Compiled output should include elan-init.sh installer step"
    );

    // Lean ecosystem domains should appear in the AWF allow-domains list
    assert!(
        compiled.contains("elan.lean-lang.org"),
        "Compiled output should include elan.lean-lang.org in allowed domains"
    );

    // With no explicit bash config, default is --allow-all-tools (gh-aw sandbox default).
    // Lean tools are implicitly covered by --allow-all-tools.
    assert!(
        compiled.contains("--allow-all-tools"),
        "Compiled output should include --allow-all-tools (default when bash is not specified)"
    );

    // The Lean runtime must contribute an AWF --mount for $HOME/.elan so the
    // elan-installed toolchain is visible inside the sandbox. This guards against
    // regressions where generate_awf_mounts is dropped from extra_replacements
    // or the {{ awf_mounts }} marker is removed from the template.
    assert!(
        compiled.contains("--mount"),
        "Compiled output should contain AWF mount for elan"
    );
    assert!(
        compiled.contains("$HOME/.elan"),
        "Compiled output should mount $HOME/.elan into the AWF sandbox"
    );

    // The Lean runtime must inject $HOME/.elan/bin into the AWF chroot PATH via
    // a dedicated "Generate GITHUB_PATH file" step and explicit GITHUB_PATH env
    // passthrough on the AWF invocation step.
    assert!(
        compiled.contains("Generate GITHUB_PATH file"),
        "Compiled output should include the Generate GITHUB_PATH file step"
    );
    assert!(
        compiled.contains("$HOME/.elan/bin"),
        "Compiled output should write $HOME/.elan/bin to the GITHUB_PATH file"
    );
    assert!(
        compiled.contains("GITHUB_PATH: $(GITHUB_PATH)"),
        "Compiled output should pass GITHUB_PATH through the AWF step env block"
    );

    // Verify no unreplaced {{ markers }} remain
    for line in compiled.lines() {
        let stripped = line.replace("${{", "");
        assert!(
            !stripped.contains("{{ "),
            "Compiled output should not contain unreplaced marker: {}",
            line.trim()
        );
    }

    let _ = fs::remove_dir_all(&temp_dir);
}

/// Integration test: `runtimes: dotnet: true` end-to-end compilation
///
/// Verifies that the dotnet runtime, when enabled with the simple boolean
/// form, produces a pipeline that includes the `UseDotNet@2` install task,
/// the `dotnet` bash command in the allow-list, and the .NET ecosystem
/// domains in the AWF allow-domains list.
#[test]
fn test_dotnet_runtime_compiled_output() {
    let temp_dir =
        std::env::temp_dir().join(format!("agentic-pipeline-dotnet-{}", std::process::id()));
    fs::create_dir_all(&temp_dir).expect("Failed to create temp directory");

    let input = r#"---
name: "Dotnet Agent"
description: "Agent with .NET runtime"
runtimes:
  dotnet: true
---

## Dotnet Agent

Build and test .NET projects.
"#;

    let input_path = temp_dir.join("dotnet-agent.md");
    let output_path = temp_dir.join("dotnet-agent.yml");
    fs::write(&input_path, input).expect("Failed to write test input");

    let binary_path = PathBuf::from(env!("CARGO_BIN_EXE_ado-aw"));
    let output = std::process::Command::new(&binary_path)
        .args([
            "compile",
            input_path.to_str().unwrap(),
            "-o",
            output_path.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to run compiler");

    assert!(
        output.status.success(),
        "Compiler should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output_path.exists(), "Compiled YAML should exist");

    let compiled = fs::read_to_string(&output_path).expect("Should read compiled YAML");

    // The dotnet runtime installs the SDK via UseDotNet@2.
    assert!(
        compiled.contains("UseDotNet@2"),
        "Should include UseDotNet@2 install task"
    );

    // The default (no version specified) should pin to 8.0.x.
    assert!(
        compiled.contains("8.0.x"),
        "Default version should be 8.0.x"
    );

    // The dotnet command should be referenced (e.g. via the bash allow-list
    // or the install step displayName).
    assert!(compiled.contains("dotnet"), "Should include dotnet command");

    // .NET ecosystem domains (e.g. nuget.org) should be in the AWF
    // allow-domains list.
    assert!(
        compiled.contains("nuget.org"),
        "Should include the .NET ecosystem domains in the AWF allow-list"
    );

    // Verify no unreplaced {{ markers }} remain.
    for line in compiled.lines() {
        let stripped = line.replace("${{", "");
        assert!(
            !stripped.contains("{{ "),
            "Compiled output should not contain unreplaced marker: {}",
            line.trim()
        );
    }

    let _ = fs::remove_dir_all(&temp_dir);
}

/// Integration test: `runtimes: dotnet:` with `feed-url:` end-to-end compilation
///
/// Verifies that when `feed-url` is set, the compiler emits both the
/// ensure-nuget.config shim and the `NuGetAuthenticate@1` step in addition
/// to the install task.
#[test]
fn test_dotnet_runtime_with_feed_url_compiled_output() {
    let temp_dir = std::env::temp_dir().join(format!(
        "agentic-pipeline-dotnet-feed-{}",
        std::process::id()
    ));
    fs::create_dir_all(&temp_dir).expect("Failed to create temp directory");

    let input = r#"---
name: "Dotnet Feed Agent"
description: "Agent with .NET runtime + private NuGet feed"
runtimes:
  dotnet:
    version: "8.0.x"
    feed-url: "https://pkgs.dev.azure.com/myorg/_packaging/myfeed/nuget/v3/index.json"
---

## Dotnet Feed Agent
"#;

    let input_path = temp_dir.join("dotnet-feed-agent.md");
    let output_path = temp_dir.join("dotnet-feed-agent.yml");
    fs::write(&input_path, input).expect("Failed to write test input");

    let binary_path = PathBuf::from(env!("CARGO_BIN_EXE_ado-aw"));
    let output = std::process::Command::new(&binary_path)
        .args([
            "compile",
            input_path.to_str().unwrap(),
            "-o",
            output_path.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to run compiler");

    assert!(
        output.status.success(),
        "Compiler should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let compiled = fs::read_to_string(&output_path).expect("Should read compiled YAML");

    assert!(
        compiled.contains("UseDotNet@2"),
        "Should include UseDotNet@2"
    );
    assert!(
        compiled.contains("NuGetAuthenticate@1"),
        "Should include NuGetAuthenticate@1 when feed-url is set"
    );
    assert!(
        compiled.contains("nuget.config"),
        "Should emit ensure-nuget.config step when feed-url is set"
    );

    let _ = fs::remove_dir_all(&temp_dir);
}

/// Integration test: `runtimes: python: true` end-to-end compilation
///
/// Verifies that a pipeline compiled with `runtimes: python: true` contains
/// the `UsePythonVersion@0` task and defaults to Python `3.x`.
#[test]
fn test_python_runtime_compiled_output() {
    let temp_dir =
        std::env::temp_dir().join(format!("agentic-pipeline-python-{}", std::process::id()));
    fs::create_dir_all(&temp_dir).expect("Failed to create temp directory");

    let input = r#"---
name: "Python Agent"
description: "Agent with Python runtime"
runtimes:
  python: true
safe-outputs:
  noop: {}
---

## Python Agent
"#;

    let input_path = temp_dir.join("python-agent.md");
    let output_path = temp_dir.join("python-agent.yml");
    fs::write(&input_path, input).expect("Failed to write test input");

    let binary_path = PathBuf::from(env!("CARGO_BIN_EXE_ado-aw"));
    let output = std::process::Command::new(&binary_path)
        .args([
            "compile",
            input_path.to_str().unwrap(),
            "-o",
            output_path.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to run compiler");

    assert!(
        output.status.success(),
        "Compiler should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let compiled = fs::read_to_string(&output_path).expect("Should read compiled YAML");
    assert!(
        compiled.contains("UsePythonVersion@0"),
        "should have Python install step"
    );
    assert!(
        compiled.contains("versionSpec: 3.x"),
        "should default to Python 3.x"
    );

    let _ = fs::remove_dir_all(&temp_dir);
}

/// Integration test: `runtimes: python:` with pinned version
#[test]
fn test_python_runtime_pinned_version_compiled_output() {
    let temp_dir = std::env::temp_dir().join(format!(
        "agentic-pipeline-python-pinned-{}",
        std::process::id()
    ));
    fs::create_dir_all(&temp_dir).expect("Failed to create temp directory");

    let input = r#"---
name: "Python 3.12 Agent"
description: "Agent with pinned Python runtime"
runtimes:
  python:
    version: "3.12"
safe-outputs:
  noop: {}
---

## Python Agent
"#;

    let input_path = temp_dir.join("python-pinned-agent.md");
    let output_path = temp_dir.join("python-pinned-agent.yml");
    fs::write(&input_path, input).expect("Failed to write test input");

    let binary_path = PathBuf::from(env!("CARGO_BIN_EXE_ado-aw"));
    let output = std::process::Command::new(&binary_path)
        .args([
            "compile",
            input_path.to_str().unwrap(),
            "-o",
            output_path.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to run compiler");

    assert!(
        output.status.success(),
        "Compiler should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let compiled = fs::read_to_string(&output_path).expect("Should read compiled YAML");
    assert!(
        compiled.contains("versionSpec: '3.12'"),
        "should use pinned version"
    );

    let _ = fs::remove_dir_all(&temp_dir);
}

/// Integration test: `runtimes: node: true` end-to-end compilation
#[test]
fn test_node_runtime_compiled_output() {
    let temp_dir =
        std::env::temp_dir().join(format!("agentic-pipeline-node-{}", std::process::id()));
    fs::create_dir_all(&temp_dir).expect("Failed to create temp directory");

    let input = r#"---
name: "Node Agent"
description: "Agent with Node runtime"
runtimes:
  node: true
safe-outputs:
  noop: {}
---

## Node Agent
"#;

    let input_path = temp_dir.join("node-agent.md");
    let output_path = temp_dir.join("node-agent.yml");
    fs::write(&input_path, input).expect("Failed to write test input");

    let binary_path = PathBuf::from(env!("CARGO_BIN_EXE_ado-aw"));
    let output = std::process::Command::new(&binary_path)
        .args([
            "compile",
            input_path.to_str().unwrap(),
            "-o",
            output_path.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to run compiler");

    assert!(
        output.status.success(),
        "Compiler should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let compiled = fs::read_to_string(&output_path).expect("Should read compiled YAML");
    assert!(
        compiled.contains("UseNode@1"),
        "should have Node install step"
    );
    assert!(
        compiled.contains("version: 22.x"),
        "should default to Node 22.x"
    );

    let _ = fs::remove_dir_all(&temp_dir);
}

/// Integration test: `runtimes: python:` with `feed-url:` end-to-end compilation
///
/// Verifies that when `feed-url` is set, the compiler emits `PipAuthenticate@1`
/// and injects `PIP_INDEX_URL` / `UV_DEFAULT_INDEX` env vars into the agent step.
#[test]
fn test_python_runtime_with_feed_url_compiled_output() {
    let temp_dir = std::env::temp_dir().join(format!(
        "agentic-pipeline-python-feed-{}",
        std::process::id()
    ));
    fs::create_dir_all(&temp_dir).expect("Failed to create temp directory");

    let input = r#"---
name: "Python Feed Agent"
description: "Python agent with internal feed"
runtimes:
  python:
    feed-url: "https://pkgs.dev.azure.com/myorg/_packaging/myfeed/pypi/simple/"
safe-outputs:
  noop: {}
---

## Python Feed Agent
"#;

    let input_path = temp_dir.join("python-feed-agent.md");
    let output_path = temp_dir.join("python-feed-agent.yml");
    fs::write(&input_path, input).expect("Failed to write test input");

    let binary_path = PathBuf::from(env!("CARGO_BIN_EXE_ado-aw"));
    let output = std::process::Command::new(&binary_path)
        .args([
            "compile",
            input_path.to_str().unwrap(),
            "-o",
            output_path.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to run compiler");

    assert!(
        output.status.success(),
        "Compiler should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let compiled = fs::read_to_string(&output_path).expect("Should read compiled YAML");

    assert!(
        compiled.contains("PipAuthenticate@1"),
        "Should include PipAuthenticate@1 when feed-url is set"
    );
    assert!(
        compiled.contains("PIP_INDEX_URL"),
        "Should inject PIP_INDEX_URL env var when feed-url is set"
    );
    assert!(
        compiled.contains("UV_DEFAULT_INDEX"),
        "Should inject UV_DEFAULT_INDEX env var when feed-url is set"
    );

    let _ = fs::remove_dir_all(&temp_dir);
}

/// Integration test: `runtimes: node:` with `feed-url:` end-to-end compilation
///
/// Verifies that when `feed-url` is set, the compiler emits `npmAuthenticate@0`
/// and injects `NPM_CONFIG_REGISTRY` env var into the agent step.
#[test]
fn test_node_runtime_with_feed_url_compiled_output() {
    let temp_dir =
        std::env::temp_dir().join(format!("agentic-pipeline-node-feed-{}", std::process::id()));
    fs::create_dir_all(&temp_dir).expect("Failed to create temp directory");

    let input = r#"---
name: "Node Feed Agent"
description: "Node agent with internal npm feed"
runtimes:
  node:
    feed-url: "https://pkgs.dev.azure.com/myorg/_packaging/myfeed/npm/registry/"
safe-outputs:
  noop: {}
---

## Node Feed Agent
"#;

    let input_path = temp_dir.join("node-feed-agent.md");
    let output_path = temp_dir.join("node-feed-agent.yml");
    fs::write(&input_path, input).expect("Failed to write test input");

    let binary_path = PathBuf::from(env!("CARGO_BIN_EXE_ado-aw"));
    let output = std::process::Command::new(&binary_path)
        .args([
            "compile",
            input_path.to_str().unwrap(),
            "-o",
            output_path.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to run compiler");

    assert!(
        output.status.success(),
        "Compiler should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let compiled = fs::read_to_string(&output_path).expect("Should read compiled YAML");

    assert!(
        compiled.contains("npmAuthenticate@0"),
        "Should include npmAuthenticate@0 when feed-url is set"
    );
    assert!(
        compiled.contains("NPM_CONFIG_REGISTRY"),
        "Should inject NPM_CONFIG_REGISTRY env var when feed-url is set"
    );

    let _ = fs::remove_dir_all(&temp_dir);
}

/// Integration test: `schedule:` object form with `branches:` end-to-end compilation
///
/// Verifies that a pipeline compiled with the object-form schedule containing
/// explicit branch filters generates a `branches.include` block in the output.
#[test]
fn test_schedule_object_form_with_branches_compiled_output() {
    let temp_dir = std::env::temp_dir().join(format!(
        "agentic-pipeline-schedule-branches-{}",
        std::process::id()
    ));
    fs::create_dir_all(&temp_dir).expect("Failed to create temp directory");

    let input = r#"---
name: "Scheduled Agent"
description: "Agent with branch-filtered schedule"
on:
  schedule:
    run: daily around 14:00
    branches:
      - main
      - release/*
---

## Scheduled Agent

Run daily on specific branches.
"#;

    let input_path = temp_dir.join("scheduled-agent.md");
    let output_path = temp_dir.join("scheduled-agent.yml");
    fs::write(&input_path, input).expect("Failed to write test input");

    let binary_path = PathBuf::from(env!("CARGO_BIN_EXE_ado-aw"));
    let output = std::process::Command::new(&binary_path)
        .args([
            "compile",
            input_path.to_str().unwrap(),
            "-o",
            output_path.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to run compiler");

    assert!(
        output.status.success(),
        "Compiler should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output_path.exists(), "Compiled YAML should exist");

    let compiled = fs::read_to_string(&output_path).expect("Should read compiled YAML");

    // Should contain a schedules block
    assert!(
        compiled.contains("schedules:"),
        "Compiled output should contain a schedules block"
    );

    // Should contain the branches.include block with both branches
    assert!(
        compiled.contains("branches:"),
        "Compiled output should contain a branches filter"
    );
    assert!(
        compiled.contains("include:"),
        "Compiled output should contain an include list under branches"
    );
    assert!(
        compiled.contains("- main"),
        "Compiled output should include 'main' branch"
    );
    assert!(
        compiled.contains("- release/*"),
        "Compiled output should include 'release/*' branch"
    );

    // Verify no unreplaced {{ markers }} remain
    for line in compiled.lines() {
        let stripped = line.replace("${{", "");
        assert!(
            !stripped.contains("{{ "),
            "Compiled output should not contain unreplaced marker: {}",
            line.trim()
        );
    }

    let _ = fs::remove_dir_all(&temp_dir);
}

/// Test that network.allowed with a bare '*' fails compilation
#[test]
fn test_network_allow_bare_wildcard_fails() {
    let temp_dir = std::env::temp_dir().join(format!(
        "agentic-pipeline-network-bare-wildcard-{}",
        std::process::id()
    ));
    fs::create_dir_all(&temp_dir).expect("Failed to create temp directory");

    let input = r#"---
name: "Network Bare Wildcard Agent"
description: "Agent with bare wildcard in network.allowed"
network:
  allowed:
    - "*"
---

## Test
"#;

    let input_path = temp_dir.join("network-bare-wildcard.md");
    let output_path = temp_dir.join("network-bare-wildcard.yml");
    fs::write(&input_path, input).unwrap();

    let binary_path = PathBuf::from(env!("CARGO_BIN_EXE_ado-aw"));
    let output = std::process::Command::new(&binary_path)
        .args([
            "compile",
            input_path.to_str().unwrap(),
            "-o",
            output_path.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to run compiler");

    assert!(
        !output.status.success(),
        "Compiler should fail for bare wildcard '*'"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unsupported position"),
        "Error message should mention unsupported position: {stderr}"
    );

    let _ = fs::remove_dir_all(&temp_dir);
}

// ─── YAML validation tests ──────────────────────────────────────────────────

const RUNTIME_IMPORT_BODY_SENTINEL: &str = "RUNTIME_IMPORT_BODY_MARKER_DO_NOT_INLINE";
const RUNTIME_IMPORT_SNIPPET_SENTINEL: &str = "RUNTIME_IMPORT_SNIPPET_INLINED_OK";

/// Helper: compile a fixture and return the compiled YAML string.
fn compile_fixture(fixture_name: &str) -> String {
    compile_fixture_with_flags(fixture_name, &[])
}

fn compile_fixture_tree_with_flags<F>(
    fixture_name: &str,
    extra_fixture_paths: &[&str],
    extra_flags: &[&str],
    transform_fixture: F,
) -> String
where
    F: FnOnce(String) -> String,
{
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let unique_id = COUNTER.fetch_add(1, Ordering::Relaxed);

    let temp_dir = std::env::temp_dir().join(format!(
        "agentic-pipeline-yaml-validation-{}-{}-{}",
        fixture_name.replace('.', "-"),
        std::process::id(),
        unique_id,
    ));
    fs::create_dir_all(&temp_dir).expect("Failed to create temp directory");

    let fixtures_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures");
    let fixture_src = fixtures_dir.join(fixture_name);

    // Copy the fixture into the temp dir before compiling. Codemods
    // (e.g. pool_object_form) may rewrite the source on disk; copying
    // keeps the canonical fixture under tests/fixtures pristine and
    // prevents parallel tests that target the same fixture from
    // racing on the lost-update guard in compile.
    let fixture_path = temp_dir.join(fixture_name);
    if let Some(parent) = fixture_path.parent() {
        fs::create_dir_all(parent).expect("Failed to create fixture parent directory in temp dir");
    }
    let fixture_contents = fs::read_to_string(&fixture_src)
        .unwrap_or_else(|e| panic!("Failed to read fixture {fixture_name}: {e}"));
    fs::write(&fixture_path, transform_fixture(fixture_contents))
        .unwrap_or_else(|e| panic!("Failed to write copied fixture {fixture_name}: {e}"));

    for extra_fixture_path in extra_fixture_paths {
        let extra_src = fixtures_dir.join(extra_fixture_path);
        let extra_dst = temp_dir.join(extra_fixture_path);
        if let Some(parent) = extra_dst.parent() {
            fs::create_dir_all(parent).unwrap_or_else(|e| {
                panic!("Failed to create temp dir for {extra_fixture_path}: {e}")
            });
        }
        fs::copy(&extra_src, &extra_dst).unwrap_or_else(|e| {
            panic!("Failed to copy extra fixture {extra_fixture_path} into temp dir: {e}")
        });
    }

    let output_path = temp_dir.join(fixture_name.replace(".md", ".yml"));
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent).expect("Failed to create output parent directory in temp dir");
    }

    let binary_path = PathBuf::from(env!("CARGO_BIN_EXE_ado-aw"));
    let mut args = vec![
        "compile".to_string(),
        fixture_path.to_str().unwrap().to_string(),
        "-o".to_string(),
        output_path.to_str().unwrap().to_string(),
    ];
    for flag in extra_flags {
        args.push(flag.to_string());
    }

    let output = std::process::Command::new(&binary_path)
        .args(&args)
        .output()
        .expect("Failed to run compiler");

    assert!(
        output.status.success(),
        "Compilation of {} with flags {:?} should succeed: {}",
        fixture_name,
        extra_flags,
        String::from_utf8_lossy(&output.stderr)
    );

    let compiled = fs::read_to_string(&output_path).expect("Should read compiled YAML");
    let _ = fs::remove_dir_all(&temp_dir);
    compiled
}

/// Compile a fixture with additional CLI flags (e.g., --skip-integrity, --debug-pipeline).
fn compile_fixture_with_flags(fixture_name: &str, extra_flags: &[&str]) -> String {
    compile_fixture_tree_with_flags(fixture_name, &[], extra_flags, |contents| contents)
}

/// Validate that compiled YAML is parseable as valid YAML.
/// Strips the leading `# @ado-aw` header comment before parsing.
fn assert_valid_yaml(compiled: &str, fixture_name: &str) {
    let yaml_content: String = compiled
        .lines()
        .skip_while(|line| line.starts_with('#') || line.is_empty())
        .collect::<Vec<_>>()
        .join("\n");

    let parsed: Result<serde_yaml::Value, _> = serde_yaml::from_str(&yaml_content);
    assert!(
        parsed.is_ok(),
        "Compiled YAML for {} should be valid YAML, got parse error: {}",
        fixture_name,
        parsed.err().unwrap()
    );

    let doc = parsed.unwrap();
    assert!(
        doc.is_mapping(),
        "Compiled YAML for {} should be a YAML mapping at top level",
        fixture_name
    );
}

fn parse_compiled_yaml(compiled: &str) -> serde_yaml::Value {
    let yaml_content: String = compiled
        .lines()
        .skip_while(|line| line.starts_with('#') || line.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    serde_yaml::from_str(&yaml_content).expect("compiled YAML must parse")
}

fn yaml_key(key: &str) -> serde_yaml::Value {
    serde_yaml::Value::String(key.to_string())
}

fn find_job_mapping<'a>(
    value: &'a serde_yaml::Value,
    job_id: &str,
) -> Option<&'a serde_yaml::Mapping> {
    match value {
        serde_yaml::Value::Mapping(map) => {
            if map.get(yaml_key("job")).and_then(|v| v.as_str()) == Some(job_id) {
                return Some(map);
            }
            map.values()
                .find_map(|child| find_job_mapping(child, job_id))
        }
        serde_yaml::Value::Sequence(items) => items
            .iter()
            .find_map(|child| find_job_mapping(child, job_id)),
        _ => None,
    }
}

fn find_job_mapping_by_display_name<'a>(
    value: &'a serde_yaml::Value,
    display_name: &str,
) -> Option<&'a serde_yaml::Mapping> {
    match value {
        serde_yaml::Value::Mapping(map) => {
            if map.get(yaml_key("displayName")).and_then(|v| v.as_str()) == Some(display_name) {
                return Some(map);
            }
            map.values()
                .find_map(|child| find_job_mapping_by_display_name(child, display_name))
        }
        serde_yaml::Value::Sequence(items) => items
            .iter()
            .find_map(|child| find_job_mapping_by_display_name(child, display_name)),
        _ => None,
    }
}

fn find_bash_step_containing<'a>(
    job: &'a serde_yaml::Mapping,
    needle: &str,
) -> Option<&'a serde_yaml::Mapping> {
    job.get(yaml_key("steps"))
        .and_then(|v| v.as_sequence())
        .and_then(|steps| {
            steps.iter().find_map(|step| {
                let map = step.as_mapping()?;
                let bash = map.get(yaml_key("bash")).and_then(|v| v.as_str())?;
                if bash.contains(needle) {
                    Some(map)
                } else {
                    None
                }
            })
        })
}

fn assert_named_pool_demands(pool: &serde_yaml::Mapping, expected_os: Option<&str>) {
    assert_eq!(
        pool.get(yaml_key("name")).and_then(|v| v.as_str()),
        Some("CustomPool")
    );
    let demands: Vec<&str> = pool
        .get(yaml_key("demands"))
        .and_then(|v| v.as_sequence())
        .expect("pool should contain a demands sequence")
        .iter()
        .map(|v| v.as_str().expect("demand should be a string"))
        .collect();
    assert_eq!(
        demands,
        vec![
            "CustomCapability -equals required-value",
            "Agent.OS -equals Linux"
        ]
    );
    if let Some(os) = expected_os {
        assert_eq!(pool.get(yaml_key("os")).and_then(|v| v.as_str()), Some(os));
    }
}

fn compile_named_pool_demands_fixture_for_target(target: Option<&str>) -> String {
    compile_fixture_tree_with_flags(
        "named-pool-demands-agent.md",
        &[],
        &["--skip-integrity"],
        |contents| {
            if let Some(target) = target {
                contents.replacen(
                    "description: \"Fixture that exercises named pool demands\"\n",
                    &format!(
                        "description: \"Fixture that exercises named pool demands\"\ntarget: {target}\n"
                    ),
                    1,
                )
            } else {
                contents
            }
        },
    )
}

#[test]
fn test_conclusion_job_is_emitted_with_expected_condition_and_dependencies() {
    let compiled = compile_fixture("conclusion_basic.md");
    let doc = parse_compiled_yaml(&compiled);

    let conclusion_job =
        find_job_mapping(&doc, "Conclusion").expect("compiled YAML should contain Conclusion job");

    assert_eq!(
        conclusion_job
            .get(yaml_key("condition"))
            .and_then(|v| v.as_str()),
        Some("and(always(), not(canceled()))"),
        "Conclusion job should run for all outcomes except explicit cancellation"
    );

    let depends_on = conclusion_job
        .get(yaml_key("dependsOn"))
        .and_then(|v| v.as_sequence())
        .expect("Conclusion job should have a dependsOn sequence");
    let deps: Vec<&str> = depends_on
        .iter()
        .map(|v| v.as_str().expect("dependsOn entries should be strings"))
        .collect();

    assert_eq!(
        deps,
        vec!["Agent", "Detection", "SafeOutputs"],
        "Conclusion job should depend on Agent, Detection, and SafeOutputs"
    );
}

#[test]
fn test_conclusion_job_starts_with_checkout_none() {
    let compiled = compile_fixture("conclusion_basic.md");
    let doc = parse_compiled_yaml(&compiled);

    let conclusion_job =
        find_job_mapping(&doc, "Conclusion").expect("compiled YAML should contain Conclusion job");
    let steps = conclusion_job
        .get(yaml_key("steps"))
        .and_then(|v| v.as_sequence())
        .expect("Conclusion job should have steps");
    let first_step = steps
        .first()
        .and_then(|v| v.as_mapping())
        .expect("Conclusion job should have a first step mapping");

    assert_eq!(
        first_step
            .get(yaml_key("checkout"))
            .and_then(|v| v.as_str()),
        Some("none"),
        "Conclusion job should explicitly disable implicit repository checkout before any task"
    );

    assert_eq!(
        steps
            .get(1)
            .and_then(|v| v.as_mapping())
            .and_then(|m| m.get(yaml_key("task")))
            .and_then(|v| v.as_str()),
        Some("UseNode@1"),
        "Conclusion job should install Node after disabling checkout"
    );
    assert!(
        steps.iter().any(|step| {
            let Some(map) = step.as_mapping() else {
                return false;
            };
            map.get(yaml_key("task")).and_then(|v| v.as_str()) == Some("DownloadPipelineArtifact@2")
                && map
                    .get(yaml_key("inputs"))
                    .and_then(|v| v.as_mapping())
                    .and_then(|inputs| inputs.get(yaml_key("artifact")))
                    .and_then(|v| v.as_str())
                    == Some("safe_outputs")
        }),
        "Conclusion job should still download the safe_outputs artifact"
    );
    assert!(
        find_bash_step_containing(conclusion_job, "conclusion.js").is_some(),
        "Conclusion job should still run conclusion.js"
    );
}

#[test]
fn test_conclusion_job_emits_expected_env_vars_for_conclusion_script() {
    let compiled = compile_fixture("conclusion_basic.md");
    let doc = parse_compiled_yaml(&compiled);

    let conclusion_job =
        find_job_mapping(&doc, "Conclusion").expect("compiled YAML should contain Conclusion job");
    let conclusion_step = find_bash_step_containing(conclusion_job, "conclusion.js")
        .expect("Conclusion job should include the conclusion.js bash step");
    let env = conclusion_step
        .get(yaml_key("env"))
        .and_then(|v| v.as_mapping())
        .expect("conclusion.js step should have an env block");

    assert_eq!(
        env.get(yaml_key("AW_REPORT_FAILURE_AS_WORK_ITEM"))
            .and_then(|v| v.as_str()),
        Some("true"),
        "default report-failure-as-work-item should be true"
    );
    assert_eq!(
        env.get(yaml_key("AW_PIPELINE_NAME"))
            .and_then(|v| v.as_str()),
        Some("Conclusion Test Agent")
    );
    assert_eq!(
        env.get(yaml_key("AW_SAFE_OUTPUT_DIR"))
            .and_then(|v| v.as_str()),
        Some("$(Pipeline.Workspace)/conclusion_inputs")
    );
    assert!(
        env.contains_key(yaml_key("SYSTEM_ACCESSTOKEN")),
        "conclusion.js step should include SYSTEM_ACCESSTOKEN"
    );
    assert_eq!(
        env.get(yaml_key("AW_NOOP_TITLE_PREFIX"))
            .and_then(|v| v.as_str()),
        Some("[ado-aw] Agent noop")
    );
    assert_eq!(
        env.get(yaml_key("AW_NOOP_AREA_PATH"))
            .and_then(|v| v.as_str()),
        Some(r#"TestProject\TestTeam"#)
    );

    // The Conclusion job must never fail the pipeline (it reports OTHER jobs'
    // failures), so the conclusion.js step is marked continueOnError: true.
    assert_eq!(
        conclusion_step
            .get(yaml_key("continueOnError"))
            .and_then(|v| v.as_bool()),
        Some(true),
        "conclusion.js step should set continueOnError so a non-zero node exit never fails the pipeline"
    );
}

#[test]
fn test_conclusion_job_is_not_emitted_without_safe_outputs() {
    let compiled = compile_fixture("minimal-agent.md");
    let doc = parse_compiled_yaml(&compiled);

    assert!(
        find_job_mapping(&doc, "Conclusion").is_none(),
        "pipelines without safe-outputs must not emit a Conclusion job"
    );
}

/// Assert that no step's `env:` block contains a `$[ ... ]` ADO runtime
/// expression. ADO ONLY evaluates `$[ ... ]` inside `variables:` mappings
/// and `condition:` fields — putting one in step `env:` passes the
/// literal expression string verbatim to the step. This caused
/// msazuresphere/4x4 build #612528 where downstream PR-identifier
/// validation rejected a `PR_ID='$[ coalesce(variables['System.Pull…'`
/// literal as "not a positive integer".
///
/// Walks every step under every job (`extends.parameters.stages[*].jobs[*]`,
/// `jobs[*]`, `stages[*].jobs[*]`) and inspects the `env:` map at the
/// step level. Job-level `variables:` and `condition:` are correctly
/// skipped — those are the legitimate places for `$[ ... ]`.
fn assert_no_dollar_bracket_in_step_env(compiled: &str) {
    let yaml_content: String = compiled
        .lines()
        .skip_while(|line| line.starts_with('#') || line.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    let doc: serde_yaml::Value =
        serde_yaml::from_str(&yaml_content).expect("compiled YAML must parse");

    let mut findings: Vec<String> = Vec::new();
    walk_steps(&doc, "$", &mut |step, path| {
        let env = step
            .as_mapping()
            .and_then(|m| m.get(serde_yaml::Value::String("env".into())))
            .and_then(|v| v.as_mapping());
        let Some(env) = env else {
            return;
        };
        for (k, v) in env {
            let value_str = match v {
                serde_yaml::Value::String(s) => s.clone(),
                _ => continue,
            };
            if value_str.contains("$[") {
                let key_str = k.as_str().unwrap_or("<non-string>");
                let display = step
                    .as_mapping()
                    .and_then(|m| m.get(serde_yaml::Value::String("displayName".into())))
                    .and_then(|v| v.as_str())
                    .unwrap_or("<no displayName>");
                findings.push(format!(
                    "  at {path} (step '{display}'): env.{key_str} = {value_str:?}"
                ));
            }
        }
    });
    assert!(
        findings.is_empty(),
        "Found `$[ ... ]` ADO runtime expressions inside step env blocks. \
         ADO only evaluates these in `variables:` mappings and `condition:` \
         fields, NOT in step env values — the literal expression string is \
         passed verbatim to the step (msazuresphere/4x4 build #612528). \
         Use a job-level `variables:` hoist + `$(name)` macro instead.\n{}",
        findings.join("\n")
    );
}

/// Helper: walk every step in the YAML document, invoking `f` with each
/// step mapping and a slash-delimited path describing where it was found.
fn walk_steps<F: FnMut(&serde_yaml::Value, &str)>(doc: &serde_yaml::Value, path: &str, f: &mut F) {
    use serde_yaml::Value;
    match doc {
        Value::Mapping(m) => {
            // If this is a job with `steps:`, visit each step.
            if let Some(Value::Sequence(steps)) = m.get(Value::String("steps".into())) {
                for (i, step) in steps.iter().enumerate() {
                    let step_path = format!("{path}/steps[{i}]");
                    f(step, &step_path);
                }
            }
            // Recurse into common containers that might hold further jobs.
            for (k, v) in m {
                let key = k.as_str().unwrap_or("?");
                let child_path = format!("{path}/{key}");
                walk_steps(v, &child_path, f);
            }
        }
        Value::Sequence(s) => {
            for (i, v) in s.iter().enumerate() {
                let child_path = format!("{path}[{i}]");
                walk_steps(v, &child_path, f);
            }
        }
        _ => {}
    }
}

// ─── ado-aw marker step (always-on extension) ───────────────────────────────

/// Assert that compiled YAML carries exactly one `# ado-aw-metadata: {…}`
/// marker line whose JSON includes the expected source path and target.
///
/// The marker step is injected by the always-on `ado-aw-marker` compiler
/// extension and is the canonical surface project-scope discovery uses
/// to find ado-aw pipelines in expanded YAML (see `src/detect.rs::parse_marker_step`).
fn assert_marker_step_present(
    compiled: &str,
    expected_source_suffix: &str,
    expected_target: &str,
    fixture_name: &str,
) {
    let marker_lines: Vec<&str> = compiled
        .lines()
        .filter(|l| l.trim_start().starts_with("# ado-aw-metadata:"))
        .collect();
    assert_eq!(
        marker_lines.len(),
        1,
        "{fixture_name}: expected exactly one '# ado-aw-metadata:' marker in compiled YAML, found {}\nLines: {:#?}",
        marker_lines.len(),
        marker_lines,
    );
    let line = marker_lines[0];
    assert!(
        line.contains(&format!("\"target\":\"{expected_target}\"")),
        "{fixture_name}: marker line does not declare target={expected_target}: {line}"
    );
    assert!(
        line.contains("\"schema\":1"),
        "{fixture_name}: marker line missing schema=1: {line}"
    );
    assert!(
        line.contains("\"source\":\"") && line.contains(expected_source_suffix),
        "{fixture_name}: marker line does not include source suffix {expected_source_suffix}: {line}"
    );
    // The runtime echo on the next line should mirror the same data
    // (this is the human-facing build-log surface).
    assert!(
        compiled.contains("ado-aw metadata: source="),
        "{fixture_name}: compiled YAML missing runtime echo line for ado-aw marker"
    );
    // displayName: ado-aw identifies the injected step uniquely.
    assert!(
        compiled.contains("displayName: ado-aw"),
        "{fixture_name}: compiled YAML missing displayName: ado-aw on injected step"
    );
}

fn assert_aw_info_step_present(
    compiled: &str,
    expected_source_suffix: &str,
    expected_target: &str,
    expected_agent_name: &str,
    fixture_name: &str,
) {
    assert!(
        compiled.contains("displayName: Emit aw_info.json"),
        "{fixture_name}: compiled YAML missing Emit aw_info.json step"
    );
    assert!(
        compiled.contains("condition: always()"),
        "{fixture_name}: compiled YAML missing always() condition on aw_info step"
    );
    assert!(
        compiled.contains("cat >\"$(Agent.TempDirectory)/staging/aw_info.json\" <<'AW_INFO_EOF'"),
        "{fixture_name}: compiled YAML missing quoted heredoc aw_info write step"
    );
    // Softer suffix check on the source path: fixtures compile under
    // a temp-dir prefix, so we can only assert the path ends with the
    // expected suffix, not an exact match. Mirrors `assert_marker_step_present`.
    assert!(
        compiled.contains("\"source\":\"") && compiled.contains(expected_source_suffix),
        "{fixture_name}: compiled YAML aw_info source does not include suffix {expected_source_suffix}"
    );
    for expected_fragment in [
        "\"schema\":\"ado-aw/aw_info/1\"".to_string(),
        format!("\"target\":\"{expected_target}\""),
        "\"engine\":\"copilot\"".to_string(),
        "\"model\":\"claude-opus-4.7\"".to_string(),
        format!("\"agent_name\":\"{expected_agent_name}\""),
        "\"build_id\":\"$(Build.BuildId)\"".to_string(),
        "\"source_version\":\"$(Build.SourceVersion)\"".to_string(),
        "\"source_branch\":\"$(Build.SourceBranch)\"".to_string(),
        "\"build_definition_id\":\"$(System.DefinitionId)\"".to_string(),
    ] {
        assert!(
            compiled.contains(&expected_fragment),
            "{fixture_name}: compiled YAML missing aw_info fragment {expected_fragment}"
        );
    }
}

fn compile_fixture_with_inlined_imports(fixture_name: &str) -> String {
    compile_fixture_tree_with_flags(fixture_name, &[], &[], |contents| {
        // If the fixture already declares `inlined-imports:` (either
        // value), don't inject a second key. serde_yaml silently uses the
        // last key on duplicates, so the test would still pass — but the
        // rewritten fixture would have a confusing duplicate and a
        // future fixture that hard-codes `inlined-imports: false` would
        // silently get flipped to `true` by this helper. Detect line-
        // starting `inlined-imports:` so we don't false-positive on the
        // string appearing inside body content.
        let already_present = contents.lines().any(|line| {
            let trimmed = line.trim_start();
            trimmed.starts_with("inlined-imports:")
        });
        if already_present {
            panic!(
                "Fixture {fixture_name} already declares `inlined-imports:`; \
                 `compile_fixture_with_inlined_imports` would produce a duplicate key. \
                 Use `compile_fixture` directly, or remove the existing key from the fixture."
            );
        }
        if let Some((front_matter, body)) = contents.split_once("\r\n---\r\n") {
            format!("{front_matter}\r\ninlined-imports: true\r\n---\r\n{body}")
        } else if let Some((front_matter, body)) = contents.split_once("\n---\n") {
            format!("{front_matter}\ninlined-imports: true\n---\n{body}")
        } else {
            panic!("Fixture {fixture_name} should contain a closing front matter delimiter");
        }
    })
}

fn assert_runtime_imports_default_output(fixture_name: &str) {
    let compiled = compile_fixture(fixture_name);

    // Exactly one runtime-import marker (agent body) — the threat-analysis
    // prompt is tooling-shipped and always inlined, so it never carries a
    // marker.
    assert_eq!(
        compiled.matches("{{#runtime-import ").count(),
        1,
        "Compiled YAML for {fixture_name} should contain exactly one runtime-import marker (agent body)"
    );
    assert!(
        compiled.contains("Resolve runtime imports (agent prompt)"),
        "Compiled YAML for {fixture_name} should resolve agent prompt imports"
    );
    assert!(
        !compiled.contains("Resolve runtime imports (threat"),
        "Compiled YAML for {fixture_name} should NOT emit a threat-prompt resolver step (threat is always inlined)"
    );
    assert!(
        compiled.contains("Download ado-aw scripts"),
        "Compiled YAML for {fixture_name} should download shared ado-aw scripts"
    );
    assert!(
        !compiled.contains(RUNTIME_IMPORT_BODY_SENTINEL),
        "Compiled YAML for {fixture_name} should not inline the markdown body in default mode"
    );
}

fn assert_runtime_imports_inlined_output(fixture_name: &str) {
    let compiled = compile_fixture_with_inlined_imports(fixture_name);

    assert!(
        compiled.contains(RUNTIME_IMPORT_BODY_SENTINEL),
        "Compiled YAML for {fixture_name} should inline the markdown body when inlined-imports is true"
    );
    assert!(
        !compiled.contains("{{#runtime-import "),
        "Compiled YAML for {fixture_name} should not contain runtime-import markers when inlined-imports is true"
    );
    assert!(
        !compiled.contains("Resolve runtime imports"),
        "Compiled YAML for {fixture_name} should not emit runtime import resolver steps when inlined-imports is true"
    );
}

fn assert_runtime_imports_author_marker_output(fixture_name: &str) {
    let compiled =
        compile_fixture_tree_with_flags(fixture_name, &["shared/snippet.md"], &[], |contents| {
            contents
        });

    assert!(
        compiled.contains(RUNTIME_IMPORT_SNIPPET_SENTINEL),
        "Compiled YAML for {fixture_name} should inline author-written runtime imports"
    );
    assert!(
        !compiled.contains("{{#runtime-import shared/snippet.md}}"),
        "Compiled YAML for {fixture_name} should not retain the author-written runtime-import marker"
    );
}

#[test]
fn test_marker_step_present_in_standalone_target() {
    let compiled = compile_fixture("minimal-agent.md");
    assert_marker_step_present(
        &compiled,
        "minimal-agent.md",
        "standalone",
        "minimal-agent.md",
    );
    assert_aw_info_step_present(
        &compiled,
        "minimal-agent.md",
        "standalone",
        "Minimal Test Agent",
        "minimal-agent.md",
    );
}

#[test]
fn test_marker_step_present_in_1es_target() {
    let compiled = compile_fixture("1es-test-agent.md");
    assert_marker_step_present(&compiled, "1es-test-agent.md", "1es", "1es-test-agent.md");
    assert_aw_info_step_present(
        &compiled,
        "1es-test-agent.md",
        "1es",
        "1ES Test Agent",
        "1es-test-agent.md",
    );
}

#[test]
fn test_marker_step_present_in_job_target() {
    let compiled = compile_fixture("job-agent.md");
    assert_marker_step_present(&compiled, "job-agent.md", "job", "job-agent.md");
    assert_aw_info_step_present(
        &compiled,
        "job-agent.md",
        "job",
        "Job Test Agent",
        "job-agent.md",
    );
}

#[test]
fn test_marker_step_present_in_stage_target() {
    let compiled = compile_fixture("stage-agent.md");
    assert_marker_step_present(&compiled, "stage-agent.md", "stage", "stage-agent.md");
    assert_aw_info_step_present(
        &compiled,
        "stage-agent.md",
        "stage",
        "Stage Test Agent",
        "stage-agent.md",
    );
}

/// Regression: the always-on `ado-aw-marker` extension used to inject
/// the marker step via `setup_steps`, which forced every compiled
/// pipeline to spawn a dedicated Setup job (a whole pool agent + the
/// extra build-log noise) just to emit a single metadata comment.
/// After moving emission to `prepare_steps`, the marker lives inside
/// the always-present Agent job — a minimal fixture without `setup:`,
/// PR filters, or other extensions that need Setup must NOT emit a
/// `- job: Setup` block.
#[test]
fn test_marker_does_not_create_setup_job_for_minimal_pipeline() {
    let compiled = compile_fixture("minimal-agent.md");
    assert!(
        !compiled.contains("- job: Setup"),
        "minimal pipeline must not emit a Setup job just for the ado-aw marker; got:\n{compiled}"
    );
    // Still must carry the marker — just inside the Agent job now.
    assert!(
        compiled.contains("# ado-aw-metadata:"),
        "minimal pipeline must still carry the marker line:\n{compiled}"
    );
}

#[test]
fn test_standalone_runtime_imports_default_emits_marker_and_resolver() {
    assert_runtime_imports_default_output("runtime_imports_standalone.md");
}

#[test]
fn test_standalone_inlined_imports_true_inlines_body() {
    assert_runtime_imports_inlined_output("runtime_imports_standalone.md");
}

#[test]
fn test_standalone_inlined_imports_true_resolves_author_markers() {
    assert_runtime_imports_author_marker_output("runtime_imports_author_marker_standalone.md");
}

#[test]
fn test_1es_runtime_imports_default_emits_marker_and_resolver() {
    assert_runtime_imports_default_output("runtime_imports_1es.md");
}

#[test]
fn test_1es_inlined_imports_true_inlines_body() {
    assert_runtime_imports_inlined_output("runtime_imports_1es.md");
}

#[test]
fn test_1es_inlined_imports_true_resolves_author_markers() {
    assert_runtime_imports_author_marker_output("runtime_imports_author_marker_1es.md");
}

#[test]
fn test_job_runtime_imports_default_emits_marker_and_resolver() {
    assert_runtime_imports_default_output("runtime_imports_job.md");
}

#[test]
fn test_job_inlined_imports_true_inlines_body() {
    assert_runtime_imports_inlined_output("runtime_imports_job.md");
}

#[test]
fn test_job_inlined_imports_true_resolves_author_markers() {
    assert_runtime_imports_author_marker_output("runtime_imports_author_marker_job.md");
}

#[test]
fn test_stage_runtime_imports_default_emits_marker_and_resolver() {
    assert_runtime_imports_default_output("runtime_imports_stage.md");
}

#[test]
fn test_stage_inlined_imports_true_inlines_body() {
    assert_runtime_imports_inlined_output("runtime_imports_stage.md");
}

#[test]
fn test_stage_inlined_imports_true_resolves_author_markers() {
    assert_runtime_imports_author_marker_output("runtime_imports_author_marker_stage.md");
}

/// Compile a default-mode (inlined-imports: false) agent whose source path
/// contains a space. The runtime resolver matches marker bodies with
/// `[^\s}]+`, so a space would silently truncate the marker at runtime and
/// surface a confusing "file not found" error (or, for optional markers,
/// leave the marker unexpanded). Reject at compile time so the failure is
/// a clear, actionable compile error rather than a runtime data-integrity
/// bug.
#[test]
fn test_runtime_imports_default_rejects_source_path_with_whitespace() {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let unique_id = COUNTER.fetch_add(1, Ordering::Relaxed);

    // Use a top-level temp dir (NOT under the repo) so the compiler can't
    // discover a git root and rebase the path on it.
    let temp_dir = std::env::temp_dir().join(format!(
        "agentic-pipeline-spaced-path-{}-{}",
        std::process::id(),
        unique_id,
    ));
    let spaced_dir = temp_dir.join("my agents");
    fs::create_dir_all(&spaced_dir).expect("Failed to create spaced temp dir");
    // generate_source_path falls back to the filename only when it can't
    // locate a git root above the input path — which would hide the space
    // from the marker. Create an empty `.git` marker so the spaced dir is
    // resolved relative to a discoverable repo root and the space ends up
    // in the runtime-import marker (i.e. exercises the new guard).
    fs::create_dir_all(temp_dir.join(".git")).expect("Failed to create .git marker");

    let input = "---\nname: \"Spaced Path Agent\"\ndescription: \"Agent whose source path contains a space\"\n---\n\n## Body\n\nhello\n";
    let input_path = spaced_dir.join("pipeline.md");
    let output_path = spaced_dir.join("pipeline.yml");
    fs::write(&input_path, input).unwrap();

    let binary_path = PathBuf::from(env!("CARGO_BIN_EXE_ado-aw"));
    let output = std::process::Command::new(&binary_path)
        .args([
            "compile",
            input_path.to_str().unwrap(),
            "-o",
            output_path.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to run compiler");

    assert!(
        !output.status.success(),
        "Compiler should fail when source path contains whitespace and inlined-imports is false"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("contains whitespace"),
        "Error message should mention whitespace: {stderr}"
    );
    assert!(
        stderr.contains("inlined-imports: true"),
        "Error message should suggest inlined-imports as an escape hatch: {stderr}"
    );

    let _ = fs::remove_dir_all(&temp_dir);
}

/// Sibling regression of the whitespace guard: the same threat model
/// applies to `}` in the source path. The runtime regex `[^\s}]+`
/// stops at the first `}` and then expects `\s*\}\}`, so a marker
/// emitted with `}` in its path silently fails to match — the marker
/// survives as literal text in the LLM's prompt. Reject at compile
/// time, matching the same `}` guard in `resolve_imports_inline`.
#[test]
fn test_runtime_imports_default_rejects_source_path_with_closing_brace() {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let unique_id = COUNTER.fetch_add(1, Ordering::Relaxed);

    let temp_dir = std::env::temp_dir().join(format!(
        "agentic-pipeline-brace-path-{}-{}",
        std::process::id(),
        unique_id,
    ));
    // Filename contains `}` which is valid on Linux/macOS/NTFS but
    // forbidden in shell-injected contexts. The whole point of the
    // guard is to reject the marker before any such surface is hit.
    let agent_dir = temp_dir.join("agents");
    fs::create_dir_all(&agent_dir).expect("Failed to create temp dir tree");
    fs::create_dir_all(temp_dir.join(".git")).expect("Failed to create .git marker");

    let input = "---\nname: \"Brace Path Agent\"\ndescription: \"Agent whose source path contains '}'\"\n---\n\n## Body\n\nhello\n";
    let input_path = agent_dir.join("fo}o.md");
    let output_path = agent_dir.join("foo.yml");
    fs::write(&input_path, input).unwrap();

    let binary_path = PathBuf::from(env!("CARGO_BIN_EXE_ado-aw"));
    let output = std::process::Command::new(&binary_path)
        .args([
            "compile",
            input_path.to_str().unwrap(),
            "-o",
            output_path.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to run compiler");

    assert!(
        !output.status.success(),
        "Compiler should fail when source path contains '}}' and inlined-imports is false"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("contains '}'"),
        "Error message should mention the `}}` character: {stderr}"
    );
    assert!(
        stderr.contains("inlined-imports: true"),
        "Error message should suggest inlined-imports as an escape hatch: {stderr}"
    );

    let _ = fs::remove_dir_all(&temp_dir);
}
#[test]
fn test_1es_compiled_output_is_valid_yaml() {
    let compiled = compile_fixture("1es-test-agent.md");
    assert_valid_yaml(&compiled, "1es-test-agent.md");

    let yaml_content: String = compiled
        .lines()
        .skip_while(|line| line.starts_with('#') || line.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    let doc: serde_yaml::Value = serde_yaml::from_str(&yaml_content).unwrap();

    // Verify 1ES wrapping structure
    assert!(
        doc.get("extends").is_some(),
        "1ES YAML should have 'extends' key"
    );
    assert!(
        doc.get("resources").is_some(),
        "1ES YAML should have 'resources' key"
    );

    // Verify key pipeline content was substituted (catches placeholder regressions)
    assert!(
        compiled.contains("Copilot.CLI.linux-x64"),
        "1ES output should contain Copilot CLI install"
    );
    assert!(
        compiled.contains("awf"),
        "1ES output should contain AWF references"
    );
    assert!(
        compiled.contains("mcpg"),
        "1ES output should contain MCPG references"
    );
    assert!(
        compiled.contains("SafeOutputs"),
        "1ES output should contain SafeOutputs references"
    );
    assert!(
        compiled.contains("copilot --prompt"),
        "1ES output should contain copilot invocation (engine_run substituted)"
    );
    assert!(
        compiled.contains("threat-analysis"),
        "1ES output should contain threat analysis step"
    );
    assert!(
        compiled.contains("ado-aw execute"),
        "1ES output should contain safe output executor step"
    );
    assert!(
        compiled.contains("job: Agent"),
        "1ES output should contain Agent job"
    );
    assert!(
        compiled.contains("job: Detection"),
        "1ES output should contain Detection job"
    );
    assert!(
        compiled.contains("job: SafeOutputs"),
        "1ES output should contain SafeOutputs job"
    );

    // Verify no Agency remnants
    assert!(
        !compiled.contains("agencyJob"),
        "1ES output should not contain agencyJob"
    );
    assert!(
        !compiled.contains("AgencyArtifact"),
        "1ES output should not contain AgencyArtifact"
    );
    assert!(
        !compiled.contains("commandOptions"),
        "1ES output should not contain commandOptions"
    );
}

/// Names with embedded `"` and `:` must survive YAML escaping in both
/// the top-level `name:` line and any `displayName:` positions.
///
/// Regression: until `{{ pipeline_agent_name }}` was introduced both positions
/// used a bare `{{ agent_name }}` substitution which broke if the
/// front-matter name contained colons (`name: a: b` parsed as a YAML
/// mapping) or embedded double quotes (`displayName: "a "b" c"` parsed
/// as broken scalars). Now both positions go through `yaml_double_quoted`
/// via a dedicated pipeline name marker.
#[test]
fn test_compiled_yaml_survives_tricky_agent_name_standalone() {
    let compiled = compile_fixture("tricky-name-agent.md");
    assert_valid_yaml(&compiled, "tricky-name-agent.md");

    // Build-number names must strip invalid characters such as `"` and `:`.
    assert!(
        compiled.contains("name: My special agent with quotes-$(BuildID)"),
        "standalone output should contain sanitized pipeline name; got:\n{compiled}"
    );
}

#[test]
fn test_compiled_yaml_survives_tricky_agent_name_1es() {
    let compiled = compile_fixture("tricky-name-1es-agent.md");
    assert_valid_yaml(&compiled, "tricky-name-1es-agent.md");

    // Top-level pipeline name carries the `-$(BuildID)` suffix because
    // the ADO build-number format needs a varying token; the stage
    // displayName does NOT carry the suffix (stage labels are static).
    assert!(
        compiled.contains("name: My special agent with quotes (1ES)-$(BuildID)"),
        "1ES output should contain sanitized pipeline name; got:\n{compiled}"
    );
    // serde_yaml's prep-PR normalisation chooses single-quoted style for
    // scalars containing both `"` and `:` (avoids needing backslash
    // escapes). The string content is identical to the pre-prep form.
    assert!(
        compiled.contains(r#"displayName: 'My "special": agent with quotes (1ES)'"#),
        "1ES output should contain escaped stage displayName; got:\n{compiled}"
    );
}

/// Test that the minimal standalone fixture produces valid YAML with correct structure
#[test]
fn test_standalone_minimal_compiled_output_is_valid_yaml() {
    let compiled = compile_fixture("minimal-agent.md");
    assert_valid_yaml(&compiled, "minimal-agent.md");

    let yaml_content: String = compiled
        .lines()
        .skip_while(|line| line.starts_with('#') || line.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    let doc: serde_yaml::Value = serde_yaml::from_str(&yaml_content).unwrap();

    assert!(
        doc.get("jobs").is_some(),
        "Standalone YAML should have 'jobs' key"
    );
}

#[test]
fn test_named_pool_demands_compile_for_standalone_job_and_stage_targets() {
    for (target, label) in [
        (None, "standalone"),
        (Some("job"), "target: job"),
        (Some("stage"), "target: stage"),
    ] {
        let compiled = compile_named_pool_demands_fixture_for_target(target);
        assert_valid_yaml(&compiled, label);
        let doc = parse_compiled_yaml(&compiled);
        let agent = find_job_mapping_by_display_name(&doc, "Agent")
            .unwrap_or_else(|| panic!("{label}: missing Agent job"));
        let pool = agent
            .get(yaml_key("pool"))
            .and_then(|v| v.as_mapping())
            .unwrap_or_else(|| panic!("{label}: Agent job should contain pool mapping"));
        assert_named_pool_demands(pool, None);
    }
}

#[test]
fn test_named_pool_demands_compile_to_1es_shared_pool() {
    let compiled = compile_named_pool_demands_fixture_for_target(Some("1es"));
    assert_valid_yaml(&compiled, "target: 1es");
    let doc = parse_compiled_yaml(&compiled);

    let pool = doc
        .get(yaml_key("extends"))
        .and_then(|v| v.as_mapping())
        .and_then(|m| m.get(yaml_key("parameters")))
        .and_then(|v| v.as_mapping())
        .and_then(|m| m.get(yaml_key("pool")))
        .and_then(|v| v.as_mapping())
        .expect("1ES output should contain extends.parameters.pool mapping");
    assert_named_pool_demands(pool, Some("linux"));

    let agent = find_job_mapping(&doc, "Agent").expect("1ES output should contain Agent job");
    assert!(
        agent.get(yaml_key("pool")).is_none(),
        "1ES jobs should inherit extends.parameters.pool rather than emitting per-job pool"
    );
}

#[test]
fn test_1es_pool_demands_require_named_pool() {
    let temp_dir = std::env::temp_dir().join(format!(
        "agentic-pipeline-1es-default-demands-{}",
        std::process::id()
    ));
    fs::create_dir_all(&temp_dir).expect("Failed to create temp directory");

    let test_input = temp_dir.join("invalid-1es-demands.md");
    fs::write(
        &test_input,
        r#"---
name: "Invalid 1ES Demands"
description: "1ES demands without an explicit named pool"
target: 1es
pool:
  demands:
    - CustomCapability -equals required-value
---

## Invalid 1ES Demands
"#,
    )
    .expect("Failed to write test input");

    let output_path = temp_dir.join("invalid-1es-demands.yml");
    let binary_path = PathBuf::from(env!("CARGO_BIN_EXE_ado-aw"));
    let output = std::process::Command::new(&binary_path)
        .args([
            "compile",
            test_input.to_str().unwrap(),
            "-o",
            output_path.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to run compiler");

    assert!(
        !output.status.success(),
        "Compiler should fail when 1ES pool.demands omits pool.name"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("pool.demands requires `pool.name`") && stderr.contains("default 1ES pool"),
        "Error message should mention the missing 1ES named pool: {stderr}"
    );

    fs::remove_dir_all(&temp_dir).unwrap_or_else(|e| {
        panic!(
            "Failed to remove temp directory {}: {e}",
            temp_dir.display()
        )
    });
}

/// Test that the complete standalone fixture emits Setup/Teardown jobs and
/// that the agentic task waits on Setup. The fixture has `setup:`,
/// `teardown:`, and `post-steps:` sections so all three should appear.
#[test]
fn test_standalone_complete_agent_has_setup_and_teardown_jobs() {
    let compiled = compile_fixture("complete-agent.md");
    assert_valid_yaml(&compiled, "complete-agent.md");
    assert!(
        compiled.contains("- job: Setup"),
        "Should generate Setup job: {compiled}"
    );
    assert!(
        compiled.contains("- job: Teardown"),
        "Should generate Teardown job"
    );
    assert!(
        compiled.contains("dependsOn: Setup"),
        "Agentic task should depend on Setup job"
    );
    assert!(
        compiled.contains("echo \"Setup step\"") || compiled.contains("echo 'Setup step'"),
        "Should include setup step content"
    );
    assert!(
        compiled.contains("echo \"Teardown step\"") || compiled.contains("echo 'Teardown step'"),
        "Should include teardown step content"
    );
}

/// Test that the pipeline-trigger fixture produces valid YAML
#[test]
fn test_standalone_pipeline_trigger_compiled_output_is_valid_yaml() {
    let compiled = compile_fixture("pipeline-trigger-agent.md");
    assert_valid_yaml(&compiled, "pipeline-trigger-agent.md");
}

// ─── --skip-integrity flag tests ─────────────────────────────────────────

/// Test that --skip-integrity omits the integrity check step from the pipeline
#[test]
fn test_skip_integrity_omits_integrity_step() {
    let compiled = compile_fixture_with_flags("minimal-agent.md", &["--skip-integrity"]);
    assert_valid_yaml(&compiled, "minimal-agent.md (skip-integrity)");

    assert!(
        !compiled.contains("Verify pipeline integrity"),
        "Pipeline compiled with --skip-integrity should NOT contain the integrity check step"
    );
}

/// Test that without --skip-integrity, the integrity check step IS present
#[test]
fn test_default_includes_integrity_step() {
    let compiled = compile_fixture("minimal-agent.md");

    assert!(
        compiled.contains("Verify pipeline integrity"),
        "Pipeline compiled without --skip-integrity should contain the integrity check step"
    );
    assert!(
        compiled.contains("agentic-pipeline-compiler/ado-aw"),
        "Pipeline compiled without --skip-integrity should reference the ado-aw binary"
    );
}

/// Test that --skip-integrity produces valid YAML for both standalone and 1ES
#[test]
fn test_skip_integrity_valid_yaml_standalone() {
    let compiled = compile_fixture_with_flags("complete-agent.md", &["--skip-integrity"]);
    assert_valid_yaml(&compiled, "complete-agent.md (skip-integrity)");
}

#[test]
fn test_skip_integrity_valid_yaml_1es() {
    let compiled = compile_fixture_with_flags("1es-test-agent.md", &["--skip-integrity"]);
    assert_valid_yaml(&compiled, "1es-test-agent.md (skip-integrity)");
}

// ─── --debug-pipeline flag tests ─────────────────────────────────────────

/// Test that --debug-pipeline includes MCPG debug diagnostics
#[test]
fn test_debug_pipeline_includes_debug_env() {
    let compiled = compile_fixture_with_flags("minimal-agent.md", &["--debug-pipeline"]);
    assert_valid_yaml(&compiled, "minimal-agent.md (debug-pipeline)");

    assert!(
        compiled.contains(r#"DEBUG="*""#),
        "Pipeline compiled with --debug-pipeline should contain DEBUG=* env var"
    );
}

/// Test that --debug-pipeline includes the MCP backend probe step
#[test]
fn test_debug_pipeline_includes_probe_step() {
    let compiled = compile_fixture_with_flags("minimal-agent.md", &["--debug-pipeline"]);

    assert!(
        compiled.contains("Verify MCP backends"),
        "Pipeline compiled with --debug-pipeline should contain probe step displayName"
    );
    assert!(
        compiled.contains("tools/list"),
        "Pipeline compiled with --debug-pipeline should contain tools/list probe"
    );
    assert!(
        compiled.contains("initialize"),
        "Pipeline compiled with --debug-pipeline should contain initialize handshake"
    );
    assert!(
        compiled.contains("MCPG_API_KEY: $(MCP_GATEWAY_API_KEY)"),
        "Pipeline compiled with --debug-pipeline should map MCPG_API_KEY env"
    );
}

/// Test that without --debug-pipeline, debug diagnostics are NOT present
#[test]
fn test_default_excludes_debug_diagnostics() {
    let compiled = compile_fixture("minimal-agent.md");

    assert!(
        !compiled.contains(r#"DEBUG="*""#),
        "Pipeline compiled without --debug-pipeline should NOT contain DEBUG=* env var"
    );
    assert!(
        !compiled.contains("Verify MCP backends"),
        "Pipeline compiled without --debug-pipeline should NOT contain probe step"
    );
}

/// Test that --debug-pipeline produces valid YAML for both targets
#[test]
fn test_debug_pipeline_valid_yaml_standalone() {
    let compiled = compile_fixture_with_flags("complete-agent.md", &["--debug-pipeline"]);
    assert_valid_yaml(&compiled, "complete-agent.md (debug-pipeline)");
}

#[test]
fn test_debug_pipeline_valid_yaml_1es() {
    let compiled = compile_fixture_with_flags("1es-test-agent.md", &["--debug-pipeline"]);
    assert_valid_yaml(&compiled, "1es-test-agent.md (debug-pipeline)");
}

/// Test that both flags can be combined
#[test]
fn test_skip_integrity_and_debug_pipeline_combined() {
    let compiled = compile_fixture_with_flags(
        "minimal-agent.md",
        &["--skip-integrity", "--debug-pipeline"],
    );
    assert_valid_yaml(
        &compiled,
        "minimal-agent.md (skip-integrity + debug-pipeline)",
    );

    // Debug content present
    assert!(
        compiled.contains(r#"DEBUG="*""#),
        "Combined flags: should contain DEBUG=*"
    );
    assert!(
        compiled.contains("Verify MCP backends"),
        "Combined flags: should contain probe step"
    );

    // Integrity content absent
    assert!(
        !compiled.contains("Verify pipeline integrity"),
        "Combined flags: should NOT contain integrity check"
    );
}

/// Test that debug probe step has no unresolved template markers
#[test]
fn test_debug_pipeline_no_unresolved_markers() {
    let compiled = compile_fixture_with_flags("minimal-agent.md", &["--debug-pipeline"]);

    // Extract lines around the probe step
    let probe_section: Vec<&str> = compiled
        .lines()
        .skip_while(|l| !l.contains("Verify MCP backends"))
        .take(5)
        .collect();
    assert!(!probe_section.is_empty(), "Should find probe step");

    // The probe step should NOT contain unresolved {{ mcpg_port }} markers
    assert!(
        !compiled.contains("{{ mcpg_port }}"),
        "Compiled output should not contain unresolved {{ mcpg_port }} marker"
    );
    assert!(
        !compiled.contains("{{ mcpg_debug_flags }}"),
        "Compiled output should not contain unresolved {{ mcpg_debug_flags }} marker"
    );
    assert!(
        !compiled.contains("{{ verify_mcp_backends }}"),
        "Compiled output should not contain unresolved {{ verify_mcp_backends }} marker"
    );
}
#[test]
fn test_debug_pipeline_probe_step_indentation_standalone() {
    let compiled = compile_fixture_with_flags("minimal-agent.md", &["--debug-pipeline"]);

    // The probe step should be a proper YAML step at the same indent level as
    // other steps in the Agent job. Find the displayName line and check indent.
    // Standalone jobs use 4 spaces for step properties.
    let line = compiled
        .lines()
        .find(|l| l.contains("displayName: Verify MCP backends"))
        .expect("Should find 'Verify MCP backends' displayName in compiled output");
    let indent = line.len() - line.trim_start().len();
    assert_eq!(
        indent, 4,
        "Verify MCP backends displayName should be at 4 spaces indent in standalone, got {}",
        indent
    );
}

/// Test that debug probe step indentation is correct in 1ES output
#[test]
fn test_debug_pipeline_probe_step_indentation_1es() {
    let compiled = compile_fixture_with_flags("1es-test-agent.md", &["--debug-pipeline"]);

    // 1ES uses 12 spaces for step properties inside templateContext.
    let line = compiled
        .lines()
        .find(|l| l.contains("displayName: Verify MCP backends"))
        .expect("Should find 'Verify MCP backends' displayName in 1ES compiled output");
    let indent = line.len() - line.trim_start().len();
    assert_eq!(
        indent, 12,
        "Verify MCP backends displayName should be at 12 spaces indent in 1ES, got {}",
        indent
    );
}

// ─── PR Filter Integration Tests ────────────────────────────────────────────

/// Tier 1 PR filters use the bundled Node evaluator via extension.
/// Also verifies the compiled output is valid YAML.
#[test]
fn test_pr_filter_tier1_has_evaluator_gate() {
    let compiled = compile_fixture("pr-filter-tier1-agent.md");
    assert_valid_yaml(&compiled, "pr-filter-tier1-agent.md");

    assert!(
        compiled.contains("- job: Setup"),
        "Should create Setup job for PR filters"
    );
    assert!(
        compiled.contains("name: prGate"),
        "Should include prGate step"
    );
    assert!(
        compiled.contains("GATE_SPEC"),
        "Should include base64-encoded spec"
    );
    assert!(
        compiled.contains("node '/tmp/ado-aw-scripts/ado-script/gate.js'"),
        "Should invoke node gate evaluator"
    );
    assert!(
        compiled.contains("ado-script.zip"),
        "Should download scripts bundle"
    );
    assert!(
        compiled.contains("Evaluate PR filters"),
        "Should have gate displayName"
    );
}

/// Returns the substring of `yaml` from `- job: {name}` (inclusive) to the
/// next `- job:` line or end-of-file. Returns None if no matching job exists.
///
/// Used by the per-job download placement tests to scope substring
/// assertions to a single job's block. Matches the `- job: <name>` line
/// literally (ignores `displayName`, indentation tolerated by `find`).
fn extract_job_block<'a>(yaml: &'a str, name: &str) -> Option<&'a str> {
    let needle = format!("- job: {name}");
    let start = yaml.find(&needle)?;
    let after = &yaml[start + needle.len()..];
    let end = after
        .find("\n- job: ")
        .map(|i| start + needle.len() + i)
        .unwrap_or(yaml.len());
    Some(&yaml[start..end])
}

/// Per-job download placement: gate-only pipeline must put the download in
/// Setup and NOT in Agent. ADO jobs run on isolated VMs, so the gate's
/// install/download has to land in the same job as the gate step.
#[test]
fn test_gate_only_pipeline_downloads_bundle_in_setup_job_not_agent() {
    let yaml = compile_fixture("dedupe_gate_only.md");
    let setup = extract_job_block(&yaml, "Setup").expect("Setup job should exist");
    let agent = extract_job_block(&yaml, "Agent").expect("Agent job should exist");
    assert!(
        setup.contains("Download ado-aw scripts"),
        "Setup job is missing the script bundle download (gate consumer lives here)"
    );
    assert!(
        !agent.contains("Download ado-aw scripts"),
        "Agent job should NOT have the script bundle download (gate-only, no runtime imports). \
         Agent block contents: {}",
        agent
    );
}

/// Per-job download placement: imports-only pipeline must put the download in
/// Agent and NOT in Setup. The import resolver runs in the Agent job, so the
/// install/download has to land on the same VM.
#[test]
fn test_imports_only_pipeline_downloads_bundle_in_agent_job_not_setup() {
    let yaml = compile_fixture("dedupe_imports_only.md");
    let agent = extract_job_block(&yaml, "Agent").expect("Agent job should exist");
    assert!(
        agent.contains("Download ado-aw scripts"),
        "Agent job is missing the script bundle download (import resolver consumer lives here)"
    );
    if let Some(setup) = extract_job_block(&yaml, "Setup") {
        assert!(
            !setup.contains("Download ado-aw scripts"),
            "Setup job should NOT have the script bundle download (imports-only). \
             Setup block contents: {}",
            setup
        );
    }
}

/// Per-job download placement: when both gate and runtime imports are active,
/// the bundle is downloaded twice — once per consuming job. ADO's VM
/// isolation makes this correct architecture, not duplication waste.
#[test]
fn test_both_features_active_downloads_bundle_in_both_jobs() {
    let yaml = compile_fixture("dedupe_both.md");
    let setup = extract_job_block(&yaml, "Setup").expect("Setup job should exist");
    let agent = extract_job_block(&yaml, "Agent").expect("Agent job should exist");
    assert!(
        setup.contains("Download ado-aw scripts"),
        "Setup job is missing the script bundle download"
    );
    assert!(
        agent.contains("Download ado-aw scripts"),
        "Agent job is missing the script bundle download"
    );
    assert_eq!(
        yaml.matches("Download ado-aw scripts").count(),
        2,
        "Expected exactly two downloads — one per consuming job (Setup + Agent)"
    );
}

/// Per-job download placement: with neither gate nor runtime imports active,
/// no Node install or script-bundle download should appear anywhere.
#[test]
fn test_neither_feature_active_emits_no_node_or_download_anywhere() {
    let yaml = compile_fixture("dedupe_neither.md");
    assert!(
        !yaml.contains("UseNode@1"),
        "No UseNode@1 expected when neither gate nor runtime imports are active"
    );
    assert!(
        !yaml.contains("Download ado-aw scripts"),
        "No script bundle download expected when neither gate nor runtime imports are active"
    );
}

/// Per-job download placement: when the gate is inactive AND runtime imports
/// are inlined, but `on.pr` is configured (default `mode: synthetic`) and
/// execution-context PR is not disabled, two bundle consumers are active:
///
///   * `synthPr` (Setup job) — emitted by the synthetic-from-ci path
///   * `exec-context-pr.js` (Agent job) — staged by the PR contributor
///
/// so the script bundle download MUST land in BOTH jobs.
///
/// Closes a coverage gap that `dedupe_gate_only.md` previously left by
/// pinning `execution-context.pr.enabled: false`.
#[test]
fn test_exec_context_pr_downloads_bundle_in_both_jobs_with_synth_mode() {
    let yaml = compile_fixture("dedupe_exec_context_pr_only.md");
    let agent = extract_job_block(&yaml, "Agent").expect("Agent job should exist");
    assert!(
        agent.contains("Download ado-aw scripts"),
        "Agent job is missing the script bundle download (exec-context-pr.js consumer lives here)"
    );
    assert!(
        agent.contains("Stage PR execution context (aw-context/pr/*)"),
        "Agent job is missing the exec-context-pr prepare step (the consumer of the download)"
    );
    if let Some(setup) = extract_job_block(&yaml, "Setup") {
        // Setup-job script bundle download IS expected when on.pr is
        // configured (default `mode: synthetic` emits the synthPr
        // step, which is a bundle consumer). Only assert the Agent
        // job has the bundle download; the Setup-job download is the
        // synth feature's correct behaviour.
        assert!(
            setup.contains("Download ado-aw scripts"),
            "Setup job SHOULD have the script bundle download when mode: synthetic is on (the synthPr step is a bundle consumer). Setup block: {}",
            setup
        );
    }
}

/// When a user pins a Node version via `runtimes.node:` AND runtime imports
/// are active, both extensions emit `UseNode@1` into the Agent job. ADO's
/// `UseNode@1` prepends to PATH, so the LAST install wins. The ado-script
/// extension must run in the `System` phase so its Node 22.x install lands
/// FIRST, and the user's Runtime-phase `UseNode@1 20.x` lands second —
/// the user's pinned version then wins on PATH for the rest of the job.
#[test]
fn test_node_runtime_install_orders_after_ado_script_so_user_version_wins() {
    let yaml = compile_fixture("dedupe_node_runtime_and_imports.md");
    let agent = extract_job_block(&yaml, "Agent").expect("Agent job should exist");

    // Find offsets within the Agent block. The ado-script Node install
    // is identifiable by its displayName; the user's runtime install
    // carries the explicit user-pinned versionSpec.
    let ado_script_install_idx = agent
        .find("displayName: Install Node.js 22.x")
        .expect("ado-script Node 22.x install step missing from Agent job");
    let user_runtime_install_idx = agent
        .find("displayName: Install Node.js 20.x")
        .expect("user runtime Node 20.x install step missing from Agent job");

    assert!(
        ado_script_install_idx < user_runtime_install_idx,
        "ado-script UseNode@1 must precede user UseNode@1 in the Agent job so the \
         user's pinned Node version wins on PATH after both run. \
         ado-script idx = {ado_script_install_idx}, user idx = {user_runtime_install_idx}"
    );

    // Both downloads of ado-script.zip remain unaffected (still exactly one
    // in the Agent job in this fixture — no filters, so no Setup-side download).
    assert_eq!(
        yaml.matches("Download ado-aw scripts").count(),
        1,
        "Expected exactly one ado-script.zip download (Agent job only; no gate active)"
    );
}

/// Tier 2 PR filters produce a Setup job with extension-based gate step.
/// Also verifies the compiled output is valid YAML.
#[test]
fn test_pr_filter_tier2_has_extension_gate() {
    let compiled = compile_fixture("pr-filter-tier2-agent.md");
    assert_valid_yaml(&compiled, "pr-filter-tier2-agent.md");

    assert!(
        compiled.contains("- job: Setup"),
        "Should create Setup job for PR filters"
    );
    assert!(
        compiled.contains("ado-script.zip"),
        "Tier 2 should download scripts bundle"
    );
    assert!(
        compiled.contains("GATE_SPEC"),
        "Tier 2 should include base64-encoded spec"
    );
    assert!(
        compiled.contains("node '/tmp/ado-aw-scripts/ado-script/gate.js'"),
        "Tier 2 should invoke node gate evaluator"
    );
    assert!(compiled.contains("name: prGate"), "Should have prGate step");
}

/// Pipeline filter fixture produces valid YAML.
#[test]
fn test_pipeline_filter_compiled_output_is_valid_yaml() {
    let compiled = compile_fixture("pipeline-filter-agent.md");
    assert_valid_yaml(&compiled, "pipeline-filter-agent.md");
}

/// Pipeline filter fixture produces correct pipeline resource + gate.
#[test]
fn test_pipeline_filter_has_resources_and_gate() {
    let compiled = compile_fixture("pipeline-filter-agent.md");

    assert!(
        compiled.contains("pipelines:"),
        "Should have pipeline resource"
    );
    assert!(
        compiled.contains("trigger: none"),
        "Should disable CI trigger"
    );
    assert!(compiled.contains("pr: none"), "Should disable PR trigger");
    assert!(
        compiled.contains("- job: Setup"),
        "Should create Setup job for pipeline filters"
    );
}

/// Agent job depends on Setup when filters are active.
#[test]
fn test_pr_filter_agent_depends_on_setup() {
    let compiled = compile_fixture("pr-filter-tier1-agent.md");

    assert!(
        compiled.contains("dependsOn: Setup"),
        "Agent job should depend on Setup"
    );
    assert!(
        compiled.contains("prGate.SHOULD_RUN"),
        "Agent job condition should reference gate output"
    );
}

/// Regression guard for the synth-mode gate-bypass bug: with `mode:
/// synthetic` (the default) AND `on.pr.filters` present, the Agent-job
/// condition must REQUIRE the gate to pass for real-PR and synth-PR
/// builds. Earlier iterations emitted `or(eq(Build.Reason, 'PullRequest'),
/// eq(synthPr.AW_SYNTHETIC_PR, 'true'), ...)` which silently bypassed the
/// gate for any PR build — defeating the purpose of `pr.filters`.
#[test]
fn test_pr_filter_synth_mode_agent_condition_enforces_gate() {
    let compiled = compile_fixture("pr-filter-tier1-agent.md");

    // Extract the Agent-job dependsOn condition body so the assertions
    // target only that section (the same strings can appear elsewhere —
    // e.g. the exec-context-pr.js step's condition — and would create
    // false positives if we matched the whole compiled output).
    //
    // Supports both legacy multi-line `condition: |\n  and(...)` form
    // and the newer single-line `condition: and(...)` form emitted by
    // the typed-IR pipeline builder.
    let agent_block = extract_job_block(&compiled, "Agent").expect("Agent job present");
    let condition_section: String = if let Some(tail) = agent_block.split("condition: |").nth(1) {
        // Multi-line block scalar — stop at the next top-level field.
        let stop_at = [
            "\n    pool:",
            "\n    steps:",
            "\n    variables:",
            "\n    workspace:",
        ];
        let end = stop_at
            .iter()
            .filter_map(|needle| tail.find(needle))
            .min()
            .unwrap_or(tail.len());
        tail[..end].to_string()
    } else if let Some(tail) = agent_block.split("condition: ").nth(1) {
        // Single-line — terminate at the next newline.
        tail.split_once('\n')
            .map(|(line, _)| line.to_string())
            .unwrap_or_else(|| tail.to_string())
    } else {
        String::new()
    };
    let condition_section = condition_section.as_str();

    // Correct shape: the AND-NOT clause requiring (not real PR) AND
    // (not synth PR) before the unconditional-run branch is taken.
    // Whitespace-agnostic substring matches.
    assert!(
        condition_section.contains("ne(variables['Build.Reason'], 'PullRequest')")
            && condition_section
                .contains("ne(dependencies.Setup.outputs['synthPr.AW_SYNTHETIC_PR'], 'true')"),
        "Agent-job dependsOn condition must contain the AND-NOT arms \
         `ne(Build.Reason, 'PullRequest')` and `ne(synthPr.AW_SYNTHETIC_PR, 'true')` \
         so the gate is enforced for PR builds (real or synth). \
         Condition section: {condition_section}"
    );
    assert!(
        condition_section.contains("eq(dependencies.Setup.outputs['prGate.SHOULD_RUN'], 'true')"),
        "Agent-job dependsOn condition must keep the gate-passed activation arm: \
         {condition_section}"
    );

    // Defensive: the old permissive bypass arms that bypassed the gate
    // for any PR build MUST NOT appear inside the Agent-job dependsOn
    // condition.
    assert!(
        !condition_section.contains("eq(variables['Build.Reason'], 'PullRequest')"),
        "Agent-job dependsOn condition must NOT contain the buggy \
         `eq(Build.Reason, 'PullRequest')` bypass arm (would auto-run on \
         every real PR build regardless of gate): {condition_section}"
    );
    assert!(
        !condition_section
            .contains("eq(dependencies.Setup.outputs['synthPr.AW_SYNTHETIC_PR'], 'true')"),
        "Agent-job dependsOn condition must NOT contain the buggy \
         `eq(synthPr.AW_SYNTHETIC_PR, 'true')` bypass arm (would auto-run on \
         every synth-promoted build regardless of gate): {condition_section}"
    );
}

/// Regression guard for the synth-mode gate-step same-job ref bug: the
/// gate step lives in the **Setup** job (same job as `synthPr`), so its
/// env block must reference the synth outputs with the **macro** form
/// `$(synthPr.X)`. The same-job runtime-expression form
/// `$[ variables['synthPr.X'] ]` resolves to empty (step outputs are not
/// exposed to runtime expressions in the producing job), and the
/// cross-job form `dependencies.Setup.outputs[...]` is undefined inside
/// the producing job — both silently coalesce to empty, leaving
/// `AW_SYNTHETIC_PR` empty and causing the gate bypass to misfire.
#[test]
fn test_pr_filter_synth_mode_gate_step_uses_same_job_synth_ref() {
    let compiled = compile_fixture("pr-filter-tier1-agent.md");

    // Gate step reads `AW_SYNTHETIC_PR` via plain `$(...)` macro — the
    // `synthPr` step (in the same Setup job) emits this name via
    // `setVar`, registering it in the regular variable namespace. ADO
    // `$[ ... ]` runtime expressions are NOT evaluated inside step
    // `env:` values, so the previous `$[ coalesce(...) ]` form passed
    // the literal expression string through to gate/facts.ts and
    // caused `Missing ADO env vars` errors (msazuresphere/4x4
    // build #612528).
    assert!(
        compiled.contains("AW_SYNTHETIC_PR: $(AW_SYNTHETIC_PR)"),
        "Gate step env must reference the same-job synthPr value via the plain \
         `$(AW_SYNTHETIC_PR)` macro — exec-context-pr-synth's `setVar` emits \
         the regular variable, and `$( )` macros work in step env (whereas \
         `$[ ... ]` runtime expressions don't)."
    );
    // The fixture exercises source-branch and target-branch filters,
    // so the gate-step env vars must reference the canonical `AW_PR_*`
    // names that synthPr always emits (resolved real-or-synth values).
    // (ADO_PR_ID is not exported by this fixture's filter set, so we
    // don't assert it here.)
    assert!(
        compiled.contains("ADO_SOURCE_BRANCH: $(AW_PR_SOURCEBRANCH)"),
        "ADO_SOURCE_BRANCH must reference the canonical AW_PR_SOURCEBRANCH variable \
         (emitted by exec-context-pr-synth via setVar — real on PR builds, \
         discovered on synth-promoted CI builds)."
    );
    assert!(
        compiled.contains("ADO_TARGET_BRANCH: $(AW_PR_TARGETBRANCH)"),
        "ADO_TARGET_BRANCH must reference the canonical AW_PR_TARGETBRANCH variable."
    );

    // Defensive: NO `$[ ... ]` runtime expressions in step env anywhere
    // in this compiled output. ADO only evaluates them inside
    // `variables:` mappings and `condition:` fields. (The Agent job's
    // `variables:` hoist legitimately uses `$[ ... ]` — that's fine
    // because it's inside `variables:`, not step env.)
    assert_no_dollar_bracket_in_step_env(&compiled);

    // The same-job gate step MUST NOT use the broken same-job runtime
    // expression form `variables['synthPr.X']` (resolves empty) nor the
    // cross-job `dependencies.Setup.outputs[...]` form (undefined in the
    // producing job) for synthPr references. (Both are fine elsewhere —
    // e.g. the Agent-job dependsOn condition — but not inside the Setup
    // job's own steps.) Bound the gate-step section by the start of the
    // next top-level job (`\n  - job: `), since extract_job_block's
    // `\n- job: ` boundary doesn't match the 2-space-indented job list
    // items produced for this target.
    let setup_block = extract_job_block(&compiled, "Setup").expect("Setup job present");
    let gate_section = setup_block
        .split("name: prGate")
        .nth(1)
        .map(|tail| {
            let stop_at = [
                "\n      - bash:",
                "\n      - task:",
                "\n      - script:",
                "\n  - job: ",
            ];
            let end = stop_at
                .iter()
                .filter_map(|needle| tail.find(needle))
                .min()
                .unwrap_or(tail.len());
            &tail[..end]
        })
        .unwrap_or("");
    assert!(
        !gate_section.contains("dependencies.Setup.outputs['synthPr."),
        "Gate step (inside Setup job) must NOT reference `dependencies.Setup.outputs['synthPr.X']` — \
         that is cross-job syntax and is undefined within the producing job. \
         Gate section: {gate_section}"
    );
    assert!(
        !gate_section.contains("variables['synthPr."),
        "Gate step (inside Setup job) must NOT reference `variables['synthPr.X']` — \
         step outputs are not exposed to runtime expressions in the producing job \
         and resolve to empty. Gate section: {gate_section}"
    );
}

/// Native ADO PR trigger block is emitted for branch/path filters.
#[test]
fn test_pr_filter_tier1_has_native_pr_trigger() {
    let compiled = compile_fixture("pr-filter-tier1-agent.md");

    assert!(compiled.contains("pr:"), "Should have native pr: block");
    assert!(
        compiled.contains("branches:"),
        "Should have branches filter"
    );
    assert!(compiled.contains("main"), "Should include main branch");
}

/// Extension gate steps are correctly nested inside the Setup job's steps: block.
#[test]
fn test_pr_filter_gate_steps_nested_in_setup_job() {
    let compiled = compile_fixture("pr-filter-tier1-agent.md");

    // Parse the YAML and verify structural nesting
    let yaml_content: String = compiled
        .lines()
        .skip_while(|line| line.starts_with('#') || line.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    let doc: serde_yaml::Value =
        serde_yaml::from_str(&yaml_content).expect("should parse as valid YAML");

    // Find the Setup job in the jobs list
    let jobs = doc.get("jobs").expect("should have jobs key");
    let jobs_seq = jobs.as_sequence().expect("jobs should be a sequence");
    let setup_job = jobs_seq
        .iter()
        .find(|j| {
            j.get("job")
                .and_then(|v| v.as_str())
                .is_some_and(|s| s == "Setup")
        })
        .expect("should have a Setup job");

    // Verify the gate step is INSIDE the Setup job's steps, not a sibling
    let steps = setup_job
        .get("steps")
        .expect("Setup job should have steps")
        .as_sequence()
        .expect("steps should be a sequence");

    // Should have: checkout + download + gate = at least 3 steps
    assert!(
        steps.len() >= 3,
        "Setup job should have at least 3 steps (checkout + download + gate), got {}",
        steps.len()
    );

    // The gate step (with name: prGate) should be inside the steps list
    let has_gate = steps.iter().any(|s| {
        s.get("name")
            .and_then(|v| v.as_str())
            .is_some_and(|n| n == "prGate")
    });
    assert!(
        has_gate,
        "prGate step should be inside Setup job's steps list"
    );

    // The download step should also be inside
    let has_download = steps.iter().any(|s| {
        s.get("displayName")
            .and_then(|v| v.as_str())
            .is_some_and(|n| n.contains("Download ado-aw scripts"))
    });
    assert!(
        has_download,
        "Download step should be inside Setup job's steps list"
    );
}

/// Test that a pipeline without `permissions.write` still emits an `env:` block
/// on the "Execute safe outputs" step that maps SYSTEM_ACCESSTOKEN from
/// `$(System.AccessToken)` (the default executor-token source).
#[test]
fn test_executor_step_uses_system_access_token_by_default() {
    // minimal-agent.md has no permissions.write — executor env must
    // still be present and use System.AccessToken
    let compiled = compile_fixture("minimal-agent.md");
    assert_valid_yaml(&compiled, "minimal-agent.md");

    let execute_block_start = compiled
        .find("Execute safe outputs (Stage 3)")
        .expect("Should have executor step");
    let after_execute = &compiled[execute_block_start..];
    let next_step_offset = after_execute[1..]
        .find("- bash:")
        .map(|i| i + 1)
        .unwrap_or(after_execute.len());
    let executor_step_text = &after_execute[..next_step_offset];

    assert!(
        executor_step_text.contains("env:"),
        "Executor step must always include an env: block (for SYSTEM_ACCESSTOKEN). \
         Step text:\n{executor_step_text}"
    );
    assert!(
        executor_step_text.contains("SYSTEM_ACCESSTOKEN: $(System.AccessToken)"),
        "Executor step must map SYSTEM_ACCESSTOKEN from $(System.AccessToken) by default. \
         Step text:\n{executor_step_text}"
    );
    assert!(
        !executor_step_text.contains("$(SC_WRITE_TOKEN)"),
        "Without permissions.write, executor step must not reference SC_WRITE_TOKEN. \
         Step text:\n{executor_step_text}"
    );
}

/// Test that a pipeline with `permissions.write` emits a correctly-indented `env:` block
/// on the "Execute safe outputs" step.
#[test]
fn test_executor_step_has_env_block_with_write_permissions() {
    // complete-agent.md has permissions.write configured
    let compiled = compile_fixture("complete-agent.md");
    assert_valid_yaml(&compiled, "complete-agent.md");

    assert!(
        compiled.contains("SYSTEM_ACCESSTOKEN: $(SC_WRITE_TOKEN)"),
        "Executor step should have SYSTEM_ACCESSTOKEN when write permissions are configured: {compiled}"
    );

    // Verify the env key and value appear in the correct block by checking both
    // are present in the same neighbourhood.
    let execute_block_start = compiled
        .find("Execute safe outputs (Stage 3)")
        .expect("Should have executor step");
    let after_execute = &compiled[execute_block_start..];
    assert!(
        after_execute.contains("env:"),
        "Executor step should contain 'env:' key after the displayName: {after_execute}"
    );
    assert!(
        after_execute.contains("SYSTEM_ACCESSTOKEN: $(SC_WRITE_TOKEN)"),
        "Executor step should include SYSTEM_ACCESSTOKEN: {after_execute}"
    );
}

/// Copilot BYOK (#1261, #1372): the dedicated `engine.provider` block maps
/// to `COPILOT_PROVIDER_*` env vars, a literal `base-url` host is added to the AWF
/// allow-domains list, and `provider.token` makes the compiler mint the credential
/// **in-job** (Agent + Detection) via `AzureCLI@2` into `COPILOT_PROVIDER_API_KEY`
/// (the var the AWF sidecar reads) — resolving via a same-job
/// `$(AW_PROVIDER_BEARER_TOKEN)` macro with no cross-job plumbing.
/// Compilation must succeed and pass integrity checks (no `--skip-integrity`).
#[test]
fn test_byom_provider_env_compiles_and_merges() {
    let compiled = compile_fixture("byom-foundry-agent.md");
    assert_valid_yaml(&compiled, "byom-foundry-agent.md");

    // engine.provider maps to the correct COPILOT_PROVIDER_* env vars; the bearer
    // token references the same-job secret the mint step publishes (unquoted
    // macro), wired into COPILOT_PROVIDER_API_KEY — the credential env var the
    // AWF sidecar actually reads. Appears in both the Agent and Detection jobs.
    assert_eq!(
        compiled
            .matches("COPILOT_PROVIDER_API_KEY: $(AW_PROVIDER_BEARER_TOKEN)\n")
            .count(),
        2,
        "provider.token must wire COPILOT_PROVIDER_API_KEY to the same-job mint var in both jobs: {compiled}"
    );
    assert!(
        !compiled.contains("COPILOT_PROVIDER_BEARER_TOKEN"),
        "the token must NOT be plumbed as COPILOT_PROVIDER_BEARER_TOKEN (the AWF sidecar ignores it): {compiled}"
    );
    assert!(
        compiled.contains("COPILOT_PROVIDER_TYPE: azure"),
        "provider.type must map to COPILOT_PROVIDER_TYPE: {compiled}"
    );
    assert!(
        compiled.contains("my-foundry.cognitiveservices.azure.com"),
        "Literal provider.base-url host must be added to the AWF allow-domains list: {compiled}"
    );

    // The compiler-owned in-job mint step runs before the Copilot invocation in
    // BOTH the Agent and Detection jobs, authenticated by the service connection.
    assert_eq!(
        compiled.matches("displayName: Acquire provider bearer token").count(),
        2,
        "provider.token must emit the AzureCLI@2 mint step in both the Agent and Detection jobs: {compiled}"
    );
    assert_eq!(
        compiled.matches("azureSubscription: my-arm-connection").count(),
        2,
        "mint step must authenticate via the configured service connection in both jobs: {compiled}"
    );
    assert_eq!(
        compiled
            .matches("##vso[task.setvariable variable=AW_PROVIDER_BEARER_TOKEN;issecret=true]")
            .count(),
        2,
        "mint step must publish the bearer token as a same-job SECRET var in both jobs: {compiled}"
    );
    assert_eq!(
        compiled
            .matches("az account get-access-token --resource 'https://cognitiveservices.azure.com'")
            .count(),
        2,
        "mint step must request a token for the default cognitiveservices resource in both jobs: {compiled}"
    );
    // Same-job minting: no cross-job Setup output plumbing.
    assert!(
        !compiled.contains("$(Setup.FOUNDRY_TOKEN)"),
        "the broken cross-job Setup-output macro must not appear: {compiled}"
    );

    // Credential isolation: the AWF api-proxy sidecar is enabled and the
    // provider credential env keys are excluded from --env-all passthrough, in
    // BOTH the Agent and Detection jobs (detection inherits BYOK routing +
    // isolation, mirroring gh-aw), so each marker appears exactly twice.
    assert_eq!(
        compiled.matches("--enable-api-proxy").count(),
        2,
        "BYOK must enable the AWF api-proxy sidecar in both the Agent and Detection jobs: {compiled}"
    );
    // --exclude-env lists exactly the provider credential keys present (derived
    // from engine.provider): COPILOT_PROVIDER_BASE_URL + COPILOT_PROVIDER_API_KEY
    // (the minted token is wired into API_KEY, not BEARER_TOKEN), in both jobs.
    for key in [
        "--exclude-env COPILOT_PROVIDER_BASE_URL",
        "--exclude-env COPILOT_PROVIDER_API_KEY",
    ] {
        assert_eq!(
            compiled.matches(key).count(),
            2,
            "BYOK must exclude provider credential from passthrough in both jobs ({key}): {compiled}"
        );
    }
    // Defense-in-depth: the intermediate same-job mint secret is also excluded
    // from --env-all in both jobs, so it can never ride the passthrough into the
    // agent container even if ADO ever exposed it as a process env var.
    assert_eq!(
        compiled.matches("--exclude-env AW_PROVIDER_BEARER_TOKEN").count(),
        2,
        "the intermediate mint secret AW_PROVIDER_BEARER_TOKEN must be excluded in both jobs: {compiled}"
    );
    // A credential key NOT configured must NOT be excluded (BEARER_TOKEN is never
    // used by ado-aw — the sidecar only reads API_KEY).
    assert_eq!(
        compiled.matches("--exclude-env COPILOT_PROVIDER_BEARER_TOKEN").count(),
        0,
        "COPILOT_PROVIDER_BEARER_TOKEN must never be referenced: {compiled}"
    );
    // The api-proxy container image must be pre-pulled (and :latest-tagged) in
    // both jobs so AWF's --skip-pull finds it locally.
    assert_eq!(
        compiled
            .matches("docker pull ghcr.io/github/gh-aw-firewall/api-proxy:")
            .count(),
        2,
        "BYOK must pre-pull the api-proxy container image in both the Agent and Detection jobs: {compiled}"
    );
}

/// Copilot BYOK via a **static `api-key`** (no compiler mint step): the
/// `engine.provider.api-key` value maps to `COPILOT_PROVIDER_API_KEY`, BYOK
/// isolation is still enabled (api-proxy + `--exclude-env`), but NO `AzureCLI@2`
/// token-mint step is emitted. Exercises the end-to-end compile path for the
/// api-key branch (the token branch is covered by
/// `test_byom_provider_env_compiles_and_merges`).
#[test]
fn test_byok_provider_api_key_compiles_without_mint_step() {
    // Reuse the token fixture but swap the `token:` block for a static `api-key`.
    let compiled = compile_fixture_tree_with_flags(
        "byom-foundry-agent.md",
        &[],
        &[],
        |contents| {
            // Line-based rewrite (robust to CRLF/LF): drop the `token:` block and
            // insert a static `api-key:` in its place.
            let newline = if contents.contains("\r\n") { "\r\n" } else { "\n" };
            contents
                .lines()
                .filter(|l| {
                    let t = l.trim();
                    t != "token:" && t != "service-connection: my-arm-connection"
                })
                .map(|l| {
                    if l.trim() == "type: azure" {
                        format!("{l}{newline}    api-key: $(FOUNDRY_API_KEY)")
                    } else {
                        l.to_string()
                    }
                })
                .collect::<Vec<_>>()
                .join(newline)
        },
    );
    assert_valid_yaml(&compiled, "byom-foundry-agent.md (api-key)");

    // api-key maps to COPILOT_PROVIDER_API_KEY in both jobs (unquoted macro).
    assert_eq!(
        compiled
            .matches("COPILOT_PROVIDER_API_KEY: $(FOUNDRY_API_KEY)\n")
            .count(),
        2,
        "provider.api-key must map to COPILOT_PROVIDER_API_KEY in both jobs: {compiled}"
    );
    // No compiler-owned mint step for the static-key path.
    assert!(
        !compiled.contains("Acquire provider bearer token"),
        "the api-key path must NOT emit the AzureCLI@2 token-mint step: {compiled}"
    );
    assert!(
        !compiled.contains("AW_PROVIDER_BEARER_TOKEN"),
        "the api-key path must not reference the mint secret var: {compiled}"
    );
    // BYOK isolation still engages (credential is present), in both jobs.
    assert_eq!(
        compiled.matches("--enable-api-proxy").count(),
        2,
        "the api-key path must still enable the api-proxy sidecar in both jobs: {compiled}"
    );
    assert_eq!(
        compiled.matches("--exclude-env COPILOT_PROVIDER_API_KEY").count(),
        2,
        "the api-key credential must be excluded from --env-all in both jobs: {compiled}"
    );
}

/// A non-BYOK agent must NOT enable the api-proxy sidecar or pre-pull its image —
/// the isolation plumbing is strictly opt-in via the presence of a
/// `COPILOT_PROVIDER_*` credential key in `engine.env`.
#[test]
fn test_non_byom_agent_has_no_api_proxy() {
    let compiled = compile_fixture("minimal-agent.md");
    assert_valid_yaml(&compiled, "minimal-agent.md");
    assert!(
        !compiled.contains("--enable-api-proxy"),
        "Non-BYOK agent must not enable the api-proxy sidecar: {compiled}"
    );
    assert!(
        !compiled.contains("api-proxy"),
        "Non-BYOK agent must not reference the api-proxy image: {compiled}"
    );
}

/// Defense-in-depth: parse a compiled pipeline as YAML, locate the **Agent**
/// job, and assert no step in that job maps `SYSTEM_ACCESSTOKEN` at all.
///
/// Background: the Stage 3 executor now defaults to mapping
/// `SYSTEM_ACCESSTOKEN: $(System.AccessToken)` (SafeOutputs job). The Setup
/// job's filter-gate step also legitimately maps it. The agent (Stage 1)
/// must NEVER see `SYSTEM_ACCESSTOKEN` — that is the cross-stage trust
/// boundary that motivates the whole three-stage model. A naive global grep
/// would false-positive on the two legitimate mappings; this test is
/// agent-job-scoped so it only fires on a real regression.
#[test]
fn test_agent_job_steps_do_not_map_system_access_token() {
    let compiled = compile_fixture("minimal-agent.md");
    assert_valid_yaml(&compiled, "minimal-agent.md");

    // Strip the leading `# @ado-aw` header comment to reach the YAML root.
    let yaml_content: String = compiled
        .lines()
        .skip_while(|line| line.starts_with('#') || line.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    let root: serde_yaml::Value =
        serde_yaml::from_str(&yaml_content).expect("compiled pipeline should be valid YAML");

    let jobs = root
        .get("jobs")
        .and_then(|v| v.as_sequence())
        .expect("pipeline should have a top-level `jobs:` sequence");

    let agent_job = jobs
        .iter()
        .find(|j| {
            j.get("job")
                .and_then(|v| v.as_str())
                .is_some_and(|s| s == "Agent")
        })
        .expect("pipeline should have a job named `Agent`");

    let steps = agent_job
        .get("steps")
        .and_then(|v| v.as_sequence())
        .expect("Agent job should have a `steps:` sequence");

    for (idx, step) in steps.iter().enumerate() {
        if let Some(env) = step.get("env").and_then(|v| v.as_mapping()) {
            for (key, value) in env.iter() {
                let key_str = key.as_str().unwrap_or("");
                let value_str = value.as_str().unwrap_or("");
                assert_ne!(
                    key_str, "SYSTEM_ACCESSTOKEN",
                    "Agent job step {} maps SYSTEM_ACCESSTOKEN ({} = {}). This is the \
                     cross-stage trust boundary: the agent (Stage 1) must never see \
                     SYSTEM_ACCESSTOKEN. Only Setup-job filter-gate and Stage 3 \
                     executor are allowed to map it.",
                    idx, key_str, value_str
                );
            }
        }
    }
}

/// Always-on Azure CLI extension: every compiled pipeline must include a
/// host-detection prepare step that conditionally sets the `AW_AZ_MOUNTS`
/// pipeline variable, and the AWF invocation must reference that
/// variable so the mounts are added at pipeline time only when az is
/// present on the runner. Also asserts that Azure auth hosts are in the
/// allow-list and guards against accidental re-introduction of an
/// install step.
#[test]
fn test_default_pipeline_mounts_az_and_allows_azure_hosts() {
    let compiled = compile_fixture("minimal-agent.md");
    assert_valid_yaml(&compiled, "minimal-agent.md");

    // (1) The detection prepare step must be present. It is the only
    // mechanism by which az gets mounted into AWF, so its presence is
    // load-bearing for the "always-on az" promise. The displayName is
    // also part of the compiled YAML and is what operators see in the
    // ADO log; if it changes the documentation in docs/network.md and
    // docs/tools.md should be updated too.
    assert!(
        compiled.contains("displayName: Detect Azure CLI on host (for AWF mount)"),
        "compiled YAML must contain the Azure CLI detection prepare step. \
         Compiled:\n{compiled}"
    );
    assert!(
        compiled.contains("[ -f /usr/bin/az ]"),
        "detection step must test for /usr/bin/az. Compiled:\n{compiled}"
    );
    assert!(
        compiled.contains("##vso[task.setvariable variable=AW_AZ_MOUNTS]"),
        "detection step must set the AW_AZ_MOUNTS pipeline variable. \
         Compiled:\n{compiled}"
    );

    // (1a) Regression guard: `setvariable` for AW_AZ_MOUNTS must appear
    // TWICE — once per branch of the if/else. If the missing-az branch
    // skips the setvariable, ADO leaves the literal `$(AW_AZ_MOUNTS)`
    // in the AWF bash step, where bash interprets it as a `$(...)`
    // command substitution, attempts to run a program named
    // `AW_AZ_MOUNTS`, gets exit 127, and `set -e` kills the pipeline —
    // the exact failure mode this PR set out to prevent on runners
    // without azure-cli installed.
    let setvar_count = compiled
        .matches("##vso[task.setvariable variable=AW_AZ_MOUNTS]")
        .count();
    assert_eq!(
        setvar_count, 2,
        "AW_AZ_MOUNTS must be set in BOTH branches of the detection step (got {setvar_count} \
         occurrences); leaving it unset in the missing-az branch breaks `set -e` in the \
         AWF invocation. See AzureCliExtension::prepare_steps for the rationale."
    );

    // (1b) Conditional prompt-append step: when az is detected, the
    // agent prompt receives an Azure CLI advisory section so the
    // agent knows az is on PATH, what it's good for, and the auth
    // model. The step is gated by `condition: ne(variables['AW_AZ_MOUNTS'], '')`
    // so agents on runners WITHOUT az never see the advisory and
    // never try to call az.
    assert!(
        compiled.contains("displayName: Append Azure CLI prompt"),
        "compiled YAML must contain the 'Append Azure CLI prompt' step \
         emitted by AzureCliExtension::prepare_steps. Compiled:\n{compiled}"
    );
    assert!(
        compiled.contains("condition: ne(variables['AW_AZ_MOUNTS'], '')"),
        "the Azure CLI prompt-append step must carry a condition: \
         ne(variables['AW_AZ_MOUNTS'], '') so it is skipped when az \
         is not detected. Compiled:\n{compiled}"
    );
    // Proximity check — the condition: must live on the SAME step as
    // the displayName, otherwise we may have accidentally gated the
    // wrong step. Find the displayName index, then check the next ~200
    // chars for the condition line.
    let display_idx = compiled
        .find("displayName: Append Azure CLI prompt")
        .expect("displayName already asserted to be present");
    let window_end = (display_idx + 300).min(compiled.len());
    let window = &compiled[display_idx..window_end];
    assert!(
        window.contains("condition: ne(variables['AW_AZ_MOUNTS'], '')"),
        "the condition: line must appear in the same step block as the \
         'Append Azure CLI prompt' displayName (looked at the 300 \
         chars after the displayName). Window:\n{window}"
    );
    // Anchor strings: lock the load-bearing parts of the advisory.
    for anchor in [
        "/usr/bin/az",
        "az devops",
        "AZURE_DEVOPS_EXT_PAT",
        "missing-tool",
    ] {
        assert!(
            compiled.contains(anchor),
            "compiled YAML must contain advisory anchor `{anchor}`. \
             Compiled:\n{compiled}"
        );
    }

    // (2) The AWF invocation must reference $(AW_AZ_MOUNTS) so the
    // pipeline-variable value (the two --mount args, or empty) is
    // word-split into the docker run command at runtime. Unquoted on
    // purpose — see the safety note in `generate_awf_mounts`.
    assert!(
        compiled.contains("$(AW_AZ_MOUNTS) \\"),
        "AWF invocation must include a `$(AW_AZ_MOUNTS) \\` line so the \
         pipeline variable expands into --mount args at runtime. \
         Compiled:\n{compiled}"
    );

    // (3) Critical guard: we must NOT emit static --mount args for az
    // paths, because that would crash `docker run` on runners without
    // azure-cli installed (bind source path does not exist). All az
    // mounting must go through the runtime-detected pipeline variable.
    assert!(
        !compiled.contains(r#"--mount "/opt/az:/opt/az:ro""#),
        "compiled YAML must NOT contain a static --mount for /opt/az — \
         that would crash `docker run` on runners without azure-cli. \
         Mounts must be contributed via the AW_AZ_MOUNTS pipeline \
         variable. Compiled:\n{compiled}"
    );
    assert!(
        !compiled.contains(r#"--mount "/usr/bin/az:/usr/bin/az:ro""#),
        "compiled YAML must NOT contain a static --mount for /usr/bin/az — \
         that would crash `docker run` on runners without azure-cli. \
         Mounts must be contributed via the AW_AZ_MOUNTS pipeline \
         variable. Compiled:\n{compiled}"
    );

    // (4) Azure auth/management hosts must be in --allow-domains.
    for host in [
        "login.microsoftonline.com",
        "management.azure.com",
        "graph.microsoft.com",
    ] {
        assert!(
            compiled.contains(host),
            "compiled --allow-domains must contain {host}. Compiled:\n{compiled}"
        );
    }

    // (5) Regression guard: we deliberately do NOT install az; the host
    // is assumed to have azure-cli pre-installed (gh-aw parity). If a
    // future contributor adds an install step we want the test suite to
    // catch it so the decision is explicit.
    assert!(
        !compiled.contains("Install Azure CLI"),
        "compiled YAML must not contain an 'Install Azure CLI' step — host is assumed \
         to have az pre-installed. If you genuinely need an install step, update this \
         test along with the AzureCliExtension. Compiled:\n{compiled}"
    );
    assert!(
        !compiled.contains("InstallAzureCLIDeb"),
        "compiled YAML must not reference the Microsoft az apt installer URL — host is \
         assumed to have az pre-installed. Compiled:\n{compiled}"
    );
}

// ─── ado-aw-debug fixture ──────────────────────────────────────────────────

/// Compile the `ado-aw-debug-agent.md` fixture and assert the
/// front-matter section's compile-time effects:
/// 1. The integrity check step is omitted (`skip-integrity: true`).
/// 2. The Stage 3 executor `env:` block exposes
///    `ADO_AW_DEBUG_GITHUB_TOKEN`.
/// 3. `--enabled-tools create-issue` is wired into the SafeOutputs MCP
///    invocation.
/// 4. The output is otherwise valid YAML.
#[test]
fn test_compile_ado_aw_debug_fixture() {
    let compiled = compile_fixture("ado-aw-debug-agent.md");
    assert_valid_yaml(&compiled, "ado-aw-debug-agent.md");

    // skip-integrity: integrity check should be absent
    assert!(
        !compiled.contains("Verify pipeline integrity"),
        "ado-aw-debug.skip-integrity: true must omit the integrity check step"
    );

    // Executor env block exposes the GitHub PAT pipeline variable
    let execute_block_start = compiled
        .find("Execute safe outputs (Stage 3)")
        .expect("Should have executor step");
    let after_execute = &compiled[execute_block_start..];
    assert!(
        after_execute.contains("ADO_AW_DEBUG_GITHUB_TOKEN: $(ADO_AW_DEBUG_GITHUB_TOKEN)"),
        "Executor step must expose ADO_AW_DEBUG_GITHUB_TOKEN when ado-aw-debug.create-issue is set: {after_execute}"
    );

    // --enabled-tools includes create-issue
    assert!(
        compiled.contains("--enabled-tools create-issue"),
        "Compiler must add --enabled-tools create-issue when ado-aw-debug.create-issue is set"
    );
}

/// The example file in `examples/dogfood-failure-reporter.md` must compile
/// cleanly. Mirror of the structural smoke test for `examples/sample-agent.md`.
#[test]
fn test_example_dogfood_failure_reporter_structure() {
    let example_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join("dogfood-failure-reporter.md");
    assert!(
        example_path.exists(),
        "examples/dogfood-failure-reporter.md should exist"
    );
    let content = fs::read_to_string(&example_path).expect("Should be able to read example");
    assert!(
        content.starts_with("---"),
        "Example should start with front matter"
    );
    assert!(
        content.contains("ado-aw-debug:"),
        "Example should declare ado-aw-debug section"
    );
    assert!(
        content.contains("create-issue:"),
        "Example should configure create-issue"
    );
    assert!(
        content.contains("target-repo: githubnext/ado-aw"),
        "Example should target githubnext/ado-aw"
    );
}

// =====================================================================
// External stage/job ordering for template targets
// =====================================================================
//
// `target: stage` and `target: job` are ADO template targets. ADO's
// `stages.template` / `jobs.template` schemas only permit `template:` and
// `parameters:` at the call site — `dependsOn:` and `condition:` as bare
// keys on the `- template:` line are rejected. The compiler surfaces them
// as auto-injected template parameters and applies them inside the
// template via ADO conditional template expressions.
//
// These tests pin the new contract: parameter declarations are emitted,
// inner stage/job blocks contain the conditional `${{ if … }}` blocks,
// internal Setup/gate behaviour is preserved when callers omit the new
// params, and standalone/1ES targets are untouched.

#[test]
fn test_stage_target_auto_injects_depends_on_and_condition_params() {
    let compiled = compile_fixture("stage-agent.md");
    assert!(
        compiled.contains("- name: dependsOn\n  type: object\n  default: []"),
        "stage target should auto-inject dependsOn param. got:\n{compiled}"
    );
    assert!(
        compiled.contains("- name: condition\n  type: string\n  default: ''"),
        "stage target should auto-inject condition param. got:\n{compiled}"
    );
}

#[test]
fn test_stage_target_emits_conditional_blocks_on_inner_stage() {
    let compiled = compile_fixture("stage-agent.md");
    assert!(
        compiled.contains("${{ if ne(length(parameters.dependsOn), 0) }}:"),
        "stage target should emit conditional dependsOn block. got:\n{compiled}"
    );
    assert!(
        compiled.contains("dependsOn: ${{ parameters.dependsOn }}"),
        "stage target should pass parameters.dependsOn through to the inner stage. got:\n{compiled}"
    );
    assert!(
        compiled.contains("${{ if ne(parameters.condition, '') }}:"),
        "stage target should emit conditional condition block. got:\n{compiled}"
    );
    assert!(
        compiled.contains("condition: ${{ parameters.condition }}"),
        "stage target should pass parameters.condition through to the inner stage. got:\n{compiled}"
    );
}

#[test]
fn test_job_target_auto_injects_depends_on_and_condition_params() {
    let compiled = compile_fixture("job-agent.md");
    assert!(
        compiled.contains("- name: dependsOn\n  type: object\n  default: []"),
        "job target should auto-inject dependsOn param. got:\n{compiled}"
    );
    assert!(
        compiled.contains("- name: condition\n  type: string\n  default: ''"),
        "job target should auto-inject condition param. got:\n{compiled}"
    );
}

#[test]
fn test_job_target_minimal_emits_non_empty_only_branches() {
    // job-agent.md is a minimal fixture without Setup steps or PR/pipeline
    // gates. The dependsOn and condition blocks should only emit the
    // `ne(..., default)` branch — there is no internal Setup/gate to
    // preserve in the alternate branch.
    let compiled = compile_fixture("job-agent.md");
    assert!(
        !compiled.contains("${{ if eq(length(parameters.dependsOn), 0) }}:"),
        "minimal job target should not emit empty-deps branch (no internal Setup). got:\n{compiled}"
    );
    assert!(
        compiled.contains("${{ if ne(length(parameters.dependsOn), 0) }}:"),
        "minimal job target should emit non-empty deps branch. got:\n{compiled}"
    );
    assert!(
        compiled.contains("dependsOn: ${{ parameters.dependsOn }}"),
        "minimal job target should pass dependsOn through directly. got:\n{compiled}"
    );
    assert!(
        !compiled.contains("${{ if eq(parameters.condition, '') }}:"),
        "minimal job target should not emit empty-condition branch (no internal condition). got:\n{compiled}"
    );
    assert!(
        compiled.contains("condition: ${{ parameters.condition }}"),
        "minimal job target should pass condition through directly. got:\n{compiled}"
    );
}

#[test]
fn test_standalone_target_does_not_auto_inject_template_params() {
    // Standalone is a root pipeline, not a template; auto-injecting
    // dependsOn/condition as runtime UI parameters would be wrong.
    let compiled = compile_fixture("minimal-agent.md");
    assert!(
        !compiled.contains("- name: dependsOn"),
        "standalone must not auto-inject dependsOn parameter. got:\n{compiled}"
    );
    assert!(
        !compiled.contains("- name: condition"),
        "standalone must not auto-inject condition parameter. got:\n{compiled}"
    );
}

#[test]
fn test_1es_target_does_not_auto_inject_template_params() {
    let compiled = compile_fixture("1es-test-agent.md");
    assert!(
        !compiled.contains("- name: dependsOn"),
        "1es must not auto-inject dependsOn parameter. got:\n{compiled}"
    );
    assert!(
        !compiled.contains("- name: condition"),
        "1es must not auto-inject condition parameter. got:\n{compiled}"
    );
}

#[test]
fn test_template_target_rejects_reserved_depends_on_parameter_name() {
    // Compile-time validation must reject user front-matter that declares
    // a parameter named dependsOn or condition under target: job/stage —
    // those names are reserved for template-invocation use.
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let unique_id = COUNTER.fetch_add(1, Ordering::Relaxed);
    let temp_dir = std::env::temp_dir().join(format!(
        "ado-aw-collision-{}-{}",
        std::process::id(),
        unique_id,
    ));
    fs::create_dir_all(&temp_dir).expect("Failed to create temp directory");
    let src = temp_dir.join("collision.md");
    fs::write(
        &src,
        "---\nname: Test Agent\ndescription: t\ntarget: job\nparameters:\n  - name: dependsOn\n    type: string\n    default: x\n---\n\n# body\n",
    )
    .expect("write fixture");

    let binary_path = PathBuf::from(env!("CARGO_BIN_EXE_ado-aw"));
    let out_path = temp_dir.join("collision.lock.yml");
    let output = std::process::Command::new(&binary_path)
        .args([
            "compile",
            src.to_str().unwrap(),
            "-o",
            out_path.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to run compiler");

    assert!(
        !output.status.success(),
        "Compilation should fail when user declares reserved param 'dependsOn' for target: job"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("dependsOn") && stderr.contains("reserved"),
        "stderr should mention dependsOn as reserved. got:\n{stderr}"
    );

    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_template_target_rejects_reserved_condition_parameter_name() {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let unique_id = COUNTER.fetch_add(1, Ordering::Relaxed);
    let temp_dir = std::env::temp_dir().join(format!(
        "ado-aw-collision-cond-{}-{}",
        std::process::id(),
        unique_id,
    ));
    fs::create_dir_all(&temp_dir).expect("Failed to create temp directory");
    let src = temp_dir.join("collision.md");
    fs::write(
        &src,
        "---\nname: Test Agent\ndescription: t\ntarget: stage\nparameters:\n  - name: condition\n    type: string\n    default: x\n---\n\n# body\n",
    )
    .expect("write fixture");

    let binary_path = PathBuf::from(env!("CARGO_BIN_EXE_ado-aw"));
    let out_path = temp_dir.join("collision.lock.yml");
    let output = std::process::Command::new(&binary_path)
        .args([
            "compile",
            src.to_str().unwrap(),
            "-o",
            out_path.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to run compiler");

    assert!(
        !output.status.success(),
        "Compilation should fail when user declares reserved param 'condition' for target: stage"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("condition") && stderr.contains("reserved"),
        "stderr should mention condition as reserved. got:\n{stderr}"
    );

    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_job_target_with_setup_emits_dual_branch_dependson_with_each() {
    // When the agent has a Setup job (e.g. via setup: steps or PR/pipeline
    // gates), the dependsOn block must emit BOTH branches: empty-external
    // preserves the existing `dependsOn: Setup`; non-empty-external uses
    // `${{ each }}` to merge `Setup` with the caller's deps into a single
    // list. This is the only place we use `${{ each }}` in the compiler;
    // the parameter MUST be declared as type: object and callers MUST
    // pass a list because ${{ each }} iterates only iterables.
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let unique_id = COUNTER.fetch_add(1, Ordering::Relaxed);
    let temp_dir = std::env::temp_dir().join(format!(
        "ado-aw-job-setup-{}-{}",
        std::process::id(),
        unique_id,
    ));
    fs::create_dir_all(&temp_dir).expect("Failed to create temp directory");
    let src = temp_dir.join("job-setup.md");
    fs::write(
        &src,
        "---\nname: Job With Setup\ndescription: t\ntarget: job\nsetup:\n  - bash: echo s\n---\n\n# body\n",
    )
    .expect("write fixture");

    let binary_path = PathBuf::from(env!("CARGO_BIN_EXE_ado-aw"));
    let out_path = temp_dir.join("job-setup.lock.yml");
    let output = std::process::Command::new(&binary_path)
        .args([
            "compile",
            src.to_str().unwrap(),
            "-o",
            out_path.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to run compiler");
    assert!(
        output.status.success(),
        "compile should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let compiled = fs::read_to_string(&out_path).expect("read output");

    // Both branches present
    assert!(
        compiled.contains("${{ if eq(length(parameters.dependsOn), 0) }}:"),
        "should emit empty-deps branch when has_setup. got:\n{compiled}"
    );
    assert!(
        compiled.contains("${{ if ne(length(parameters.dependsOn), 0) }}:"),
        "should emit non-empty-deps branch when has_setup. got:\n{compiled}"
    );
    // Empty-deps branch preserves internal Setup dependency
    assert!(
        compiled.contains("dependsOn: Setup"),
        "empty-deps branch should keep dependsOn: Setup. got:\n{compiled}"
    );
    // Non-empty branch uses ${{ each }} over a list, with Setup as the first item
    assert!(
        compiled.contains("${{ each d in parameters.dependsOn }}:"),
        "non-empty-deps branch should use ${{{{ each }}}} to iterate. got:\n{compiled}"
    );

    let _ = fs::remove_dir_all(&temp_dir);
}

// ============================================================================
// Execution-context extension (issue #860)
// ============================================================================

/// The execution-context extension is always-on and emits an `aw-context`
/// prepare step on PR-triggered agents. This sanity check makes sure the
/// generated YAML round-trips through `serde_yaml`.
#[test]
fn test_execution_context_pr_compiled_output_is_valid_yaml() {
    let compiled = compile_fixture("execution-context-agent.md");
    assert_valid_yaml(&compiled, "execution-context-agent.md");
}

/// Spot-checks the key components of the precompute step. v7 ports
/// the precompute logic to an `ado-script` bundle
/// (`exec-context-pr.js`), so the bash step is now a slim node
/// invocation. Body-level behavioural coverage (regex validation,
/// merge-base resolution, GIT_CONFIG_* bearer injection, prompt
/// fragment shape) lives in vitest unit + smoke tests under
/// `scripts/ado-script/src/exec-context-pr/__tests__/` and
/// `scripts/ado-script/test/smoke.test.ts`.
#[test]
fn test_execution_context_pr_emits_prepare_step_and_prompt_supplement() {
    let compiled = compile_fixture("execution-context-agent.md");

    assert!(
        compiled.contains("Stage PR execution context (aw-context/pr/*)"),
        "Should emit the PR context prepare step displayName"
    );
    // The synth-active prepare step uses `condition: succeeded()` and
    // gates in bash (cross-job `dependencies.Setup.outputs[...]` refs
    // are ILLEGAL in step-level `condition:` — ADO rejects them with
    // "Unrecognized value: 'dependencies'"). `synthPr` now always runs
    // and emits a single resolved `AW_PR_ID` (real on PR builds,
    // discovered on synth-promoted CI builds, empty otherwise), so the
    // gate collapses to a single empty-check.
    assert!(
        compiled.contains("if [ -z \"$AW_PR_ID\" ]; then"),
        "Prepare step must include the bash gate on empty AW_PR_ID (replaces the previous \
         BUILD_REASON + AW_SYNTHETIC_PR pair — the merge now happens inside synthPr)"
    );
    // The Stage step's own env block must NOT contain a direct
    // `dependencies.Setup.outputs[...]` reference. (The same expression
    // IS expected at Agent-job-level `variables:` scope, the documented
    // safe location — that hoist is asserted separately.) Scope this
    // check by isolating the Stage step's bash + env body.
    let stage_step = compiled
        .split("Stage PR execution context")
        .nth(1)
        .map(|tail| {
            // Stop at the next step (`- bash:` / `- task:` / `- script:`)
            // or end of the job (a less-indented key).
            let stop_at = ["\n      - bash:", "\n      - task:", "\n      - script:"];
            let end = stop_at
                .iter()
                .filter_map(|needle| tail.find(needle))
                .min()
                .unwrap_or(tail.len());
            &tail[..end]
        })
        .unwrap_or("");
    assert!(
        !stage_step.contains("dependencies.Setup.outputs['synthPr."),
        "Stage step's own env block must NOT reference \
         `dependencies.Setup.outputs[...]` — that is cross-job syntax. The cross-job \
         output is hoisted into Agent-job-level `variables:` (see \
         `generate_agent_job_variables`) and the Stage step reads it via the \
         `$(AW_PR_*)` macros. Stage step body: {stage_step}"
    );
    // ADO does NOT evaluate `$[ ... ]` runtime expressions inside step
    // `env:` values — only inside `variables:` mappings and
    // `condition:` fields. Any `$[ ` in this step's env block would be
    // passed through to bash as a literal string (the bug fixed here).
    assert!(
        !stage_step.contains("$["),
        "Stage step's env block must not contain `$[ ` runtime expressions \
         (ADO doesn't evaluate them at step-env scope). Use the Agent-job-level \
         `variables:` hoist + `$(name)` macros instead. Stage step body: {stage_step}"
    );
    assert!(
        compiled.contains("SYSTEM_ACCESSTOKEN: $(System.AccessToken)"),
        "Prepare step must map the system access token into its own env"
    );

    // v7: the prepare step is a node invocation of the bundle. The
    // path is the literal `EXEC_CONTEXT_PR_PATH` constant exported by
    // `ado_script.rs`. The bundle is installed in the Agent job by
    // `AdoScriptExtension::prepare_steps`, which runs in
    // `ExtensionPhase::System` and thus appears before this step
    // (which runs in `ExtensionPhase::Tool`).
    assert!(
        compiled.contains("node '/tmp/ado-aw-scripts/ado-script/exec-context-pr.js'"),
        "v7: prepare step must invoke the exec-context-pr.js bundle"
    );

    // v7: all the bash-side specifics (GIT_CONFIG_*, regex validation,
    // $AW_PR_DIR, status.txt, etc.) have moved into the TS bundle.
    // They MUST NOT appear in the generated YAML.
    assert!(
        !compiled.contains("GIT_CONFIG_KEY_0"),
        "v7: GIT_CONFIG_* bearer injection moved into the bundle's git child env; \
         it must not appear in the emitted prepare step's bash"
    );
    assert!(
        !compiled.contains("AW_PR_DIR"),
        "v7: artefact path construction lives in the bundle; \
         the prepare step must not reference $AW_PR_DIR"
    );
    assert!(
        !compiled.contains("git_fetch()"),
        "v7: the git_fetch wrapper moved into the bundle"
    );

    // v7 + mode: synthetic (the default): env passthrough — the bundle
    // reads ADO predefined vars from `process.env`. The compiler emits
    // plain `$(AW_PR_*)` macros that read the Agent-job-level hoisted
    // variables (populated from the `synthPr` Setup-job outputs which
    // hold the resolved real-or-synth PR identifiers). No `$[ ... ]`
    // in step env — see `generate_agent_job_variables` for the hoist.
    assert!(
        compiled.contains("SYSTEM_PULLREQUEST_PULLREQUESTID: $(AW_PR_ID)"),
        "Prepare step must pass the PR id via the hoisted AW_PR_ID job-variable"
    );
    assert!(
        compiled.contains("SYSTEM_PULLREQUEST_TARGETBRANCH: $(AW_PR_TARGETBRANCH)"),
        "Prepare step must pass the PR target branch via the hoisted AW_PR_TARGETBRANCH job-variable"
    );
    // ADO auto-injects the predefined System.*/Build.* context vars the
    // bundle reads, so the prepare step must NOT re-project them (they were
    // redundant mirrors — see src/compile/ado_bundle.rs).
    assert!(
        !compiled.contains("SYSTEM_TEAMPROJECT: $(System.TeamProject)"),
        "SYSTEM_TEAMPROJECT is auto-injected and must not be re-projected"
    );
    assert!(
        !compiled.contains("BUILD_REPOSITORY_NAME: $(Build.Repository.Name)"),
        "BUILD_REPOSITORY_NAME is auto-injected and must not be re-projected"
    );
    assert!(
        !compiled.contains("BUILD_SOURCESDIRECTORY: $(Build.SourcesDirectory)"),
        "BUILD_SOURCESDIRECTORY is auto-injected and must not be re-projected"
    );

    // v7: the bundle install/download must be present in the Agent
    // job. AdoScriptExtension owns this — it fires whenever EITHER
    // import.js OR exec-context-pr.js is needed.
    assert!(
        compiled.contains("ado-script.zip"),
        "v7: AdoScriptExtension must install + download the bundle in the Agent job \
         when the PR contributor is active"
    );

    // v6.2 carry-over: the prompt_supplement trait is NOT implemented
    // by ExecContextExtension. The wrapper step must not be emitted.
    assert!(
        !compiled.contains("Append Execution Context prompt"),
        "ExecContextExtension::prompt_supplement is removed; \
         the wrapper step must not be emitted"
    );
}

/// **Trust-boundary regression test.** Asserts the exec-context PR
/// contributor never writes credentials to disk (`.git/config`,
/// `persistCredentials: true`) and that every `env:` block declaring
/// `SYSTEM_ACCESSTOKEN` is one of the three sanctioned locations:
///
/// 1. The exec-context PR prepare step — owned by this extension.
/// 2. The Stage 3 SafeOutputs executor step — runs in its own
///    non-agent job and legitimately needs the token to apply safe
///    outputs (PRs, work items, etc.). See PR #873.
/// 3. The Setup-job filter-gate evaluator (when filters are configured).
///
/// The token MUST NOT appear in the agent step's env — that is the
/// cross-stage trust boundary enforced separately by
/// `test_agent_job_steps_do_not_map_system_access_token`.
#[test]
fn test_execution_context_pr_does_not_leak_system_accesstoken() {
    let compiled = compile_fixture("execution-context-agent.md");

    assert!(
        !compiled.contains("persistCredentials: true"),
        "execution-context must NEVER emit `persistCredentials: true`."
    );
    assert!(
        !compiled.contains(".git/config"),
        "execution-context must NEVER write to .git/config."
    );

    // Parse the YAML and walk every mapping. For any mapping that has
    // an `env:` child mapping containing `SYSTEM_ACCESSTOKEN`, the
    // enclosing step's `displayName` MUST be one of the sanctioned
    // mappings listed in the docstring above. Anything else is a leak.
    use serde_yaml::Value;
    let yaml: Value =
        serde_yaml::from_str(&compiled).expect("compiled output should parse as YAML");

    fn walk(v: &Value, found: &mut Vec<Option<String>>) {
        match v {
            Value::Mapping(m) => {
                // Inspect this mapping: if it has an `env:` child mapping
                // that contains SYSTEM_ACCESSTOKEN, capture the
                // sibling `displayName` (if any).
                if let Some(Value::Mapping(env_map)) = m.get(Value::String("env".to_string())) {
                    let has_token = env_map
                        .iter()
                        .any(|(k, _v)| matches!(k, Value::String(s) if s == "SYSTEM_ACCESSTOKEN"));
                    if has_token {
                        let display = m
                            .get(Value::String("displayName".to_string()))
                            .and_then(|d| d.as_str())
                            .map(|s| s.to_string());
                        found.push(display);
                    }
                }
                for (_, vv) in m {
                    walk(vv, found);
                }
            }
            Value::Sequence(seq) => {
                for item in seq {
                    walk(item, found);
                }
            }
            _ => {}
        }
    }

    let mut env_blocks_with_token: Vec<Option<String>> = Vec::new();
    walk(&yaml, &mut env_blocks_with_token);

    assert!(
        !env_blocks_with_token.is_empty(),
        "expected at least one env block with SYSTEM_ACCESSTOKEN (the exec-context prepare step)"
    );

    // The full set of sanctioned step displayNames that may legitimately
    // map SYSTEM_ACCESSTOKEN. Keep this list narrow and audited — adding
    // a new entry requires confirming the step runs outside the AWF
    // sandbox and the token is not reachable from the agent step.
    const ALLOWED_DISPLAY_NAMES: &[&str] = &[
        // Owned by this extension.
        "Stage PR execution context (aw-context/pr/*)",
        // Pipeline contributor (Stage 2 of plan.md). Activates on
        // on.pipeline / Build.Reason == ResourceTrigger. Needs the
        // token to call the Build REST API to fetch upstream
        // metadata. Same trust-boundary posture as the PR
        // contributor — token mapped only into this step's env.
        "Stage pipeline execution context (aw-context/pipeline/*)",
        // CI-push contributor (Stage 3 of plan.md). Opt-in,
        // default OFF. Activates on IndividualCI / BatchedCI runs.
        // Bearer for "last successful build" lookup + git fetch
        // deepening.
        "Stage ci-push execution context (aw-context/ci-push/*)",
        // Workitem contributor (Stage 4 of plan.md). Activates whenever
        // the PR contributor activates. Needs the token to call the
        // ADO REST API to look up linked work items. Same trust-boundary
        // posture as the PR contributor — token mapped only into this
        // step's env, never reachable from the agent step.
        "Stage workitem execution context (aw-context/workitem/*)",
        // Schedule contributor (Stage 5 of plan.md). Opt-in, default
        // OFF. Activates on Build.Reason == Schedule. Bearer for
        // REST + git fetch — same posture as ci-push.
        "Stage schedule execution context (aw-context/schedule/*)",
        // PR-checks extension (Stage 6 of plan.md). Activates whenever
        // the PR contributor activates AND `pr.checks.enabled: true`.
        // Needs the token to call the Build REST API. Same posture.
        "Stage PR-checks execution context (aw-context/pr/checks/*)",
        // Stage 3 SafeOutputs executor — separate non-agent job; needs
        // the token to apply safe outputs against ADO. See PR #873.
        "Execute safe outputs (Stage 3)",
        // Setup-job synth-PR step. Needs the token to call the ADO REST
        // API to look up the active PR for `Build.SourceBranch` on
        // CI-triggered builds (issue #916). Runs in the Setup job, well
        // before the AWF sandbox is provisioned for the Agent job.
        "Resolve synthetic PR context",
    ];

    let mut saw_exec_context_step = false;
    for display in &env_blocks_with_token {
        match display {
            Some(d) if ALLOWED_DISPLAY_NAMES.contains(&d.as_str()) => {
                if d == "Stage PR execution context (aw-context/pr/*)" {
                    saw_exec_context_step = true;
                }
            }
            other => panic!(
                "SYSTEM_ACCESSTOKEN was found in a step env block whose displayName \
                 is not in the sanctioned allow-list {:?}. displayName = {:?}. \
                 This indicates a credential leak into another step.",
                ALLOWED_DISPLAY_NAMES, other
            ),
        }
    }

    assert!(
        saw_exec_context_step,
        "expected to find the exec-context PR prepare step (\
         displayName = \"Stage PR execution context (aw-context/pr/*)\") \
         among the env blocks declaring SYSTEM_ACCESSTOKEN, but it was missing. \
         The PR contributor did not activate as expected."
    );
}

/// Churn guard for the bundle env-var contract refactor: no bundle step
/// re-projects an ADO predefined variable that the runtime already
/// auto-injects (e.g. `SYSTEM_TEAMPROJECT: $(System.TeamProject)`). Such
/// redundant mirrors were stripped when the auth contract was centralised in
/// `src/compile/ado_bundle.rs`; this test walks the compiled YAML and fails if
/// any reappear.
///
/// The invariant: for a step `env:` entry `KEY: $(Dotted.Var)`, it is a
/// redundant mirror iff `KEY == Dotted.Var.replace('.', "_").to_uppercase()`.
/// `SYSTEM_ACCESSTOKEN: $(System.AccessToken)` is the sanctioned exception —
/// the token is NOT auto-injected and must be projected (that projection is
/// what fixed #1307).
///
/// Runs against multiple fixture families so the exec-context prepare steps,
/// the synth-PR step, the conclusion step, AND the filter `gate.js` step are
/// all covered.
#[test]
fn test_bundle_steps_do_not_reproject_auto_injected_ado_vars() {
    use serde_yaml::Value;

    // Keys retained deliberately even though they mirror an auto-injected var:
    //  - SYSTEM_ACCESSTOKEN: the bearer, which is NOT auto-injected (ADO maps
    //    it only on explicit reference), so it must be projected.
    //  - BUILD_REQUESTEDFOR / BUILD_REQUESTEDFOREMAIL: the manual contributor's
    //    requestor-identity vars. These ARE auto-injected, so stripping them
    //    would not change runtime behaviour — but they sit behind the
    //    `include_email_resolved()` email-hygiene opt-in, and projecting them
    //    keeps that privacy intent visible at the call site. See the comment in
    //    src/compile/extensions/exec_context/manual.rs::prepare_step_typed.
    const RETAINED: &[&str] = &[
        "SYSTEM_ACCESSTOKEN",
        "BUILD_REQUESTEDFOR",
        "BUILD_REQUESTEDFOREMAIL",
    ];

    fn screaming_snake(dotted: &str) -> String {
        dotted.replace('.', "_").to_uppercase()
    }

    fn walk(v: &Value, offenders: &mut Vec<String>) {
        match v {
            Value::Mapping(m) => {
                if let Some(Value::Mapping(env_map)) = m.get(Value::String("env".to_string())) {
                    for (k, val) in env_map {
                        let (key, value) = match (k.as_str(), val.as_str()) {
                            (Some(k), Some(v)) => (k, v),
                            _ => continue,
                        };
                        if RETAINED.contains(&key) {
                            continue;
                        }
                        // Match a bare `$(Dotted.Var)` macro value.
                        let inner = value.strip_prefix("$(").and_then(|s| s.strip_suffix(')'));
                        if let Some(dotted) = inner
                            && dotted.contains('.')
                            && key == screaming_snake(dotted)
                        {
                            offenders.push(format!("{key}: {value}"));
                        }
                    }
                }
                for (_, vv) in m {
                    walk(vv, offenders);
                }
            }
            Value::Sequence(seq) => {
                for item in seq {
                    walk(item, offenders);
                }
            }
            _ => {}
        }
    }

    // Fixture families that between them exercise every bundle step shape:
    //  - execution-context-agent.md: the exec-context prepare steps + synth-PR
    //    + conclusion (safe-outputs) steps.
    //  - pr-filter-tier1-agent.md / pipeline-filter-agent.md: the filter
    //    `gate.js` step (prGate / pipelineGate).
    const FIXTURES: &[&str] = &[
        "execution-context-agent.md",
        "pr-filter-tier1-agent.md",
        "pipeline-filter-agent.md",
    ];

    for fixture in FIXTURES {
        let compiled = compile_fixture(fixture);
        let yaml: Value = serde_yaml::from_str(&compiled)
            .unwrap_or_else(|e| panic!("{fixture} should parse as YAML: {e}"));
        let mut offenders: Vec<String> = Vec::new();
        walk(&yaml, &mut offenders);
        assert!(
            offenders.is_empty(),
            "{fixture}: found redundant re-projections of auto-injected ADO variables in \
             compiled bundle steps (these must be dropped — the runtime auto-injects them): \
             {offenders:?}"
        );
    }
}

/// Regression trap for an entire bug class: ADO step-level `condition:`
/// fields MUST NOT reference `dependencies.<Job>.outputs[...]`. That
/// syntax is only legal in **job**-level conditions, in `variables:`
/// mappings, and in step-level `env:` values (via `$[ ... ]`). Using
/// it in a step condition produces a pipeline-validation error
/// ("Unrecognized value: 'dependencies'") and the build fails before
/// any job step runs.
///
/// This test walks compiled YAML for fixtures exercising both the
/// synth-active (default-mode) and synth-inactive (no-synth) PR
/// contributor paths AND a fixture with `mode: synthetic` + explicit
/// `pr.filters`. If any step's `condition:` value contains the
/// substring `dependencies.`, the test fails with a pointer to the
/// offending step. This protects every extension that emits a step
/// condition, not just `exec_context/pr`.
///
/// History: v6.x emitted `condition: or(eq(variables['Build.Reason'],
/// 'PullRequest'), eq(dependencies.Setup.outputs['synthPr.AW_SYNTHETIC_PR'],
/// 'true'))` on the "Stage PR execution context" step. ADO accepted
/// the YAML at parse time (it IS valid YAML) but rejected the
/// expression at expand time, breaking every synth-active pipeline.
#[test]
fn test_no_step_condition_references_cross_job_dependencies() {
    use serde_yaml::Value;

    /// Fixtures that should exercise different branches of step
    /// `condition:` emission (synth-active, synth-inactive, with
    /// filters, etc.). Add new fixtures here as we add new extensions
    /// or new step-condition emission paths.
    const FIXTURES: &[&str] = &[
        "synthetic-pr-default.md",    // synth-active, no pr.filters
        "execution-context-agent.md", // exec_context_pr active, default config
        "pr-mode-policy.md",          // synth-inactive, on.pr present
        "minimal-agent.md",           // no on.pr at all
    ];

    /// Walk the YAML. For every mapping that has a `condition:` key,
    /// if the value contains `dependencies.`, record the enclosing
    /// `displayName` (or `job` / `stage` as fallback identifiers) so
    /// the failure points at a specific step.
    ///
    /// Distinguishes step vs job/stage by the presence of sibling
    /// keys that are step-exclusive (`bash`, `script`, `task`,
    /// `displayName` is shared, but together with one of the
    /// step keys it's clearly a step). Jobs/stages have their own
    /// `condition:` and DO allow cross-job dep refs, so they must
    /// be filtered OUT.
    fn walk(v: &Value, hits: &mut Vec<String>) {
        match v {
            Value::Mapping(m) => {
                if let Some(Value::String(cond)) = m.get(Value::String("condition".to_string()))
                    && cond.contains("dependencies.")
                {
                    // Decide: step or job/stage? A mapping is a STEP if it
                    // has any of the step-exclusive keys. Otherwise treat
                    // as job/stage (legal location for the ref) and skip.
                    let is_step = ["bash", "script", "task", "powershell", "pwsh", "checkout"]
                        .iter()
                        .any(|k| m.contains_key(Value::String((*k).to_string())));
                    if is_step {
                        let display = m
                            .get(Value::String("displayName".to_string()))
                            .and_then(|d| d.as_str())
                            .unwrap_or("<no displayName>");
                        hits.push(format!(
                            "step `{display}` has illegal cross-job dep ref in condition: `{cond}`"
                        ));
                    }
                }
                for (_, vv) in m {
                    walk(vv, hits);
                }
            }
            Value::Sequence(seq) => {
                for item in seq {
                    walk(item, hits);
                }
            }
            _ => {}
        }
    }

    for fixture in FIXTURES {
        let compiled = compile_fixture_with_flags(fixture, &["--skip-integrity"]);
        let yaml_content: String = compiled
            .lines()
            .skip_while(|line| line.starts_with('#') || line.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        let parsed: Value = serde_yaml::from_str(&yaml_content)
            .unwrap_or_else(|e| panic!("{fixture}: compiled YAML must parse: {e}"));

        let mut hits: Vec<String> = Vec::new();
        walk(&parsed, &mut hits);

        assert!(
            hits.is_empty(),
            "{fixture}: found {} step(s) with illegal cross-job `dependencies.X.outputs[...]` \
             refs in `condition:`. ADO rejects these at pipeline-expansion time with \
             \"Unrecognized value: 'dependencies'\". Project the value through step `env:` \
             (legal via `$[ ... ]`) and gate in the script body instead. Offenders:\n  - {}",
            hits.len(),
            hits.join("\n  - ")
        );
    }
}

/// When the agent is not PR-triggered, the execution-context extension
/// must NOT emit the PR prepare step.
#[test]
fn test_execution_context_pr_not_emitted_when_no_pr_trigger() {
    let compiled = compile_fixture("minimal-agent.md");
    assert!(
        !compiled.contains("Stage PR execution context"),
        "minimal-agent has no on.pr trigger - PR contributor must not activate."
    );
    assert!(
        !compiled.contains("aw-context/pr"),
        "Prompt supplement should not mention PR context when there's no PR trigger"
    );
}

/// v6.2: When the PR contributor activates AND the agent has an
/// explicit (non-wildcard) bash allow-list, the agent's bash allow-list
/// MUST include the read-only git commands so the agent can `git diff`
/// the PR locally inside the AWF sandbox.
#[test]
fn test_execution_context_pr_auto_extends_bash_allowlist() {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let unique_id = COUNTER.fetch_add(1, Ordering::Relaxed);

    let temp_dir = std::env::temp_dir().join(format!(
        "agentic-pipeline-exec-context-bash-{}-{}",
        std::process::id(),
        unique_id,
    ));
    fs::create_dir_all(&temp_dir).expect("Failed to create temp directory");

    let fixture_path = temp_dir.join("pr-bash-restricted.md");
    let body = r#"---
name: "PR bash restricted"
description: "PR-triggered agent with an explicit, restricted bash allow-list"
on:
  pr:
    branches:
      include: [main]
tools:
  bash:
    - "echo"
    - "cat"
---

# PR bash restricted

Body.
"#;
    fs::write(&fixture_path, body).expect("write fixture");

    let output_path = temp_dir.join("pr-bash-restricted.yml");
    let binary_path = PathBuf::from(env!("CARGO_BIN_EXE_ado-aw"));
    let output = std::process::Command::new(&binary_path)
        .args([
            "compile",
            fixture_path.to_str().unwrap(),
            "-o",
            output_path.to_str().unwrap(),
            "--force",
        ])
        .output()
        .expect("run compiler");

    assert!(
        output.status.success(),
        "Compilation must succeed for PR-triggered restricted-bash fixture. stderr: {}",
        String::from_utf8_lossy(&output.stderr),
    );

    let compiled = fs::read_to_string(&output_path).expect("read compiled YAML");

    // Each of these should appear as a `shell(...)` entry in the
    // agent's --allow-tool list.
    for cmd in [
        "shell(git)",
        "shell(git diff)",
        "shell(git log)",
        "shell(git show)",
        "shell(git status)",
        "shell(git rev-parse)",
        "shell(git symbolic-ref)",
    ] {
        assert!(
            compiled.contains(cmd),
            "agent --allow-tool list should contain {cmd:?} after the exec-context PR contributor extends it; \
             not found in compiled YAML"
        );
    }
    // The user's original commands must still be present.
    assert!(
        compiled.contains("shell(echo)") && compiled.contains("shell(cat)"),
        "user-supplied bash entries must remain in the allow-list"
    );

    let _ = fs::remove_dir_all(&temp_dir);
}

/// v6.2: When `execution-context.pr.enabled: false`, the PR contributor
/// MUST NOT extend the agent bash allow-list even on a PR-triggered
/// agent with a restricted bash list.
#[test]
fn test_execution_context_pr_does_not_extend_bash_when_disabled() {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let unique_id = COUNTER.fetch_add(1, Ordering::Relaxed);

    let temp_dir = std::env::temp_dir().join(format!(
        "agentic-pipeline-exec-context-bash-disabled-{}-{}",
        std::process::id(),
        unique_id,
    ));
    fs::create_dir_all(&temp_dir).expect("Failed to create temp directory");

    let fixture_path = temp_dir.join("pr-disabled.md");
    let body = r#"---
name: "PR exec-context disabled"
description: "PR-triggered agent that opts out of execution-context"
on:
  pr:
    branches:
      include: [main]
execution-context:
  pr:
    enabled: false
tools:
  bash:
    - "echo"
---

# PR disabled

Body.
"#;
    fs::write(&fixture_path, body).expect("write fixture");

    let output_path = temp_dir.join("pr-disabled.yml");
    let binary_path = PathBuf::from(env!("CARGO_BIN_EXE_ado-aw"));
    let output = std::process::Command::new(&binary_path)
        .args([
            "compile",
            fixture_path.to_str().unwrap(),
            "-o",
            output_path.to_str().unwrap(),
            "--force",
        ])
        .output()
        .expect("run compiler");

    assert!(
        output.status.success(),
        "Compilation must succeed. stderr: {}",
        String::from_utf8_lossy(&output.stderr),
    );

    let compiled = fs::read_to_string(&output_path).expect("read compiled YAML");

    assert!(
        !compiled.contains("Stage PR execution context"),
        "Prepare step must not be emitted when exec-context.pr is explicitly disabled"
    );
    assert!(
        !compiled.contains("shell(git diff)"),
        "Bash allow-list must NOT be extended with git commands when exec-context.pr is disabled"
    );
}

/// v6.2 footgun fix: explicit `execution-context.pr.enabled: true` on
/// an agent WITHOUT an `on.pr` trigger must NOT activate the PR
/// contributor. Otherwise the agent's bash allow-list silently widens
/// (the 7 git commands get added at compile time) for a step that can
/// never run (the runtime condition gate is `Build.Reason ==
/// 'PullRequest'`).
#[test]
fn test_execution_context_pr_enabled_true_without_on_pr_is_inactive() {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let unique_id = COUNTER.fetch_add(1, Ordering::Relaxed);

    let temp_dir = std::env::temp_dir().join(format!(
        "agentic-pipeline-exec-context-pr-no-trigger-{}-{}",
        std::process::id(),
        unique_id,
    ));
    fs::create_dir_all(&temp_dir).expect("Failed to create temp directory");

    let fixture_path = temp_dir.join("pr-enabled-no-trigger.md");
    // No `on.pr` is configured; `execution-context.pr.enabled: true`
    // alone must not be enough to activate the contributor.
    let body = r#"---
name: "PR enabled without on.pr"
description: "Verify explicit pr.enabled does not override missing on.pr"
on:
  schedule: daily around 14:00
execution-context:
  pr:
    enabled: true
tools:
  bash:
    - "echo"
---

# PR enabled without on.pr

Body.
"#;
    fs::write(&fixture_path, body).expect("write fixture");

    let output_path = temp_dir.join("pr-enabled-no-trigger.yml");
    let binary_path = PathBuf::from(env!("CARGO_BIN_EXE_ado-aw"));
    let output = std::process::Command::new(&binary_path)
        .args([
            "compile",
            fixture_path.to_str().unwrap(),
            "-o",
            output_path.to_str().unwrap(),
            "--force",
        ])
        .output()
        .expect("run compiler");

    assert!(
        output.status.success(),
        "Compilation must succeed. stderr: {}",
        String::from_utf8_lossy(&output.stderr),
    );

    let compiled = fs::read_to_string(&output_path).expect("read compiled YAML");

    assert!(
        !compiled.contains("Stage PR execution context"),
        "Prepare step must not be emitted when on.pr is not configured, \
         even with explicit `pr.enabled: true`"
    );
    assert!(
        !compiled.contains("shell(git diff)"),
        "Bash allow-list must NOT be silently widened with git commands when \
         on.pr is not configured (compile-time artifact for a step that can \
         never run is a footgun)"
    );
}

// v6.2 correctness fix: in BOTH the synthetic-merge-commit path and
// the progressive-deepening path, `BASE_SHA` is the true common
// ancestor (`git merge-base`), so `git diff $BASE..$HEAD` produces
// the same change set regardless of which path runs. v7: this
// invariant is now enforced by the `exec-context-pr.js` bundle (see
// `merge-base.ts::resolveMergeBase`); the vitest tests for synthetic
// merge-base deepening and fail-closed behavior guard the regression
// there. This Rust-side test is removed —
// asserting bash literals against a node-invocation step makes no
// sense.

// v6.2 defence-in-depth: PR target/source branches (which get
// interpolated into git refspecs) are validated with a strict allowlist regex.
// v7: this validation now lives in the `exec-context-pr.js` bundle
// (see `validate.ts::PR_BRANCH_RE`); the vitest tests under
// `validate.test.ts` guard the regression there. This Rust-side
// test is removed for the same reason.

// ─── Synthetic-from-ci snapshot fixtures (issue #916) ────────────────────────

/// Fixture A: agent with on.pr and default `mode: synthetic`.
/// Compiled YAML must contain the full synth wiring (synthPr Setup step,
/// PR_SYNTH_SPEC env, broadened exec-context-pr.js condition, agent-job
/// AW_SYNTHETIC_PR_SKIP guard). The CI trigger must NOT be auto-narrowed
/// to `pr.branches.include` — those are PR target branches, and narrowing
/// would suppress CI on the feature branches synthPr actually needs.
#[test]
fn test_synthetic_pr_default_emits_full_synth_wiring() {
    let compiled = compile_fixture_with_flags("synthetic-pr-default.md", &["--skip-integrity"]);
    assert_valid_yaml(&compiled, "synthetic-pr-default.md");

    // synthPr step in Setup job, before prGate.
    assert!(
        compiled.contains("name: synthPr"),
        "Fixture A must emit the synthPr Setup-job step"
    );
    assert!(
        compiled.contains("PR_SYNTH_SPEC:"),
        "Fixture A must emit the PR_SYNTH_SPEC env var carrying the base64 spec"
    );
    assert!(
        compiled.contains("exec-context-pr-synth.js"),
        "Fixture A must reference the synth bundle path"
    );

    // Broadened exec-context-pr gate — now a bash guard rather than a
    // step-level `condition:` (cross-job `dependencies.Setup.outputs[...]`
    // refs are ILLEGAL in step `condition:`; ADO rejects them with
    // "Unrecognized value: 'dependencies'"). `synthPr` now always runs
    // and emits a single resolved `AW_PR_ID` (real on PR builds,
    // discovered on synth-promoted CI builds), so the gate collapses
    // to a single empty-check.
    assert!(
        compiled.contains("if [ -z \"$AW_PR_ID\" ]; then"),
        "Fixture A must include the bash gate on empty AW_PR_ID (replaces the previous \
         BUILD_REASON + AW_SYNTHETIC_PR pair — the merge now happens inside synthPr)"
    );
    assert!(
        compiled.contains("SYSTEM_PULLREQUEST_PULLREQUESTID: $(AW_PR_ID)"),
        "Fixture A's exec-context-pr step must read the hoisted Agent-job-level \
         AW_PR_ID via the $() macro (NOT a $[ ... ] runtime expression in step env — \
         ADO doesn't evaluate those there, see build #612528)"
    );
    assert!(
        compiled.contains("SYSTEM_PULLREQUEST_SOURCEBRANCH: $(AW_PR_SOURCEBRANCH)"),
        "Fixture A's exec-context-pr step must receive the hoisted PR source ref so \
         synthetic-merge fallback deepening can fetch both PR parents"
    );
    // The Agent-job-level hoist itself must be present and pull from
    // the cross-job synth outputs (legal scope for `dependencies.X.outputs[...]`).
    for name in &[
        "AW_PR_ID",
        "AW_PR_TARGETBRANCH",
        "AW_PR_SOURCEBRANCH",
        "AW_SYNTHETIC_PR",
    ] {
        let needle =
            format!("{name}: $[ coalesce(dependencies.Setup.outputs['synthPr.{name}'], '') ]");
        assert!(
            compiled.contains(&needle),
            "Fixture A must hoist `synthPr.{name}` into Agent-job-level `variables:` \
             so step-env consumers can read `$({name})` safely"
        );
    }

    // Agent-job AW_SYNTHETIC_PR_SKIP guard.
    assert!(
        compiled.contains("ne(dependencies.Setup.outputs['synthPr.AW_SYNTHETIC_PR_SKIP'], 'true')"),
        "Fixture A's Agent-job condition must honour the synth-skip flag"
    );
    // NOTE: this fixture does not declare `on.pr.filters`, so the
    // Agent-job condition has only the skip guard (no AND-NOT gate
    // clause). The `eq(synthPr.AW_SYNTHETIC_PR, 'true')` literal is
    // therefore expected ONLY in the Agent-job-level `variables:` hoist
    // and the cross-job condition arms — never as an Agent-job OR-arm
    // in the dependsOn condition, which would silently bypass the gate
    // for real-PR or synth-PR builds. A separate fixture covers the
    // gate-enforced shape when `pr.filters` is present.

    // No auto-narrowed CI trigger — `pr.branches.include` lists PR TARGET
    // branches, and ADO `trigger:` fires on pushes TO listed branches, so
    // narrowing would suppress CI on the feature branches synthPr needs.
    assert!(
        !compiled.contains("trigger:\n  branches:\n    include:\n      - 'main'"),
        "Fixture A must NOT auto-narrow the top-level CI trigger to pr.branches.include — \
         narrowing to PR target branches would defeat synthPr by suppressing CI on the \
         feature branches it must react to"
    );
}

/// Fixture B: agent with on.pr and `mode: policy`.
/// The compiled YAML must contain NONE of the synthesis artefacts AND
/// must emit `trigger: none` so feature-branch pushes do not queue
/// duplicate CI builds alongside the operator's branch-policy-driven PR
/// builds.
#[test]
fn test_pr_mode_policy_omits_synth_and_emits_trigger_none() {
    let compiled = compile_fixture_with_flags("pr-mode-policy.md", &["--skip-integrity"]);
    assert_valid_yaml(&compiled, "pr-mode-policy.md");

    for needle in &[
        "synthPr",
        "AW_SYNTHETIC_PR",
        "PR_SYNTH_SPEC",
        "exec-context-pr-synth",
    ] {
        assert!(
            !compiled.contains(needle),
            "mode: policy must produce zero synth artefacts; \
             found {needle} in compiled YAML"
        );
    }

    // CI trigger must be suppressed so we don't double-queue with the
    // policy-driven PR build.
    assert!(
        compiled.contains("trigger: none"),
        "mode: policy must emit `trigger: none` so feature-branch pushes do not \
         queue duplicate CI builds alongside the branch-policy-driven PR build"
    );

    // And of course must NOT auto-narrow either (defensive).
    assert!(
        !compiled.contains("trigger:\n  branches:\n    include:\n      - 'main'"),
        "mode: policy must not emit a narrowed CI trigger"
    );
}

// ─── Internal supply-chain (feed + registry mirror) tests ───────────────────

/// Compile a small inline agent body and return (success, stdout, stderr).
fn compile_inline_source(name: &str, source: &str) -> (bool, String, String) {
    let temp_dir = std::env::temp_dir().join(format!(
        "agentic-pipeline-supply-chain-{name}-{}",
        std::process::id()
    ));
    fs::create_dir_all(&temp_dir).expect("Failed to create temp directory");
    let input_path = temp_dir.join(format!("{name}.md"));
    let output_path = temp_dir.join(format!("{name}.yml"));
    fs::write(&input_path, source).unwrap();

    let binary_path = PathBuf::from(env!("CARGO_BIN_EXE_ado-aw"));
    let output = std::process::Command::new(&binary_path)
        .args([
            "compile",
            input_path.to_str().unwrap(),
            "-o",
            output_path.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to run compiler");
    let compiled = fs::read_to_string(&output_path).unwrap_or_default();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let _ = fs::remove_dir_all(&temp_dir);
    (output.status.success(), compiled, stderr)
}

/// Extract a single `- job: <name>` block from compiled YAML, from its header
/// up to (but not including) the next top-level job header. Used to assert that
/// a step lands in the expected job.
fn job_block<'a>(compiled: &'a str, job: &str) -> &'a str {
    // Anchor with a trailing newline so e.g. job "Agent" does not match a
    // hypothetical "Agent_Reviewed" header that happens to appear first.
    let header = format!("- job: {job}\n");
    let start = compiled
        .find(&header)
        .unwrap_or_else(|| panic!("job '{job}' not found in:\n{compiled}"));
    let rest = &compiled[start..];
    // `rest` begins with this job's header (no leading newline), so the first
    // "\n- job: " match is the NEXT top-level job header. Slicing at that
    // newline's byte offset (always a valid UTF-8 boundary) yields this job's
    // block — no fixed byte-width assumption about the leading character.
    match rest.find("\n- job: ") {
        Some(next) => &rest[..next],
        None => rest,
    }
}

/// With `supply-chain.feed` + `supply-chain.registry` configured, every
/// GitHub/GHCR fetch is rerouted to the internal feed + registry while
/// checksum verification is preserved.
#[test]
fn test_supply_chain_full_reroutes_all_artifacts() {
    let compiled = compile_fixture("supply-chain-agent.md");
    assert_valid_yaml(&compiled, "supply-chain-agent.md");

    // (a) No GitHub release-download URLs remain.
    assert!(
        !compiled.contains("github.com/githubnext/ado-aw/releases"),
        "ado-aw/ado-script GitHub release URLs must be gone in feed mode"
    );
    assert!(
        !compiled.contains("github.com/github/gh-aw-firewall/releases"),
        "AWF GitHub release URLs must be gone in feed mode"
    );
    // (b) No GHCR image *pulls* remain — every pull comes from the internal
    // registry. The local `:latest` aliases intentionally keep the GHCR names
    // that AWF resolves by default with `--skip-pull` (see (d2) below).
    assert!(
        !compiled.contains("docker pull ghcr.io"),
        "no image may be pulled from GHCR in registry mode"
    );

    // (c) Standard ADO tasks are present for the binary mirror.
    assert!(
        compiled.contains("- task: NuGetAuthenticate@1"),
        "NuGetAuthenticate@1 must be emitted for the feed mirror"
    );
    assert!(
        compiled.contains("nuGetServiceConnections: feed-conn"),
        "feed's resolved service connection must be passed to NuGetAuthenticate@1"
    );
    for definition in [
        "definition: ado-aw",
        "definition: awf",
        "definition: ado-script",
    ] {
        assert!(
            compiled.contains(definition),
            "DownloadPackage@1 must pull {definition}"
        );
    }
    assert!(
        compiled.contains("feed: my-project/my-internal-feed"),
        "DownloadPackage@1 must target the configured feed"
    );
    // Auth is hoisted to one NuGetAuthenticate@1 per job, not per artifact, so
    // there are strictly fewer auth steps than DownloadPackage@1 steps.
    let auth_count = compiled.matches("- task: NuGetAuthenticate@1").count();
    let download_count = compiled.matches("- task: DownloadPackage@1").count();
    assert!(
        auth_count < download_count,
        "NuGetAuthenticate@1 should be hoisted per-job (got {auth_count} auth vs {download_count} downloads)"
    );

    // (d) Internal registry rewrite + ACR auth for images. The configured
    // registry base path (`myacr.azurecr.io/oss-mirror`) is an arbitrary
    // namespace — the original GHCR prefix (`github/...`) is NOT preserved;
    // only the artifact name (`squid`/`agent`/`gh-aw-mcpg`) sits directly under
    // the base path. ACR login derives the registry name from the host.
    assert!(
        compiled.contains("- task: AzureCLI@2") && compiled.contains("az acr login --name myacr"),
        "ACR login must be emitted before docker pull in registry mode"
    );
    assert!(
        compiled.contains("docker pull myacr.azurecr.io/oss-mirror/squid:"),
        "AWF images must be pulled from the internal registry base path (artifact name only)"
    );
    assert!(
        compiled.contains("myacr.azurecr.io/oss-mirror/gh-aw-mcpg:"),
        "MCPG image must be rewritten onto the internal registry base path (pull + docker run)"
    );
    assert!(
        !compiled.contains("myacr.azurecr.io/oss-mirror/github/"),
        "the original GHCR `github/...` prefix must not be carried under the internal base path"
    );

    // (d2) The local `:latest` aliases must be tagged under the GHCR names AWF
    // resolves by default with `--skip-pull` — tagged from the internally
    // pulled image, never pulled from GHCR. Regression guard for the firewall
    // failing to find its images at runtime. Use version-agnostic split
    // assertions so an AWF_VERSION bump (unimportable in a binary-only crate)
    // does not break the test.
    assert!(
        compiled.contains("docker tag myacr.azurecr.io/oss-mirror/squid:")
            && compiled.contains(" ghcr.io/github/gh-aw-firewall/squid:latest"),
        "AWF squid image must be re-tagged to the GHCR :latest name AWF expects"
    );
    assert!(
        compiled.contains("docker tag myacr.azurecr.io/oss-mirror/agent:")
            && compiled.contains(" ghcr.io/github/gh-aw-firewall/agent:latest"),
        "AWF agent image must be re-tagged to the GHCR :latest name AWF expects"
    );

    // (e) Checksum verification is retained.
    assert!(
        compiled.contains("sha256sum -c"),
        "checksum verification must be preserved on the internal branch"
    );
}

/// Absent `supply-chain:` leaves the default GitHub/GHCR fetch path intact.
#[test]
fn test_supply_chain_absent_uses_github_and_ghcr() {
    let compiled = compile_fixture("minimal-agent.md");
    assert!(
        compiled.contains("github.com/githubnext/ado-aw/releases"),
        "default path must fetch the compiler from GitHub Releases"
    );
    assert!(
        compiled.contains("ghcr.io/github/gh-aw-firewall/squid:"),
        "default path must pull AWF images from GHCR"
    );
    assert!(
        !compiled.contains("DownloadPackage@1"),
        "default path must not emit DownloadPackage@1"
    );
    assert!(
        !compiled.contains("az acr login"),
        "default path must not emit ACR login"
    );
}

/// `feed` only (scalar, same-org) mirrors binaries via `$(System.AccessToken)`
/// — no `nuGetServiceConnections` — and leaves images on GHCR.
#[test]
fn test_supply_chain_feed_only_keeps_ghcr_and_uses_system_token() {
    let source = r#"---
name: "Feed Only"
description: "feed scalar, same-org System.AccessToken"
supply-chain:
  feed: my-internal-feed
---

## Body
"#;
    let (ok, compiled, stderr) = compile_inline_source("feed-only", source);
    assert!(ok, "feed-only config should compile: {stderr}");

    assert!(
        compiled.contains("- task: NuGetAuthenticate@1"),
        "feed mirror must authenticate"
    );
    assert!(
        !compiled.contains("nuGetServiceConnections"),
        "same-org feed with no connection must use System.AccessToken (no nuGetServiceConnections)"
    );
    assert!(
        compiled.contains("definition: ado-script"),
        "ado-script bundle must come from the feed"
    );
    // Registry not configured → images stay on GHCR, no ACR login.
    assert!(
        compiled.contains("ghcr.io/github/gh-aw-firewall/squid:"),
        "images must stay on GHCR when registry is unset"
    );
    assert!(
        !compiled.contains("az acr login"),
        "no ACR login when registry is unset"
    );
}

/// `registry` without any resolvable service connection is rejected at compile
/// time (ACR has no `$(System.AccessToken)` path).
#[test]
fn test_supply_chain_registry_without_connection_fails() {
    let source = r#"---
name: "Bad Registry"
description: "registry without a connection"
supply-chain:
  registry: myacr.azurecr.io
---

## Body
"#;
    let (ok, _compiled, stderr) = compile_inline_source("bad-registry", source);
    assert!(!ok, "registry without a connection must fail to compile");
    assert!(
        stderr.contains("supply-chain.registry requires a service connection"),
        "error must explain the missing registry connection: {stderr}"
    );
}

/// A top-level `service-connection` is used as the fallback for both targets
/// when neither declares its own.
#[test]
fn test_supply_chain_top_level_connection_fallback() {
    let source = r#"---
name: "Shared Conn"
description: "shared fallback connection"
supply-chain:
  feed: my-project/my-internal-feed
  registry: myacr.azurecr.io
  service-connection: shared-conn
---

## Body
"#;
    let (ok, compiled, stderr) = compile_inline_source("shared-conn", source);
    assert!(ok, "shared-connection config should compile: {stderr}");
    assert!(
        compiled.contains("nuGetServiceConnections: shared-conn"),
        "feed must fall back to the top-level connection"
    );
    assert!(
        compiled.contains("azureSubscription: shared-conn"),
        "registry ACR login must fall back to the top-level connection"
    );
}

// ─────────────────────────────────────────────────────────────────────
// Manual review gate (ManualValidation@1 agentless job)
// ─────────────────────────────────────────────────────────────────────

/// A global `safe-outputs.require-approval: true` inserts a single agentless
/// ManualReview gate between Detection and SafeOutputs, and SafeOutputs depends
/// on it (fail-closed).
#[test]
fn test_require_approval_global_emits_manual_review_gate() {
    let source = r#"---
name: "Approval Agent"
description: "Agent whose outputs require manual review"
safe-outputs:
  require-approval: true
  create-pull-request:
    target-branch: main
---

## Body
"#;
    let (ok, compiled, stderr) = compile_inline_source("approval-global", source);
    assert!(ok, "require-approval pipeline should compile: {stderr}");
    assert!(
        compiled.contains("job: ManualReview"),
        "expected a ManualReview job:\n{compiled}"
    );
    assert!(
        compiled.contains("pool: server"),
        "ManualReview must be an agentless server job:\n{compiled}"
    );
    assert!(
        compiled.contains("task: ManualValidation@1"),
        "expected a ManualValidation@1 task:\n{compiled}"
    );
    // The gate only fires when the agent actually proposed a reviewed output.
    assert!(
        compiled.contains("HasReviewedProposals"),
        "expected a HasReviewedProposals detection/gate:\n{compiled}"
    );
    // SafeOutputs is gated behind the review job.
    let so_idx = compiled
        .find("job: SafeOutputs")
        .expect("SafeOutputs job present");
    let so_tail = &compiled[so_idx..];
    assert!(
        so_tail.contains("ManualReview"),
        "SafeOutputs dependsOn must include ManualReview:\n{so_tail}"
    );
}

/// 1ES-target variant: the agentless `ManualReview` server job must emit
/// `pool: server` and must NOT be wrapped in a `templateContext` block (the
/// 1ES path skips the wrap for server jobs — see `onees_ir.rs`).
#[test]
fn test_require_approval_emits_server_pool_on_1es_target() {
    let source = r#"---
name: "Approval Agent 1ES"
description: "Manual review on the 1ES target"
target: 1es
safe-outputs:
  require-approval: true
  create-pull-request:
    target-branch: main
---

## Body
"#;
    let (ok, compiled, stderr) = compile_inline_source("approval-1es", source);
    assert!(ok, "1ES require-approval pipeline should compile: {stderr}");

    // Isolate the ManualReview job block (1ES nests jobs under stages, so the
    // `- job:` header is indented — match on the bare "job: ManualReview" and
    // slice to the next "- job: " header).
    let review_idx = compiled
        .find("job: ManualReview")
        .expect("ManualReview job present on 1ES");
    let review_block = &compiled[review_idx..];
    let review_block = match review_block.find("- job: ") {
        // Skip the current header, then find the following job header.
        Some(_) => {
            let after_header = &review_block["job: ManualReview".len()..];
            match after_header.find("- job: ") {
                Some(rel) => &review_block[.."job: ManualReview".len() + rel],
                None => review_block,
            }
        }
        None => review_block,
    };

    assert!(
        review_block.contains("pool: server"),
        "ManualReview must be an agentless server job on 1ES:\n{review_block}"
    );
    assert!(
        review_block.contains("task: ManualValidation@1"),
        "expected the ManualValidation@1 task on 1ES:\n{review_block}"
    );
    // The 1ES server job must not carry a templateContext wrapper.
    assert!(
        !review_block.contains("templateContext"),
        "1ES ManualReview server job must NOT be wrapped in templateContext:\n{review_block}"
    );
}

/// Without any `require-approval`, no ManualReview job or ManualValidation task
/// is emitted (zero behavior change for existing pipelines).
#[test]
fn test_no_require_approval_omits_manual_review_gate() {
    let source = r#"---
name: "Plain Agent"
description: "Agent with no manual review"
safe-outputs:
  create-pull-request:
    target-branch: main
---

## Body
"#;
    let (ok, compiled, stderr) = compile_inline_source("approval-none", source);
    assert!(ok, "pipeline should compile: {stderr}");
    assert!(
        !compiled.contains("ManualReview"),
        "no ManualReview job expected:\n{compiled}"
    );
    assert!(
        !compiled.contains("ManualValidation@1"),
        "no ManualValidation task expected:\n{compiled}"
    );
}

/// Mixed approval — one reviewed tool, one automatic — splits execution into
/// an automatic SafeOutputs job (runs immediately) and a gated
/// SafeOutputs_Reviewed job behind ManualReview, each with the right filter.
#[test]
fn test_mixed_approval_splits_execution_jobs() {
    let source = r#"---
name: "Mixed Agent"
description: "Mixed approval split"
safe-outputs:
  create-pull-request:
    target-branch: main
    require-approval: true
  add-pr-comment: {}
---

## Body
"#;
    let (ok, compiled, stderr) = compile_inline_source("approval-mixed", source);
    assert!(ok, "mixed approval pipeline should compile: {stderr}");
    // Two execution jobs exist.
    assert!(
        compiled.contains("job: SafeOutputs_Reviewed"),
        "reviewed SafeOutputs job expected:\n{compiled}"
    );
    // Automatic job excludes the reviewed tool; reviewed job runs only it.
    assert!(
        compiled.contains("--exclude create-pull-request"),
        "automatic job must exclude the reviewed tool:\n{compiled}"
    );
    assert!(
        compiled.contains("--only create-pull-request"),
        "reviewed job must run only the reviewed tool:\n{compiled}"
    );
    // Distinct published artifacts (no collision).
    assert!(
        compiled.contains("artifact: safe_outputs_reviewed"),
        "reviewed job must publish a distinct artifact:\n{compiled}"
    );
    // The reviewed job is gated behind ManualReview; the auto job is not.
    let rev_idx = compiled
        .find("job: SafeOutputs_Reviewed")
        .expect("reviewed job present");
    assert!(
        compiled[rev_idx..].contains("ManualReview"),
        "reviewed job must depend on ManualReview:\n{}",
        &compiled[rev_idx..]
    );
}

/// In the mixed split, the Conclusion job DOES depend on the reviewed
/// SafeOutputs job and surfaces its result via AW_SAFEOUTPUTS_REVIEWED_RESULT,
/// so a reviewer rejection (failing SafeOutputs_Reviewed) is reported.
#[test]
fn test_mixed_approval_conclusion_reports_reviewed_result() {
    let source = r#"---
name: "Mixed Conclusion Agent"
description: "Mixed approval split conclusion wiring"
safe-outputs:
  create-pull-request:
    target-branch: main
    require-approval: true
  add-pr-comment: {}
---

## Body
"#;
    let (ok, compiled, stderr) = compile_inline_source("approval-mixed-conclusion", source);
    assert!(ok, "mixed approval pipeline should compile: {stderr}");
    assert!(
        compiled.contains("job: SafeOutputs_Reviewed"),
        "reviewed SafeOutputs job expected:\n{compiled}"
    );
    assert!(
        compiled.contains("AW_SAFEOUTPUTS_REVIEWED_RESULT"),
        "Conclusion must surface the reviewed job result:\n{compiled}"
    );
}

/// In the mixed split, the Teardown job depends only on the automatic
/// `SafeOutputs` job — never on the human-gated `SafeOutputs_Reviewed` job —
/// so cleanup still fires on the common no-reviewed-proposal path (where the
/// reviewed job is skipped) and never blocks behind the approval gate.
#[test]
fn test_mixed_approval_teardown_skips_reviewed_dependency() {
    let source = r#"---
name: "Mixed Teardown Agent"
description: "Mixed approval split with teardown"
safe-outputs:
  create-pull-request:
    target-branch: main
    require-approval: true
  add-pr-comment: {}
teardown:
  - script: echo "cleanup"
    displayName: "Cleanup"
---

## Body
"#;
    let (ok, compiled, stderr) = compile_inline_source("approval-mixed-teardown", source);
    assert!(ok, "mixed approval + teardown should compile: {stderr}");
    assert!(
        compiled.contains("job: SafeOutputs_Reviewed"),
        "reviewed SafeOutputs job expected:\n{compiled}"
    );

    // Isolate the Teardown job block (from its header to the next job header).
    let td_idx = compiled
        .find("job: Teardown")
        .expect("Teardown job present");
    let td_block = &compiled[td_idx..];
    let td_block = match td_block[1..].find("- job: ") {
        Some(rel) => &td_block[..rel + 1],
        None => td_block,
    };

    assert!(
        td_block.contains("SafeOutputs"),
        "Teardown must depend on the automatic SafeOutputs job:\n{td_block}"
    );
    assert!(
        !td_block.contains("SafeOutputs_Reviewed"),
        "Teardown must NOT depend on the human-gated SafeOutputs_Reviewed job:\n{td_block}"
    );
}

/// The safe-outputs summary step is rendered at the END of the Agent job (and
/// NOT in the Detection job) whenever a manual-review pipeline is compiled, and
/// it passes the reviewed tool list to the bundle.
#[test]
fn test_safe_outputs_summary_step_emitted_for_review_pipeline() {
    let source = r#"---
name: "Summary Review Agent"
description: "Manual review with summary"
safe-outputs:
  create-pull-request:
    target-branch: main
    require-approval: true
  add-pr-comment: {}
---

## Body
"#;
    let (ok, compiled, stderr) = compile_inline_source("summary-review", source);
    assert!(ok, "pipeline should compile: {stderr}");

    // The render step + uploadsummary wiring is present.
    assert!(
        compiled.contains("Render safe-outputs summary"),
        "expected the summary render step:\n{compiled}"
    );
    assert!(
        compiled.contains("approval-summary.js"),
        "expected the approval-summary bundle invocation:\n{compiled}"
    );
    // Namespaced output base name (no tab-title collision with consumers).
    assert!(
        compiled.contains("ado-aw-safe-outputs.md"),
        "expected the namespaced summary output path:\n{compiled}"
    );
    // The reviewed tool is passed; the automatic tool is not in the reviewed list.
    assert!(
        compiled.contains("AW_REVIEWED_TOOLS: create-pull-request"),
        "expected the reviewed tool list passed via env:\n{compiled}"
    );

    // The step lives in the Agent job, not the Detection job.
    let agent_block = job_block(&compiled, "Agent");
    let detection_block = job_block(&compiled, "Detection");
    assert!(
        agent_block.contains("Render safe-outputs summary"),
        "summary step must be in the Agent job:\n{agent_block}"
    );
    assert!(
        !detection_block.contains("Render safe-outputs summary"),
        "summary step must NOT be in the Detection job:\n{detection_block}"
    );
}

/// The summary step is also emitted for a plain safe-outputs pipeline with no
/// approval configured (always-on transparency), with an empty reviewed list.
#[test]
fn test_safe_outputs_summary_step_emitted_for_plain_pipeline() {
    let source = r#"---
name: "Summary Plain Agent"
description: "Plain safe outputs with summary"
safe-outputs:
  create-pull-request:
    target-branch: main
---

## Body
"#;
    let (ok, compiled, stderr) = compile_inline_source("summary-plain", source);
    assert!(ok, "pipeline should compile: {stderr}");
    assert!(
        compiled.contains("Render safe-outputs summary"),
        "expected the summary render step for a plain pipeline:\n{compiled}"
    );
    // No reviewed tools → the env var is present but empty.
    assert!(
        compiled.contains("AW_REVIEWED_TOOLS:"),
        "expected the reviewed-tools env var (empty):\n{compiled}"
    );
    assert!(
        !compiled.contains("AW_REVIEWED_TOOLS: create-pull-request"),
        "no tool should be reviewed in a plain pipeline:\n{compiled}"
    );
}

/// With no safe-output tools enabled at all, the summary step is not emitted.
#[test]
fn test_safe_outputs_summary_step_absent_without_safe_outputs() {
    let source = r#"---
name: "No Safe Outputs Agent"
description: "Agent with no safe outputs"
---

## Body
"#;
    let (ok, compiled, stderr) = compile_inline_source("summary-absent", source);
    assert!(ok, "pipeline should compile: {stderr}");
    assert!(
        !compiled.contains("Render safe-outputs summary"),
        "no summary step expected without safe outputs:\n{compiled}"
    );
    assert!(
        !compiled.contains("approval-summary.js"),
        "no approval-summary bundle invocation expected:\n{compiled}"
    );
}

/// A detailed `require-approval` object propagates approvers, notify-users, and
/// the author-supplied instructions message into the ManualValidation task.
#[test]
fn test_require_approval_object_propagates_settings() {
    let source = r#"---
name: "Detailed Approval Agent"
description: "Agent with detailed approval settings"
safe-outputs:
  create-pull-request:
    target-branch: main
    require-approval:
      approvers: ["[MyOrg]\\release-team"]
      notify-users: ["ops@example.com"]
      instructions: "Please double-check the proposed PR before approving."
      on-timeout: reject
---

## Body
"#;
    let (ok, compiled, stderr) = compile_inline_source("approval-object", source);
    assert!(ok, "detailed approval pipeline should compile: {stderr}");
    assert!(
        compiled.contains("task: ManualValidation@1"),
        "expected ManualValidation@1:\n{compiled}"
    );
    assert!(
        compiled.contains("ops@example.com"),
        "notifyUsers should carry the configured email:\n{compiled}"
    );
    assert!(
        compiled.contains("release-team"),
        "approvers should carry the configured group:\n{compiled}"
    );
    assert!(
        compiled.contains("Please double-check the proposed PR before approving."),
        "author instructions should be emitted:\n{compiled}"
    );
}

/// When multiple tools require approval and several carry their own
/// `instructions`, the single gate message must list every reviewed tool and
/// include ALL author notes — none silently dropped (regression test for the
/// old "first tool wins" behaviour).
#[test]
fn test_require_approval_aggregates_all_tool_instructions() {
    let source = r#"---
name: "Multi Approval Agent"
description: "Several reviewed tools with distinct instructions"
safe-outputs:
  create-pull-request:
    target-branch: main
    require-approval:
      instructions: "Verify the PR targets main and has tests."
  create-work-item:
    require-approval:
      instructions: "Check the work-item priority and area path."
  add-pr-comment:
    require-approval: true
---

## Body
"#;
    let (ok, compiled, stderr) = compile_inline_source("approval-multi-instr", source);
    assert!(ok, "multi-tool approval pipeline should compile: {stderr}");
    // Every reviewed tool is enumerated in the gate message.
    assert!(
        compiled.contains("add-pr-comment, create-pull-request, create-work-item"),
        "gate message must list every reviewed tool:\n{compiled}"
    );
    // BOTH distinct author notes are present — not just the first.
    assert!(
        compiled.contains("Verify the PR targets main and has tests."),
        "first tool's instructions must be present:\n{compiled}"
    );
    assert!(
        compiled.contains("Check the work-item priority and area path."),
        "second tool's instructions must also be present (not dropped):\n{compiled}"
    );
    // On this branch the aggregated message points reviewers at the summary tab.
    assert!(
        compiled.contains("'ado-aw-safe-outputs' summary tab"),
        "aggregated gate message must point at the summary tab:\n{compiled}"
    );
}

/// `timeout-minutes` must bound the **task** (`ManualValidation@1`'s
/// `timeoutInMinutes`) — that is the timeout that fires the `onTimeout`
/// handler. The agentless job carries a strictly-larger outer bound so a
/// job-level cancellation never preempts the task's graceful `onTimeout`.
#[test]
fn test_require_approval_timeout_bounds_task_not_just_job() {
    let source = r#"---
name: "Timeout Approval Agent"
description: "Approval with a pending-period timeout"
safe-outputs:
  create-pull-request:
    target-branch: main
    require-approval:
      timeout-minutes: 120
      on-timeout: resume
---

## Body
"#;
    let (ok, compiled, stderr) = compile_inline_source("approval-timeout", source);
    assert!(ok, "timeout approval pipeline should compile: {stderr}");

    // Isolate the ManualReview job block.
    let idx = compiled
        .find("- job: ManualReview")
        .expect("ManualReview job present");
    let block = &compiled[idx..];
    let block = match block[1..].find("\n- job: ") {
        Some(rel) => &block[..rel + 1],
        None => block,
    };

    // The ManualValidation@1 task carries the configured timeout (the one that
    // triggers onTimeout: resume).
    let task_idx = block
        .find("task: ManualValidation@1")
        .expect("ManualValidation task present");
    assert!(
        block[task_idx..].contains("timeoutInMinutes: 120"),
        "the ManualValidation task must carry timeoutInMinutes: 120:\n{block}"
    );
    // The job-level timeout is strictly larger so it can't preempt the task's
    // graceful onTimeout (120 + 5 grace = 125).
    let job_timeout_idx = block
        .find("timeoutInMinutes:")
        .expect("job timeout present");
    assert!(
        block[job_timeout_idx..].starts_with("timeoutInMinutes: 125"),
        "the job-level timeout must be the strictly-larger outer bound (125):\n{block}"
    );
}

// ── Front-matter task-step validation (advisory; surfaced via lint) ──────────

/// An authored task step with invalid inputs — here `CopyFiles@2` missing the
/// required `TargetFolder` and carrying an unknown `Bogus` input — must:
///   1. NOT fail the compile (validation is advisory and lives in `lint`), and
///   2. still be passed through to the generated YAML verbatim, and
///   3. NOT print a warning during compile — the validation feedback is
///      surfaced through `ado-aw lint` / the `lint_workflow` MCP tool instead,
///      so compile (which also re-runs in-pipeline for integrity) stays quiet.
#[test]
fn invalid_task_input_compiles_silently_and_preserves_passthrough() {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let unique_id = COUNTER.fetch_add(1, Ordering::Relaxed);

    let temp_dir = std::env::temp_dir().join(format!(
        "agentic-pipeline-task-validate-{}-{}",
        std::process::id(),
        unique_id,
    ));
    fs::create_dir_all(&temp_dir).expect("create temp dir");

    let fixture_src = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("invalid-task-input-agent.md");
    let fixture_path = temp_dir.join("invalid-task-input-agent.md");
    fs::copy(&fixture_src, &fixture_path).expect("copy fixture");
    let output_path = temp_dir.join("invalid-task-input-agent.yml");

    let binary_path = PathBuf::from(env!("CARGO_BIN_EXE_ado-aw"));
    let output = std::process::Command::new(&binary_path)
        .args([
            "compile",
            fixture_path.to_str().unwrap(),
            "-o",
            output_path.to_str().unwrap(),
        ])
        .output()
        .expect("run compiler");

    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let compiled = fs::read_to_string(&output_path).unwrap_or_default();
    let _ = fs::remove_dir_all(&temp_dir);

    // (1) Never fail: the process still exits 0.
    assert!(
        output.status.success(),
        "compile must succeed despite invalid task inputs; stderr:\n{stderr}"
    );
    // (2) Passthrough preserved verbatim — validation never alters output.
    assert!(
        compiled.contains("CopyFiles@2"),
        "the authored task step must be emitted unchanged:\n{compiled}"
    );
    assert!(
        compiled.contains("Bogus"),
        "the (invalid) input must be passed through unchanged:\n{compiled}"
    );
    // (3) Compile does NOT emit the task-validation warning — that feedback is
    // surfaced through lint, not compile (see `ado-aw lint` integration test).
    assert!(
        !stderr.contains("CopyFiles@2"),
        "compile must not warn about task inputs (that belongs to lint); got:\n{stderr}"
    );
}

/// A reserved `self` entry in `repos:` tunes the auto-generated
/// `checkout: self` step in every job (fetchDepth + fetchTags) without emitting
/// a repository resource or an extra checkout.
#[test]
fn test_repos_self_entry_tunes_all_self_checkouts() {
    let source = r#"---
name: "Self Checkout Tuning"
description: "shallow, no tags on self"
repos:
  - name: self
    fetch-depth: 1
    fetch-tags: false
---

## Body
"#;
    let (ok, compiled, stderr) = compile_inline_source("self-checkout-tuning", source);
    assert!(ok, "self-checkout tuning should compile: {stderr}");

    let self_checkouts = compiled.matches("- checkout: self").count();
    assert!(
        self_checkouts >= 2,
        "expected multiple `checkout: self` steps, found {self_checkouts}:\n{compiled}"
    );
    // Every self checkout carries the tuned fetch options.
    assert_eq!(
        compiled.matches("fetchDepth: 1").count(),
        self_checkouts,
        "each `checkout: self` must carry fetchDepth: 1:\n{compiled}"
    );
    assert_eq!(
        compiled.matches("fetchTags: false").count(),
        self_checkouts,
        "each `checkout: self` must carry fetchTags: false:\n{compiled}"
    );
    // `self` must NOT add an extra repository resource — only the canonical
    // `SelfRepo` (always emitted) should be present.
    assert_eq!(
        compiled.matches("repository: self").count(),
        1,
        "the reserved `self` entry must not emit an extra repository resource:\n{compiled}"
    );
}

/// `fetch-depth` / `fetch-tags` on a named `repos:` entry tune that
/// repository's checkout step (which lands in the Agent job).
#[test]
fn test_repos_named_entry_fetch_tuning_emitted() {
    let source = r#"---
name: "Named Checkout Tuning"
description: "shallow named repo"
repos:
  - name: my-org/monorepo
    fetch-depth: 1
    fetch-tags: false
---

## Body
"#;
    let (ok, compiled, stderr) = compile_inline_source("named-checkout-tuning", source);
    assert!(ok, "named checkout tuning should compile: {stderr}");

    let agent = job_block(&compiled, "Agent");
    assert!(
        agent.contains("- checkout: monorepo"),
        "the named repo must be checked out in the Agent job:\n{agent}"
    );
    assert!(
        agent.contains("fetchDepth: 1") && agent.contains("fetchTags: false"),
        "the named checkout must carry the tuned fetch options:\n{agent}"
    );
    // Without a `self` entry, `checkout: self` stays at ADO defaults.
    assert!(
        !agent.contains("- checkout: self\n  fetchDepth"),
        "checkout: self must stay untuned when only a named repo is tuned:\n{agent}"
    );
}

/// Regression: with no `repos:` fetch tuning, checkout steps stay bare (ADO
/// defaults), so existing agents compile byte-for-byte unchanged.
#[test]
fn test_no_repos_fetch_tuning_emits_bare_checkout() {
    let source = r#"---
name: "Bare Checkout"
description: "no fetch tuning"
---

## Body
"#;
    let (ok, compiled, stderr) = compile_inline_source("bare-checkout", source);
    assert!(ok, "bare agent should compile: {stderr}");
    assert!(
        compiled.contains("- checkout: self"),
        "the self checkout must still be emitted:\n{compiled}"
    );
    assert!(
        !compiled.contains("fetchDepth"),
        "no fetchDepth key without tuning:\n{compiled}"
    );
    assert!(
        !compiled.contains("fetchTags"),
        "no fetchTags key without tuning:\n{compiled}"
    );
}

// ─── GitHub App-backed Copilot engine auth (issue #1316) ─────────────────────

/// Compile inline agent `content` in an isolated temp dir and return the
/// compiled YAML. Panics on compile failure.
fn compile_inline_agent(tag: &str, content: &str) -> String {
    let temp_dir =
        std::env::temp_dir().join(format!("agentic-pipeline-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&temp_dir);
    fs::create_dir_all(&temp_dir).expect("Failed to create temp directory");
    let input = temp_dir.join(format!("{tag}-agent.md"));
    fs::write(&input, content).expect("Failed to write test input");
    let output_path = temp_dir.join(format!("{tag}-agent.yml"));
    let binary_path = PathBuf::from(env!("CARGO_BIN_EXE_ado-aw"));
    let output = std::process::Command::new(&binary_path)
        .args([
            "compile",
            input.to_str().unwrap(),
            "-o",
            output_path.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to run compiler");
    assert!(
        output.status.success(),
        "compile should succeed for {tag}.\nstderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let compiled = fs::read_to_string(&output_path).expect("Compiled YAML should exist");
    let _ = fs::remove_dir_all(&temp_dir);
    compiled
}

const GITHUB_APP_TOKEN_FM: &str = r#"
engine:
  id: copilot
  github-app-token:
    app-id: 1234567
    owner: octo-org
    repositories: [octo-repo]
"#;

/// Assert the GitHub App token wiring is present in exactly the Agent and
/// Detection jobs (two mint steps, two `GITHUB_APP_TOKEN`-sourced tokens) and
/// nowhere else, for a given compiled pipeline.
fn assert_github_app_token_wiring(compiled: &str) {
    // Total bundle invocations = mint (Agent + Detection) + revoke
    // (Agent + Detection) = 4. Revoke invocations carry the ` revoke` arg.
    let total_bundle = compiled
        .matches("node '/tmp/ado-aw-scripts/ado-script/github-app-token.js'")
        .count();
    let revoke_hits = compiled
        .matches("node '/tmp/ado-aw-scripts/ado-script/github-app-token.js' revoke")
        .count();
    let mint_hits = total_bundle - revoke_hits;
    assert_eq!(
        mint_hits, 2,
        "expected the mint step in exactly Agent + Detection, found {mint_hits}:\n{compiled}"
    );
    let mint_display = compiled
        .matches("Mint GitHub App token (Copilot engine auth)")
        .count();
    assert_eq!(mint_display, 2, "expected two mint-step display names");

    // GITHUB_TOKEN is sourced from the minted masked variable in both Copilot
    // envs (agent + detection), never from the operator's $(GITHUB_TOKEN).
    let app_token_src = compiled
        .matches("GITHUB_TOKEN: $(GITHUB_APP_TOKEN)")
        .count();
    assert_eq!(
        app_token_src, 2,
        "expected GITHUB_TOKEN sourced from $(GITHUB_APP_TOKEN) in agent + detection:\n{compiled}"
    );
    assert!(
        !compiled.contains("GITHUB_TOKEN: $(GITHUB_TOKEN)"),
        "no Copilot env should use the operator $(GITHUB_TOKEN) when App auth is configured:\n{compiled}"
    );

    // Non-secret inputs are single-quoted argv flags (shadow-proof); the app-id
    // is a single-quoted literal and the private key (default variable name,
    // since the fixture omits `private-key`) is the only GH_APP_* env var.
    assert!(
        compiled.contains("--app-id '1234567'"),
        "app-id must be a single-quoted literal argv flag:\n{compiled}"
    );
    assert!(compiled.contains("GH_APP_PRIVATE_KEY: $(GITHUB_APP_PRIVATE_KEY)"));
    // The private key is the ONLY GH_APP_* env var; every other input is argv.
    for key in [
        "GH_APP_ID:",
        "GH_APP_OWNER:",
        "GH_APP_REPOSITORIES:",
        "GH_APP_OUTPUT_VAR:",
        "GH_APP_API_URL:",
    ] {
        assert!(
            !compiled.contains(key),
            "{key} must not be an env var (non-secret inputs are argv):\n{compiled}"
        );
    }

    // The output variable name is pinned as an argv flag in both mint steps, so
    // no pipeline variable named GH_APP_OUTPUT_VAR can redirect the minted
    // token (argv comes only from the compiler-authored script).
    let output_var_pins = compiled.matches("--output-var 'GITHUB_APP_TOKEN'").count();
    assert_eq!(
        output_var_pins, 2,
        "expected --output-var pinned to GITHUB_APP_TOKEN in both mint steps:\n{compiled}"
    );

    // By default the token is revoked after the Copilot run in both jobs
    // (revoke_hits computed above).
    assert_eq!(
        revoke_hits, 2,
        "expected a revoke step in Agent + Detection by default, found {revoke_hits}:\n{compiled}"
    );
    assert!(
        compiled.contains("GH_APP_TOKEN: $(GITHUB_APP_TOKEN)"),
        "revoke step reads the minted token from $(GITHUB_APP_TOKEN):\n{compiled}"
    );
}

#[test]
fn test_github_app_token_wiring_standalone() {
    let content = format!(
        "---\nname: \"GH App Standalone\"\ndescription: \"gh app token\"{GITHUB_APP_TOKEN_FM}---\n\n## Agent\n\nDo work.\n"
    );
    let compiled = compile_inline_agent("ghapp-standalone", &content);
    assert_github_app_token_wiring(&compiled);
}

#[test]
fn test_github_app_token_wiring_all_targets() {
    for target in ["1es", "job", "stage"] {
        let content = format!(
            "---\nname: \"GH App {target}\"\ndescription: \"gh app token\"\ntarget: {target}{GITHUB_APP_TOKEN_FM}---\n\n## Agent\n\nDo work.\n"
        );
        let compiled = compile_inline_agent(&format!("ghapp-{target}"), &content);
        assert_github_app_token_wiring(&compiled);
    }
}

#[test]
fn test_no_github_app_token_by_default() {
    let content =
        "---\nname: \"No GH App\"\ndescription: \"default token\"\n---\n\n## Agent\n\nDo work.\n";
    let compiled = compile_inline_agent("ghapp-absent", content);
    assert!(
        !compiled.contains("github-app-token.js"),
        "default agent must not emit the mint step:\n{compiled}"
    );
    assert!(
        !compiled.contains("GITHUB_APP_TOKEN"),
        "default agent must not reference GITHUB_APP_TOKEN:\n{compiled}"
    );
    assert!(
        compiled.contains("GITHUB_TOKEN: $(GITHUB_TOKEN)"),
        "default agent sources GITHUB_TOKEN from the operator variable:\n{compiled}"
    );
}

#[test]
fn test_github_app_token_skip_revocation() {
    let content = "---\nname: \"GH App No Revoke\"\ndescription: \"gh app token, no revoke\"\nengine:\n  id: copilot\n  github-app-token:\n    app-id: 1234567\n    owner: octo-org\n    skip-token-revocation: true\n---\n\n## Agent\n\nDo work.\n";
    let compiled = compile_inline_agent("ghapp-norevoke", content);
    // Mint step still present in Agent + Detection.
    assert_eq!(
        compiled
            .matches("node '/tmp/ado-aw-scripts/ado-script/github-app-token.js'")
            .count()
            - compiled
                .matches("node '/tmp/ado-aw-scripts/ado-script/github-app-token.js' revoke")
                .count(),
        2,
        "mint step must still be present:\n{compiled}"
    );
    // ...but no revoke step.
    assert!(
        !compiled.contains("github-app-token.js' revoke"),
        "skip-token-revocation must suppress the revoke step:\n{compiled}"
    );
}

#[test]
fn test_github_app_token_rejected_on_non_copilot_engine() {
    // github-app-token on a non-copilot engine must be a hard compile error
    // (not a silent no-op): the minted token is only wired into GITHUB_TOKEN on
    // the Copilot path.
    let source = "---\nname: \"GH App Wrong Engine\"\ndescription: \"non-copilot + app token\"\nengine:\n  id: claude\n  github-app-token:\n    app-id: 1234567\n    owner: octo-org\n---\n\n## Agent\n\nDo work.\n";
    let (ok, _compiled, stderr) = compile_inline_source("ghapp-wrong-engine", source);
    assert!(
        !ok,
        "compile must fail for github-app-token on a non-copilot engine.\nstderr: {stderr}"
    );
    assert!(
        stderr.contains("github-app-token") && stderr.contains("copilot"),
        "error must explain github-app-token requires the copilot engine.\nstderr: {stderr}"
    );
}

#[test]
fn test_github_app_token_literal_app_id_and_api_url() {
    // This fixture also exercises the private-key OVERRIDE (explicit GH_APP_KEY).
    let content = "---\nname: \"GH App Literal\"\ndescription: \"literal app id + ghes\"\nengine:\n  id: copilot\n  github-app-token:\n    app-id: 1234567\n    private-key: GH_APP_KEY\n    owner: octo-org\n    api-url: https://ghe.example.com/api/v3\n---\n\n## Agent\n\nDo work.\n";
    let compiled = compile_inline_agent("ghapp-literal", content);
    // Numeric app-id is emitted verbatim as a single-quoted argv flag, not a macro.
    assert!(
        compiled.contains("--app-id '1234567'"),
        "literal numeric app-id must be a single-quoted verbatim argv flag:\n{compiled}"
    );
    assert!(
        !compiled.contains("--app-id '$("),
        "literal app-id must not be treated as a variable macro:\n{compiled}"
    );
    // The private-key override names GH_APP_KEY as the masked secret env var.
    assert!(
        compiled.contains("GH_APP_PRIVATE_KEY: $(GH_APP_KEY)"),
        "private-key override must source the secret from $(GH_APP_KEY):\n{compiled}"
    );
    assert!(
        !compiled.contains("$(GITHUB_APP_PRIVATE_KEY)"),
        "override must replace the default private-key variable:\n{compiled}"
    );
    // GHES api-url flows into both mint and revoke steps as an argv flag.
    // Mint: `... --api-url '...'`; revoke: `revoke --api-url '...'`.
    let api_url_args = compiled
        .matches("--api-url 'https://ghe.example.com/api/v3'")
        .count();
    assert_eq!(
        api_url_args, 4,
        "api-url must appear as an argv flag in both mint and both revoke steps (2 jobs x 2):\n{compiled}"
    );
    assert!(
        !compiled.contains("GH_APP_API_URL:"),
        "api-url must be argv, never an env var:\n{compiled}"
    );
}

#[test]
fn test_github_app_token_hyphenated_private_key_variable() {
    let content = "---\nname: \"GH App Hyphen Key\"\ndescription: \"hyphenated key vault variable\"\nengine:\n  id: copilot\n  github-app-token:\n    app-id: 1234567\n    private-key: AGENTIC-WORKFLOWS-GITHUB-APP-PRIVATE-KEY\n    owner: octo-org\n---\n\n## Agent\n\nDo work.\n";
    let compiled = compile_inline_agent("ghapp-hyphen-key", content);
    assert!(
        compiled.contains("GH_APP_PRIVATE_KEY: $(AGENTIC-WORKFLOWS-GITHUB-APP-PRIVATE-KEY)"),
        "hyphenated private-key override must be preserved as the ADO macro target:\n{compiled}"
    );
}

/// When another ado-script bundle feature is active in the Agent job (here a
/// safe-output activates the approval-summary bundle download), the mint step
/// must NOT trigger a second bundle download in that job — it reuses the
/// already-staged bundle. Proven by a delta: adding `github-app-token` to an
/// otherwise-identical workflow adds exactly ONE bundle download (the
/// Detection job, which has no extension-prepare phase), never two.
#[test]
fn test_github_app_token_reuses_staged_bundle_in_agent() {
    fn count_downloads(compiled: &str) -> usize {
        compiled.matches("Download ado-aw scripts").count()
            + compiled.matches("Stage ado-aw scripts").count()
    }
    let safe_output = "safe-outputs:\n  create-work-item:\n    work-item-type: Task\n";

    let without = compile_inline_agent(
        "ghapp-dedupe-without",
        &format!(
            "---\nname: \"No GH App SO\"\ndescription: \"safe output only\"\n{safe_output}---\n\n## Agent\n\nDo work.\n"
        ),
    );
    let with = compile_inline_agent(
        "ghapp-dedupe-with",
        &format!(
            "---\nname: \"GH App SO\"\ndescription: \"gh app token with safe output\"{GITHUB_APP_TOKEN_FM}{safe_output}---\n\n## Agent\n\nDo work.\n"
        ),
    );
    assert_github_app_token_wiring(&with);

    // Adding github-app-token stages the bundle only in Detection (Agent
    // reuses its already-staged copy), so the download count grows by exactly 1.
    assert_eq!(
        count_downloads(&with),
        count_downloads(&without) + 1,
        "github-app-token must add exactly one bundle download (Detection), \
         proving the Agent job reuses its staged bundle rather than \
         double-downloading. without={}, with={}",
        count_downloads(&without),
        count_downloads(&with),
    );
}

// ─── Variable group imports (issue #1385) ────────────────────────────────

/// A standalone pipeline with `variable-groups:` emits a top-level
/// `variables:` block containing one `- group: <name>` import per entry, in
/// declaration order. Only group names appear — never any variable values.
#[test]
fn variable_groups_standalone_emits_group_imports_in_order() {
    let (ok, compiled, stderr) = compile_inline_source(
        "vg-standalone",
        "---\nname: vg-standalone\ndescription: variable group import test\nvariable-groups:\n  - Agentic Workflows\n  - Shared Secrets\n---\n\n## Agent\n\nDo work.\n",
    );
    assert!(ok, "standalone compile with variable-groups should succeed.\nstderr: {stderr}");
    assert!(compiled.contains("variables:"), "top-level variables: block must be present:\n{compiled}");
    assert!(compiled.contains("- group: Agentic Workflows"), "first group import missing:\n{compiled}");
    assert!(compiled.contains("- group: Shared Secrets"), "second group import missing:\n{compiled}");
    let first = compiled.find("Agentic Workflows").unwrap();
    let second = compiled.find("Shared Secrets").unwrap();
    assert!(first < second, "group imports must preserve declaration order");
}

/// A 1ES pipeline (`target: 1es`) also emits the `variables:` group import at
/// the pipeline root, alongside the `extends:` block.
#[test]
fn variable_groups_onees_emits_group_imports() {
    let (ok, compiled, stderr) = compile_inline_source(
        "vg-onees",
        "---\nname: vg-onees\ndescription: variable group import test\ntarget: 1es\nvariable-groups:\n  - Agentic Workflows\n---\n\n## Agent\n\nDo work.\n",
    );
    assert!(ok, "1es compile with variable-groups should succeed.\nstderr: {stderr}");
    assert!(compiled.contains("extends:"), "1ES pipeline must still emit extends:\n{compiled}");
    assert!(compiled.contains("variables:"), "top-level variables: block must be present:\n{compiled}");
    assert!(compiled.contains("- group: Agentic Workflows"), "group import missing:\n{compiled}");
}

/// `target: job` cannot carry pipeline-level `variables:`, so a non-empty
/// `variable-groups:` there is a hard compile-time error naming the target.
#[test]
fn variable_groups_rejected_for_job_target() {
    let (ok, _compiled, stderr) = compile_inline_source(
        "vg-job",
        "---\nname: vg-job\ndescription: variable group import test\ntarget: job\nvariable-groups:\n  - Agentic Workflows\n---\n\n## Agent\n\nDo work.\n",
    );
    assert!(!ok, "job target with variable-groups must fail to compile");
    assert!(stderr.contains("variable-groups"), "error must mention variable-groups:\n{stderr}");
    assert!(stderr.contains("target: job"), "error must name the job target:\n{stderr}");
}

/// `target: stage` is rejected for the same reason as `target: job`.
#[test]
fn variable_groups_rejected_for_stage_target() {
    let (ok, _compiled, stderr) = compile_inline_source(
        "vg-stage",
        "---\nname: vg-stage\ndescription: variable group import test\ntarget: stage\nvariable-groups:\n  - Agentic Workflows\n---\n\n## Agent\n\nDo work.\n",
    );
    assert!(!ok, "stage target with variable-groups must fail to compile");
    assert!(stderr.contains("variable-groups"), "error must mention variable-groups:\n{stderr}");
    assert!(stderr.contains("target: stage"), "error must name the stage target:\n{stderr}");
}

/// A group name that carries an ADO macro expression is rejected as an unsafe
/// reference (defense in depth — only literal group names belong here).
#[test]
fn variable_groups_rejects_injection_name() {
    let (ok, _compiled, stderr) = compile_inline_source(
        "vg-inject",
        "---\nname: vg-inject\ndescription: variable group import test\nvariable-groups:\n  - \"$(evil)\"\n---\n\n## Agent\n\nDo work.\n",
    );
    assert!(!ok, "an injection-bearing group name must fail to compile");
    assert!(stderr.contains("variable group name") || stderr.contains("variable-groups entry"),
        "error must explain the invalid group name:\n{stderr}");
}

/// A workflow that references a GitHub App private key held in a project-level
/// variable group can import that group AND mint the token from the same
/// source — no generated-lock hand-patching required. The compiled lock
/// contains both the `- group:` import and the `$(...)` macro reference.
#[test]
fn variable_groups_with_github_app_token_compiles_without_patch() {
    let (ok, compiled, stderr) = compile_inline_source(
        "vg-ghapp",
        "---\nname: vg-ghapp\ndescription: variable group + github app token\nvariable-groups:\n  - Agentic Workflows\nengine:\n  id: copilot\n  github-app-token:\n    app-id: 1234567\n    owner: octo-org\n    repositories: [octo-repo]\n    private-key: AGENTIC_WORKFLOWS_GITHUB_APP_PRIVATE_KEY\n---\n\n## Agent\n\nDo work.\n",
    );
    assert!(ok, "compile with variable-groups + github-app-token should succeed.\nstderr: {stderr}");
    assert!(compiled.contains("- group: Agentic Workflows"), "group import missing:\n{compiled}");
    assert!(compiled.contains("$(AGENTIC_WORKFLOWS_GITHUB_APP_PRIVATE_KEY)"),
        "private-key macro reference missing:\n{compiled}");
}

// ─────────────────────────────────────────────────────────────────────
// create-pull-request base-ref prepare step (issue #1413)
// ─────────────────────────────────────────────────────────────────────

/// When `create-pull-request` is configured, the Agent job runs the
/// `prepare-pr-base` bundle before the Copilot step, passing the default
/// target branch and projecting the ADO bearer, so the host-side SafeOutputs
/// MCP server can compute a diff base on shallow-default pools.
#[test]
fn test_create_pull_request_emits_prepare_pr_base_step_in_agent() {
    let compiled = compile_inline_agent(
        "prepare-pr-base-default",
        "---\nname: \"PR Agent\"\ndescription: \"opens a PR\"\nsafe-outputs:\n  create-pull-request:\n    target-branch: main\n---\n\n## Agent\n\nDo work.\n",
    );
    let agent = job_block(&compiled, "Agent");
    assert!(
        agent.contains("node '/tmp/ado-aw-scripts/ado-script/prepare-pr-base.js' --repo-dir \"$(Build.SourcesDirectory)\" --target-branch 'main'"),
        "Agent job must invoke prepare-pr-base with the self dir/target pair:\n{agent}"
    );
    assert!(
        agent.contains("Prepare create-pull-request base ref (fetch/deepen)"),
        "Agent job must contain the prepare step display name:\n{agent}"
    );
    assert!(
        agent.contains("SYSTEM_ACCESSTOKEN: $(System.AccessToken)"),
        "prepare-pr-base step must project the ADO bearer:\n{agent}"
    );
}

/// The configured `target-branch` is threaded verbatim into the prepare step.
#[test]
fn test_create_pull_request_prepare_step_uses_configured_target_branch() {
    let compiled = compile_inline_agent(
        "prepare-pr-base-custom",
        "---\nname: \"PR Agent\"\ndescription: \"opens a PR\"\nsafe-outputs:\n  create-pull-request:\n    target-branch: release/2.x\n---\n\n## Agent\n\nDo work.\n",
    );
    let agent = job_block(&compiled, "Agent");
    assert!(
        agent.contains("--target-branch 'release/2.x'"),
        "Agent job must pass the configured target branch:\n{agent}"
    );
}

/// A bare `create-pull-request: {}` (no `target-branch`) still emits the prepare
/// step, targeting the default branch (`main`) — exercises the implicit-default
/// fallback end-to-end.
#[test]
fn test_create_pull_request_prepare_step_defaults_target_branch() {
    let compiled = compile_inline_agent(
        "prepare-pr-base-implicit",
        "---\nname: \"PR Agent\"\ndescription: \"opens a PR\"\nsafe-outputs:\n  create-pull-request: {}\n---\n\n## Agent\n\nDo work.\n",
    );
    let agent = job_block(&compiled, "Agent");
    assert!(
        agent.contains("node '/tmp/ado-aw-scripts/ado-script/prepare-pr-base.js' --repo-dir \"$(Build.SourcesDirectory)\" --target-branch 'main'"),
        "bare create-pull-request must emit the prepare step targeting 'main':\n{agent}"
    );
    // Single `self` checkout ⇒ exactly one --repo-dir (the working directory).
    assert_eq!(
        agent.matches("--repo-dir ").count(),
        1,
        "self-only agent must emit exactly one --repo-dir:\n{agent}"
    );
    assert!(
        agent.contains("--repo-dir \"$(Build.SourcesDirectory)\""),
        "self-only --repo-dir must be the working directory:\n{agent}"
    );
}

/// Multi-repo: with additional `checkout:` repos, the prepare step deepens the
/// target branch in the self working dir AND each alias dir — mirroring
/// `mcp.rs::resolve_git_dir_for_patch` so a PR to any allowed repo works on a
/// shallow pool.
#[test]
fn test_create_pull_request_prepare_step_covers_all_checkout_repos() {
    let compiled = compile_inline_agent(
        "prepare-pr-base-multi",
        "---\nname: \"Multi PR Agent\"\ndescription: \"opens PRs across repos\"\nworkspace: root\nrepos:\n  - my-org/tools\n  - my-org/lib\nsafe-outputs:\n  create-pull-request:\n    target-branch: main\n---\n\n## Agent\n\nDo work.\n",
    );
    let agent = job_block(&compiled, "Agent");
    // self (working_directory == $(Build.SourcesDirectory) under workspace: root)
    // + one dir per derived alias (my-org/tools -> tools, my-org/lib -> lib).
    assert_eq!(
        agent.matches("--repo-dir ").count(),
        3,
        "multi-repo agent must emit one --repo-dir per allowed repo (self + 2 aliases):\n{agent}"
    );
    assert!(
        agent.contains("--repo-dir \"$(Build.SourcesDirectory)\""),
        "self dir must be covered:\n{agent}"
    );
    assert!(
        agent.contains("--repo-dir \"$(Build.SourcesDirectory)/tools\""),
        "tools alias dir must be covered:\n{agent}"
    );
    assert!(
        agent.contains("--repo-dir \"$(Build.SourcesDirectory)/lib\""),
        "lib alias dir must be covered:\n{agent}"
    );
}

/// Per-repo target branches: with `infer-target-from-checkout-ref`, each checkout
/// repo's prepare pair targets its OWN `repos: ref`; an explicit `target-branches`
/// override wins; `self` falls back to the literal `target-branch`.
#[test]
fn test_create_pull_request_prepare_step_per_repo_targets() {
    let compiled = compile_inline_agent(
        "prepare-pr-base-per-repo",
        "---\nname: \"Meta PR Agent\"\ndescription: \"per-repo targets\"\nworkspace: root\nrepos:\n  - name: my-org/tools\n    ref: refs/heads/release\n  - name: my-org/docs\n    ref: refs/heads/main\nsafe-outputs:\n  create-pull-request:\n    target-branch: main\n    infer-target-from-checkout-ref: true\n    target-branches:\n      docs: gh-pages\n---\n\n## Agent\n\nDo work.\n",
    );
    let agent = job_block(&compiled, "Agent");
    // self ⇒ literal default 'main' (self never infers).
    assert!(
        agent.contains("--repo-dir \"$(Build.SourcesDirectory)\" --target-branch 'main'"),
        "self must target the literal default 'main':\n{agent}"
    );
    // tools ⇒ inferred from its checkout ref (refs/heads/release → release).
    assert!(
        agent.contains("--repo-dir \"$(Build.SourcesDirectory)/tools\" --target-branch 'release'"),
        "tools must target its inferred checkout ref 'release':\n{agent}"
    );
    // docs ⇒ explicit target-branches override wins over inference.
    assert!(
        agent.contains("--repo-dir \"$(Build.SourcesDirectory)/docs\" --target-branch 'gh-pages'"),
        "docs must target the explicit override 'gh-pages':\n{agent}"
    );
}

/// Issue #1453: the executor (`ado-aw execute`) that runs `create-pull-request`
/// lives in the **SafeOutputs** job, which has its own fresh checkout. The
/// prepare step must therefore ALSO be emitted in the SafeOutputs job —
/// immediately before the executor and after staging the ado-script bundle —
/// so `git worktree add origin/<target>` resolves on shallow-default pools.
#[test]
fn test_create_pull_request_emits_prepare_pr_base_step_in_safeoutputs() {
    let compiled = compile_inline_agent(
        "prepare-pr-base-safeoutputs",
        "---\nname: \"PR Agent\"\ndescription: \"opens a PR\"\nsafe-outputs:\n  create-pull-request:\n    target-branch: main\n---\n\n## Agent\n\nDo work.\n",
    );
    let safeoutputs = job_block(&compiled, "SafeOutputs");
    assert!(
        safeoutputs.contains("node '/tmp/ado-aw-scripts/ado-script/prepare-pr-base.js' --repo-dir \"$(Build.SourcesDirectory)\" --target-branch 'main'"),
        "SafeOutputs job must invoke prepare-pr-base with the self dir/target pair:\n{safeoutputs}"
    );
    assert!(
        safeoutputs.contains("Prepare create-pull-request base ref (fetch/deepen)"),
        "SafeOutputs job must contain the prepare step display name:\n{safeoutputs}"
    );
    // The bundle is otherwise only staged in the Agent/Setup jobs; the
    // SafeOutputs job must stage it too so the prepare step's script exists.
    assert!(
        safeoutputs.contains("/tmp/ado-aw-scripts/ado-script.zip"),
        "SafeOutputs job must stage the ado-script bundle:\n{safeoutputs}"
    );
    // The prepare step must run BEFORE the executor so the ref is landed first.
    let prepare_at = safeoutputs
        .find("Prepare create-pull-request base ref")
        .expect("prepare step present");
    let execute_at = safeoutputs
        .find("Execute safe outputs (Stage 3)")
        .expect("executor present");
    assert!(
        prepare_at < execute_at,
        "prepare step must precede the executor in the SafeOutputs job:\n{safeoutputs}"
    );
}

/// The SafeOutputs-job prepare step honours per-repo targets identically to the
/// Agent-job step (shared `create_pr_prepare_repos` resolver), so the branch it
/// deepens always matches the branch the executor opens the PR into.
#[test]
fn test_create_pull_request_safeoutputs_prepare_step_covers_all_checkout_repos() {
    let compiled = compile_inline_agent(
        "prepare-pr-base-safeoutputs-multi",
        "---\nname: \"Meta PR Agent\"\ndescription: \"per-repo targets\"\nworkspace: root\nrepos:\n  - name: my-org/tools\n    ref: refs/heads/release\n  - name: my-org/docs\n    ref: refs/heads/main\nsafe-outputs:\n  create-pull-request:\n    target-branch: main\n    infer-target-from-checkout-ref: true\n    target-branches:\n      docs: gh-pages\n---\n\n## Agent\n\nDo work.\n",
    );
    let safeoutputs = job_block(&compiled, "SafeOutputs");
    assert!(
        safeoutputs
            .contains("--repo-dir \"$(Build.SourcesDirectory)\" --target-branch 'main'"),
        "self must target the literal default 'main' in the SafeOutputs job:\n{safeoutputs}"
    );
    assert!(
        safeoutputs
            .contains("--repo-dir \"$(Build.SourcesDirectory)/tools\" --target-branch 'release'"),
        "tools must target its inferred checkout ref 'release':\n{safeoutputs}"
    );
    assert!(
        safeoutputs
            .contains("--repo-dir \"$(Build.SourcesDirectory)/docs\" --target-branch 'gh-pages'"),
        "docs must target the explicit override 'gh-pages':\n{safeoutputs}"
    );
}

/// Split-approval: when `create-pull-request` is review-gated but another tool
/// is not, execution splits into an auto `SafeOutputs` job (excludes the PR
/// tool) and a `SafeOutputs_Reviewed` job (runs only it). The prepare step +
/// bundle download must appear ONLY in the reviewed job — the auto job never
/// runs `create-pull-request`, so paying for the fetch/deepen there is wasted
/// (issue #1453 review).
#[test]
fn test_create_pull_request_prepare_step_only_in_running_variant_when_gated() {
    let compiled = compile_inline_agent(
        "prepare-pr-base-gated",
        "---\nname: \"PR Agent\"\ndescription: \"opens a PR\"\nsafe-outputs:\n  add-build-tag:\n    tag: ci\n  create-pull-request:\n    target-branch: main\n    require-approval: true\n---\n\n## Agent\n\nDo work.\n",
    );
    let auto = job_block(&compiled, "SafeOutputs");
    let reviewed = job_block(&compiled, "SafeOutputs_Reviewed");
    // Reviewed job runs create-pull-request → gets the prepare step + bundle.
    assert!(
        reviewed.contains("Prepare create-pull-request base ref (fetch/deepen)"),
        "reviewed job must contain the prepare step:\n{reviewed}"
    );
    assert!(
        reviewed.contains("/tmp/ado-aw-scripts/ado-script.zip"),
        "reviewed job must stage the ado-script bundle:\n{reviewed}"
    );
    // Auto job excludes create-pull-request → no prepare step, no bundle.
    assert!(
        !auto.contains("Prepare create-pull-request base ref"),
        "auto job must NOT contain the prepare step (it never runs the PR tool):\n{auto}"
    );
    assert!(
        !auto.contains("prepare-pr-base.js"),
        "auto job must NOT invoke prepare-pr-base:\n{auto}"
    );
}

/// The mirror of the gated case: when `create-pull-request` is NOT gated but a
/// sibling tool is, the PR tool runs in the auto `SafeOutputs` job, so the
/// prepare step belongs there and NOT in `SafeOutputs_Reviewed`.
#[test]
fn test_create_pull_request_prepare_step_in_auto_variant_when_sibling_gated() {
    let compiled = compile_inline_agent(
        "prepare-pr-base-auto",
        "---\nname: \"PR Agent\"\ndescription: \"opens a PR\"\nsafe-outputs:\n  add-build-tag:\n    tag: ci\n    require-approval: true\n  create-pull-request:\n    target-branch: main\n---\n\n## Agent\n\nDo work.\n",
    );
    let auto = job_block(&compiled, "SafeOutputs");
    let reviewed = job_block(&compiled, "SafeOutputs_Reviewed");
    assert!(
        auto.contains("Prepare create-pull-request base ref (fetch/deepen)"),
        "auto job must contain the prepare step (it runs the PR tool):\n{auto}"
    );
    assert!(
        !reviewed.contains("Prepare create-pull-request base ref"),
        "reviewed job must NOT contain the prepare step:\n{reviewed}"
    );
}

/// An agent WITHOUT `create-pull-request` emits no prepare-pr-base step and
/// never downloads the bundle.
#[test]
fn test_no_create_pull_request_omits_prepare_pr_base_step() {
    let compiled = compile_inline_agent(
        "prepare-pr-base-absent",
        "---\nname: \"WI Agent\"\ndescription: \"files a work item\"\nsafe-outputs:\n  create-work-item:\n    work-item-type: Task\n---\n\n## Agent\n\nDo work.\n",
    );
    assert!(
        !compiled.contains("prepare-pr-base.js"),
        "prepare-pr-base must not appear without create-pull-request:\n{compiled}"
    );
    assert!(
        !compiled.contains("Prepare create-pull-request base ref"),
        "prepare step display name must be absent:\n{compiled}"
    );
}
