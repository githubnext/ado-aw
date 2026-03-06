//! MCP Firewall - A filtering proxy for Model Context Protocol servers
//!
//! The firewall acts as a single MCP server that:
//! 1. Loads tool definitions from pre-generated metadata (mcp-metadata.json)
//! 2. Exposes only allowed tools (namespaced as `upstream:tool_name`)
//! 3. Spawns upstream MCP servers lazily when tools are called
//! 4. Routes tool calls to the appropriate upstream
//! 5. Logs all tool call attempts for auditing

use anyhow::{Context, Result};
use log::{debug, error, info, warn};
use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler, ServiceExt, model::*,
    service::RequestContext, transport::stdio,
};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::RwLock;

use crate::mcp_metadata::{McpMetadataFile, ToolMetadata};

// ============================================================================
// Configuration
// ============================================================================

/// Configuration for a single upstream MCP server
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct UpstreamConfig {
    /// Command to spawn the MCP server
    pub command: String,
    /// Arguments to pass to the command
    #[serde(default)]
    pub args: Vec<String>,
    /// Environment variables for the MCP server process
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// List of allowed tool names (without namespace prefix)
    /// Use ["*"] to allow all tools
    pub allowed: Vec<String>,
    /// Timeout in seconds for spawning and initializing the upstream MCP server
    /// Defaults to 30 seconds if not specified
    #[serde(default = "default_spawn_timeout")]
    pub spawn_timeout_secs: u64,
}

fn default_spawn_timeout() -> u64 {
    30
}

impl UpstreamConfig {
    /// Check if a tool name is allowed by this upstream's policy
    pub fn is_tool_allowed(&self, tool_name: &str) -> bool {
        self.allowed.iter().any(|pattern| {
            if pattern == "*" {
                true
            } else if pattern.ends_with('*') {
                // Prefix wildcard: "get_*" matches "get_incident", "get_user"
                let prefix = &pattern[..pattern.len() - 1];
                tool_name.starts_with(prefix)
            } else {
                pattern == tool_name
            }
        })
    }
}

/// Full firewall configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FirewallConfig {
    /// Map of upstream name to configuration
    pub upstreams: HashMap<String, UpstreamConfig>,
    /// Path to MCP metadata file (optional, uses bundled metadata if not provided)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata_path: Option<PathBuf>,
}

impl FirewallConfig {
    /// Load configuration from a JSON file
    pub fn from_file(path: &PathBuf) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read config file: {}", path.display()))?;
        serde_json::from_str(&content)
            .with_context(|| format!("Failed to parse config file: {}", path.display()))
    }

    /// Load MCP metadata (from file if specified, otherwise bundled)
    pub fn load_metadata(&self) -> Result<McpMetadataFile> {
        if let Some(ref path) = self.metadata_path {
            let content = std::fs::read_to_string(path)
                .with_context(|| format!("Failed to read metadata file: {}", path.display()))?;
            serde_json::from_str(&content)
                .with_context(|| format!("Failed to parse metadata file: {}", path.display()))
        } else {
            Ok(McpMetadataFile::bundled())
        }
    }

    /// Create a new empty configuration
    pub fn new() -> Self {
        Self {
            upstreams: HashMap::new(),
            metadata_path: None,
        }
    }
}

impl Default for FirewallConfig {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Upstream MCP Client
// ============================================================================

/// A connection to an upstream MCP server (spawned lazily)
struct UpstreamConnection {
    name: String,
    #[allow(dead_code)]
    child: Child,
    stdin: tokio::process::ChildStdin,
    stdout_reader: BufReader<tokio::process::ChildStdout>,
    request_id: u64,
}

impl UpstreamConnection {
    /// Spawn and initialize an upstream MCP server with timeout
    async fn spawn(name: String, config: &UpstreamConfig) -> Result<Self> {
        let timeout_duration = std::time::Duration::from_secs(config.spawn_timeout_secs);
        let start_time = std::time::Instant::now();

        info!(
            "[{}] Spawning upstream MCP server (timeout: {}s)",
            name, config.spawn_timeout_secs
        );

        // Wrap the entire spawn+initialize sequence in a timeout
        let result = tokio::time::timeout(timeout_duration, async {
            let mut cmd = Command::new(&config.command);
            cmd.args(&config.args);

            for (key, value) in &config.env {
                cmd.env(key, value);
            }

            cmd.stdin(Stdio::piped());
            cmd.stdout(Stdio::piped());
            cmd.stderr(Stdio::inherit()); // Let upstream errors flow to our stderr

            let mut child = cmd.spawn().with_context(|| {
                format!("Failed to spawn upstream '{}': {}", name, config.command)
            })?;

            let stdin = child.stdin.take().ok_or_else(|| {
                anyhow::anyhow!("Failed to capture stdin for upstream '{}'", name)
            })?;
            let stdout = child.stdout.take().ok_or_else(|| {
                anyhow::anyhow!("Failed to capture stdout for upstream '{}'", name)
            })?;

            let mut conn = Self {
                name: name.clone(),
                child,
                stdin,
                stdout_reader: BufReader::new(stdout),
                request_id: 0,
            };

            // Initialize the MCP connection
            conn.initialize().await?;

            Ok::<Self, anyhow::Error>(conn)
        })
        .await;

        let duration = start_time.elapsed();

        match result {
            Ok(Ok(conn)) => {
                info!(
                    "[{}] Successfully spawned and initialized in {:.2}s",
                    name,
                    duration.as_secs_f64()
                );
                Ok(conn)
            }
            Ok(Err(e)) => {
                error!(
                    "[{}] Failed to spawn/initialize after {:.2}s: {}",
                    name,
                    duration.as_secs_f64(),
                    e
                );
                Err(e)
            }
            Err(_) => {
                error!(
                    "[{}] Spawn timeout after {:.2}s (limit: {}s)",
                    name,
                    duration.as_secs_f64(),
                    config.spawn_timeout_secs
                );
                anyhow::bail!(
                    "Timeout spawning upstream '{}' after {}s. The MCP server may be hanging during initialization. \
                     Check that the command '{}' is responsive and properly configured.",
                    name,
                    config.spawn_timeout_secs,
                    config.command
                )
            }
        }
    }

    /// Send a JSON-RPC request and wait for response
    async fn send_request<T: Serialize>(
        &mut self,
        method: &str,
        params: Option<T>,
    ) -> Result<serde_json::Value> {
        self.request_id += 1;
        let id = self.request_id;

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

        let request_str = serde_json::to_string(&request)?;
        debug!("[{}] Sending: {}", self.name, request_str);

        self.stdin.write_all(request_str.as_bytes()).await?;
        self.stdin.write_all(b"\n").await?;
        self.stdin.flush().await?;

        // Read response
        let mut line = String::new();
        self.stdout_reader
            .read_line(&mut line)
            .await
            .with_context(|| format!("Failed to read response from upstream '{}'", self.name))?;

        debug!("[{}] Received: {}", self.name, line.trim());

        let response: serde_json::Value = serde_json::from_str(&line).with_context(|| {
            format!(
                "Failed to parse response from upstream '{}': {}",
                self.name, line
            )
        })?;

        // Check for error
        if let Some(error) = response.get("error") {
            anyhow::bail!("Upstream '{}' returned error: {}", self.name, error);
        }

        Ok(response
            .get("result")
            .cloned()
            .unwrap_or(serde_json::Value::Null))
    }

    /// Initialize the MCP connection
    async fn initialize(&mut self) -> Result<()> {
        let params = serde_json::json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {
                "tools": {}
            },
            "clientInfo": {
                "name": "mcp-firewall",
                "version": env!("CARGO_PKG_VERSION")
            }
        });

        let result = self.send_request("initialize", Some(params)).await?;
        info!(
            "[{}] Initialized: {:?}",
            self.name,
            result.get("serverInfo")
        );

        // Send initialized notification
        let notification = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        });
        let notification_str = serde_json::to_string(&notification)?;
        self.stdin.write_all(notification_str.as_bytes()).await?;
        self.stdin.write_all(b"\n").await?;
        self.stdin.flush().await?;

        Ok(())
    }

    /// Call a tool on this upstream
    async fn call_tool(
        &mut self,
        tool_name: &str,
        arguments: Option<serde_json::Map<String, serde_json::Value>>,
    ) -> Result<CallToolResult> {
        let params = serde_json::json!({
            "name": tool_name,
            "arguments": arguments.unwrap_or_default()
        });

        let result = self.send_request("tools/call", Some(params)).await?;

        // Parse the result into CallToolResult
        let content: Vec<Content> =
            if let Some(content_array) = result.get("content").and_then(|c| c.as_array()) {
                content_array
                    .iter()
                    .filter_map(|c| serde_json::from_value(c.clone()).ok())
                    .collect()
            } else {
                vec![]
            };

        let is_error = result
            .get("isError")
            .and_then(|e| e.as_bool())
            .unwrap_or(false);

        let mut result = CallToolResult::success(content);
        result.is_error = Some(is_error);
        Ok(result)
    }
}

// ============================================================================
// MCP Firewall Server
// ============================================================================

/// Policy for the MCP firewall
#[derive(Debug, Clone)]
pub struct FirewallPolicy {
    pub config: FirewallConfig,
}

/// The MCP Firewall server
pub struct McpFirewall {
    /// Upstream configs (for lazy spawning)
    upstream_configs: HashMap<String, UpstreamConfig>,
    /// Lazily spawned upstream connections
    upstreams: RwLock<HashMap<String, UpstreamConnection>>,
    /// Combined list of all allowed tools (namespaced)
    tools: Vec<Tool>,
}

impl McpFirewall {
    /// Create the firewall from policy and metadata
    pub fn new(policy: FirewallPolicy) -> Result<Self> {
        // Load metadata
        let metadata = policy.config.load_metadata()?;
        let mut all_tools = Vec::new();

        // Build tool list from metadata (filtered by allowed list)
        for (upstream_name, upstream_config) in &policy.config.upstreams {
            if let Some(mcp_meta) = metadata.get(upstream_name) {
                for tool_meta in &mcp_meta.tools {
                    // Check if this tool is allowed
                    if upstream_config.is_tool_allowed(&tool_meta.name) {
                        all_tools.push(Self::metadata_to_tool(upstream_name, tool_meta));
                    }
                }
                info!(
                    "[{}] Loaded {} tools from metadata ({} allowed)",
                    upstream_name,
                    mcp_meta.tools.len(),
                    all_tools
                        .iter()
                        .filter(|t| t.name.starts_with(&format!("{}:", upstream_name)))
                        .count()
                );
            } else {
                warn!(
                    "[{}] No metadata found - tools will be unavailable",
                    upstream_name
                );
            }
        }

        info!(
            "MCP Firewall initialized with {} upstreams, {} total tools (lazy spawning enabled)",
            policy.config.upstreams.len(),
            all_tools.len()
        );

        Ok(Self {
            upstream_configs: policy.config.upstreams.clone(),
            upstreams: RwLock::new(HashMap::new()),
            tools: all_tools,
        })
    }

    /// Convert ToolMetadata to rmcp Tool with namespace prefix
    fn metadata_to_tool(upstream_name: &str, meta: &ToolMetadata) -> Tool {
        Tool {
            name: Cow::Owned(format!("{}:{}", upstream_name, meta.name)),
            description: meta.description.clone().map(Cow::Owned),
            input_schema: meta
                .input_schema
                .clone()
                .and_then(|v| serde_json::from_value(v).ok())
                .unwrap_or_default(),
            annotations: None,
            icons: None,
            output_schema: None,
            title: None,
        }
    }

    /// Get or spawn an upstream connection
    async fn get_or_spawn_upstream(&self, upstream_name: &str) -> Result<(), McpError> {
        // Fast path: check if already spawned with read lock
        {
            let upstreams = self.upstreams.read().await;
            if upstreams.contains_key(upstream_name) {
                return Ok(());
            }
        }

        // Need to spawn - get config first
        let config = self.upstream_configs.get(upstream_name).ok_or_else(|| {
            McpError::invalid_params(format!("Unknown upstream: '{}'", upstream_name), None)
        })?;

        info!("[{}] Spawning upstream MCP server (lazy)", upstream_name);

        // Spawn the connection outside of any locks (this is the expensive operation)
        let conn = UpstreamConnection::spawn(upstream_name.to_string(), config)
            .await
            .map_err(|e| {
                McpError::internal_error(format!("Failed to spawn upstream: {}", e), None)
            })?;

        // Acquire write lock and check again (double-check pattern)
        // Another task might have spawned and inserted while we were spawning above
        let mut upstreams = self.upstreams.write().await;
        if !upstreams.contains_key(upstream_name) {
            upstreams.insert(upstream_name.to_string(), conn);
        }
        // If another task already inserted, our `conn` is dropped here, which terminates
        // the child process via Drop. This prevents duplicate upstream connections.

        Ok(())
    }

    /// Log a message via centralized logging
    fn log(&self, message: &str) {
        info!(target: "firewall", "{}", message);
    }

    /// Parse a namespaced tool name into (upstream, tool_name)
    fn parse_tool_name(namespaced: &str) -> Option<(&str, &str)> {
        namespaced.split_once(':')
    }

    /// Check if a tool is allowed by upstream config
    fn is_tool_allowed(&self, upstream_name: &str, tool_name: &str) -> bool {
        self.upstream_configs
            .get(upstream_name)
            .map(|c| c.is_tool_allowed(tool_name))
            .unwrap_or(false)
    }
}

impl ServerHandler for McpFirewall {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            instructions: Some(
                "MCP Firewall - A secure proxy for accessing multiple MCP servers with policy-based filtering.".into()
            ),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParam>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        Ok(ListToolsResult {
            tools: self.tools.clone(),
            next_cursor: None,
        })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParam,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let tool_name = &request.name;

        // Parse namespaced tool name
        let (upstream_name, local_tool_name) = match Self::parse_tool_name(tool_name) {
            Some((u, t)) => (u, t),
            None => {
                self.log(&format!(
                    "BLOCKED {} (invalid format, expected 'upstream:tool')",
                    tool_name
                ));
                return Err(McpError::invalid_params(
                    format!(
                        "Invalid tool name format. Expected 'upstream:tool', got '{}'",
                        tool_name
                    ),
                    None,
                ));
            }
        };

        // Check if upstream exists in config
        if !self.upstream_configs.contains_key(upstream_name) {
            self.log(&format!(
                "BLOCKED {} (unknown upstream '{}')",
                tool_name, upstream_name
            ));
            return Err(McpError::invalid_params(
                format!("Unknown upstream: '{}'", upstream_name),
                None,
            ));
        }

        // Check if tool is allowed
        if !self.is_tool_allowed(upstream_name, local_tool_name) {
            self.log(&format!("BLOCKED {} (not in allowlist)", tool_name));
            return Err(McpError::invalid_params(
                format!("Tool '{}' is not allowed by firewall policy", tool_name),
                None,
            ));
        }

        // Ensure upstream is spawned (lazy initialization)
        self.get_or_spawn_upstream(upstream_name).await?;

        // Log the allowed call
        let args_summary = request
            .arguments
            .as_ref()
            .map(|a| {
                let s = serde_json::to_string(a).unwrap_or_default();
                if s.len() > 100 {
                    format!("{}...", &s[..100])
                } else {
                    s
                }
            })
            .unwrap_or_default();
        self.log(&format!("ALLOWED {} (args: {})", tool_name, args_summary));

        // Forward the call to upstream
        let mut upstreams = self.upstreams.write().await;
        let conn = upstreams.get_mut(upstream_name).ok_or_else(|| {
            McpError::internal_error("Upstream connection lost after spawn", None)
        })?;

        match conn.call_tool(local_tool_name, request.arguments).await {
            Ok(result) => Ok(result),
            Err(e) => {
                warn!(
                    "Upstream '{}' error calling '{}': {}",
                    upstream_name, local_tool_name, e
                );
                Err(McpError::internal_error(e.to_string(), None))
            }
        }
    }
}

// ============================================================================
// Entry Point
// ============================================================================

/// Start the MCP firewall server
pub async fn run(config_path: &PathBuf) -> Result<()> {
    let config = FirewallConfig::from_file(config_path)?;

    let policy = FirewallPolicy { config };

    let firewall = McpFirewall::new(policy)?;

    firewall.log("MCP Firewall started");
    firewall.log(&format!("Upstreams ({}):", firewall.upstream_configs.len()));
    for (name, config) in &firewall.upstream_configs {
        firewall.log(&format!("  [{}] command: {}", name, config.command));
        firewall.log(&format!("  [{}] allowed: {:?}", name, config.allowed));
    }
    firewall.log(&format!("Total tools exposed: {}", firewall.tools.len()));

    // Run as MCP server on stdio
    let service = firewall.serve(stdio()).await.inspect_err(|e| {
        error!("Error starting MCP firewall: {}", e);
    })?;

    service
        .waiting()
        .await
        .map_err(|e| anyhow::anyhow!("MCP firewall exited with error: {:?}", e))?;

    Ok(())
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_upstream_config_is_tool_allowed_exact() {
        let config = UpstreamConfig {
            command: "test".to_string(),
            args: vec![],
            env: HashMap::new(),
            allowed: vec!["create_incident".to_string(), "get_incident".to_string()],
            spawn_timeout_secs: 30,
        };

        assert!(config.is_tool_allowed("create_incident"));
        assert!(config.is_tool_allowed("get_incident"));
        assert!(!config.is_tool_allowed("delete_incident"));
        assert!(!config.is_tool_allowed("list_incidents"));
    }

    #[test]
    fn test_upstream_config_is_tool_allowed_wildcard() {
        let config = UpstreamConfig {
            command: "test".to_string(),
            args: vec![],
            env: HashMap::new(),
            allowed: vec!["*".to_string()],
            spawn_timeout_secs: 30,
        };

        assert!(config.is_tool_allowed("anything"));
        assert!(config.is_tool_allowed("create_incident"));
        assert!(config.is_tool_allowed("dangerous_delete_all"));
    }

    #[test]
    fn test_upstream_config_is_tool_allowed_prefix_wildcard() {
        let config = UpstreamConfig {
            command: "test".to_string(),
            args: vec![],
            env: HashMap::new(),
            allowed: vec!["get_*".to_string(), "list_*".to_string()],
            spawn_timeout_secs: 30,
        };

        assert!(config.is_tool_allowed("get_incident"));
        assert!(config.is_tool_allowed("get_user"));
        assert!(config.is_tool_allowed("list_incidents"));
        assert!(!config.is_tool_allowed("create_incident"));
        assert!(!config.is_tool_allowed("delete_all"));
    }

    #[test]
    fn test_firewall_config_from_json() {
        let json = r#"{
            "upstreams": {
                "icm": {
                    "command": "icm-mcp",
                    "args": ["--verbose"],
                    "allowed": ["create_incident", "get_incident"]
                },
                "kusto": {
                    "command": "kusto-mcp",
                    "allowed": ["query"]
                }
            }
        }"#;

        let config: FirewallConfig = serde_json::from_str(json).unwrap();

        assert_eq!(config.upstreams.len(), 2);
        assert!(config.upstreams.contains_key("icm"));
        assert!(config.upstreams.contains_key("kusto"));

        let icm = &config.upstreams["icm"];
        assert_eq!(icm.command, "icm-mcp");
        assert_eq!(icm.args, vec!["--verbose"]);
        assert_eq!(icm.allowed, vec!["create_incident", "get_incident"]);
        assert_eq!(icm.spawn_timeout_secs, 30, "Should default to 30 seconds");

        let kusto = &config.upstreams["kusto"];
        assert_eq!(kusto.command, "kusto-mcp");
        assert!(kusto.args.is_empty());
        assert_eq!(kusto.allowed, vec!["query"]);
        assert_eq!(kusto.spawn_timeout_secs, 30, "Should default to 30 seconds");
    }

    #[test]
    fn test_parse_tool_name() {
        assert_eq!(
            McpFirewall::parse_tool_name("icm:create_incident"),
            Some(("icm", "create_incident"))
        );
        assert_eq!(
            McpFirewall::parse_tool_name("kusto:query"),
            Some(("kusto", "query"))
        );
        assert_eq!(McpFirewall::parse_tool_name("no_colon"), None);
        assert_eq!(
            McpFirewall::parse_tool_name("multiple:colons:here"),
            Some(("multiple", "colons:here"))
        );
    }

    #[test]
    fn test_firewall_config_default() {
        let config = FirewallConfig::default();
        assert!(config.upstreams.is_empty());
    }

    #[test]
    fn test_upstream_config_timeout_custom() {
        let json = r#"{
            "upstreams": {
                "slow-service": {
                    "command": "slow-mcp",
                    "allowed": ["*"],
                    "spawn_timeout_secs": 60
                }
            }
        }"#;

        let config: FirewallConfig = serde_json::from_str(json).unwrap();
        let slow_service = &config.upstreams["slow-service"];

        assert_eq!(
            slow_service.spawn_timeout_secs, 60,
            "Should use custom timeout of 60 seconds"
        );
    }

    #[test]
    fn test_upstream_config_timeout_default() {
        let json = r#"{
            "upstreams": {
                "normal-service": {
                    "command": "normal-mcp",
                    "allowed": ["*"]
                }
            }
        }"#;

        let config: FirewallConfig = serde_json::from_str(json).unwrap();
        let normal_service = &config.upstreams["normal-service"];

        assert_eq!(
            normal_service.spawn_timeout_secs, 30,
            "Should default to 30 seconds when not specified"
        );
    }
}
