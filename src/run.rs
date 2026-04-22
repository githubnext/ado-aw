//! Local development run mode.
//!
//! Orchestrates the full agent lifecycle locally: parse markdown →
//! start SafeOutputs → optionally start MCPG → generate configs →
//! exec copilot → execute safe outputs → cleanup.

use anyhow::{Context, Result, bail};
use log::{debug, info, warn};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use crate::compile;
use crate::sanitize::SanitizeConfig;

/// Arguments for the `run` subcommand.
pub struct RunArgs {
    pub agent_path: PathBuf,
    pub pat: Option<String>,
    pub org: Option<String>,
    pub project: Option<String>,
    pub dry_run: bool,
    pub skip_mcpg: bool,
    pub output_dir: Option<PathBuf>,
    pub debug: bool,
}

/// Result of [`setup_mcp_config`]: the client config path and optional MCPG
/// process handles that must be transferred into [`CleanupGuard`].
struct McpSetupResult {
    mcp_config_path: PathBuf,
    mcpg_child: Option<Child>,
    mcpg_env_file: Option<tempfile::NamedTempFile>,
}

/// Guard that kills child processes on drop (normal exit, error, or panic).
struct CleanupGuard {
    safeoutputs_child: Option<Child>,
    mcpg_child: Option<Child>,
    /// Keeps the env file alive until MCPG exits. The file must outlive
    /// `spawn()` because spawn is just fork — the Docker CLI reads
    /// `--env-file` after exec, not before spawn returns.
    #[allow(dead_code)]
    mcpg_env_file: Option<tempfile::NamedTempFile>,
}

impl Drop for CleanupGuard {
    fn drop(&mut self) {
        if let Some(ref mut child) = self.safeoutputs_child {
            info!("Stopping SafeOutputs server...");
            let _ = child.kill();
            let _ = child.wait();
        }
        if self.mcpg_child.is_some() {
            info!("Stopping MCPG container...");
            let _ = Command::new("docker")
                .args(["stop", "ado-aw-mcpg"])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            // Reap the docker run process to prevent zombie
            if let Some(ref mut child) = self.mcpg_child {
                let _ = child.wait();
            }
        }
    }
}

/// Strip the `\\?\` extended-length path prefix that Windows `canonicalize()`
/// adds. This prefix breaks many tools (including Copilot CLI) that don't
/// understand UNC paths. Only strips when the path is a regular drive path
/// (e.g., `\\?\C:\...` → `C:\...`), not a true UNC path (`\\?\UNC\...`).
fn strip_unc_prefix(path: PathBuf) -> PathBuf {
    let s = path.to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\") {
        if !rest.starts_with(r"UNC\") {
            return PathBuf::from(rest);
        }
    }
    path
}

/// Find a free TCP port by binding to port 0.
///
/// Note: There is an inherent TOCTOU race — the port is released before the
/// child process binds it. Another process could grab it in the gap. This is
/// acceptable for a local dev tool; fixing it would require refactoring the
/// mcp-http server to accept a pre-bound listener.
fn find_free_port() -> Result<u16> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")
        .context("Failed to bind to a free port")?;
    let port = listener.local_addr()?.port();
    drop(listener);
    Ok(port)
}

/// Generate a random alphanumeric API key (at least 40 chars).
fn generate_api_key() -> String {
    use rand::RngExt;
    let mut bytes = [0u8; 48];
    rand::rng().fill(&mut bytes);
    base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, &bytes)
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect()
}

/// Start the SafeOutputs MCP HTTP server as a child process.
async fn start_safeoutputs(
    output_dir: &Path,
    bounding_dir: &Path,
    enabled_tools: &[String],
) -> Result<(Child, u16, String)> {
    let port = find_free_port()?;
    let api_key = generate_api_key();

    let exe = std::env::current_exe().context("Failed to determine current executable path")?;

    let mut cmd = Command::new(&exe);
    cmd.arg("mcp-http")
        .arg("--port")
        .arg(port.to_string())
        .arg("--api-key")
        .arg(&api_key);

    for tool in enabled_tools {
        cmd.arg("--enabled-tools").arg(tool);
    }

    cmd.arg(output_dir.to_string_lossy().as_ref())
        .arg(bounding_dir.to_string_lossy().as_ref());

    // Redirect output to log files
    let log_dir = output_dir.join("logs");
    std::fs::create_dir_all(&log_dir)?;
    let stdout_log = log_dir.join("safeoutputs.stdout.log");
    let stderr_log = log_dir.join("safeoutputs.stderr.log");
    let stdout_file = std::fs::File::create(&stdout_log)
        .with_context(|| format!("Failed to create log file: {}", stdout_log.display()))?;
    let stderr_file = std::fs::File::create(&stderr_log)
        .with_context(|| format!("Failed to create log file: {}", stderr_log.display()))?;

    cmd.stdout(stdout_file).stderr(stderr_file);

    let mut child = cmd.spawn().context("Failed to start SafeOutputs HTTP server")?;
    info!("SafeOutputs started (PID: {}, port: {})", child.id(), port);

    // Health check — also detect early crash via try_wait()
    let client = reqwest::Client::new();
    let health_url = format!("http://127.0.0.1:{}/health", port);
    let mut ready = false;
    for _ in 0..30 {
        // Check if process crashed before polling the endpoint
        if let Some(status) = child.try_wait()? {
            bail!(
                "SafeOutputs HTTP server exited during startup with {}. \
                 Check logs at {}",
                status,
                log_dir.display()
            );
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        match client.get(&health_url).send().await {
            Ok(resp) if resp.status().is_success() => {
                ready = true;
                break;
            }
            _ => continue,
        }
    }
    if !ready {
        bail!("SafeOutputs HTTP server did not become ready within 30s on port {}", port);
    }

    Ok((child, port, api_key))
}

/// Check if Docker is available.
fn is_docker_available() -> bool {
    Command::new("docker")
        .arg("info")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Probe all MCP backends via MCPG to force eager launch and surface failures.
///
/// Sends MCP `initialize` + `tools/list` handshakes to each server listed in
/// the MCP client config. Results are printed to stdout — failures are warnings,
/// not hard errors, since some backends may intentionally be unavailable.
async fn probe_mcp_backends(
    client: &reqwest::Client,
    mcpg_port: u16,
    api_key: &str,
    mcp_config_path: &Path,
) {
    println!("\n=== Probing MCP backends ===");

    let config_str = match tokio::fs::read_to_string(mcp_config_path).await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Warning: Could not read MCP config for probing: {}", e);
            return;
        }
    };
    let config: serde_json::Value = match serde_json::from_str(&config_str) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Warning: Could not parse MCP config for probing: {}", e);
            return;
        }
    };

    let servers = match config.get("mcpServers").and_then(|s| s.as_object()) {
        Some(s) => s,
        None => {
            eprintln!("Warning: No mcpServers found in MCP config");
            return;
        }
    };

    let mut any_failed = false;

    for server_name in servers.keys() {
        print!("  {} ... ", server_name);

        // Extract the server's URL to determine the routed path
        let server_url = format!("http://127.0.0.1:{}/mcp/{}", mcpg_port, server_name);

        // Step 1: MCP initialize handshake
        let init_body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-03-26",
                "capabilities": {},
                "clientInfo": {
                    "name": "ado-aw-probe",
                    "version": "1.0"
                }
            }
        });

        // MCPG expects the raw API key in Authorization (not Bearer scheme)
        let init_result = client
            .post(&server_url)
            .header("Authorization", api_key)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .timeout(std::time::Duration::from_secs(120))
            .json(&init_body)
            .send()
            .await;

        let session_id = match init_result {
            Ok(resp) => {
                let session = resp
                    .headers()
                    .get("mcp-session-id")
                    .and_then(|v| v.to_str().ok())
                    .map(String::from);
                if session.is_none() {
                    println!("⚠ no session ID returned");
                    any_failed = true;
                    continue;
                }
                session.unwrap()
            }
            Err(e) => {
                println!("✗ initialize failed: {}", e);
                any_failed = true;
                continue;
            }
        };

        // Step 2: tools/list with session ID
        let list_body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list"
        });

        let list_result = client
            .post(&server_url)
            .header("Authorization", api_key)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .header("Mcp-Session-Id", &session_id)
            .timeout(std::time::Duration::from_secs(120))
            .json(&list_body)
            .send()
            .await;

        match list_result {
            Ok(resp) if resp.status().is_success() => {
                // Try to extract tool count from the response body.
                // MCPG may return SSE (text/event-stream) or plain JSON.
                let body = resp.text().await.unwrap_or_default();
                let tool_count = extract_tool_count(&body);
                match tool_count {
                    Some(n) => println!("✓ {} tools available", n),
                    None => println!("✓ ready (tool count unknown)"),
                }
            }
            Ok(resp) => {
                println!("⚠ tools/list returned HTTP {}", resp.status());
                any_failed = true;
            }
            Err(e) => {
                println!("✗ tools/list failed: {}", e);
                any_failed = true;
            }
        }
    }

    if any_failed {
        println!("\n  ⚠ One or more MCP backends failed to initialize — check MCPG logs");
    }
    println!();
}

/// Extract tool count from an MCP tools/list response body.
/// Handles both plain JSON and SSE (text/event-stream with `data:` lines).
fn extract_tool_count(body: &str) -> Option<usize> {
    // Try SSE format first: look for `data: {...}` lines
    for line in body.lines() {
        if let Some(data) = line.strip_prefix("data: ").or_else(|| line.strip_prefix("data:")) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(data.trim()) {
                if let Some(tools) = v.pointer("/result/tools").and_then(|t| t.as_array()) {
                    return Some(tools.len());
                }
            }
        }
    }
    // Try plain JSON
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(body) {
        if let Some(tools) = v.pointer("/result/tools").and_then(|t| t.as_array()) {
            return Some(tools.len());
        }
    }
    None
}

/// Dump MCPG log files to stderr for diagnostics. Called when MCPG fails to
/// start or crashes during health check. Reads stderr.log (MCPG's own output)
/// and mcp-gateway.log (unified per-server log) if they exist.
fn dump_mcpg_logs(mcpg_log_dir: &Path) {
    eprintln!("\n--- MCPG diagnostic logs ---");

    let stderr_log = mcpg_log_dir.join("stderr.log");
    if stderr_log.exists() {
        if let Ok(content) = std::fs::read_to_string(&stderr_log) {
            let content = content.trim();
            if !content.is_empty() {
                eprintln!("\n[stderr.log]:");
                // Limit output to last 100 lines to avoid flooding the terminal
                let lines: Vec<&str> = content.lines().collect();
                let start = lines.len().saturating_sub(100);
                for line in &lines[start..] {
                    eprintln!("  {}", line);
                }
                if start > 0 {
                    eprintln!("  ... ({} earlier lines omitted)", start);
                }
            }
        }
    }

    let gateway_log = mcpg_log_dir.join("mcp-gateway.log");
    if gateway_log.exists() {
        if let Ok(content) = std::fs::read_to_string(&gateway_log) {
            let content = content.trim();
            if !content.is_empty() {
                eprintln!("\n[mcp-gateway.log]:");
                let lines: Vec<&str> = content.lines().collect();
                let start = lines.len().saturating_sub(50);
                for line in &lines[start..] {
                    eprintln!("  {}", line);
                }
                if start > 0 {
                    eprintln!("  ... ({} earlier lines omitted)", start);
                }
            }
        }
    }

    // List any per-server log files for hints
    if let Ok(entries) = std::fs::read_dir(mcpg_log_dir) {
        let server_logs: Vec<_> = entries
            .filter_map(|e| e.ok())
            .filter(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                name.ends_with(".log")
                    && name != "stderr.log"
                    && name != "mcp-gateway.log"
            })
            .collect();
        if !server_logs.is_empty() {
            eprintln!("\nPer-server log files:");
            for entry in &server_logs {
                eprintln!("  {}", entry.path().display());
            }
        }
    }

    eprintln!("--- end MCPG logs ---\n");
}

/// Start the MCPG Docker container. Returns the `docker run` child process
/// and the env file handle. Both must be stored in `CleanupGuard`:
/// - The child is reaped after `docker stop`
/// - The env file must outlive the child because `spawn()` returns after
///   fork() — the Docker CLI hasn't yet exec'd or read `--env-file`
///
/// `gateway_output_path` receives MCPG's stdout — the runtime gateway config
/// JSON that is later transformed into the copilot MCP client config.
fn start_mcpg(
    mcpg_config_json: &str,
    mcpg_api_key: &str,
    port: u16,
    gateway_output_path: &Path,
    mcpg_log_dir: &Path,
    pat: Option<&str>,
    needs_ado_token: bool,
    debug: bool,
) -> Result<(Child, tempfile::NamedTempFile)> {
    // Remove stale container
    let _ = Command::new("docker")
        .args(["rm", "-f", "ado-aw-mcpg"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    // Write secrets to a temp file to avoid exposing in ps/cmdline.
    // The caller must keep the returned NamedTempFile alive — spawn() is
    // just fork(), so the Docker CLI reads --env-file after exec, which
    // happens after spawn() returns.
    let env_file = tempfile::NamedTempFile::new()
        .context("Failed to create temp env file for MCPG secrets")?;
    let mut env_contents = format!(
        "MCP_GATEWAY_PORT={}\nMCP_GATEWAY_DOMAIN=127.0.0.1\nMCP_GATEWAY_API_KEY={}\n",
        port, mcpg_api_key,
    );
    if needs_ado_token {
        if let Some(pat) = pat {
            // Set both vars so MCPG can passthrough whichever the auth mode needs:
            //   PERSONAL_ACCESS_TOKEN — read by ADO MCP in `-a pat` mode (local dev).
            //     The ADO MCP returns this value as-is for Basic auth, so it must be
            //     base64(":<rawPAT>") per the microsoft/azure-devops-mcp auth contract.
            //   AZURE_DEVOPS_EXT_PAT  — used by copilot and execute stages (raw PAT)
            use base64::Engine;
            let b64_pat =
                base64::engine::general_purpose::STANDARD.encode(format!(":{}", pat));
            env_contents.push_str(&format!("PERSONAL_ACCESS_TOKEN={}\n", b64_pat));
            env_contents.push_str(&format!("AZURE_DEVOPS_EXT_PAT={}\n", pat));
        }
    }
    std::fs::write(env_file.path(), &env_contents)
        .with_context(|| format!("Failed to write MCPG env file: {}", env_file.path().display()))?;

    // Enable verbose MCPG logging in debug mode. This sets DEBUG on the
    // MCPG gateway process itself. Note: do NOT set DEBUG in individual
    // server configs (McpgServerConfig.env) — MCPG passes those to child
    // MCP containers where the npm `debug` package may write to stdout,
    // corrupting the JSON-RPC stdio protocol.
    if debug {
        let mut contents = std::fs::read_to_string(env_file.path())
            .with_context(|| "Failed to re-read MCPG env file for DEBUG injection")?;
        contents.push_str("DEBUG=*\n");
        std::fs::write(env_file.path(), &contents)
            .with_context(|| "Failed to write DEBUG env to MCPG env file")?;
    }

    let mut args = vec![
        "run".to_string(),
        "-i".to_string(),
        "--rm".to_string(),
        "--name".to_string(),
        "ado-aw-mcpg".to_string(),
    ];

    // Network strategy differs by platform:
    //   Linux:         --network host shares the host stack; 127.0.0.1 works both ways.
    //   Windows/macOS: Docker Desktop runs containers in a VM. --network host doesn't
    //                  expose container ports on the host. Use -p for port mapping and
    //                  host.docker.internal for container→host communication.
    if cfg!(target_os = "linux") {
        args.extend(["--network".to_string(), "host".to_string()]);
    } else {
        args.extend(["-p".to_string(), format!("{}:{}", port, port)]);
    }

    args.extend([
        "--entrypoint".to_string(),
        "/app/awmg".to_string(),
        "-v".to_string(),
        "/var/run/docker.sock:/var/run/docker.sock".to_string(),
    ]);

    // Mount the MCPG log dir to the host so logs survive container removal (--rm).
    std::fs::create_dir_all(mcpg_log_dir)
        .with_context(|| format!("Failed to create MCPG log dir: {}", mcpg_log_dir.display()))?;
    args.extend([
        "-v".to_string(),
        format!("{}:/tmp/gh-aw/mcp-logs", mcpg_log_dir.to_string_lossy()),
    ]);

    args.extend([
        "--env-file".to_string(),
        env_file.path().to_string_lossy().into_owned(),
    ]);

    args.push(format!("{}:v{}", compile::MCPG_IMAGE, compile::MCPG_VERSION));
    args.push("--routed".to_string());
    args.push("--listen".to_string());
    // Linux (--network host): bind to loopback only.
    // Windows/macOS (-p mapping): bind to 0.0.0.0 so Docker can forward traffic.
    if cfg!(target_os = "linux") {
        args.push(format!("127.0.0.1:{}", port));
    } else {
        args.push(format!("0.0.0.0:{}", port));
    }
    args.push("--config-stdin".to_string());
    args.push("--log-dir".to_string());
    args.push("/tmp/gh-aw/mcp-logs".to_string());

    // Redirect stdout to the gateway output file — MCPG writes its runtime
    // config (with actual URLs) to stdout once it finishes initialising servers.
    let gateway_file = std::fs::File::create(gateway_output_path)
        .with_context(|| format!("Failed to create gateway output file: {}", gateway_output_path.display()))?;

    // Stderr handling:
    //   debug=true  → inherit to terminal for live visibility
    //   debug=false → capture to stderr.log (not lost, just quiet)
    let stderr_cfg = if debug {
        Stdio::inherit()
    } else {
        let stderr_path = mcpg_log_dir.join("stderr.log");
        let stderr_file = std::fs::File::create(&stderr_path)
            .with_context(|| format!("Failed to create MCPG stderr log: {}", stderr_path.display()))?;
        Stdio::from(stderr_file)
    };

    let mut child = Command::new("docker")
        .args(&args)
        .stdin(Stdio::piped())
        .stdout(gateway_file)
        .stderr(stderr_cfg)
        .spawn()
        .context("Failed to start MCPG Docker container")?;

    // Pipe config to stdin, then close so container sees EOF
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(mcpg_config_json.as_bytes())
            .context("Failed to write MCPG config to stdin")?;
        drop(stdin);
    }

    // Caller must keep env_file alive until Docker has read it
    Ok((child, env_file))
}

/// Transform MCPG's gateway output JSON into a copilot-compatible MCP client config.
///
/// Mirrors the pipeline's `jq` transformation:
///   - Keep URLs as-is (local run: copilot runs on host, same as MCPG)
///   - Ensure `tools: ["*"]` on each server entry (Copilot CLI requirement)
///   - Preserve headers and other fields
fn transform_gateway_output(gateway_json: &str) -> Result<String> {
    let mut config: serde_json::Value = serde_json::from_str(gateway_json)
        .context("Failed to parse MCPG gateway output as JSON")?;

    let servers = config
        .get_mut("mcpServers")
        .and_then(|v| v.as_object_mut())
        .context("Gateway output missing mcpServers")?;

    for (_name, entry) in servers.iter_mut() {
        if let Some(obj) = entry.as_object_mut() {
            obj.insert(
                "tools".into(),
                serde_json::Value::Array(vec![serde_json::Value::String("*".into())]),
            );
        }
    }

    serde_json::to_string_pretty(&config)
        .context("Failed to serialize MCP client config")
}

/// Build a `std::process::Command` for a program that may be a script wrapper.
///
/// On Windows, npm-installed tools like `copilot` are `.cmd`/`.ps1` wrappers.
/// `Command::new("copilot")` won't find them — `cmd /C` resolves `.cmd`/`.bat`
/// from PATH and handles execution natively.
/// On Unix (Linux/macOS), `Command::new` resolves scripts via the shebang.
#[allow(dead_code)]
fn host_command(program: &str) -> Command {
    if cfg!(windows) {
        let mut cmd = Command::new("cmd");
        cmd.args(["/C", program]);
        cmd
    } else {
        Command::new(program)
    }
}

/// Async variant of [`host_command`] using `tokio::process::Command`.
fn host_command_async(program: &str) -> tokio::process::Command {
    if cfg!(windows) {
        let mut cmd = tokio::process::Command::new("cmd");
        cmd.args(["/C", program]);
        cmd
    } else {
        tokio::process::Command::new(program)
    }
}

/// Check if an executable is available on PATH.
/// Uses `where` (Windows) or `which` (Unix) to avoid running the program.
fn is_on_path(name: &str) -> bool {
    let (checker, args) = if cfg!(windows) {
        ("where", vec![name.to_string()])
    } else {
        ("which", vec![name.to_string()])
    };
    Command::new(checker)
        .args(&args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Walk up from `start` to find the nearest directory containing `.git`.
fn find_repo_root(start: &Path) -> Option<PathBuf> {
    let mut current = if start.is_absolute() {
        start.to_path_buf()
    } else {
        std::env::current_dir().ok()?.join(start)
    };
    loop {
        if current.join(".git").exists() {
            return Some(current);
        }
        if !current.pop() {
            return None;
        }
    }
}

/// Generate the MCP client config and optionally start the MCPG gateway.
///
/// When `use_mcpg` is true, this:
/// 1. Generates and serialises the MCPG config with runtime placeholder substitution
/// 2. Starts the MCPG Docker container
/// 3. Waits for the MCPG health endpoint
/// 4. Polls for the gateway's runtime output JSON
/// 5. Transforms that output into the Copilot MCP client config
///
/// When `use_mcpg` is false, writes a minimal direct-to-SafeOutputs config.
///
/// Returns an [`McpSetupResult`] whose `mcpg_child` / `mcpg_env_file` fields
/// must be stored in the caller's [`CleanupGuard`].
async fn setup_mcp_config(
    use_mcpg: bool,
    output_dir: &Path,
    so_port: u16,
    so_api_key: &str,
    front_matter: &compile::types::FrontMatter,
    compile_ctx: &compile::extensions::CompileContext<'_>,
    extensions: &[compile::extensions::Extension],
    pat: Option<&str>,
    needs_ado_token: bool,
    debug: bool,
) -> Result<McpSetupResult> {
    let mcp_config_path = output_dir.join("mcp-config.json");

    if use_mcpg {
        let mcpg_api_key = generate_api_key();
        let mcpg_port = find_free_port().context("Failed to find a free port for MCPG")?;

        println!("\n=== Generating MCPG config ===");

        let mut mcpg_config =
            compile::generate_mcpg_config(front_matter, compile_ctx, extensions)?;
        mcpg_config.gateway.port = mcpg_port;

        let mcpg_json = serde_json::to_string_pretty(&mcpg_config)
            .context("Failed to serialize MCPG config")?;
        let mcpg_json = mcpg_json
            .replace("${SAFE_OUTPUTS_PORT}", &so_port.to_string())
            .replace("${SAFE_OUTPUTS_API_KEY}", so_api_key)
            .replace("${MCP_GATEWAY_API_KEY}", &mcpg_api_key);

        // Rewrite SafeOutputs URL for the container→host network path.
        // The compile-time config uses "localhost" which works in pipelines
        // (Linux, --network host shares the host stack). Locally:
        //   - Linux: "localhost" may resolve to ::1 (IPv6) but SafeOutputs
        //     binds IPv4 only, so use 127.0.0.1 explicitly.
        //   - Windows/macOS: Docker Desktop runs containers in a VM, so
        //     "localhost" is the VM loopback. Use host.docker.internal.
        let mcpg_json = if cfg!(target_os = "linux") {
            mcpg_json.replace("http://localhost:", "http://127.0.0.1:")
        } else {
            mcpg_json.replace("http://localhost:", "http://host.docker.internal:")
        };

        tokio::fs::write(output_dir.join("mcpg-config.json"), &mcpg_json)
            .await
            .with_context(|| {
                format!(
                    "Failed to write MCPG config: {}",
                    output_dir.join("mcpg-config.json").display()
                )
            })?;
        debug!("MCPG config written");

        println!("\n=== Starting MCP Gateway (MCPG) ===");
        if needs_ado_token && pat.is_none() {
            warn!(
                "ADO MCP requires a PAT but none was provided (--pat or AZURE_DEVOPS_EXT_PAT). \
                 ADO MCP tool calls will likely fail at runtime."
            );
            println!("Warning: ADO MCP enabled but no PAT provided — tool calls may fail");
        }

        let gateway_output_path = output_dir.join("gateway-output.json");
        let mcpg_log_dir = output_dir.join("mcpg-logs");
        if debug {
            println!("MCPG logs will be written to: {}", mcpg_log_dir.display());
        }

        let (mcpg_child, mcpg_env_file) = start_mcpg(
            &mcpg_json,
            &mcpg_api_key,
            mcpg_port,
            &gateway_output_path,
            &mcpg_log_dir,
            pat,
            needs_ado_token,
            debug,
        )?;

        wait_for_mcpg_health(mcpg_port, &mcpg_log_dir).await?;

        println!("MCPG ready on port {}", mcpg_port);
        println!("MCPG logs: {}", mcpg_log_dir.display());
        println!("  Tip: tail -f {}/mcp-gateway.log", mcpg_log_dir.display());

        wait_for_gateway_output(&gateway_output_path, &mcpg_log_dir).await?;

        let gateway_json = tokio::fs::read_to_string(&gateway_output_path)
            .await
            .context("Failed to read MCPG gateway output")?;
        debug!("Gateway output: {}", gateway_json);
        let mcp_client_json = transform_gateway_output(&gateway_json)?;

        tokio::fs::write(&mcp_config_path, &mcp_client_json)
            .await
            .with_context(|| {
                format!("Failed to write MCP config: {}", mcp_config_path.display())
            })?;
        debug!("MCP client config written");
        println!("MCP client config generated from gateway output");

        if debug {
            let client = reqwest::Client::new();
            probe_mcp_backends(&client, mcpg_port, &mcpg_api_key, &mcp_config_path).await;
        }

        Ok(McpSetupResult {
            mcp_config_path,
            mcpg_child: Some(mcpg_child),
            mcpg_env_file: Some(mcpg_env_file),
        })
    } else {
        println!("\n=== Generating direct MCP config (no MCPG) ===");
        let direct_config = serde_json::json!({
            "mcpServers": {
                "safeoutputs": {
                    "type": "http",
                    "url": format!("http://127.0.0.1:{}/mcp", so_port),
                    "headers": {
                        "Authorization": format!("Bearer {}", so_api_key)
                    },
                    "tools": ["*"]
                }
            }
        });
        let mcp_client_json = serde_json::to_string_pretty(&direct_config)
            .context("Failed to serialize direct MCP config")?;
        tokio::fs::write(&mcp_config_path, &mcp_client_json)
            .await
            .with_context(|| {
                format!("Failed to write MCP config: {}", mcp_config_path.display())
            })?;
        println!("MCP config written (direct SafeOutputs, no MCPG)");

        Ok(McpSetupResult {
            mcp_config_path,
            mcpg_child: None,
            mcpg_env_file: None,
        })
    }
}

/// Poll the MCPG health endpoint until it responds, detecting early crashes.
async fn wait_for_mcpg_health(mcpg_port: u16, mcpg_log_dir: &Path) -> Result<()> {
    let client = reqwest::Client::new();
    let health_url = format!("http://127.0.0.1:{}/health", mcpg_port);
    for _ in 0..30 {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        if let Ok(resp) = client.get(&health_url).send().await {
            if resp.status().is_success() {
                return Ok(());
            }
        }
    }
    dump_mcpg_logs(mcpg_log_dir);
    bail!("MCPG did not become ready within 30s");
}

/// Poll the MCPG gateway output file until it contains valid JSON with `mcpServers`.
async fn wait_for_gateway_output(gateway_output_path: &Path, mcpg_log_dir: &Path) -> Result<()> {
    println!("Waiting for gateway output...");
    for _ in 0..15 {
        if let Ok(content) = tokio::fs::read_to_string(gateway_output_path).await {
            let has_servers = serde_json::from_str::<serde_json::Value>(&content)
                .ok()
                .and_then(|v| v.get("mcpServers").cloned())
                .is_some();
            if has_servers {
                return Ok(());
            }
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
    let content = tokio::fs::read_to_string(gateway_output_path)
        .await
        .unwrap_or_default();
    dump_mcpg_logs(mcpg_log_dir);
    bail!(
        "MCPG gateway output not ready within 15s. Content: {}",
        if content.is_empty() { "(empty)" } else { &content }
    );
}

/// Run the Copilot CLI agent, or print instructions when Copilot is not on PATH.
async fn run_copilot_agent(
    working_dir: &Path,
    prompt_path: &Path,
    mcp_config_path: &Path,
    copilot_params: &str,
    debug: bool,
    pat: Option<&str>,
) -> Result<()> {
    if is_on_path("copilot") {
        run_copilot_on_path(working_dir, prompt_path, mcp_config_path, copilot_params, debug, pat)
            .await
    } else {
        print_copilot_manual_instructions(prompt_path, mcp_config_path, copilot_params, debug, pat)
            .await
    }
}

/// Execute the Copilot CLI command when it is present on PATH.
async fn run_copilot_on_path(
    working_dir: &Path,
    prompt_path: &Path,
    mcp_config_path: &Path,
    copilot_params: &str,
    debug: bool,
    pat: Option<&str>,
) -> Result<()> {
    let mut cmd = host_command_async("copilot");
    let mut visible_args: Vec<String> = Vec::new();

    // Pass prompt via @file to avoid cmd.exe argument length limits on Windows
    let prompt_file_ref = format!("@{}", prompt_path.display());
    cmd.arg("--prompt").arg(&prompt_file_ref);
    visible_args.push("--prompt".into());
    visible_args.push(prompt_file_ref);

    let mcp_config_ref = format!("@{}", mcp_config_path.display());
    cmd.arg("--additional-mcp-config").arg(&mcp_config_ref);
    visible_args.push("--additional-mcp-config".into());
    visible_args.push(mcp_config_ref);

    for param in shell_words(copilot_params) {
        visible_args.push(param.clone());
        cmd.arg(param);
    }

    if debug {
        cmd.arg("--log-level").arg("debug");
        visible_args.extend(["--log-level".into(), "debug".into()]);
    }

    println!("Running: copilot {}", visible_args.join(" "));
    cmd.current_dir(working_dir);

    if let Some(pat) = pat {
        cmd.env("AZURE_DEVOPS_EXT_PAT", pat);
        cmd.env("SYSTEM_ACCESSTOKEN", pat);
    }

    let status = cmd.status().await.context("Failed to run copilot")?;
    if !status.success() {
        warn!("Copilot exited with status: {}", status);
        println!("Copilot exited with status: {}", status);
    }
    Ok(())
}

/// Print manual invocation instructions when Copilot CLI is absent from PATH.
async fn print_copilot_manual_instructions(
    prompt_path: &Path,
    mcp_config_path: &Path,
    copilot_params: &str,
    debug: bool,
    pat: Option<&str>,
) -> Result<()> {
    let debug_flags = if debug { " --log-level debug" } else { "" };
    println!("Copilot CLI not found on PATH.");
    println!("To run the agent, execute this command:\n");
    println!(
        "  copilot --prompt @{} --additional-mcp-config @{} {}{}\n",
        prompt_path.display(),
        mcp_config_path.display(),
        copilot_params,
        debug_flags,
    );

    if let Some(pat) = pat {
        println!("With environment:");
        println!("  export AZURE_DEVOPS_EXT_PAT=\"{}...\"", &pat[..4.min(pat.len())]);
    }

    println!("\nPress Enter after the agent completes to continue with execution...");
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    Ok(())
}

/// Build the [`ExecutionContext`] for the safe-outputs stage from run arguments.
fn build_execution_context(
    front_matter: &compile::types::FrontMatter,
    output_dir: PathBuf,
    working_dir: PathBuf,
    args: &RunArgs,
) -> crate::safeoutputs::ExecutionContext {
    let mut ctx = crate::safeoutputs::ExecutionContext::default();
    ctx.dry_run = args.dry_run;
    ctx.working_directory = output_dir;
    ctx.source_directory = working_dir;
    ctx.tool_configs = front_matter.safe_outputs.clone();
    ctx.ado_org_url = args.org.clone();
    ctx.ado_project = args.project.clone();
    ctx.access_token = args.pat.clone();

    // Build allowed repositories mapping (checkout aliases → repo names)
    ctx.allowed_repositories = front_matter
        .checkout
        .iter()
        .filter_map(|alias| {
            front_matter
                .repositories
                .iter()
                .find(|r| &r.repository == alias)
                .map(|repo| (alias.clone(), repo.name.clone()))
        })
        .collect();

    ctx
}

pub async fn run(args: &RunArgs) -> Result<()> {
    // ── 1. Parse agent markdown ──────────────────────────────────────
    let content = tokio::fs::read_to_string(&args.agent_path)
        .await
        .with_context(|| format!("Failed to read agent file: {}", args.agent_path.display()))?;

    let (mut front_matter, markdown_body) = compile::parse_markdown(&content)?;
    front_matter.sanitize_config_fields();

    println!("=== ado-aw run: {} ===", front_matter.name);
    println!("Description: {}", front_matter.description);
    println!("Engine: {} (model: {})", front_matter.engine.engine_id(), front_matter.engine.model().unwrap_or("default"));
    if args.dry_run {
        println!("Mode: dry-run (ADO API calls will be skipped in execute stage)");
    }

    // If --org is provided and tools.azure-devops has no explicit org,
    // inject it so AzureDevOpsExtension picks it up during config generation.
    // Sanitize the value since it's injected after sanitize_config_fields().
    if let (Some(org), Some(tools)) = (&args.org, front_matter.tools.as_mut()) {
        if let Some(ado) = tools.azure_devops.as_mut() {
            if ado.org().is_none() {
                ado.set_org(crate::sanitize::sanitize_config(org));
            }
        }
    }

    // ── 2. Collect extensions ────────────────────────────────────────
    // Local run uses PAT auth for ADO MCP (users have PATs, not bearer JWTs).
    let extensions = compile::extensions::collect_extensions_with_auth(
        &front_matter,
        compile::extensions::AdoAuthMode::Pat,
    );

    // ── 3. Create output directory ───────────────────────────────────
    let output_dir = match &args.output_dir {
        Some(dir) => {
            tokio::fs::create_dir_all(dir).await
                .with_context(|| format!("Failed to create output directory: {}", dir.display()))?;
            dir.clone()
        }
        None => {
            let dir = std::env::temp_dir().join(format!("ado-aw-run-{}", std::process::id()));
            tokio::fs::create_dir_all(&dir).await
                .with_context(|| format!("Failed to create output directory: {}", dir.display()))?;
            dir
        }
    };

    // Working directory: use the repository root (where .git lives) so that
    // safe-output tools like create-pull-request operate on the full repo,
    // not just the subdirectory containing the agent file.
    let agent_dir = args
        .agent_path
        .parent()
        .unwrap_or(Path::new("."));
    let working_dir = find_repo_root(agent_dir)
        .unwrap_or_else(|| agent_dir.to_path_buf());
    // Resolve the full path for display. On Windows, std canonicalize()
    // returns UNC-style `\\?\` paths which break many tools. Use
    // dunce-style stripping to get a normal path.
    let output_dir = match output_dir.canonicalize() {
        Ok(p) => strip_unc_prefix(p),
        Err(_) => output_dir,
    };
    println!("Output directory: {}", output_dir.display());
    println!("Working directory: {}", working_dir.display());
    if args.debug {
        println!("Debug log locations:");
        println!("  Copilot logs: ~/.copilot/logs/");
        println!("  SafeOutputs logs: {}", output_dir.join("logs").display());
    }

    // ── 4. Start SafeOutputs HTTP server ─────────────────────────────
    let mut guard = CleanupGuard {
        safeoutputs_child: None,
        mcpg_child: None,
        mcpg_env_file: None,
    };

    println!("\n=== Starting SafeOutputs HTTP server ===");
    let (child, so_port, so_api_key) =
        start_safeoutputs(&output_dir, &working_dir, &[]).await?;
    guard.safeoutputs_child = Some(child);
    println!("SafeOutputs ready on port {}", so_port);

    // ── 5. Generate configs + optionally start MCPG ──────────────────

    // Check if any MCP or ADO tool needs a token
    let needs_ado_token = front_matter
        .tools
        .as_ref()
        .and_then(|t| t.azure_devops.as_ref())
        .is_some_and(|ado| ado.is_enabled())
        || front_matter.mcp_servers.values().any(|config| {
            matches!(config, compile::types::McpConfig::WithOptions(opts)
                if opts.enabled.unwrap_or(true)
                    && opts.container.is_some()
                    && opts.env.contains_key("AZURE_DEVOPS_EXT_PAT"))
        });

    let use_mcpg = !args.skip_mcpg && is_docker_available();
    if !args.skip_mcpg && !use_mcpg {
        warn!("Docker is not available — falling back to --skip-mcpg mode");
        println!("Warning: Docker not available, running without MCPG");
    }

    // Build compile context (resolves engine + ADO context) — needed by both
    // MCPG config generation and copilot params generation.
    let compile_ctx =
        compile::extensions::CompileContext::new(&front_matter, &working_dir).await?;

    let mcp_setup = setup_mcp_config(
        use_mcpg,
        &output_dir,
        so_port,
        &so_api_key,
        &front_matter,
        &compile_ctx,
        &extensions,
        args.pat.as_deref(),
        needs_ado_token,
        args.debug,
    )
    .await?;
    guard.mcpg_child = mcp_setup.mcpg_child;
    guard.mcpg_env_file = mcp_setup.mcpg_env_file;
    let mcp_config_path = mcp_setup.mcp_config_path;

    // ── 6. Write agent prompt ────────────────────────────────────────
    let prompt_path = output_dir.join("agent-prompt.md");
    tokio::fs::write(&prompt_path, &markdown_body).await
        .with_context(|| format!("Failed to write agent prompt: {}", prompt_path.display()))?;
    debug!("Agent prompt written to {}", prompt_path.display());

    // ── 7. Build and run copilot command ─────────────────────────────
    let copilot_params = compile_ctx.engine.args(compile_ctx.front_matter, &extensions)?;

    println!("\n=== Copilot CLI ===");
    run_copilot_agent(
        &working_dir,
        &prompt_path,
        &mcp_config_path,
        &copilot_params,
        args.debug,
        args.pat.as_deref(),
    )
    .await?;

    // ── 8. Execute safe outputs ──────────────────────────────────────
    println!("\n=== Executing safe outputs ===");

    let ctx = build_execution_context(&front_matter, output_dir.clone(), working_dir.clone(), args);
    let results = crate::execute::execute_safe_outputs(&output_dir, &ctx).await?;

    // Print summary
    let success_count = results.iter().filter(|r| r.success && !r.is_warning()).count();
    let warning_count = results.iter().filter(|r| r.is_warning()).count();
    let failure_count = results.iter().filter(|r| !r.success).count();

    println!("\n--- Execution Summary ---");
    println!(
        "Total: {} | Success: {} | Warnings: {} | Failed: {}",
        results.len(),
        success_count,
        warning_count,
        failure_count
    );

    // ── 9. Cleanup (via CleanupGuard drop) ───────────────────────────
    println!("\n=== Cleanup ===");
    drop(guard);
    println!("Done.");

    // process::exit skips async runtime teardown. This is safe because:
    // - CleanupGuard is explicitly dropped above (child processes reaped)
    // - No tokio background tasks are spawned after the guard is set
    // If background tasks are added in the future, they must complete before this point.
    if failure_count > 0 {
        std::process::exit(1);
    }

    Ok(())
}

/// Simple shell-like word splitting for copilot params.
///
/// Handles double-quoted strings (e.g., `--allow-tool "shell(cat)"`).
/// Does NOT handle backslash escapes, single quotes, or nested quotes.
///
/// This is safe because the input is compiler-controlled output from
/// `Engine::args()`, which only produces double-quoted values
/// with no escapes. If params ever gain more complex quoting, consider
/// using the `shell-words` crate.
fn shell_words(s: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;

    for ch in s.chars() {
        match ch {
            '"' => in_quotes = !in_quotes,
            ' ' if !in_quotes => {
                if !current.is_empty() {
                    words.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(ch),
        }
    }
    if !current.is_empty() {
        words.push(current);
    }

    words
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_api_key_is_alphanumeric() {
        let key = generate_api_key();
        assert!(!key.is_empty(), "API key should not be empty");
        assert!(key.len() >= 40, "API key should be at least 40 chars, got {}", key.len());
        assert!(
            key.chars().all(|c| c.is_ascii_alphanumeric()),
            "API key should be alphanumeric, got: {}",
            key
        );
    }

    #[test]
    fn test_find_free_port() {
        let port = find_free_port().unwrap();
        assert!(port > 0, "Port should be positive");
    }

    #[test]
    fn test_shell_words_simple() {
        let words = shell_words("--model claude-opus-4.5 --no-ask-user");
        assert_eq!(words, vec!["--model", "claude-opus-4.5", "--no-ask-user"]);
    }

    #[test]
    fn test_shell_words_quoted() {
        let words = shell_words(r#"--allow-tool "shell(cat)" --allow-tool write"#);
        assert_eq!(words, vec!["--allow-tool", "shell(cat)", "--allow-tool", "write"]);
    }

    #[test]
    fn test_shell_words_empty() {
        let words = shell_words("");
        assert!(words.is_empty());
    }

    #[test]
    fn test_skip_mcpg_direct_config_structure() {
        // Verify the direct SafeOutputs config has the right structure
        let port = 8100u16;
        let api_key = "test-key";
        let config = serde_json::json!({
            "mcpServers": {
                "safeoutputs": {
                    "type": "http",
                    "url": format!("http://127.0.0.1:{}/mcp", port),
                    "headers": {
                        "Authorization": format!("Bearer {}", api_key)
                    },
                    "tools": ["*"]
                }
            }
        });

        let json = serde_json::to_string_pretty(&config).unwrap();
        assert!(json.contains("127.0.0.1:8100"));
        assert!(json.contains("Bearer test-key"));
        assert!(json.contains("\"type\": \"http\""));
    }

    #[test]
    fn test_transform_gateway_output() {
        let gateway_json = serde_json::json!({
            "mcpServers": {
                "safeoutputs": {
                    "type": "http",
                    "url": "http://127.0.0.1:54321/mcp/safeoutputs",
                    "headers": {
                        "Authorization": "Bearer secret-key"
                    }
                },
                "azure-devops": {
                    "type": "http",
                    "url": "http://127.0.0.1:54321/mcp/azure-devops"
                }
            }
        });

        let result = transform_gateway_output(&gateway_json.to_string()).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();

        // Each server should have tools: ["*"]
        let servers = parsed["mcpServers"].as_object().unwrap();
        for (name, entry) in servers {
            assert_eq!(
                entry["tools"],
                serde_json::json!(["*"]),
                "Server '{}' should have tools: [\"*\"]", name,
            );
        }

        // URLs should be preserved as-is (local run, no rewriting needed)
        assert!(result.contains("127.0.0.1:54321"));

        // Headers should be preserved
        assert!(result.contains("Bearer secret-key"));
    }
}
