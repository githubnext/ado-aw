mod allowed_hosts;
mod compile;
mod configure;
mod create;
mod detect;
mod execute;
mod fuzzy_schedule;
mod logging;
mod mcp;
mod mcp_firewall;
mod mcp_metadata;
mod ndjson;
mod proxy;
pub mod sanitize;
mod tools;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use log::debug;
use std::path::PathBuf;

use crate::tools::ExecutionContext;

#[derive(Subcommand, Debug)]
enum Commands {
    /// Create a new agent markdown file interactively
    Create {
        /// Output directory for the generated markdown file (defaults to current directory)
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// Compile markdown to pipeline definition (or recompile all detected pipelines)
    Compile {
        /// Path to the input markdown file. If omitted, auto-discovers and
        /// recompiles all existing agentic pipelines in the current directory.
        path: Option<String>,
        /// Optional output path for the generated YAML file
        #[arg(short, long)]
        output: Option<String>,
    },
    /// Check that a compiled pipeline matches its source markdown
    Check {
        /// Path to the pipeline YAML file to verify (source auto-detected from header)
        pipeline: String,
    },
    /// Run as an MCP server
    Mcp {
        // Specify the location where out.json should be placed.
        output_directory: String,
        /// Guard against directory traversal attacks by specifying the agent cannot influence folders outside this path
        bounding_directory: String,
    },
    /// Execute safe outputs from Stage 1 (Stage 2 of the pipeline)
    Execute {
        /// Path to the source markdown file (used to read tool configs from front matter)
        #[arg(short, long)]
        source: PathBuf,
        /// Directory containing safe output NDJSON file
        #[arg(long, default_value = ".")]
        safe_output_dir: PathBuf,
        /// Output directory for processed artifacts (e.g., agent memory)
        #[arg(long)]
        output_dir: Option<PathBuf>,
        /// Azure DevOps organization URL (overrides AZURE_DEVOPS_ORG_URL env var)
        #[arg(long)]
        ado_org_url: Option<String>,
        /// Azure DevOps project name (overrides SYSTEM_TEAMPROJECT env var)
        #[arg(long)]
        ado_project: Option<String>,
    },
    /// Start an HTTP proxy for network filtering
    Proxy {
        /// Allowed hosts (can be specified multiple times, supports wildcards like *.github.com)
        #[arg(long = "allow")]
        allowed_hosts: Vec<String>,
    },
    /// Start an MCP firewall server that proxies and filters tool calls to upstream MCPs
    McpFirewall {
        /// Path to the firewall configuration JSON file
        #[arg(short, long)]
        config: PathBuf,
    },
    /// Detect agentic pipelines and update GITHUB_TOKEN on their ADO definitions
    Configure {
        /// The new GITHUB_TOKEN value (defaults to GITHUB_TOKEN env var; prompted if omitted)
        #[arg(long, env = "GITHUB_TOKEN")]
        token: Option<String>,
        /// Override: Azure DevOps organization URL (inferred from git remote by default)
        #[arg(long)]
        org: Option<String>,
        /// Override: Azure DevOps project name (inferred from git remote by default)
        #[arg(long)]
        project: Option<String>,
        /// PAT for ADO API authentication (prefer setting AZURE_DEVOPS_EXT_PAT env var; prompted if omitted)
        #[arg(long, env = "AZURE_DEVOPS_EXT_PAT")]
        pat: Option<String>,
        /// Path to the repository root (defaults to current directory)
        #[arg(long)]
        path: Option<PathBuf>,
        /// Preview changes without applying them
        #[arg(long)]
        dry_run: bool,
        /// Explicit pipeline definition IDs to update (skips auto-detection)
        #[arg(long, value_delimiter = ',')]
        definition_ids: Option<Vec<u64>>,
    },
}

#[derive(Parser, Debug)]
#[command(version, about = "Compiler for Azure DevOps agentic pipelines")]
struct Args {
    /// Enable verbose logging (info level)
    #[arg(short, long, global = true)]
    verbose: bool,
    /// Enable debug logging (debug level, implies verbose)
    #[arg(short, long, global = true)]
    debug: bool,
    #[command(subcommand)]
    command: Option<Commands>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Determine command name for logging
    let command_name = match &args.command {
        Some(Commands::Create { .. }) => "create",
        Some(Commands::Compile { .. }) => "compile",
        Some(Commands::Check { .. }) => "check",
        Some(Commands::Mcp { .. }) => "mcp",
        Some(Commands::Execute { .. }) => "execute",
        Some(Commands::Proxy { .. }) => "proxy",
        Some(Commands::McpFirewall { .. }) => "mcp-firewall",
        Some(Commands::Configure { .. }) => "configure",
        None => "ado-aw",
    };

    // Initialize file-based logging to $HOME/.ado-aw/logs/{command}.log
    let _log_path = logging::init_logging(command_name, args.debug, args.verbose);

    if let Some(command) = args.command {
        match command {
            Commands::Create { output } => {
                create::create_agent(output).await?;
            }
            Commands::Compile { path, output } => match path {
                Some(p) => compile::compile_pipeline(&p, output.as_deref()).await?,
                None => {
                    if output.is_some() {
                        anyhow::bail!(
                            "--output cannot be used with auto-discovery mode. \
                             Specify a path to compile a single file with a custom output."
                        );
                    }
                    compile::compile_all_pipelines().await?
                }
            },
            Commands::Check { pipeline } => {
                compile::check_pipeline(&pipeline).await?;
            }
            Commands::Mcp {
                output_directory,
                bounding_directory,
            } => mcp::run(&output_directory, &bounding_directory).await?,
            Commands::Execute {
                source,
                safe_output_dir,
                output_dir,
                ado_org_url,
                ado_project,
            } => {
                // Read and parse source markdown to get tool configs
                let content = tokio::fs::read_to_string(&source)
                    .await
                    .with_context(|| format!("Failed to read source file: {}", source.display()))?;

                let (front_matter, _) = compile::parse_markdown(&content).with_context(|| {
                    format!("Failed to parse source file: {}", source.display())
                })?;

                println!("Loaded tool configs from: {}", source.display());

                // Build allowed repositories mapping from checkout + repositories
                let mut allowed_repositories = std::collections::HashMap::new();
                for checkout_alias in &front_matter.checkout {
                    // Find the repository with this alias
                    if let Some(repo) = front_matter
                        .repositories
                        .iter()
                        .find(|r| &r.repository == checkout_alias)
                    {
                        // Map alias to the ADO repo name (e.g., "org/repo-name")
                        allowed_repositories.insert(checkout_alias.clone(), repo.name.clone());
                    }
                }

                // Build execution context from args and environment
                let mut ctx = ExecutionContext::default();
                if let Some(url) = ado_org_url {
                    ctx.ado_org_url = Some(url);
                }
                if let Some(project) = ado_project {
                    ctx.ado_project = Some(project);
                }
                ctx.working_directory = safe_output_dir.clone();
                ctx.tool_configs = front_matter.safe_outputs.clone();
                ctx.allowed_repositories = allowed_repositories;

                let results = execute::execute_safe_outputs(&safe_output_dir, &ctx).await?;

                // Process agent memory if memory config is present
                if let Some(memory_value) = front_matter.safe_outputs.get("memory") {
                    let memory_config: execute::MemoryConfig =
                        serde_json::from_value(memory_value.clone()).unwrap_or_default();
                    let memory_output = output_dir
                        .as_ref()
                        .cloned()
                        .unwrap_or_else(|| safe_output_dir.clone());
                    let memory_result = execute::process_agent_memory(
                        &safe_output_dir,
                        &memory_output,
                        &memory_config,
                    )
                    .await?;
                    println!(
                        "Memory: {} - {}",
                        if memory_result.success { "✓" } else { "✗" },
                        memory_result.message
                    );
                }

                // Print summary
                let success_count = results.iter().filter(|r| r.success).count();
                let failure_count = results.len() - success_count;

                println!("\n--- Execution Summary ---");
                println!(
                    "Total: {} | Success: {} | Failed: {}",
                    results.len(),
                    success_count,
                    failure_count
                );

                if failure_count > 0 {
                    std::process::exit(1);
                }
            }
            Commands::Proxy { allowed_hosts } => {
                // NetworkPolicy::new() includes default hosts plus any user-specified additional hosts
                let policy = proxy::NetworkPolicy::new(allowed_hosts);

                // start_proxy prints the port and flushes stdout before spawning the listener
                let _port = proxy::start_proxy(policy).await?;

                debug!("Proxy started, waiting for termination signal");

                // Keep running until terminated - the shell backgrounds this process
                // and captures the PID for cleanup
                #[cfg(unix)]
                tokio::signal::ctrl_c().await?;

                #[cfg(windows)]
                std::future::pending::<()>().await;
            }
            Commands::McpFirewall { config } => {
                mcp_firewall::run(&config).await?;
            }
            Commands::Configure {
                token,
                org,
                project,
                pat,
                path,
                dry_run,
                definition_ids,
            } => {
                configure::run(
                    token.as_deref(),
                    org.as_deref(),
                    project.as_deref(),
                    pat.as_deref(),
                    path.as_deref(),
                    dry_run,
                    definition_ids.as_deref(),
                )
                .await?;
            }
        }
    } else {
        println!("No subcommand was used. Try `compile <path>`");
    };
    Ok(())
}
