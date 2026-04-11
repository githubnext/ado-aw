use std::io::BufRead;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

/// Integration tests for the SafeOutputs HTTP server (`mcp-http` subcommand).
///
/// These tests validate the HTTP transport layer that MCPG uses to reach
/// SafeOutputs. They do NOT require Docker or the MCPG gateway — they test
/// the ado-aw HTTP server directly.

/// Guard that kills the child process on drop (even on panic).
struct ServerGuard {
    child: Child,
    port: u16,
    api_key: String,
    _temp_dir: tempfile::TempDir,
    #[allow(dead_code)]
    stderr_thread: Option<std::thread::JoinHandle<()>>,
}

impl Drop for ServerGuard {
    fn drop(&mut self) {
        self.child.kill().ok();
        self.child.wait().ok();
    }
}

/// Helper: find a free TCP port on localhost.
fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

/// Start SafeOutputs HTTP server as a subprocess. Returns a guard that stops
/// the server on drop.
fn start_server() -> ServerGuard {
    let binary_path = env!("CARGO_BIN_EXE_ado-aw");
    let port = free_port();
    let api_key = "test-api-key-12345".to_string();
    let temp_dir = tempfile::tempdir().unwrap();
    let dir_path = temp_dir.path().to_str().unwrap().to_string();

    let mut cmd = Command::new(binary_path);
    cmd.args([
        "mcp-http",
        "--port",
        &port.to_string(),
        "--api-key",
        &api_key,
        &dir_path,
        &dir_path,
    ]);
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let mut child = cmd.spawn().expect("Failed to start mcp-http server");

    // Consume stdout to read the startup output (SAFE_OUTPUTS_PORT=...)
    let stdout = child.stdout.take().expect("Failed to capture stdout");
    let _stdout_thread = std::thread::spawn(move || {
        let reader = std::io::BufReader::new(stdout);
        for line in reader.lines() {
            if line.is_err() {
                break;
            }
        }
    });

    // Consume stderr to prevent buffer fill-up
    let stderr = child.stderr.take().expect("Failed to capture stderr");
    let stderr_thread = std::thread::spawn(move || {
        let reader = std::io::BufReader::new(stderr);
        for line in reader.lines() {
            if line.is_err() {
                break;
            }
        }
    });

    // Wait for the server to become ready (up to 5 s)
    let health_url = format!("http://127.0.0.1:{}/health", port);
    let client = reqwest::blocking::Client::new();
    for _ in 0..50 {
        if client.get(&health_url).send().is_ok() {
            return ServerGuard {
                child,
                port,
                api_key,
                _temp_dir: temp_dir,
                stderr_thread: Some(stderr_thread),
            };
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    // Kill and panic if not ready
    child.kill().ok();
    panic!("SafeOutputs HTTP server did not become ready within 5 s");
}

/// Send a JSON-RPC request to the SafeOutputs MCP endpoint.
fn mcp_request(
    client: &reqwest::blocking::Client,
    server: &ServerGuard,
    body: serde_json::Value,
    session_id: Option<&str>,
) -> reqwest::blocking::Response {
    let mut req = client
        .post(format!("http://127.0.0.1:{}/mcp", server.port))
        .header("Authorization", format!("Bearer {}", server.api_key))
        .header("Content-Type", "application/json")
        .header("Accept", "text/event-stream, application/json");
    if let Some(sid) = session_id {
        req = req.header("mcp-session-id", sid);
    }
    req.json(&body).send().expect("Failed to send MCP request")
}

/// Perform the MCP initialize + initialized handshake, return session ID.
fn mcp_handshake(client: &reqwest::blocking::Client, server: &ServerGuard) -> Option<String> {
    let init_resp = mcp_request(
        client,
        server,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-03-26",
                "capabilities": {},
                "clientInfo": { "name": "test-client", "version": "1.0" }
            }
        }),
        None,
    );
    assert!(
        init_resp.status().is_success(),
        "Initialize should succeed, got {}",
        init_resp.status()
    );

    let session_id = init_resp
        .headers()
        .get("mcp-session-id")
        .map(|v| v.to_str().unwrap().to_string());

    // Send initialized notification
    let mut notif_req = client
        .post(format!("http://127.0.0.1:{}/mcp", server.port))
        .header("Authorization", format!("Bearer {}", server.api_key))
        .header("Content-Type", "application/json")
        .header("Accept", "text/event-stream, application/json");
    if let Some(ref sid) = session_id {
        notif_req = notif_req.header("mcp-session-id", sid);
    }
    let _ = notif_req
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        }))
        .send()
        .unwrap();

    session_id
}

// ──────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────

#[test]
fn test_health_endpoint_returns_ok() {
    let server = start_server();
    let client = reqwest::blocking::Client::new();
    let resp = client
        .get(format!("http://127.0.0.1:{}/health", server.port))
        .send()
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().unwrap(), "ok");
}

#[test]
fn test_auth_rejects_missing_token() {
    let server = start_server();
    let client = reqwest::blocking::Client::new();
    let resp = client
        .post(format!("http://127.0.0.1:{}/mcp", server.port))
        .header("Content-Type", "application/json")
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {}
        }))
        .send()
        .unwrap();
    assert_eq!(resp.status(), 401);
}

#[test]
fn test_auth_rejects_wrong_token() {
    let server = start_server();
    let client = reqwest::blocking::Client::new();
    let resp = client
        .post(format!("http://127.0.0.1:{}/mcp", server.port))
        .header("Authorization", "Bearer wrong-key")
        .header("Content-Type", "application/json")
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {}
        }))
        .send()
        .unwrap();
    assert_eq!(resp.status(), 401);
}

#[test]
fn test_auth_accepts_correct_token() {
    let server = start_server();
    let client = reqwest::blocking::Client::new();
    let resp = mcp_request(
        &client,
        &server,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-03-26",
                "capabilities": {},
                "clientInfo": { "name": "test-client", "version": "1.0" }
            }
        }),
        None,
    );
    assert_ne!(resp.status(), 401, "Correct API key should not be rejected");
}

#[test]
fn test_health_endpoint_no_auth_required() {
    let server = start_server();
    // Health endpoint should work without any auth header
    let client = reqwest::blocking::Client::new();
    let resp = client
        .get(format!("http://127.0.0.1:{}/health", server.port))
        .send()
        .unwrap();
    assert_eq!(resp.status(), 200);
}

#[test]
fn test_mcp_initialize_and_tools_list() {
    let server = start_server();
    let client = reqwest::blocking::Client::new();
    let session_id = mcp_handshake(&client, &server);

    // List tools
    let mut tools_req = client
        .post(format!("http://127.0.0.1:{}/mcp", server.port))
        .header("Authorization", format!("Bearer {}", server.api_key))
        .header("Content-Type", "application/json")
        .header("Accept", "text/event-stream, application/json");
    if let Some(ref sid) = session_id {
        tools_req = tools_req.header("mcp-session-id", sid);
    }
    let tools_resp = tools_req
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list",
            "params": {}
        }))
        .send()
        .unwrap();

    assert!(
        tools_resp.status().is_success(),
        "tools/list should succeed, got {}",
        tools_resp.status()
    );

    let body = tools_resp.text().unwrap();

    // The response should contain our known tools
    assert!(body.contains("noop"), "Should list noop tool, body: {body}");
    assert!(
        body.contains("create-work-item"),
        "Should list create-work-item tool, body: {body}"
    );
    assert!(
        body.contains("create-pull-request"),
        "Should list create-pull-request tool, body: {body}"
    );
    assert!(
        body.contains("missing-tool"),
        "Should list missing-tool tool, body: {body}"
    );
    assert!(
        body.contains("missing-data"),
        "Should list missing-data tool, body: {body}"
    );
}

#[test]
fn test_mcp_call_noop_tool() {
    let server = start_server();
    let client = reqwest::blocking::Client::new();
    let session_id = mcp_handshake(&client, &server);

    // Call noop tool
    let call_resp = mcp_request(
        &client,
        &server,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": "noop",
                "arguments": {
                    "context": "Test run - no action needed"
                }
            }
        }),
        session_id.as_deref(),
    );

    assert!(
        call_resp.status().is_success(),
        "tools/call noop should succeed, got {}",
        call_resp.status()
    );

    // Consume the full response body (SSE stream) to ensure the server-side
    // handler has completed before we check the NDJSON file.
    let _body = call_resp.text().unwrap();

    // Verify NDJSON file was written
    std::thread::sleep(Duration::from_millis(500));
    let ndjson_path = server._temp_dir.path().join("safe_outputs.ndjson");
    assert!(
        ndjson_path.exists(),
        "Safe outputs NDJSON file should exist at {:?}",
        ndjson_path
    );

    let content = std::fs::read_to_string(&ndjson_path).unwrap();
    assert!(
        content.contains("noop"),
        "NDJSON should contain noop entry: {content}"
    );
}

#[test]
fn test_mcp_call_create_work_item() {
    let server = start_server();
    let client = reqwest::blocking::Client::new();
    let session_id = mcp_handshake(&client, &server);

    // Call create-work-item
    let call_resp = mcp_request(
        &client,
        &server,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": "create-work-item",
                "arguments": {
                    "title": "Test work item from integration test",
                    "description": "This is a test work item created during integration testing of the SafeOutputs HTTP server."
                }
            }
        }),
        session_id.as_deref(),
    );

    assert!(
        call_resp.status().is_success(),
        "tools/call create-work-item should succeed, got {}",
        call_resp.status()
    );

    // Consume the full SSE response to ensure handler completion
    let _body = call_resp.text().unwrap();

    // Verify NDJSON file contains the work item
    std::thread::sleep(Duration::from_millis(500));
    let ndjson_path = server._temp_dir.path().join("safe_outputs.ndjson");
    let content = std::fs::read_to_string(&ndjson_path).unwrap();
    assert!(
        content.contains("create-work-item"),
        "NDJSON should contain create-work-item entry: {content}"
    );
    assert!(
        content.contains("Test work item from integration test"),
        "NDJSON should contain work item title: {content}"
    );
}

