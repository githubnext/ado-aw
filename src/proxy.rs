use anyhow::{Context, Result};
use log::info;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

use crate::allowed_hosts::CORE_ALLOWED_HOSTS;

/// Network policy for the proxy
#[derive(Debug, Clone)]
pub struct NetworkPolicy {
    /// List of allowed host patterns (supports wildcards like *.github.com)
    /// This includes both default hosts and user-specified hosts.
    pub allowed_hosts: Vec<String>,
}

impl NetworkPolicy {
    /// Create a new NetworkPolicy with core hosts plus additional user-specified hosts
    pub fn new(additional_hosts: Vec<String>) -> Self {
        let mut allowed_hosts: Vec<String> = CORE_ALLOWED_HOSTS
            .iter()
            .map(|s| (*s).to_string())
            .collect();
        allowed_hosts.extend(additional_hosts);
        // Deduplicate
        allowed_hosts.sort();
        allowed_hosts.dedup();
        Self { allowed_hosts }
    }

    /// Check if a host is allowed by the policy
    pub fn is_allowed(&self, host: &str) -> bool {
        // Strip port if present
        let host = host.split(':').next().unwrap_or(host);

        for pattern in &self.allowed_hosts {
            if pattern.starts_with("*.") {
                // Wildcard pattern - match suffix
                let suffix = &pattern[1..]; // Keep the dot: ".github.com"
                if host.ends_with(suffix) || host == &pattern[2..] {
                    return true;
                }
            } else if pattern == host {
                return true;
            }
        }
        false
    }
}

/// Shared state for the proxy
struct ProxyState {
    policy: NetworkPolicy,
}

impl ProxyState {
    fn log(&self, message: &str) {
        // Log via centralized logging infrastructure (stderr + daily log file)
        info!(target: "proxy", "{}", message);
    }
}

/// Start the HTTP proxy server
///
/// Returns the port number the proxy is listening on.
/// Prints the port to stdout immediately, then runs forever until the process is killed.
/// The caller (typically a shell script) should background this process and capture the PID.
pub async fn start_proxy(policy: NetworkPolicy) -> Result<u16> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .context("Failed to bind proxy listener")?;

    let port = listener
        .local_addr()
        .context("Failed to get local address")?
        .port();

    let state = Arc::new(ProxyState { policy });

    // Print port and flush BEFORE spawning the loop to avoid race conditions
    // where log messages could be printed before the port
    println!("{}", port);
    use std::io::Write;
    std::io::stdout().flush().ok();

    // Now spawn the proxy loop
    tokio::spawn(async move {
        run_proxy_loop(listener, state).await;
    });

    Ok(port)
}

async fn run_proxy_loop(listener: TcpListener, state: Arc<ProxyState>) {
    state.log("Proxy started");
    state.log(&format!(
        "Allowed hosts ({}):",
        state.policy.allowed_hosts.len()
    ));
    for host in &state.policy.allowed_hosts {
        state.log(&format!("  - {}", host));
    }

    loop {
        match listener.accept().await {
            Ok((stream, addr)) => {
                let state = Arc::clone(&state);
                tokio::spawn(async move {
                    if let Err(e) = handle_connection(stream, addr, Arc::clone(&state)).await {
                        state.log(&format!("ERROR connection error: {}", e));
                    }
                });
            }
            Err(e) => {
                state.log(&format!("ERROR accept error: {}", e));
            }
        }
    }
}

async fn handle_connection(
    mut client: TcpStream,
    addr: SocketAddr,
    state: Arc<ProxyState>,
) -> Result<()> {
    let mut reader = BufReader::new(&mut client);
    let mut request_line = String::new();
    reader.read_line(&mut request_line).await?;

    let parts: Vec<&str> = request_line.trim().split_whitespace().collect();
    if parts.len() < 2 {
        return Ok(());
    }

    let method = parts[0];
    let target = parts[1];

    // Extract host from CONNECT request or absolute URL
    let host = if method == "CONNECT" {
        // For CONNECT, the host is in the target directly
        // But we still need to consume the remaining headers
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).await?;
            if line.trim().is_empty() {
                break;
            }
        }
        target.to_string()
    } else if target.starts_with("http://") {
        // Extract host from absolute URL
        target
            .strip_prefix("http://")
            .and_then(|s| s.split('/').next())
            .unwrap_or("")
            .to_string()
    } else if target.starts_with("https://") {
        target
            .strip_prefix("https://")
            .and_then(|s| s.split('/').next())
            .unwrap_or("")
            .to_string()
    } else {
        // Relative URL - need to find Host header
        let mut host = String::new();
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).await?;
            if line.trim().is_empty() {
                break;
            }
            if line.to_lowercase().starts_with("host:") {
                host = line[5..].trim().to_string();
            }
        }
        host
    };

    // Check policy
    if !state.policy.is_allowed(&host) {
        state.log(&format!("BLOCKED {} {} (from {})", method, host, addr));

        let response = if method == "CONNECT" {
            "HTTP/1.1 403 Forbidden\r\n\r\nBlocked by network policy"
        } else {
            "HTTP/1.1 403 Forbidden\r\nContent-Type: text/plain\r\n\r\nBlocked by network policy"
        };
        client.write_all(response.as_bytes()).await?;
        client.flush().await?;
        return Ok(());
    }

    state.log(&format!("ALLOWED {} {} (from {})", method, host, addr));

    if method == "CONNECT" {
        // HTTPS tunneling - drop reader first to release borrow
        drop(reader);
        handle_connect(client, &host, Arc::clone(&state)).await
    } else {
        // Plain HTTP proxy - pass the reader through to preserve buffered data
        handle_http(reader, &request_line, &host).await
    }
}

async fn handle_connect(mut client: TcpStream, host: &str, _state: Arc<ProxyState>) -> Result<()> {
    // Note: Headers have already been consumed by handle_connection

    // Connect to target
    let target = if host.contains(':') {
        host.to_string()
    } else {
        format!("{}:443", host)
    };

    let server = TcpStream::connect(&target)
        .await
        .with_context(|| format!("Failed to connect to {}", target))?;

    // Send 200 Connection Established
    client
        .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
        .await?;
    client.flush().await?;

    // Bidirectional copy with proper shutdown handling
    // We need to handle the case where one direction completes (EOF) but the other
    // is still active (e.g., long-lived connections, HTTP/2, keep-alive).
    let (mut client_read, mut client_write) = client.into_split();
    let (mut server_read, mut server_write) = server.into_split();

    // Use select to detect when either direction completes, then gracefully
    // shutdown to signal the other side
    tokio::select! {
        result = tokio::io::copy(&mut client_read, &mut server_write) => {
            // Client finished sending (or errored) - shutdown server write side
            // This signals EOF to the server
            log::debug!("Client->Server copy completed: {:?}", result);
            let _ = server_write.shutdown().await;
            // Now drain server->client
            let _ = tokio::io::copy(&mut server_read, &mut client_write).await;
        }
        result = tokio::io::copy(&mut server_read, &mut client_write) => {
            // Server finished sending (or errored) - shutdown client write side
            // This signals EOF to the client
            log::debug!("Server->Client copy completed: {:?}", result);
            let _ = client_write.shutdown().await;
            // Now drain client->server
            let _ = tokio::io::copy(&mut client_read, &mut server_write).await;
        }
    }

    Ok(())
}

async fn handle_http(
    mut reader: BufReader<&mut TcpStream>,
    request_line: &str,
    host: &str,
) -> Result<()> {
    // Connect to target
    let target = if host.contains(':') {
        host.to_string()
    } else {
        format!("{}:80", host)
    };

    let mut server = TcpStream::connect(&target)
        .await
        .with_context(|| format!("Failed to connect to {}", target))?;

    // Forward the request line
    server.write_all(request_line.as_bytes()).await?;

    // Forward headers and body
    let mut content_length: usize = 0;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).await?;
        server.write_all(line.as_bytes()).await?;

        if line.to_lowercase().starts_with("content-length:") {
            content_length = line[15..].trim().parse().unwrap_or(0);
        }

        if line.trim().is_empty() {
            break;
        }
    }

    // Forward body if present
    if content_length > 0 {
        let mut body = vec![0u8; content_length];
        reader.read_exact(&mut body).await?;
        server.write_all(&body).await?;
    }

    // Forward response back to client
    // Get the underlying client stream from the reader
    let client = reader.into_inner();
    tokio::io::copy(&mut server, client).await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_policy_exact_match() {
        let policy = NetworkPolicy {
            allowed_hosts: vec!["api.github.com".to_string()],
        };

        assert!(policy.is_allowed("api.github.com"));
        assert!(policy.is_allowed("api.github.com:443"));
        assert!(!policy.is_allowed("evil.com"));
        assert!(!policy.is_allowed("github.com"));
    }

    #[test]
    fn test_policy_wildcard_match() {
        let policy = NetworkPolicy {
            allowed_hosts: vec!["*.github.com".to_string()],
        };

        assert!(policy.is_allowed("api.github.com"));
        assert!(policy.is_allowed("raw.githubusercontent.github.com"));
        assert!(policy.is_allowed("github.com")); // Matches the suffix without wildcard part
        assert!(!policy.is_allowed("evil.com"));
        assert!(!policy.is_allowed("github.com.evil.com"));
    }

    #[test]
    fn test_policy_multiple_patterns() {
        let policy = NetworkPolicy {
            allowed_hosts: vec![
                "api.github.com".to_string(),
                "*.azure.com".to_string(),
                "dev.azure.com".to_string(),
            ],
        };

        assert!(policy.is_allowed("api.github.com"));
        assert!(policy.is_allowed("dev.azure.com"));
        assert!(policy.is_allowed("management.azure.com"));
        assert!(!policy.is_allowed("evil.com"));
    }

    #[test]
    fn test_policy_with_port() {
        let policy = NetworkPolicy {
            allowed_hosts: vec!["api.github.com".to_string()],
        };

        assert!(policy.is_allowed("api.github.com:443"));
        assert!(policy.is_allowed("api.github.com:8080"));
    }

    #[test]
    fn test_policy_nested_wildcard() {
        let policy = NetworkPolicy {
            allowed_hosts: vec![
                "*.in.applicationinsights.azure.com".to_string(),
                "*.applicationinsights.azure.com".to_string(),
            ],
        };

        // Nested subdomain should match *.in.applicationinsights.azure.com
        assert!(policy.is_allowed("westus3-1.in.applicationinsights.azure.com"));
        assert!(policy.is_allowed("eastus-1.in.applicationinsights.azure.com"));
        // Should also match the broader pattern
        assert!(policy.is_allowed("dc.applicationinsights.azure.com"));
        // Should not match unrelated hosts
        assert!(!policy.is_allowed("evil.applicationinsights.com"));
    }

    #[test]
    fn test_default_allowed_hosts() {
        use crate::allowed_hosts::CORE_ALLOWED_HOSTS;

        // Should include essential hosts for agent operation
        assert!(CORE_ALLOWED_HOSTS.contains(&"github.com"));
        assert!(CORE_ALLOWED_HOSTS.contains(&"api.github.com"));
        assert!(CORE_ALLOWED_HOSTS.contains(&"*.github.com"));
        assert!(CORE_ALLOWED_HOSTS.contains(&"dev.azure.com"));
        assert!(CORE_ALLOWED_HOSTS.contains(&"*.dev.azure.com"));
        assert!(CORE_ALLOWED_HOSTS.contains(&"*.blob.core.windows.net"));
        // Microsoft identity endpoints for OAuth
        assert!(CORE_ALLOWED_HOSTS.contains(&"login.microsoftonline.com"));
        assert!(CORE_ALLOWED_HOSTS.contains(&"login.live.com"));
    }

    #[test]
    fn test_network_policy_new_includes_defaults() {
        // Creating a policy with no additional hosts should include defaults
        let policy = NetworkPolicy::new(vec![]);

        assert!(policy.is_allowed("api.github.com"));
        assert!(policy.is_allowed("dev.azure.com"));
        assert!(policy.is_allowed("something.blob.core.windows.net"));
        assert!(!policy.is_allowed("evil.com"));
    }

    #[test]
    fn test_network_policy_new_merges_additional_hosts() {
        // Creating a policy with additional hosts should include both defaults and additional
        let policy = NetworkPolicy::new(vec!["custom.api.com".to_string()]);

        // Should have custom host
        assert!(policy.is_allowed("custom.api.com"));
        // Should still have defaults
        assert!(policy.is_allowed("api.github.com"));
        assert!(policy.is_allowed("dev.azure.com"));
    }
}
