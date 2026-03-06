use std::io::{BufRead, Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

/// Guard that kills the child process on drop (even on panic)
struct ProxyGuard {
    child: Child,
    port: u16,
    #[allow(dead_code)] // Thread is kept alive to consume stderr
    stderr_thread: Option<std::thread::JoinHandle<()>>,
}

impl Drop for ProxyGuard {
    fn drop(&mut self) {
        self.child.kill().ok();
        self.child.wait().ok(); // Reap the process
    }
}

/// Helper to start the proxy as a foreground process and return the port
/// The proxy runs until the Child is dropped/killed
fn start_proxy(allowed_hosts: &[&str]) -> ProxyGuard {
    let binary_path = env!("CARGO_BIN_EXE_ado-aw");

    let mut cmd = Command::new(binary_path);
    cmd.arg("proxy");

    for host in allowed_hosts {
        cmd.arg("--allow").arg(host);
    }

    // Run in foreground mode - process stays alive until killed
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let mut child = cmd.spawn().expect("Failed to start proxy");

    // Read the port from stdout (first line only)
    let stdout = child.stdout.take().expect("Failed to capture stdout");
    let mut reader = std::io::BufReader::new(stdout);
    let mut first_line = String::new();

    reader
        .read_line(&mut first_line)
        .expect("Failed to read first line");

    let port: u16 = first_line
        .trim()
        .parse()
        .unwrap_or_else(|_| panic!("Failed to parse port from output: '{}'", first_line.trim()));

    // Spawn a thread to consume stderr from the proxy (prevents buffer fill-up)
    let stderr = child.stderr.take().expect("Failed to capture stderr");
    let stderr_thread = std::thread::spawn(move || {
        let reader = std::io::BufReader::new(stderr);
        for line in reader.lines() {
            if line.is_err() {
                break;
            }
        }
    });

    // Give the proxy a moment to fully start accepting connections
    std::thread::sleep(Duration::from_millis(200));

    ProxyGuard {
        child,
        port,
        stderr_thread: Some(stderr_thread),
    }
}

/// Helper to make an HTTP CONNECT request through the proxy
fn send_connect_request(proxy_port: u16, target_host: &str) -> Result<String, String> {
    let mut stream =
        TcpStream::connect(format!("127.0.0.1:{}", proxy_port)).map_err(|e| e.to_string())?;

    stream.set_read_timeout(Some(Duration::from_secs(5))).ok();
    stream.set_write_timeout(Some(Duration::from_secs(5))).ok();

    let request = format!(
        "CONNECT {} HTTP/1.1\r\nHost: {}\r\n\r\n",
        target_host, target_host
    );

    stream
        .write_all(request.as_bytes())
        .map_err(|e| e.to_string())?;
    stream.flush().map_err(|e| e.to_string())?;

    // Read the response - may need multiple attempts as data arrives
    let mut response = Vec::new();
    let mut buf = [0u8; 1024];
    loop {
        match stream.read(&mut buf) {
            Ok(0) => break, // Connection closed
            Ok(n) => {
                response.extend_from_slice(&buf[..n]);
                // Check if we have a complete HTTP response line
                if response.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                // Timeout - use what we have
                break;
            }
            Err(e) => return Err(format!("Read error: {}", e)),
        }
    }

    if response.is_empty() {
        return Err("Empty response (0 bytes read)".to_string());
    }

    let full_response = String::from_utf8_lossy(&response).to_string();

    // Return the first line (status line)
    full_response
        .split("\r\n")
        .next()
        .map(|s| s.to_string())
        .ok_or_else(|| format!("No status line in response: {}", full_response))
}

/// Helper to make a plain HTTP request through the proxy
fn send_http_request(proxy_port: u16, url: &str, host: &str) -> Result<String, String> {
    let mut stream =
        TcpStream::connect(format!("127.0.0.1:{}", proxy_port)).map_err(|e| e.to_string())?;

    stream.set_read_timeout(Some(Duration::from_secs(5))).ok();
    stream.set_write_timeout(Some(Duration::from_secs(5))).ok();

    let request = format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
        url, host
    );

    stream
        .write_all(request.as_bytes())
        .map_err(|e| e.to_string())?;

    let mut response = vec![0u8; 1024];
    let n = stream.read(&mut response).map_err(|e| e.to_string())?;

    String::from_utf8_lossy(&response[..n])
        .to_string()
        .split("\r\n")
        .next()
        .map(|s| s.to_string())
        .ok_or_else(|| "Empty response".to_string())
}

#[test]
fn test_proxy_starts_and_listens() {
    let proxy = start_proxy(&["api.github.com"]);

    // Verify we can connect to the proxy
    let result = TcpStream::connect(format!("127.0.0.1:{}", proxy.port));
    assert!(
        result.is_ok(),
        "Should be able to connect to proxy on port {}",
        proxy.port
    );
    // ProxyGuard automatically cleans up on drop
}

#[test]
fn test_proxy_blocks_disallowed_host() {
    let proxy = start_proxy(&["api.github.com"]);

    // Try to connect to a host that's not in the allow list
    let response = send_connect_request(proxy.port, "evil.com:443");

    assert!(response.is_ok(), "Should get a response from proxy");
    let response_text = response.unwrap();
    assert!(
        response_text.contains("403"),
        "Should return 403 Forbidden for blocked host, got: {}",
        response_text
    );
    // ProxyGuard automatically cleans up on drop
}

#[test]
fn test_proxy_allows_exact_match() {
    let proxy = start_proxy(&["api.github.com"]);

    // Try to CONNECT to an allowed host
    // Note: The proxy will try to actually connect to api.github.com which may
    // fail/timeout in a test environment. What matters is we don't get 403.
    let response = send_connect_request(proxy.port, "api.github.com:443");

    // Either we get a response that isn't 403, or we get a timeout/error (which
    // is fine - it means the host was allowed and the proxy tried to connect)
    match response {
        Ok(response_text) => {
            assert!(
                !response_text.contains("403"),
                "Should not return 403 for allowed host, got: {}",
                response_text
            );
        }
        Err(_) => {
            // Timeout or connection error is acceptable - it means the host was allowed
        }
    }
    // ProxyGuard automatically cleans up on drop
}

#[test]
fn test_proxy_allows_wildcard_match() {
    let proxy = start_proxy(&["*.github.com"]);

    // Try to connect to a subdomain that matches the wildcard
    // Note: The proxy will try to actually connect upstream which may timeout
    let response = send_connect_request(proxy.port, "api.github.com:443");

    match response {
        Ok(response_text) => {
            assert!(
                !response_text.contains("403"),
                "Should not return 403 for wildcard-matched host, got: {}",
                response_text
            );
        }
        Err(_) => {
            // Timeout or error is acceptable - it means the host was allowed
        }
    }

    // Also test another subdomain
    let response2 = send_connect_request(proxy.port, "raw.github.com:443");
    match response2 {
        Ok(response_text2) => {
            assert!(
                !response_text2.contains("403"),
                "Should not return 403 for wildcard-matched subdomain, got: {}",
                response_text2
            );
        }
        Err(_) => {
            // Timeout or error is acceptable - it means the host was allowed
        }
    }
    // ProxyGuard automatically cleans up on drop
}

#[test]
fn test_proxy_blocks_http_request_to_disallowed_host() {
    let proxy = start_proxy(&["api.github.com"]);

    // Try an HTTP request to a blocked host
    let response = send_http_request(proxy.port, "http://evil.com/test", "evil.com");

    assert!(response.is_ok(), "Should get a response from proxy");
    assert!(
        response.unwrap().contains("403"),
        "Should return 403 Forbidden for blocked HTTP request"
    );
    // ProxyGuard automatically cleans up on drop
}

#[test]
fn test_proxy_multiple_allowed_hosts() {
    let proxy = start_proxy(&["api.github.com", "dev.azure.com", "*.visualstudio.com"]);

    // All these should be allowed
    let response1 = send_connect_request(proxy.port, "api.github.com:443");
    assert!(
        !response1.unwrap_or_default().contains("403"),
        "api.github.com should be allowed"
    );

    let response2 = send_connect_request(proxy.port, "dev.azure.com:443");
    assert!(
        !response2.unwrap_or_default().contains("403"),
        "dev.azure.com should be allowed"
    );

    let response3 = send_connect_request(proxy.port, "msazuresphere.visualstudio.com:443");
    assert!(
        !response3.unwrap_or_default().contains("403"),
        "*.visualstudio.com should match msazuresphere.visualstudio.com"
    );

    // This should be blocked
    let response4 = send_connect_request(proxy.port, "malicious.com:443");
    assert!(
        response4.unwrap_or_default().contains("403"),
        "malicious.com should be blocked"
    );
    // ProxyGuard automatically cleans up on drop
}

#[test]
fn test_proxy_handles_port_in_host() {
    let proxy = start_proxy(&["api.github.com"]);

    // Request with explicit port should still match
    let response = send_connect_request(proxy.port, "api.github.com:8080");
    assert!(
        !response.unwrap_or_default().contains("403"),
        "api.github.com:8080 should match api.github.com allow rule"
    );
    // ProxyGuard automatically cleans up on drop
}

/// Test that long-lived connections (like streaming/telemetry) work correctly.
/// This simulates a server that sends data slowly over time, which is typical
/// for Application Insights telemetry or SSE (Server-Sent Events).
#[test]
fn test_proxy_handles_long_lived_streaming_connection() {
    use std::net::TcpListener;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    // Start a mock "streaming" server
    let mock_server = TcpListener::bind("127.0.0.1:0").expect("Failed to bind mock server");
    let mock_port = mock_server.local_addr().unwrap().port();
    let mock_host = format!("127.0.0.1:{}", mock_port);

    let server_done = Arc::new(AtomicBool::new(false));
    let server_done_clone = server_done.clone();

    // Mock server thread - simulates a streaming endpoint
    let server_thread = std::thread::spawn(move || {
        if let Ok((mut stream, _)) = mock_server.accept() {
            stream.set_read_timeout(Some(Duration::from_secs(5))).ok();

            // Read the incoming request (HTTP after CONNECT tunnel established)
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf);

            // Send HTTP response with chunked/streaming data
            let header = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n";
            let _ = stream.write_all(header.as_bytes());
            let _ = stream.flush();

            // Send several chunks with delays (simulating streaming)
            for i in 1..=3 {
                std::thread::sleep(Duration::from_millis(300));
                let chunk = format!("data: message {}\n\n", i);
                let chunk_frame = format!("{:x}\r\n{}\r\n", chunk.len(), chunk);
                if stream.write_all(chunk_frame.as_bytes()).is_err() {
                    break;
                }
                let _ = stream.flush();
            }

            // Send final chunk
            let _ = stream.write_all(b"0\r\n\r\n");
            let _ = stream.flush();

            server_done_clone.store(true, Ordering::SeqCst);
        }
    });

    // Give server time to start listening
    std::thread::sleep(Duration::from_millis(100));

    // Start proxy allowing 127.0.0.1
    let proxy = start_proxy(&["127.0.0.1"]);

    // Verify a blocked host gets 403 (to confirm proxy is working)
    let blocked_response = send_connect_request(proxy.port, "evil.com:443");
    assert!(
        blocked_response.is_ok() && blocked_response.as_ref().unwrap().contains("403"),
        "Should get 403 for blocked host, got: {:?}",
        blocked_response
    );

    // Verify CONNECT to allowed host works
    let response = send_connect_request(proxy.port, &mock_host);
    assert!(
        response.is_ok(),
        "CONNECT request should succeed, got: {:?}",
        response
    );

    let response_text = response.unwrap();
    assert!(
        response_text.contains("200"),
        "Should get 200 Connection Established, got: {}",
        response_text
    );

    // Clean up
    drop(proxy);
    server_thread.join().ok();
}

/// Test that the proxy doesn't hang when a client disconnects mid-stream
#[test]
fn test_proxy_handles_client_disconnect_during_stream() {
    use std::net::TcpListener;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    // Start a mock server that tries to send a lot of data
    let mock_server = TcpListener::bind("127.0.0.1:0").expect("Failed to bind mock server");
    let mock_port = mock_server.local_addr().unwrap().port();
    // Use 127.0.0.1 directly
    let mock_host = format!("127.0.0.1:{}", mock_port);

    let server_saw_error = Arc::new(AtomicBool::new(false));
    let server_saw_error_clone = server_saw_error.clone();

    // Mock server thread - tries to send data continuously
    let server_thread = std::thread::spawn(move || {
        mock_server.set_nonblocking(false).ok();
        if let Ok((mut stream, _)) = mock_server.accept() {
            stream.set_read_timeout(Some(Duration::from_secs(2))).ok();
            stream.set_write_timeout(Some(Duration::from_secs(2))).ok();

            // Read request
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf);

            // Send response header
            let header = "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n\r\n";
            let _ = stream.write_all(header.as_bytes());

            // Try to send lots of data - client will disconnect partway through
            for _ in 0..50 {
                std::thread::sleep(Duration::from_millis(100));
                if stream.write_all(b"data chunk\n").is_err() {
                    server_saw_error_clone.store(true, Ordering::SeqCst);
                    break;
                }
                if stream.flush().is_err() {
                    server_saw_error_clone.store(true, Ordering::SeqCst);
                    break;
                }
            }
        }
    });

    // Start proxy allowing 127.0.0.1
    let proxy = start_proxy(&["127.0.0.1"]);

    // Connect and establish tunnel
    let mut client = TcpStream::connect(format!("127.0.0.1:{}", proxy.port))
        .expect("Failed to connect to proxy");
    client.set_read_timeout(Some(Duration::from_secs(5))).ok();

    let connect_req = format!(
        "CONNECT {} HTTP/1.1\r\nHost: {}\r\n\r\n",
        mock_host, mock_host
    );
    client.write_all(connect_req.as_bytes()).ok();
    client.flush().ok();

    // Read CONNECT response
    let mut response = vec![0u8; 256];
    let _ = client.read(&mut response);

    // Send HTTP request
    client
        .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .ok();
    client.flush().ok();

    // Read just a bit of data then disconnect abruptly
    std::thread::sleep(Duration::from_millis(200));
    let mut buf = [0u8; 100];
    let _ = client.read(&mut buf);

    // Disconnect client abruptly (simulating network issue or client crash)
    drop(client);

    // Wait for server thread to finish (with timeout)
    let server_join_result = server_thread.join();
    assert!(
        server_join_result.is_ok(),
        "Server thread should complete without panic"
    );

    // Verify the server eventually saw the disconnect
    assert!(
        server_saw_error.load(Ordering::SeqCst),
        "Server should see write error after client disconnect"
    );

    // Clean up
    drop(proxy);
}
