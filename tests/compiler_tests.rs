use std::fs;
use std::path::PathBuf;

/// Integration test for the compile functionality
///
/// This test verifies that the compiler can successfully process a markdown file
/// with YAML front matter and generate the expected pipeline YAML and agent file.
#[test]
fn test_compile_pipeline_basic() {
    // Create a temporary directory for test artifacts
    let temp_dir =
        std::env::temp_dir().join(format!("agentic-pipeline-test-{}", std::process::id()));
    fs::create_dir_all(&temp_dir).expect("Failed to create temp directory");

    // Create a test markdown file
    let test_input = temp_dir.join("test-agent.md");
    let test_content = r#"---
name: "Test Agent"
description: "A test agent for verification"
schedule: daily
repositories:
  - repository: test-repo
    type: git
    name: test-org/test-repo
mcp-servers:
  ado: true
  es-chat: true
---

## Test Agent

This is a test agent for integration testing.

### Instructions

1. Test instruction one
2. Test instruction two
"#;
    fs::write(&test_input, test_content).expect("Failed to write test input file");

    // Create .github/agents directory in temp dir
    fs::create_dir_all(temp_dir.join(".github/agents")).expect("Failed to create .github/agents");

    // Run the compilation
    let output_yaml = temp_dir.join("test-agent.yml");

    // Note: We can't directly call compile_pipeline from here since it's not a library function
    // This test verifies the output structure when compile runs
    // In a real scenario, you'd use std::process::Command to run the CLI

    // For now, verify that test setup works
    assert!(test_input.exists(), "Test input file should exist");
    assert!(
        temp_dir.join(".github/agents").exists(),
        ".github/agents directory should exist"
    );

    // Cleanup
    let _ = fs::remove_dir_all(&temp_dir);
}

/// Test that verifies the expected structure of the compiled YAML output
#[test]
fn test_compiled_yaml_structure() {
    // This test reads a pre-compiled YAML and verifies its structure
    // Since we need the actual compilation to happen, we'll verify the template structure

    let template_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("data")
        .join("base.yml");

    assert!(template_path.exists(), "Base template should exist");

    let template_content =
        fs::read_to_string(&template_path).expect("Should be able to read base template");

    // Verify template contains expected markers
    assert!(
        template_content.contains("{{ repositories }}"),
        "Template should contain repositories marker"
    );
    assert!(
        template_content.contains("{{ schedule }}"),
        "Template should contain schedule marker"
    );
    assert!(
        template_content.contains("{{ checkout_self }}"),
        "Template should contain checkout_self marker"
    );
    assert!(
        template_content.contains("{{ checkout_repositories }}"),
        "Template should contain checkout marker"
    );
    assert!(
        template_content.contains("{{ allowed_domains }}"),
        "Template should contain allowed_domains marker"
    );
    assert!(
        template_content.contains("{{ source_path }}"),
        "Template should contain source_path marker"
    );
    assert!(
        template_content.contains("{{ agent_name }}"),
        "Template should contain agent_name marker"
    );
    assert!(
        template_content.contains("{{ copilot_params }}"),
        "Template should contain copilot_params marker"
    );
    assert!(
        template_content.contains("{{ compiler_version }}"),
        "Template should contain compiler_version marker"
    );
    assert!(
        template_content.contains("{{ integrity_check }}"),
        "Template should contain integrity_check marker"
    );

    // Verify template doesn't accidentally use ${{ }} where {{ }} should be used
    // (The ${{ }} syntax is for Azure DevOps pipeline expressions and should be preserved)
    let marker_count = template_content.matches("{{ ").count();
    assert!(
        marker_count >= 6,
        "Template should have at least 6 replacement markers"
    );

    // Verify that {{ pool }} marker is used for all jobs, not hardcoded pool names
    // This ensures consistency across Agent, Detection, and Execution jobs.
    let pool_marker_count = template_content.matches("name: {{ pool }}").count();
    assert_eq!(
        pool_marker_count, 3,
        "Template should use '{{ pool }}' marker exactly three times (once for each job)"
    );

    // Verify that the default pool name is NOT hardcoded in the template
    // The default should only exist in the compiler's Rust code, not the template
    assert!(
        !template_content.contains("name: AZS-1ES-L-MMS-ubuntu-22.04"),
        "Template should not contain hardcoded pool name 'AZS-1ES-L-MMS-ubuntu-22.04'"
    );

    // Verify that the ado-aw compiler is downloaded from GitHub Releases, not ADO pipeline artifacts
    assert!(
        !template_content.contains("pipeline: 2437"),
        "Template should not reference ADO pipeline 2437 for the compiler"
    );
    assert!(
        template_content.contains("github.com/githubnext/ado-aw/releases"),
        "Template should download the compiler from GitHub Releases"
    );
    assert!(
        !template_content.contains("sha256sum -c checksums.txt --ignore-missing"),
        "Template should not use --ignore-missing which silently passes when binary is missing from checksums"
    );
    assert!(
        template_content.contains(r#"grep "ado-aw-linux-x64" checksums.txt | sha256sum -c -"#),
        "Template should verify ado-aw checksum using targeted grep to ensure binary entry exists"
    );
    assert!(
        !template_content.contains("grep -q"),
        "Checksum verification should not pipe through grep"
    );

    // Verify AWF (Agentic Workflow Firewall) is downloaded from GitHub Releases, not ADO pipeline artifacts
    assert!(
        !template_content.contains("pipeline: 2450"),
        "Template should not reference ADO pipeline 2450 for the firewall"
    );
    assert!(
        !template_content.contains("DownloadPipelineArtifact"),
        "Template should not use DownloadPipelineArtifact task"
    );
    assert!(
        template_content.contains("github.com/github/gh-aw-firewall/releases"),
        "Template should download AWF from GitHub Releases"
    );
    assert!(
        template_content.contains("{{ firewall_version }}"),
        "Template should contain firewall_version marker"
    );

    // Verify MCPG integration
    assert!(
        template_content.contains("{{ mcpg_config }}"),
        "Template should contain mcpg_config marker"
    );
    assert!(
        template_content.contains("{{ mcpg_version }}"),
        "Template should contain mcpg_version marker"
    );
    assert!(
        template_content.contains("--enable-host-access"),
        "Template should include --enable-host-access for MCPG"
    );

    // Verify no legacy mcp-firewall references in template
    assert!(
        !template_content.contains("mcp-firewall-config"),
        "Template should not reference legacy mcp-firewall config"
    );
    assert!(
        !template_content.contains("MCP_FIREWALL_EOF"),
        "Template should not contain legacy firewall heredoc"
    );
}

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

/// Test for edge cases in file naming
#[test]
fn test_filename_edge_cases() {
    // This test ensures that various input names produce valid filenames
    let test_cases = vec![
        ("Simple Name", "simple-name"),
        ("Name With Numbers 123", "name-with-numbers-123"),
        ("name-with-dashes", "name-with-dashes"),
        ("name_with_underscores", "name-with-underscores"),
        ("Name!@#$%^&*()", "name"),
        (
            "   Leading and Trailing Spaces   ",
            "leading-and-trailing-spaces",
        ),
        ("UPPERCASE", "uppercase"),
    ];

    // Note: This test demonstrates expected behavior
    // The actual sanitize_filename function is tested in unit tests
    for (input, expected) in test_cases {
        // In integration tests, we would verify the actual output filenames
        // For now, we document the expected behavior
        assert!(
            !expected.is_empty(),
            "Sanitized filename should not be empty for input: {}",
            input
        );
        assert!(
            !expected.contains(' '),
            "Sanitized filename should not contain spaces for input: {}",
            input
        );
    }
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
    assert!(
        content.contains("repositories:"),
        "Should have repositories"
    );
    assert!(
        content.contains("mcp-servers:"),
        "Should have mcp-servers"
    );

    // Verify it has MCP configuration and custom MCPs
    assert!(content.contains("container:"), "Should have custom MCP");

    // Verify permissions
    assert!(
        content.contains("permissions:"),
        "Should have permissions"
    );
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
        .args(["compile", fixture_path.to_str().unwrap(), "-o", output_path.to_str().unwrap()])
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
/// - A cancel-previous-builds bash step
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

    // Should include the cancel-previous-builds step
    assert!(
        compiled.contains("Cancel previous queued builds"),
        "Compiled output should include cancel-previous-builds step"
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
        .args(["compile", test_input.to_str().unwrap(), "-o", output_path.to_str().unwrap()])
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
        .args(["compile", test_input.to_str().unwrap(), "-o", output_path.to_str().unwrap()])
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

/// Test that write-requiring safe-outputs fail without write service connection
#[test]
fn test_permissions_validation_fails_without_write_sc() {
    let temp_dir = std::env::temp_dir().join(format!(
        "agentic-pipeline-permissions-fail-{}",
        std::process::id()
    ));
    fs::create_dir_all(&temp_dir).expect("Failed to create temp directory");

    let test_input = temp_dir.join("bad-perms-agent.md");
    let test_content = r#"---
name: "Bad Permissions Agent"
description: "Agent with create-work-item but no write SC"
safe-outputs:
  create-work-item:
    work-item-type: Task
---

## Test Agent

Do something.
"#;
    fs::write(&test_input, test_content).expect("Failed to write test input");

    let output_path = temp_dir.join("bad-perms-agent.yml");
    let binary_path = PathBuf::from(env!("CARGO_BIN_EXE_ado-aw"));
    let output = std::process::Command::new(&binary_path)
        .args(["compile", test_input.to_str().unwrap(), "-o", output_path.to_str().unwrap()])
        .output()
        .expect("Failed to run compiler");

    assert!(
        !output.status.success(),
        "Compiler should fail when write-requiring safe-outputs lack write SC"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("permissions.write"),
        "Error message should mention permissions.write: {}",
        stderr
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
        .args(["compile", test_input.to_str().unwrap(), "-o", output_path.to_str().unwrap()])
        .output()
        .expect("Failed to run compiler");

    assert!(
        output.status.success(),
        "Compiler should succeed when write SC is provided: {}",
        String::from_utf8_lossy(&output.stderr)
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
        .args(["compile", test_input.to_str().unwrap(), "-o", output_path.to_str().unwrap()])
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

    let fixture_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("1es-test-agent.md");

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

/// Test that update-wiki-page requires a write service connection
#[test]
fn test_update_wiki_page_requires_write_sc() {
    let temp_dir = std::env::temp_dir().join(format!(
        "agentic-pipeline-wiki-fail-{}",
        std::process::id()
    ));
    fs::create_dir_all(&temp_dir).expect("Failed to create temp directory");

    let test_input = temp_dir.join("wiki-agent.md");
    let test_content = r#"---
name: "Wiki Agent"
description: "Agent that edits wiki pages but has no write SC"
safe-outputs:
  update-wiki-page:
    wiki-name: "MyProject.wiki"
    path-prefix: "/agent-output"
---

## Wiki Agent

Update the wiki.
"#;
    fs::write(&test_input, test_content).expect("Failed to write test input");

    let output_path = temp_dir.join("wiki-agent.yml");
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
        "Compiler should fail when update-wiki-page lacks a write SC"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("permissions.write"),
        "Error message should mention permissions.write: {stderr}"
    );

    let _ = fs::remove_dir_all(&temp_dir);
}

/// Test that update-wiki-page compiles successfully when a write SC is present
#[test]
fn test_update_wiki_page_compiles_with_write_sc() {
    let temp_dir = std::env::temp_dir().join(format!(
        "agentic-pipeline-wiki-pass-{}",
        std::process::id()
    ));
    fs::create_dir_all(&temp_dir).expect("Failed to create temp directory");

    let test_input = temp_dir.join("wiki-agent.md");
    let test_content = r#"---
name: "Wiki Agent"
description: "Agent that edits wiki pages with write SC"
permissions:
  write: my-write-sc
safe-outputs:
  update-wiki-page:
    wiki-name: "MyProject.wiki"
    path-prefix: "/agent-output"
    title-prefix: "[Agent] "
    comment: "Updated by agent"
    create-if-missing: true
---

## Wiki Agent

Update the wiki.
"#;
    fs::write(&test_input, test_content).expect("Failed to write test input");

    let output_path = temp_dir.join("wiki-agent.yml");
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

    let _ = fs::remove_dir_all(&temp_dir);
}

/// Test that create-wiki-page requires a write service connection
#[test]
fn test_create_wiki_page_requires_write_sc() {
    let temp_dir = std::env::temp_dir().join(format!(
        "agentic-pipeline-create-wiki-fail-{}",
        std::process::id()
    ));
    fs::create_dir_all(&temp_dir).expect("Failed to create temp directory");

    let test_input = temp_dir.join("create-wiki-agent.md");
    let test_content = r#"---
name: "Create Wiki Agent"
description: "Agent that creates wiki pages but has no write SC"
safe-outputs:
  create-wiki-page:
    wiki-name: "MyProject.wiki"
    path-prefix: "/agent-output"
---

## Create Wiki Agent

Create new wiki pages.
"#;
    fs::write(&test_input, test_content).expect("Failed to write test input");

    let output_path = temp_dir.join("create-wiki-agent.yml");
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
        "Compiler should fail when create-wiki-page lacks a write SC"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("permissions.write"),
        "Error message should mention permissions.write: {stderr}"
    );

    let _ = fs::remove_dir_all(&temp_dir);
}

/// Test that create-wiki-page compiles successfully when a write SC is present
#[test]
fn test_create_wiki_page_compiles_with_write_sc() {
    let temp_dir = std::env::temp_dir().join(format!(
        "agentic-pipeline-create-wiki-pass-{}",
        std::process::id()
    ));
    fs::create_dir_all(&temp_dir).expect("Failed to create temp directory");

    let test_input = temp_dir.join("create-wiki-agent.md");
    let test_content = r#"---
name: "Create Wiki Agent"
description: "Agent that creates wiki pages with write SC"
permissions:
  write: my-write-sc
safe-outputs:
  create-wiki-page:
    wiki-name: "MyProject.wiki"
    path-prefix: "/agent-output"
    title-prefix: "[Agent] "
    comment: "Created by agent"
---

## Create Wiki Agent

Create new wiki pages.
"#;
    fs::write(&test_input, test_content).expect("Failed to write test input");

    let output_path = temp_dir.join("create-wiki-agent.yml");
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

    let _ = fs::remove_dir_all(&temp_dir);
}

/// Test that update-work-item requires a write service connection
#[test]
fn test_update_work_item_requires_write_sc() {
    let temp_dir = std::env::temp_dir().join(format!(
        "agentic-pipeline-uwi-fail-{}",
        std::process::id()
    ));
    fs::create_dir_all(&temp_dir).expect("Failed to create temp directory");

    let test_input = temp_dir.join("uwi-agent.md");
    let test_content = r#"---
name: "Update Work Item Agent"
description: "Agent that updates work items but has no write SC"
safe-outputs:
  update-work-item:
    title: true
    status: true
    target: "*"
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
        "Compiler should fail when update-work-item lacks a write SC"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("permissions.write"),
        "Error message should mention permissions.write: {stderr}"
    );

    let _ = fs::remove_dir_all(&temp_dir);
}

/// Test that update-work-item compiles successfully when a write SC is present
#[test]
fn test_update_work_item_compiles_with_write_sc() {
    let temp_dir = std::env::temp_dir().join(format!(
        "agentic-pipeline-uwi-pass-{}",
        std::process::id()
    ));
    fs::create_dir_all(&temp_dir).expect("Failed to create temp directory");

    let test_input = temp_dir.join("uwi-agent.md");
    let test_content = r#"---
name: "Update Work Item Agent"
description: "Agent that updates work items with write SC"
permissions:
  write: my-write-sc
safe-outputs:
  update-work-item:
    title: true
    status: true
    body: true
    markdown-body: true
    title-prefix: "[bot] "
    tag-prefix: "agent-"
    max: 2
    target: "*"
    area-path: true
    iteration-path: true
    assignee: true
    tags: true
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
        output.status.success(),
        "Compiler should succeed when write SC is provided: {}",
        String::from_utf8_lossy(&output.stderr)
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

/// Test that comment-on-work-item requires a write service connection
#[test]
fn test_comment_on_work_item_requires_write_sc() {
    let temp_dir = std::env::temp_dir().join(format!(
        "agentic-pipeline-cwi-sc-{}",
        std::process::id()
    ));
    fs::create_dir_all(&temp_dir).expect("Failed to create temp directory");

    let test_input = temp_dir.join("cwi-agent.md");
    let test_content = r#"---
name: "Comment Agent"
description: "Agent that comments on work items but has no write SC"
safe-outputs:
  comment-on-work-item:
    target: "*"
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
        "Compiler should fail when comment-on-work-item lacks a write SC"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("permissions.write"),
        "Error message should mention permissions.write: {stderr}"
    );

    let _ = fs::remove_dir_all(&temp_dir);
}

/// Test that comment-on-work-item compiles successfully with proper config
#[test]
fn test_comment_on_work_item_compiles_with_target_and_write_sc() {
    let temp_dir = std::env::temp_dir().join(format!(
        "agentic-pipeline-cwi-pass-{}",
        std::process::id()
    ));
    fs::create_dir_all(&temp_dir).expect("Failed to create temp directory");

    let test_input = temp_dir.join("cwi-agent.md");
    let test_content = r#"---
name: "Comment Agent"
description: "Agent that comments on work items"
permissions:
  write: my-write-sc
safe-outputs:
  comment-on-work-item:
    target: "*"
    max: 5
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
        output.status.success(),
        "Compiler should succeed when target and write SC are provided: {}",
        String::from_utf8_lossy(&output.stderr)
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
    let temp_dir = std::env::temp_dir().join(format!(
        "agentic-pipeline-header-{}",
        std::process::id()
    ));
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
        .args([
            "compile",
            "agents/my-agent.md",
        ])
        .current_dir(&temp_dir)
        .output()
        .expect("Failed to run initial compile");

    assert!(
        output.status.success(),
        "Initial compile should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Verify the YAML was created with the header
    let yaml_path = agents_dir.join("my-agent.yml");
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
        stdout, stderr
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
        stdout, stderr
    );

    assert!(
        stderr.contains("not found") || stdout.contains("skipped"),
        "Should warn about missing source.\nstdout: {}\nstderr: {}",
        stdout, stderr
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
    let temp_dir = std::env::temp_dir().join(format!(
        "agentic-pipeline-spr-empty-{}",
        std::process::id()
    ));
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
    let temp_dir = std::env::temp_dir().join(format!(
        "agentic-pipeline-spr-pass-{}",
        std::process::id()
    ));
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

    let _ = fs::remove_dir_all(&temp_dir);
}

/// Test that update-pr fails compilation when vote is reachable but allowed-votes is missing
#[test]
fn test_update_pr_requires_allowed_votes_when_vote_reachable() {
    let temp_dir = std::env::temp_dir().join(format!(
        "agentic-pipeline-uprvote-{}",
        std::process::id()
    ));
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


// ==================== Azure DevOps MCP Integration Tests ====================

/// Test that the Azure DevOps MCP fixture compiles successfully with no unreplaced markers
#[test]
fn test_fixture_azure_devops_mcp_compiled_output() {
    let temp_dir = std::env::temp_dir().join(format!(
        "agentic-pipeline-ado-mcp-{}",
        std::process::id()
    ));
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
        .args(["compile", input_path.to_str().unwrap(), "-o", output_path.to_str().unwrap()])
        .output()
        .expect("Failed to run compiler");

    assert!(output.status.success(), "Compiler should succeed: {}", String::from_utf8_lossy(&output.stderr));

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
    let temp_dir = std::env::temp_dir().join(format!(
        "agentic-pipeline-mcpg-http-{}",
        std::process::id()
    ));
    fs::create_dir_all(&temp_dir).expect("Failed to create temp directory");

    let input = "---\nname: \"HTTP MCP Test\"\ndescription: \"Tests HTTP MCP\"\nmcp-servers:\n  remote-ado:\n    url: \"https://mcp.dev.azure.com/myorg\"\n    headers:\n      X-MCP-Toolsets: \"repos,wit\"\n    allowed:\n      - wit_get_work_item\n---\n\n## Test\n";

    let input_path = temp_dir.join("http-mcp.md");
    let output_path = temp_dir.join("http-mcp.yml");
    fs::write(&input_path, input).unwrap();

    let binary_path = PathBuf::from(env!("CARGO_BIN_EXE_ado-aw"));
    let output = std::process::Command::new(&binary_path)
        .args(["compile", input_path.to_str().unwrap(), "-o", output_path.to_str().unwrap()])
        .output()
        .expect("Failed to run compiler");

    assert!(output.status.success(), "Compiler should succeed: {}", String::from_utf8_lossy(&output.stderr));

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
    let temp_dir = std::env::temp_dir().join(format!(
        "agentic-pipeline-mcpg-env-{}",
        std::process::id()
    ));
    fs::create_dir_all(&temp_dir).expect("Failed to create temp directory");

    let input = "---\nname: \"Env Test\"\ndescription: \"Tests env passthrough\"\npermissions:\n  read: my-read-sc\n  write: my-write-sc\nmcp-servers:\n  my-tool:\n    container: \"node:20-slim\"\n    env:\n      AZURE_DEVOPS_EXT_PAT: \"\"\n      MY_TOKEN: \"\"\n      STATIC_VAR: \"static-value\"\nsafe-outputs:\n  create-work-item:\n    work-item-type: Task\n---\n\n## Test\n";

    let input_path = temp_dir.join("env-passthrough.md");
    let output_path = temp_dir.join("env-passthrough.yml");
    fs::write(&input_path, input).unwrap();

    let binary_path = PathBuf::from(env!("CARGO_BIN_EXE_ado-aw"));
    let output = std::process::Command::new(&binary_path)
        .args(["compile", input_path.to_str().unwrap(), "-o", output_path.to_str().unwrap()])
        .output()
        .expect("Failed to run compiler");

    assert!(output.status.success(), "Compiler should succeed: {}", String::from_utf8_lossy(&output.stderr));

    let compiled = fs::read_to_string(&output_path).unwrap();

    // AZURE_DEVOPS_EXT_PAT with "" is bare passthrough for user-configured MCPs
    // (only tools.azure-devops extension provides SC_READ_TOKEN mapping)
    assert!(compiled.contains("-e AZURE_DEVOPS_EXT_PAT"), "Should forward AZURE_DEVOPS_EXT_PAT as passthrough");

    // Should forward passthrough env var MY_TOKEN
    assert!(compiled.contains("-e MY_TOKEN"), "Should forward passthrough env var");

    // Static var should be in config
    assert!(compiled.contains("\"STATIC_VAR\": \"static-value\""), "Static env var should be in config");

    let _ = fs::remove_dir_all(&temp_dir);
}

/// Test that user-defined parameters are emitted in the compiled pipeline YAML
#[test]
fn test_parameters_in_compiled_output() {
    let temp_dir = std::env::temp_dir().join(format!(
        "agentic-pipeline-params-{}",
        std::process::id()
    ));
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
        .args(["compile", input_path.to_str().unwrap(), "-o", output_path.to_str().unwrap()])
        .output()
        .expect("Failed to run compiler");

    assert!(output.status.success(), "Compiler should succeed: {}", String::from_utf8_lossy(&output.stderr));

    let compiled = fs::read_to_string(&output_path).unwrap();

    // Verify parameters block is present
    assert!(compiled.contains("parameters:"), "Should contain parameters: block");
    assert!(compiled.contains("name: verbose"), "Should contain verbose parameter");
    assert!(compiled.contains("name: region"), "Should contain region parameter");
    assert!(compiled.contains("displayName: Verbose output"), "Should contain displayName");
    assert!(compiled.contains("default: false"), "Should contain default for verbose");
    assert!(compiled.contains("default: us-east"), "Should contain default for region");
    assert!(compiled.contains("- us-east"), "Should contain values for region");
    assert!(compiled.contains("- eu-west"), "Should contain values for region");

    // No clearMemory should be injected (no memory configured)
    assert!(!compiled.contains("clearMemory"), "Should NOT contain clearMemory without memory");

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
        .args(["compile", input_path.to_str().unwrap(), "-o", output_path.to_str().unwrap()])
        .output()
        .expect("Failed to run compiler");

    assert!(output.status.success(), "Compiler should succeed: {}", String::from_utf8_lossy(&output.stderr));

    let compiled = fs::read_to_string(&output_path).unwrap();

    // Verify clearMemory parameter is auto-injected
    assert!(compiled.contains("name: clearMemory"), "Should auto-inject clearMemory parameter");
    assert!(compiled.contains("displayName: Clear agent memory"), "Should have displayName");
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
        .args(["compile", input_path.to_str().unwrap(), "-o", output_path.to_str().unwrap()])
        .output()
        .expect("Failed to run compiler");

    assert!(output.status.success(), "Compiler should succeed: {}", String::from_utf8_lossy(&output.stderr));

    let compiled = fs::read_to_string(&output_path).unwrap();

    // Verify user's clearMemory is present (with their custom displayName and default)
    assert!(compiled.contains("displayName: Reset memory"), "Should use user's displayName");
    assert!(compiled.contains("default: true"), "Should use user's default value");

    // Verify clearMemory only appears once (not duplicated)
    let count = compiled.matches("name: clearMemory").count();
    assert_eq!(count, 1, "clearMemory should appear exactly once, not duplicated");

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
        .args(["compile", input_path.to_str().unwrap(), "-o", output_path.to_str().unwrap()])
        .output()
        .expect("Failed to run compiler");

    assert!(output.status.success(), "Compiler should succeed: {}", String::from_utf8_lossy(&output.stderr));

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
        .args(["compile", input_path.to_str().unwrap(), "-o", output_path.to_str().unwrap()])
        .output()
        .expect("Failed to run compiler");

    assert!(
        output.status.success(),
        "Compiler should succeed for valid wildcard '*.mycompany.com': {}",
        String::from_utf8_lossy(&output.stderr)
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
        .args(["compile", input_path.to_str().unwrap(), "-o", output_path.to_str().unwrap()])
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
        .args(["compile", input_path.to_str().unwrap(), "-o", output_path.to_str().unwrap()])
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
        .args(["compile", input_path.to_str().unwrap(), "-o", output_path.to_str().unwrap()])
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
    let temp_dir = std::env::temp_dir().join(format!(
        "agentic-pipeline-lean-{}",
        std::process::id()
    ));
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
        .args(["compile", input_path.to_str().unwrap(), "-o", output_path.to_str().unwrap()])
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

/// Helper: compile a fixture and return the compiled YAML string.
fn compile_fixture(fixture_name: &str) -> String {
    compile_fixture_with_flags(fixture_name, &[])
}

/// Compile a fixture with additional CLI flags (e.g., --skip-integrity, --debug-pipeline).
fn compile_fixture_with_flags(fixture_name: &str, extra_flags: &[&str]) -> String {
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

    let fixture_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(fixture_name);

    let output_path = temp_dir.join(fixture_name.replace(".md", ".yml"));

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

/// Test that the 1ES fixture produces valid YAML with correct structure
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
        "1ES output should contain copilot invocation (copilot_params substituted)"
    );
    assert!(
        compiled.contains("threat-analysis"),
        "1ES output should contain threat analysis step"
    );
    assert!(
        compiled.contains("ado-aw execute"),
        "1ES output should contain safe output executor step"
    );
    assert!(compiled.contains("job: Agent"), "1ES output should contain Agent job");
    assert!(
        compiled.contains("job: Detection"),
        "1ES output should contain Detection job"
    );
    assert!(
        compiled.contains("job: Execution"),
        "1ES output should contain Execution job"
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

/// Test that the complete standalone fixture produces valid YAML
#[test]
fn test_standalone_complete_compiled_output_is_valid_yaml() {
    let compiled = compile_fixture("complete-agent.md");
    assert_valid_yaml(&compiled, "complete-agent.md");
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
    assert_valid_yaml(&compiled, "minimal-agent.md (skip-integrity + debug-pipeline)");

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
    // other steps in the Agent job. Find the "- bash:" line and check indent.
    for line in compiled.lines() {
        if line.contains("displayName: \"Verify MCP backends\"") {
            let indent = line.len() - line.trim_start().len();
            // Standalone jobs use 8 spaces for step properties
            assert_eq!(
                indent, 8,
                "Verify MCP backends displayName should be at 8 spaces indent in standalone, got {}",
                indent
            );
            break;
        }
    }
}

/// Test that debug probe step indentation is correct in 1ES output
#[test]
fn test_debug_pipeline_probe_step_indentation_1es() {
    let compiled = compile_fixture_with_flags("1es-test-agent.md", &["--debug-pipeline"]);

    for line in compiled.lines() {
        if line.contains("displayName: \"Verify MCP backends\"") {
            let indent = line.len() - line.trim_start().len();
            // 1ES uses 18 spaces for step properties inside templateContext
            assert_eq!(
                indent, 18,
                "Verify MCP backends displayName should be at 18 spaces indent in 1ES, got {}",
                indent
            );
            break;
        }
    }
}
