use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::json;

const PROMPT_CONTEXT: &str = "real-copilot-cli-safeoutputs-contract";

struct ServerGuard {
    child: Child,
    port: u16,
    api_key: String,
    output_dir: tempfile::TempDir,
    stdout_log: PathBuf,
    stderr_log: PathBuf,
}

impl Drop for ServerGuard {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

fn artifact_dir() -> PathBuf {
    std::env::var_os("ADO_AW_COPILOT_CLI_ARTIFACT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::temp_dir().join(format!(
                "ado-aw-copilot-cli-safeoutputs-{}",
                std::process::id()
            ))
        })
}

fn start_server(artifact_dir: &Path) -> ServerGuard {
    let binary_path = PathBuf::from(env!("CARGO_BIN_EXE_ado-aw"));
    let port = free_port();
    let api_key = "test-api-key-12345".to_string();
    let output_dir = tempfile::tempdir().unwrap();
    let stdout_log = artifact_dir.join("safeoutputs.stdout.log");
    let stderr_log = artifact_dir.join("safeoutputs.stderr.log");

    let stdout = File::create(&stdout_log).expect("create SafeOutputs stdout log");
    let stderr = File::create(&stderr_log).expect("create SafeOutputs stderr log");

    let mut child = Command::new(&binary_path)
        .args([
            "mcp-http",
            "--port",
            &port.to_string(),
            "--api-key",
            &api_key,
            output_dir.path().to_str().unwrap(),
            output_dir.path().to_str().unwrap(),
        ])
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .spawn()
        .expect("Failed to start mcp-http server");

    let health_url = format!("http://127.0.0.1:{port}/health");
    let client = reqwest::blocking::Client::new();
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(10) {
        if client.get(&health_url).send().is_ok() {
            return ServerGuard {
                child,
                port,
                api_key,
                output_dir,
                stdout_log,
                stderr_log,
            };
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    let _ = child.kill();
    let _ = child.wait();
    panic!("SafeOutputs HTTP server did not become ready within 10 s");
}

fn noop_contract_source() -> String {
    format!(
        r#"---
name: "Real Copilot CLI SafeOutputs Contract"
description: "Local contract test for compiler-emitted Copilot CLI flags"
engine:
  id: copilot
  model: gpt-5-mini
safe-outputs:
  noop: {{}}
---

## Contract

Call the `noop` tool exactly once with `context` set to `{PROMPT_CONTEXT}`.
Do not call any other tool.
"#
    )
}

fn compile_inline_agent(tag: &str, content: &str, artifact_dir: &Path) -> String {
    let temp_dir =
        std::env::temp_dir().join(format!("copilot-cli-safeoutputs-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&temp_dir);
    fs::create_dir_all(&temp_dir).expect("create compile temp dir");

    let input = temp_dir.join(format!("{tag}.md"));
    let output = temp_dir.join(format!("{tag}.yml"));
    fs::write(&input, content).expect("write test agent source");

    let binary_path = PathBuf::from(env!("CARGO_BIN_EXE_ado-aw"));
    let compile = Command::new(&binary_path)
        .args([
            "compile",
            input.to_str().unwrap(),
            "-o",
            output.to_str().unwrap(),
        ])
        .output()
        .expect("run compiler");

    fs::write(artifact_dir.join("compiler.stdout.log"), &compile.stdout)
        .expect("write compiler stdout");
    fs::write(artifact_dir.join("compiler.stderr.log"), &compile.stderr)
        .expect("write compiler stderr");

    assert!(
        compile.status.success(),
        "compile should succeed.\nstderr: {}",
        String::from_utf8_lossy(&compile.stderr)
    );

    let compiled = fs::read_to_string(&output).expect("read compiled YAML");
    fs::write(artifact_dir.join("compiled.yml"), &compiled).expect("write compiled YAML artifact");
    compiled
}

fn extract_heredoc_body<'a>(compiled: &'a str, target_line: &str) -> &'a str {
    let start = compiled
        .find(target_line)
        .unwrap_or_else(|| panic!("missing heredoc start: {target_line}"));
    let after_start = &compiled[start + target_line.len()..];
    let newline = after_start
        .find('\n')
        .expect("heredoc start line should end with newline");
    let sentinel = after_start[..newline]
        .trim()
        .trim_matches('\'')
        .to_string();
    let body = &after_start[newline + 1..];
    let end_marker = format!("\n      {sentinel}\n");
    let end = body
        .find(&end_marker)
        .unwrap_or_else(|| panic!("missing heredoc end marker: {sentinel}"));
    &body[..end]
}

fn extract_mcpg_config_template(compiled: &str) -> String {
    extract_heredoc_body(
        compiled,
        "cat > \"$(Agent.TempDirectory)/staging/mcpg-config.json\" << ",
    )
    .to_string()
}

fn extract_agent_invocation(compiled: &str) -> String {
    let line = compiled
        .lines()
        .find(|line| line.contains("/tmp/awf-tools/copilot --prompt \"$(cat /tmp/awf-tools/agent-prompt.md)\""))
        .expect("agent Copilot invocation should be present");
    let start = line.find('\'').expect("invocation should start with single quote");
    let end = line.rfind('\'').expect("invocation should end with single quote");
    line[start + 1..end].to_string()
}

/// Splits the compiler-emitted Copilot command line enough for this contract
/// test: whitespace separates words, single/double quotes group words, and
/// backslash escapes the next character inside double quotes. It intentionally
/// does not perform shell expansion or model every shell escape form.
fn split_shell_words(input: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut chars = input.chars().peekable();
    let mut quote = None;

    while let Some(ch) = chars.next() {
        match (quote, ch) {
            (None, '\'') => quote = Some('\''),
            (None, '"') => quote = Some('"'),
            (None, ch) if ch.is_whitespace() => {
                if !current.is_empty() {
                    words.push(std::mem::take(&mut current));
                }
            }
            (Some(q), ch) if ch == q => quote = None,
            (Some('"'), '\\') => {
                if let Some(next) = chars.next() {
                    current.push(next);
                }
            }
            (_, ch) => current.push(ch),
        }
    }

    if let Some(quote) = quote {
        panic!("unterminated {quote:?} shell quote in invocation: {input}");
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}

fn build_local_copilot_mcp_config(
    mcpg_template: &str,
    port: u16,
    api_key: &str,
) -> serde_json::Value {
    let materialized = mcpg_template
        .replace("${SAFE_OUTPUTS_PORT}", &port.to_string())
        .replace("${SAFE_OUTPUTS_API_KEY}", api_key)
        .replace("${MCP_GATEWAY_API_KEY}", "unused-in-local-test");

    let parsed: serde_json::Value =
        serde_json::from_str(&materialized).expect("materialized MCPG config must be valid JSON");
    let servers = parsed["mcpServers"]
        .as_object()
        .expect("mcpg config should contain mcpServers");

    let mut copilot_servers = serde_json::Map::new();
    for (name, server) in servers {
        let mut server = server
            .as_object()
            .expect("mcpg server entry should be an object")
            .clone();
        server.insert("tools".to_string(), json!(["*"]));
        if name == "safeoutputs" {
            server.insert("isDefaultServer".to_string(), json!(true));
        }
        copilot_servers.insert(name.clone(), serde_json::Value::Object(server));
    }

    let mut root = serde_json::Map::new();
    root.insert(
        "mcpServers".to_string(),
        serde_json::Value::Object(copilot_servers),
    );
    serde_json::Value::Object(root)
}

fn wait_for_ndjson(path: &Path) {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(10) {
        if path.exists() && fs::metadata(path).map(|meta| meta.len() > 0).unwrap_or(false) {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("safe_outputs.ndjson was not written within 10 s");
}

#[test]
#[ignore = "known regression tracked by #1452"]
fn compile_only_copilot_invocation_explicitly_allows_safeoutputs() {
    let artifact_dir = tempfile::tempdir().expect("create artifact dir");
    let compiled = compile_inline_agent(
        "real-copilot-cli-noop-explicit-safeoutputs-tool",
        &noop_contract_source(),
        artifact_dir.path(),
    );
    let invocation = extract_agent_invocation(&compiled);

    assert!(
        invocation.contains("--allow-all-tools"),
        "expected wildcard tool grant in compiler-emitted invocation: {invocation}"
    );
    assert!(
        invocation.contains("--allow-tool safeoutputs"),
        "expected explicit safeoutputs grant alongside --allow-all-tools: {invocation}"
    );
}

#[test]
#[ignore = "known regression tracked by #1452"]
fn compile_only_copilot_config_marks_safeoutputs_as_default_server() {
    let artifact_dir = tempfile::tempdir().expect("create artifact dir");
    let compiled = compile_inline_agent(
        "real-copilot-cli-noop-default-server",
        &noop_contract_source(),
        artifact_dir.path(),
    );

    assert!(
        compiled.contains(".value.isDefaultServer = true"),
        "expected generated Copilot MCP config conversion to mark safeoutputs as trusted/default:\n{compiled}"
    );
}

/// Contract test for the live Copilot CLI + SafeOutputs path.
///
/// This proves that the compiler-emitted Copilot CLI surface for the Agent job
/// can drive a real SafeOutputs MCP tool call and materialize
/// `safe_outputs.ndjson` locally.
///
/// Non-goals: this does not exercise the threat-detection job, the Stage 3
/// executor, or any Azure DevOps write path.
#[test]
#[ignore = "requires installed/authenticated GitHub Copilot CLI; exercised by dedicated workflow"]
fn real_copilot_cli_noop_contract() {
    let artifact_dir = artifact_dir();
    fs::create_dir_all(&artifact_dir).expect("create artifact dir");

    let server = start_server(&artifact_dir);
    let compiled = compile_inline_agent("real-copilot-cli-noop", &noop_contract_source(), &artifact_dir);
    let invocation = extract_agent_invocation(&compiled);
    fs::write(artifact_dir.join("agent-invocation.txt"), &invocation)
        .expect("write invocation artifact");

    assert!(
        invocation.contains("--additional-mcp-config @/tmp/awf-tools/mcp-config.json"),
        "agent invocation must carry compiler-emitted MCP config flag: {invocation}"
    );
    assert!(
        invocation.contains("--allow-all-tools"),
        "default tools path should use --allow-all-tools: {invocation}"
    );
    assert!(
        invocation.contains("--allow-tool safeoutputs"),
        "safeoutputs should stay explicitly allowed even on the wildcard path: {invocation}"
    );

    let mcpg_template = extract_mcpg_config_template(&compiled);
    fs::write(artifact_dir.join("mcpg-config.template.json"), &mcpg_template)
        .expect("write MCPG config artifact");

    let copilot_mcp_config = build_local_copilot_mcp_config(
        &mcpg_template,
        server.port,
        &server.api_key,
    );
    assert_eq!(
        copilot_mcp_config["mcpServers"]["safeoutputs"]["isDefaultServer"],
        json!(true),
        "localized Copilot MCP config should trust the compiler-owned safeoutputs server"
    );
    let copilot_mcp_config_path = artifact_dir.join("mcp-config.json");
    fs::write(
        &copilot_mcp_config_path,
        serde_json::to_string_pretty(&copilot_mcp_config).unwrap(),
    )
    .expect("write Copilot MCP config");

    let prompt_path = artifact_dir.join("prompt.txt");
    let prompt = format!(
        "Call the noop tool exactly once with context `{PROMPT_CONTEXT}`. Stop immediately after the tool call."
    );
    fs::write(&prompt_path, &prompt).expect("write prompt");

    let copilot_bin =
        std::env::var("ADO_AW_COPILOT_CLI_PATH").unwrap_or_else(|_| "copilot".to_string());
    let invocation_args = split_shell_words(&invocation);
    assert_eq!(
        invocation_args.first().map(String::as_str),
        Some("/tmp/awf-tools/copilot"),
        "unexpected compiler-emitted Copilot binary in invocation: {invocation}"
    );

    let mut command = Command::new(&copilot_bin);
    let mut args = invocation_args.iter().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--prompt" => {
                let _ = args.next().unwrap_or_else(|| {
                    panic!("missing --prompt value in invocation: {invocation}")
                });
                command.arg("--prompt").arg(&prompt);
            }
            "--additional-mcp-config" => {
                let _ = args.next().unwrap_or_else(|| {
                    panic!("missing --additional-mcp-config value in invocation: {invocation}")
                });
                command
                    .arg("--additional-mcp-config")
                    .arg(format!("@{}", copilot_mcp_config_path.display()));
            }
            _ => {
                command.arg(arg);
            }
        }
    }
    command.current_dir(server.output_dir.path());
    command.env("XDG_CONFIG_HOME", artifact_dir.join("xdg"));

    let output = command.output().unwrap_or_else(|err| {
        panic!("failed to execute Copilot CLI '{copilot_bin}': {err}");
    });

    fs::write(artifact_dir.join("copilot.stdout.log"), &output.stdout)
        .expect("write Copilot stdout");
    fs::write(artifact_dir.join("copilot.stderr.log"), &output.stderr)
        .expect("write Copilot stderr");

    assert!(
        output.status.success(),
        "Copilot CLI should succeed.\nstdout:\n{}\n\nstderr:\n{}\n\nSafeOutputs logs: {:?} {:?}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
        server.stdout_log,
        server.stderr_log
    );

    let ndjson_path = server.output_dir.path().join("safe_outputs.ndjson");
    wait_for_ndjson(&ndjson_path);
    let ndjson = fs::read_to_string(&ndjson_path).expect("read SafeOutputs NDJSON");
    fs::write(artifact_dir.join("safe_outputs.ndjson"), &ndjson).expect("write NDJSON artifact");

    let mut noop_entries = 0usize;
    for line in ndjson.lines().filter(|line| !line.trim().is_empty()) {
        let value: serde_json::Value =
            serde_json::from_str(line).expect("NDJSON entry should be valid JSON");
        if value["name"] == "noop" {
            noop_entries += 1;
            assert_eq!(
                value["context"].as_str(),
                Some(PROMPT_CONTEXT),
                "noop entry should preserve the deterministic context"
            );
        }
    }

    assert_eq!(
        noop_entries, 1,
        "expected exactly one noop proposal in safe_outputs.ndjson:\n{ndjson}"
    );
}
