use std::io::{BufRead, Write};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

/// Guard that kills the child process on drop (even on panic)
struct FirewallGuard {
    child: Child,
    #[allow(dead_code)]
    stderr_thread: Option<std::thread::JoinHandle<()>>,
}

impl Drop for FirewallGuard {
    fn drop(&mut self) {
        self.child.kill().ok();
        self.child.wait().ok();
    }
}

/// Helper to create a temporary config file
fn create_config_file(config: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let temp_dir = tempfile::tempdir().unwrap();
    let config_path = temp_dir.path().join("firewall-config.json");
    std::fs::write(&config_path, config).unwrap();
    (temp_dir, config_path)
}

/// Helper to start the firewall with a config file
fn start_firewall(config_path: &std::path::PathBuf) -> FirewallGuard {
    let binary_path = env!("CARGO_BIN_EXE_ado-aw");

    let mut cmd = Command::new(binary_path);
    cmd.arg("mcp-firewall");
    cmd.arg("--config").arg(config_path);

    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let mut child = cmd.spawn().expect("Failed to start firewall");

    // Spawn thread to consume stderr
    let stderr = child.stderr.take().expect("Failed to capture stderr");
    let stderr_thread = std::thread::spawn(move || {
        let reader = std::io::BufReader::new(stderr);
        for line in reader.lines() {
            if let Ok(line) = line {
                eprintln!("[firewall stderr] {}", line);
            } else {
                break;
            }
        }
    });

    // Give the firewall a moment to start
    std::thread::sleep(Duration::from_millis(200));

    FirewallGuard {
        child,
        stderr_thread: Some(stderr_thread),
    }
}

/// Send a JSON-RPC request and get response
fn send_jsonrpc(
    child: &mut Child,
    method: &str,
    params: Option<serde_json::Value>,
) -> serde_json::Value {
    static REQUEST_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let id = REQUEST_ID.fetch_add(1, std::sync::atomic::Ordering::SeqCst);

    let request = if let Some(p) = params {
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": p
        })
    } else {
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method
        })
    };

    let stdin = child.stdin.as_mut().expect("Failed to get stdin");
    let stdout = child.stdout.as_mut().expect("Failed to get stdout");

    writeln!(stdin, "{}", serde_json::to_string(&request).unwrap()).unwrap();
    stdin.flush().unwrap();

    let mut reader = std::io::BufReader::new(stdout);
    let mut response_line = String::new();
    reader.read_line(&mut response_line).unwrap();

    serde_json::from_str(&response_line).unwrap()
}

#[test]
fn test_firewall_starts_with_empty_config() {
    let config = r#"{"upstreams": {}}"#;
    let (_temp_dir, config_path) = create_config_file(config);

    let mut guard = start_firewall(&config_path);

    // Initialize the MCP connection
    let init_params = serde_json::json!({
        "protocolVersion": "2024-11-05",
        "capabilities": {},
        "clientInfo": {
            "name": "test-client",
            "version": "1.0"
        }
    });

    let response = send_jsonrpc(&mut guard.child, "initialize", Some(init_params));

    assert!(
        response.get("result").is_some(),
        "Should get initialize result"
    );
    assert!(
        response["result"]["serverInfo"]["name"].as_str().is_some(),
        "Should have server info"
    );
}

#[test]
fn test_firewall_lists_no_tools_with_empty_config() {
    let config = r#"{"upstreams": {}}"#;
    let (_temp_dir, config_path) = create_config_file(config);

    let mut guard = start_firewall(&config_path);

    // Initialize first
    let init_params = serde_json::json!({
        "protocolVersion": "2024-11-05",
        "capabilities": {},
        "clientInfo": { "name": "test", "version": "1.0" }
    });
    send_jsonrpc(&mut guard.child, "initialize", Some(init_params));

    // Send initialized notification (no response expected, but we need to send it)
    let stdin = guard.child.stdin.as_mut().unwrap();
    writeln!(
        stdin,
        r#"{{"jsonrpc":"2.0","method":"notifications/initialized"}}"#
    )
    .unwrap();
    stdin.flush().unwrap();

    // List tools
    let response = send_jsonrpc(&mut guard.child, "tools/list", None);

    assert!(
        response.get("result").is_some(),
        "Should get tools/list result"
    );
    let tools = response["result"]["tools"]
        .as_array()
        .expect("tools should be array");
    assert!(tools.is_empty(), "Should have no tools with empty config");
}

#[test]
fn test_firewall_rejects_unknown_tool() {
    let config = r#"{"upstreams": {}}"#;
    let (_temp_dir, config_path) = create_config_file(config);

    let mut guard = start_firewall(&config_path);

    // Initialize
    let init_params = serde_json::json!({
        "protocolVersion": "2024-11-05",
        "capabilities": {},
        "clientInfo": { "name": "test", "version": "1.0" }
    });
    send_jsonrpc(&mut guard.child, "initialize", Some(init_params));

    let stdin = guard.child.stdin.as_mut().unwrap();
    writeln!(
        stdin,
        r#"{{"jsonrpc":"2.0","method":"notifications/initialized"}}"#
    )
    .unwrap();
    stdin.flush().unwrap();

    // Try to call a tool that doesn't exist
    let call_params = serde_json::json!({
        "name": "unknown:tool",
        "arguments": {}
    });
    let response = send_jsonrpc(&mut guard.child, "tools/call", Some(call_params));

    assert!(
        response.get("error").is_some(),
        "Should get error for unknown tool"
    );
    let error = &response["error"];
    assert!(
        error["message"]
            .as_str()
            .unwrap_or("")
            .contains("Unknown upstream"),
        "Error should mention unknown upstream, got: {:?}",
        error
    );
}

#[test]
fn test_firewall_rejects_invalid_tool_format() {
    let config = r#"{"upstreams": {}}"#;
    let (_temp_dir, config_path) = create_config_file(config);

    let mut guard = start_firewall(&config_path);

    // Initialize
    let init_params = serde_json::json!({
        "protocolVersion": "2024-11-05",
        "capabilities": {},
        "clientInfo": { "name": "test", "version": "1.0" }
    });
    send_jsonrpc(&mut guard.child, "initialize", Some(init_params));

    let stdin = guard.child.stdin.as_mut().unwrap();
    writeln!(
        stdin,
        r#"{{"jsonrpc":"2.0","method":"notifications/initialized"}}"#
    )
    .unwrap();
    stdin.flush().unwrap();

    // Try to call a tool without namespace
    let call_params = serde_json::json!({
        "name": "no_colon_here",
        "arguments": {}
    });
    let response = send_jsonrpc(&mut guard.child, "tools/call", Some(call_params));

    assert!(
        response.get("error").is_some(),
        "Should get error for invalid format"
    );
    let error = &response["error"];
    assert!(
        error["message"]
            .as_str()
            .unwrap_or("")
            .contains("Invalid tool name format"),
        "Error should mention invalid format, got: {:?}",
        error
    );
}

#[test]
fn test_config_parsing() {
    // Test that we can parse a realistic config
    let config = r#"{
        "upstreams": {
            "icm": {
                "command": "icm-mcp",
                "args": ["--verbose"],
                "env": {"ICM_TOKEN": "secret"},
                "allowed": ["create_incident", "get_*"]
            },
            "kusto": {
                "command": "kusto-mcp",
                "allowed": ["query"]
            }
        }
    }"#;

    let parsed: serde_json::Value = serde_json::from_str(config).unwrap();

    assert_eq!(parsed["upstreams"]["icm"]["command"], "icm-mcp");
    assert_eq!(parsed["upstreams"]["icm"]["args"][0], "--verbose");
    assert_eq!(parsed["upstreams"]["icm"]["allowed"][0], "create_incident");
    assert_eq!(parsed["upstreams"]["icm"]["allowed"][1], "get_*");
    assert_eq!(parsed["upstreams"]["kusto"]["allowed"][0], "query");
}
