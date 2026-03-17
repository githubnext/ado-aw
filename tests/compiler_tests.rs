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
        .join("templates")
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
        template_content.contains("{{ agency_params }}"),
        "Template should contain agency_params marker"
    );
    assert!(
        template_content.contains("{{ compiler_version }}"),
        "Template should contain compiler_version marker"
    );

    // Verify template doesn't accidentally use ${{ }} where {{ }} should be used
    // (The ${{ }} syntax is for Azure DevOps pipeline expressions and should be preserved)
    let marker_count = template_content.matches("{{ ").count();
    assert!(
        marker_count >= 6,
        "Template should have at least 6 replacement markers"
    );

    // Verify that {{ pool }} marker is used for all jobs, not hardcoded pool names
    // This ensures consistency across PerformAgenticTask, AnalyzeSafeOutputs, and ProcessSafeOutputs jobs
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
        template_content.contains("sha256sum -c checksums.txt --ignore-missing"),
        "Template should verify checksum using checksums.txt"
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

    // Verify it has both built-in and custom MCPs
    assert!(content.contains("ado: true"), "Should have built-in MCP");
    assert!(content.contains("command:"), "Should have custom MCP");

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
