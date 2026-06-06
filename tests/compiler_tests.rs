use std::fs;
use std::path::PathBuf;


/// Asserts that all required `{{ marker }}` placeholders are present in the template.
fn assert_required_markers(content: &str) {
    let required = [
        "{{ repositories }}",
        "{{ schedule }}",
        "{{ checkout_self }}",
        "{{ checkout_repositories }}",
        "{{ allowed_domains }}",
        "{{ source_path }}",
        "{{ pipeline_agent_name }}",
        "{{ engine_run }}",
        "{{ compiler_version }}",
        "{{ integrity_check }}",
        "{{ firewall_version }}",
        "{{ mcpg_config }}",
        "{{ mcpg_version }}",
    ];
    for marker in &required {
        assert!(
            content.contains(marker),
            "Template should contain marker: {marker}"
        );
    }
    // Sanity-check that at least 6 replacement markers exist in total.
    // (${{ }} is valid ADO pipeline syntax and must be preserved.)
    let marker_count = content.matches("{{ ").count();
    assert!(
        marker_count >= 6,
        "Template should have at least 6 replacement markers"
    );
}

/// Asserts that the pool configuration uses the `{{ pool }}` marker everywhere
/// and that no hardcoded pool name leaks into the template.
fn assert_pool_config(content: &str) {
    // Must appear once per job: Agent, Detection, SafeOutputs.
    let pool_marker_count = content.matches("{{ pool }}").count();
    assert_eq!(
        pool_marker_count, 3,
        "Template should use '{{ pool }}' marker exactly three times (once for each job)"
    );
    assert!(
        !content.contains("name: AZS-1ES-L-MMS-ubuntu-22.04"),
        "Template should not contain hardcoded pool name 'AZS-1ES-L-MMS-ubuntu-22.04'"
    );
}

/// Asserts that the `ado-aw` compiler binary is fetched from GitHub Releases
/// with a correct, targeted checksum verification.
fn assert_compiler_download(content: &str) {
    assert!(
        !content.contains("pipeline: 2437"),
        "Template should not reference ADO pipeline 2437 for the compiler"
    );
    assert!(
        content.contains("github.com/githubnext/ado-aw/releases"),
        "Template should download the compiler from GitHub Releases"
    );
    // --ignore-missing silently passes when the binary is absent from checksums.txt.
    assert!(
        !content.contains("sha256sum -c checksums.txt --ignore-missing"),
        "Template should not use --ignore-missing in checksum verification"
    );
    assert!(
        content.contains(r#"grep "ado-aw-linux-x64" checksums.txt | sha256sum -c -"#),
        "Template should verify ado-aw checksum using targeted grep to ensure binary entry exists"
    );
    assert!(
        !content.contains("grep -q"),
        "Checksum verification should not pipe through grep -q"
    );
}

/// Asserts that the AWF binary is fetched from GitHub Releases, not ADO
/// pipeline artifacts, and that no legacy artifact tasks remain.
fn assert_awf_download(content: &str) {
    assert!(
        !content.contains("pipeline: 2450"),
        "Template should not reference ADO pipeline 2450 for the firewall"
    );
    assert!(
        !content.contains("DownloadPipelineArtifact"),
        "Template should not use DownloadPipelineArtifact task"
    );
    assert!(
        content.contains("github.com/github/gh-aw-firewall/releases"),
        "Template should download AWF from GitHub Releases"
    );
}

/// Asserts that MCPG is integrated correctly and that no legacy mcp-firewall
/// artefacts remain in the template.
fn assert_mcpg_integration(content: &str) {
    assert!(
        content.contains("--enable-host-access"),
        "Template should include --enable-host-access for MCPG"
    );
    assert!(
        !content.contains("mcp-firewall-config"),
        "Template should not reference legacy mcp-firewall config"
    );
    assert!(
        !content.contains("MCP_FIREWALL_EOF"),
        "Template should not contain legacy firewall heredoc"
    );
}

/// Test that verifies the expected structure of the compiled YAML output
#[test]
fn test_compiled_yaml_structure() {
    let template_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("data")
        .join("base.yml");

    assert!(template_path.exists(), "Base template should exist");

    let content = fs::read_to_string(&template_path).expect("Should be able to read base template");

    assert_required_markers(&content);
    assert_pool_config(&content);
    assert_compiler_download(&content);
    assert_awf_download(&content);
    assert_mcpg_integration(&content);
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

    let compiled =
        fs::read_to_string(&output_path).expect("Compiled YAML should exist on success");
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

/// Test that comment-on-work-item compiles successfully with proper config
#[test]
fn test_comment_on_work_item_compiles_with_target_and_write_sc() {
    let temp_dir =
        std::env::temp_dir().join(format!("agentic-pipeline-cwi-pass-{}", std::process::id()));
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
        compiled.contains("versionSpec: '3.x'"),
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
        compiled.contains("NodeTool@0"),
        "should have Node install step"
    );
    assert!(
        compiled.contains("versionSpec: '22.x'"),
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
    // displayName: "ado-aw" identifies the injected step uniquely.
    assert!(
        compiled.contains("displayName: \"ado-aw\""),
        "{fixture_name}: compiled YAML missing displayName: \"ado-aw\" on injected step"
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
        compiled.contains("displayName: \"Emit aw_info.json\""),
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
        compiled.contains(r#"name: "My special agent with quotes-$(BuildID)""#),
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
        compiled.contains(r#"name: "My special agent with quotes (1ES)-$(BuildID)""#),
        "1ES output should contain sanitized pipeline name; got:\n{compiled}"
    );
    assert!(
        compiled.contains(r#"displayName: "My \"special\": agent with quotes (1ES)""#),
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

/// Test that the complete standalone fixture produces valid YAML
#[test]
fn test_standalone_complete_compiled_output_is_valid_yaml() {
    let compiled = compile_fixture("complete-agent.md");
    assert_valid_yaml(&compiled, "complete-agent.md");
}

/// Test that the complete standalone fixture emits Setup/Teardown jobs and
/// that the agentic task waits on Setup. The fixture has `setup:`,
/// `teardown:`, and `post-steps:` sections so all three should appear.
#[test]
fn test_standalone_complete_agent_has_setup_and_teardown_jobs() {
    let compiled = compile_fixture("complete-agent.md");
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

// ─── PR Filter Integration Tests ────────────────────────────────────────────

/// Tier 1 PR filter fixture produces valid YAML with inline gate step.
#[test]
fn test_pr_filter_tier1_compiled_output_is_valid_yaml() {
    let compiled = compile_fixture("pr-filter-tier1-agent.md");
    assert_valid_yaml(&compiled, "pr-filter-tier1-agent.md");
}

/// Tier 1 PR filters use the bundled Node evaluator via extension.
#[test]
fn test_pr_filter_tier1_has_evaluator_gate() {
    let compiled = compile_fixture("pr-filter-tier1-agent.md");

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
        !yaml.contains("NodeTool@0"),
        "No NodeTool@0 expected when neither gate nor runtime imports are active"
    );
    assert!(
        !yaml.contains("Download ado-aw scripts"),
        "No script bundle download expected when neither gate nor runtime imports are active"
    );
}

/// When a user pins a Node version via `runtimes.node:` AND runtime imports
/// are active, both extensions emit `NodeTool@0` into the Agent job. ADO's
/// `NodeTool@0` prepends to PATH, so the LAST install wins. The ado-script
/// extension must run in the `System` phase so its Node 20.x install lands
/// FIRST, and the user's Runtime-phase `NodeTool@0 22.x` lands second —
/// the user's pinned version then wins on PATH for the rest of the job.
#[test]
fn test_node_runtime_install_orders_after_ado_script_so_user_version_wins() {
    let yaml = compile_fixture("dedupe_node_runtime_and_imports.md");
    let agent = extract_job_block(&yaml, "Agent").expect("Agent job should exist");

    // Find offsets within the Agent block. The ado-script Node install
    // is identifiable by its displayName; the user's runtime install
    // carries the explicit user-pinned versionSpec.
    let ado_script_install_idx = agent
        .find("displayName: \"Install Node.js 20.x\"")
        .expect("ado-script Node 20.x install step missing from Agent job");
    let user_runtime_install_idx = agent
        .find("'Install Node.js 22.x'")
        .expect("user runtime Node 22.x install step missing from Agent job");

    assert!(
        ado_script_install_idx < user_runtime_install_idx,
        "ado-script NodeTool@0 must precede user NodeTool@0 in the Agent job so the \
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

/// Tier 2 PR filter fixture produces valid YAML.
#[test]
fn test_pr_filter_tier2_compiled_output_is_valid_yaml() {
    let compiled = compile_fixture("pr-filter-tier2-agent.md");
    assert_valid_yaml(&compiled, "pr-filter-tier2-agent.md");
}

/// Tier 2 PR filters produce a Setup job with extension-based gate step.
#[test]
fn test_pr_filter_tier2_has_extension_gate() {
    let compiled = compile_fixture("pr-filter-tier2-agent.md");

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
        compiled.contains(r#"displayName: "Detect Azure CLI on host (for AWF mount)""#),
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

/// Test that every `{{ marker }}` used in `src/data/*.yml` has a corresponding
/// `## {{ marker }}` heading in `docs/template-markers.md`.
///
/// This is the CI/docs marker-drift guard: if a marker is added to a template
/// without updating the docs, this test fails.
#[test]
fn test_template_marker_docs_coverage() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let data_dir = manifest_dir.join("src").join("data");
    let docs_file = manifest_dir.join("docs").join("template-markers.md");

    // --- collect markers from src/data/*.yml ---
    let yml_entries = fs::read_dir(&data_dir)
        .unwrap_or_else(|e| panic!("Cannot read {}: {e}", data_dir.display()));

    let mut yml_markers: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for entry in yml_entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("yml") {
            continue;
        }
        let content = fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("Cannot read {}: {e}", path.display()));
        for cap in regex_captures_markers(&content) {
            yml_markers.insert(cap);
        }
    }

    // --- collect documented marker headings from docs/template-markers.md ---
    let docs = fs::read_to_string(&docs_file)
        .unwrap_or_else(|e| panic!("Cannot read {}: {e}", docs_file.display()));

    let mut documented: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for line in docs.lines() {
        // Match lines like: ## {{ marker_name }}
        if let Some(rest) = line.strip_prefix("## {{ ")
            && let Some(name) = rest.split("}}").next()
        {
            documented.insert(name.trim().to_string());
        }
    }

    // Every marker that appears in the yml files must have a docs heading.
    let mut missing: Vec<String> = Vec::new();
    for marker in &yml_markers {
        if !documented.contains(marker.as_str()) {
            missing.push(format!("{{{{ {marker} }}}}"));
        }
    }

    assert!(
        missing.is_empty(),
        "The following template markers appear in src/data/*.yml but have no \
         '## {{{{ marker }}}}' heading in docs/template-markers.md — add docs or \
         update the marker name:\n  {}",
        missing.join("\n  ")
    );
}

/// Extract all `{{ name }}` marker names from `content` (excluding `${{ }}` ADO expressions).
fn regex_captures_markers(content: &str) -> Vec<String> {
    let mut results = Vec::new();
    let mut s: &str = content;
    while let Some(start) = s.find("{{ ") {
        // Skip ADO ${{ }} expressions
        if start > 0 && s.as_bytes().get(start - 1) == Some(&b'$') {
            s = &s[start + 3..];
            continue;
        }
        let after = &s[start + 3..];
        if let Some(end) = after.find("}}") {
            let name = after[..end].trim().to_string();
            if !name.is_empty() {
                results.push(name);
            }
            s = &after[end + 2..];
        } else {
            break;
        }
    }
    results
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
