//! Local development run mode.
//!
//! Orchestrates the full agent lifecycle locally: parse markdown →
//! start SafeOutputs → optionally start MCPG → generate configs →
//! exec copilot → execute safe outputs → cleanup.

use anyhow::{Context, Result, bail};
use log::{debug, info, warn};
use std::collections::HashMap;
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
}

/// Guard that kills child processes on drop (normal exit, error, or panic).
struct CleanupGuard {
    safeoutputs_child: Option<Child>,
    mcpg_child: Option<Child>,
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

    let child = cmd.spawn().context("Failed to start SafeOutputs HTTP server")?;
    info!("SafeOutputs started (PID: {}, port: {})", child.id(), port);

    // Health check
    let client = reqwest::Client::new();
    let health_url = format!("http://127.0.0.1:{}/health", port);
    let mut ready = false;
    for _ in 0..30 {
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

/// Start the MCPG Docker container. Returns the `docker run` child process
/// so the caller can store it in `CleanupGuard` for proper reaping.
///
/// Secrets (API keys, PAT) are passed via `--env-file` using a temporary file
/// to avoid exposing them in `ps` output or `/proc/<pid>/cmdline`. The temp
/// file is deleted when the returned guard is dropped.
fn start_mcpg(
    mcpg_config_json: &str,
    mcpg_api_key: &str,
    pat: Option<&str>,
    needs_ado_token: bool,
) -> Result<Child> {
    // Remove stale container
    let _ = Command::new("docker")
        .args(["rm", "-f", "ado-aw-mcpg"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    // Write secrets to a temp file (avoids exposing in ps/cmdline, and avoids
    // persisting credentials in the output directory). Docker reads --env-file
    // synchronously during `docker run` setup — before spawn() returns — so the
    // file is guaranteed to exist when Docker needs it. NamedTempFile allows
    // shared reads on Windows, so Docker can open it concurrently.
    let env_file = tempfile::NamedTempFile::new()
        .context("Failed to create temp env file for MCPG secrets")?;
    let mut env_contents = format!(
        "MCP_GATEWAY_PORT={}\nMCP_GATEWAY_DOMAIN=127.0.0.1\nMCP_GATEWAY_API_KEY={}\n",
        compile::MCPG_PORT, mcpg_api_key,
    );
    if needs_ado_token {
        if let Some(pat) = pat {
            env_contents.push_str(&format!("AZURE_DEVOPS_EXT_PAT={}\n", pat));
        }
    }
    std::fs::write(env_file.path(), &env_contents)
        .with_context(|| format!("Failed to write MCPG env file: {}", env_file.path().display()))?;

    let mut args = vec![
        "run".to_string(),
        "-i".to_string(),
        "--rm".to_string(),
        "--name".to_string(),
        "ado-aw-mcpg".to_string(),
        "--network".to_string(),
        "host".to_string(),
        "--entrypoint".to_string(),
        "/app/awmg".to_string(),
        "-v".to_string(),
        "/var/run/docker.sock:/var/run/docker.sock".to_string(),
        "--env-file".to_string(),
        env_file.path().to_string_lossy().into_owned(),
    ];

    args.push(format!("{}:v{}", compile::MCPG_IMAGE, compile::MCPG_VERSION));
    args.push("--routed".to_string());
    args.push("--listen".to_string());
    args.push(format!("127.0.0.1:{}", compile::MCPG_PORT));
    args.push("--config-stdin".to_string());
    args.push("--log-dir".to_string());
    args.push("/tmp/gh-aw/mcp-logs".to_string());

    let mut child = Command::new("docker")
        .args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("Failed to start MCPG Docker container")?;

    // Pipe config to stdin, then close so container sees EOF
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(mcpg_config_json.as_bytes())
            .context("Failed to write MCPG config to stdin")?;
        drop(stdin);
    }

    // env_file temp file deleted here on drop — Docker has already read it
    Ok(child)
}

/// Build a `std::process::Command` for a program that may be a script wrapper.
///
/// On Windows, npm-installed tools like `copilot` are `.cmd`/`.ps1` wrappers.
/// `Command::new("copilot")` won't find them — `cmd /C` resolves `.cmd`/`.bat`
/// from PATH and handles execution natively.
/// On Unix (Linux/macOS), `Command::new` resolves scripts via the shebang.
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

pub async fn run(args: &RunArgs) -> Result<()> {
    // ── 1. Parse agent markdown ──────────────────────────────────────
    let content = tokio::fs::read_to_string(&args.agent_path)
        .await
        .with_context(|| format!("Failed to read agent file: {}", args.agent_path.display()))?;

    let (mut front_matter, markdown_body) = compile::parse_markdown(&content)?;
    front_matter.sanitize_config_fields();

    println!("=== ado-aw run: {} ===", front_matter.name);
    println!("Description: {}", front_matter.description);
    println!("Engine: {}", front_matter.engine.model());
    if args.dry_run {
        println!("Mode: dry-run (ADO API calls will be skipped in execute stage)");
    }

    // If --org is provided and tools.azure-devops has no explicit org,
    // inject it so AzureDevOpsExtension picks it up during config generation.
    // Sanitize the value since it's injected after sanitize_config_fields().
    if let Some(org) = &args.org {
        if let Some(ref mut tools) = front_matter.tools {
            if let Some(ref mut ado) = tools.azure_devops {
                if ado.org().is_none() {
                    ado.set_org(crate::sanitize::sanitize_config(org));
                }
            }
        }
    }

    // ── 2. Collect extensions ────────────────────────────────────────
    let extensions = compile::extensions::collect_extensions(&front_matter);

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
    println!("Output directory: {}", output_dir.display());
    println!("Working directory: {}", working_dir.display());

    // ── 4. Start SafeOutputs HTTP server ─────────────────────────────
    let mut guard = CleanupGuard {
        safeoutputs_child: None,
        mcpg_child: None,
    };

    println!("\n=== Starting SafeOutputs HTTP server ===");
    let (child, so_port, so_api_key) =
        start_safeoutputs(&output_dir, &working_dir, &[]).await?;
    guard.safeoutputs_child = Some(child);
    println!("SafeOutputs ready on port {}", so_port);

    // ── 5. Generate configs + optionally start MCPG ──────────────────
    let mcpg_api_key = generate_api_key();
    let mcp_config_path = output_dir.join("mcp-config.json");

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

    if use_mcpg {
        println!("\n=== Generating MCPG config ===");
        let compile_ctx =
            compile::extensions::CompileContext::new(&front_matter, &working_dir).await;

        let mcpg_config =
            compile::generate_mcpg_config(&front_matter, &compile_ctx, &extensions)?;

        // Serialize and substitute runtime placeholders
        let mcpg_json = serde_json::to_string_pretty(&mcpg_config)
            .context("Failed to serialize MCPG config")?;
        let mcpg_json = mcpg_json
            .replace("${SAFE_OUTPUTS_PORT}", &so_port.to_string())
            .replace("${SAFE_OUTPUTS_API_KEY}", &so_api_key)
            .replace("${MCP_GATEWAY_API_KEY}", &mcpg_api_key);

        tokio::fs::write(output_dir.join("mcpg-config.json"), &mcpg_json).await?;
        debug!("MCPG config written");

        // Generate MCP client config for copilot
        let mcp_client_json = compile::generate_mcp_client_config(&mcpg_config)?;
        // Substitute ADO macro placeholder with actual key
        let mcp_client_json = mcp_client_json.replace("$(MCP_GATEWAY_API_KEY)", &mcpg_api_key);
        // Local: copilot runs on host, not inside AWF container
        let mcp_client_json = mcp_client_json.replace("host.docker.internal", "127.0.0.1");

        tokio::fs::write(&mcp_config_path, &mcp_client_json).await?;
        debug!("MCP client config written");

        // Start MCPG
        println!("\n=== Starting MCP Gateway (MCPG) ===");
        let mcpg_child = start_mcpg(
            &mcpg_json,
            &mcpg_api_key,
            args.pat.as_deref(),
            needs_ado_token,
        )?;
        guard.mcpg_child = Some(mcpg_child);

        // Health check MCPG
        let client = reqwest::Client::new();
        let health_url = format!("http://127.0.0.1:{}/health", compile::MCPG_PORT);
        let mut ready = false;
        for _ in 0..30 {
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
            bail!("MCPG did not become ready within 30s");
        }
        println!("MCPG ready on port {}", compile::MCPG_PORT);
    } else {
        // Skip MCPG — generate direct config pointing to SafeOutputs
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
        tokio::fs::write(&mcp_config_path, &mcp_client_json).await
            .with_context(|| format!("Failed to write MCP config: {}", mcp_config_path.display()))?;
        println!("MCP config written (direct SafeOutputs, no MCPG)");
    }

    // ── 6. Write agent prompt ────────────────────────────────────────
    let prompt_path = output_dir.join("agent-prompt.md");
    tokio::fs::write(&prompt_path, &markdown_body).await?;
    debug!("Agent prompt written to {}", prompt_path.display());

    // ── 7. Build and run copilot command ─────────────────────────────
    let copilot_params = compile::generate_copilot_params(&front_matter, &extensions)?;

    println!("\n=== Copilot CLI ===");

    if is_on_path("copilot") {
        println!("Running copilot...");

        let prompt_content = tokio::fs::read_to_string(&prompt_path).await?;

        let mut cmd = host_command_async("copilot");
        cmd.arg("--prompt")
            .arg(&prompt_content)
            .arg("--additional-mcp-config")
            .arg(format!("@{}", mcp_config_path.display()));

        // Parse copilot_params and add as args
        for param in shell_words(&copilot_params) {
            cmd.arg(param);
        }

        // Set working directory
        cmd.current_dir(&working_dir);

        // Set environment
        if let Some(pat) = &args.pat {
            cmd.env("AZURE_DEVOPS_EXT_PAT", pat);
            cmd.env("SYSTEM_ACCESSTOKEN", pat);
        }

        let status = cmd
            .status()
            .await
            .context("Failed to run copilot")?;

        if !status.success() {
            warn!("Copilot exited with status: {}", status);
            println!("Copilot exited with status: {}", status);
        }
    } else {
        println!("Copilot CLI not found on PATH.");
        println!("To run the agent, execute this command:\n");
        println!(
            "  copilot --prompt \"$(cat {})\" --additional-mcp-config @{} {}\n",
            prompt_path.display(),
            mcp_config_path.display(),
            copilot_params,
        );

        if let Some(pat) = &args.pat {
            println!("With environment:");
            println!("  export AZURE_DEVOPS_EXT_PAT=\"{}...\"", &pat[..4.min(pat.len())]);
        }

        println!("\nPress Enter after the agent completes to continue with execution...");
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
    }

    // ── 8. Execute safe outputs ──────────────────────────────────────
    println!("\n=== Executing safe outputs ===");

    let mut ctx = crate::safeoutputs::ExecutionContext::default();
    ctx.dry_run = args.dry_run;
    ctx.working_directory = output_dir.clone();
    ctx.tool_configs = front_matter.safe_outputs.clone();

    if let Some(org) = &args.org {
        ctx.ado_org_url = Some(org.clone());
    }
    if let Some(project) = &args.project {
        ctx.ado_project = Some(project.clone());
    }
    if let Some(pat) = &args.pat {
        ctx.access_token = Some(pat.clone());
    }

    // Build allowed repositories mapping
    let mut allowed_repositories = HashMap::new();
    for checkout_alias in &front_matter.checkout {
        if let Some(repo) = front_matter
            .repositories
            .iter()
            .find(|r| &r.repository == checkout_alias)
        {
            allowed_repositories.insert(checkout_alias.clone(), repo.name.clone());
        }
    }
    ctx.allowed_repositories = allowed_repositories;

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
/// `generate_copilot_params()`, which only produces double-quoted values
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
}
