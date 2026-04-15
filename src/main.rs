mod agent_stats;
mod allowed_hosts;
mod compile;
mod configure;
mod detect;
mod ecosystem_domains;
mod execute;
mod fuzzy_schedule;
mod init;
mod logging;
mod mcp;
mod ndjson;
pub mod runtimes;
pub mod sanitize;
mod safeoutputs;
mod tools;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

use crate::safeoutputs::ExecutionContext;

#[derive(Subcommand, Debug)]
enum Commands {
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
        /// Only expose these safe output tools (can be repeated). If omitted, all tools are exposed.
        #[arg(long = "enabled-tools")]
        enabled_tools: Vec<String>,
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
    /// Run SafeOutputs MCP server over HTTP (for MCPG integration)
    McpHttp {
        /// Port to listen on
        #[arg(long, default_value = "8100")]
        port: u16,
        /// API key for authentication (if not provided, one is generated)
        #[arg(long)]
        api_key: Option<String>,
        /// Directory for safe output files
        output_directory: String,
        /// Guard against directory traversal attacks
        bounding_directory: String,
        /// Only expose these safe output tools (can be repeated). If omitted, all tools are exposed.
        #[arg(long = "enabled-tools")]
        enabled_tools: Vec<String>,
    },
    /// Initialize a repository for AI-first agentic pipeline authoring
    Init {
        /// Target directory (defaults to current directory)
        #[arg(long)]
        path: Option<PathBuf>,
        /// Overwrite existing agent file
        #[arg(long)]
        force: bool,
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
        Some(Commands::Compile { .. }) => "compile",
        Some(Commands::Check { .. }) => "check",
        Some(Commands::Mcp { .. }) => "mcp",
        Some(Commands::Execute { .. }) => "execute",
        Some(Commands::McpHttp { .. }) => "mcp-http",
        Some(Commands::Init { .. }) => "init",
        Some(Commands::Configure { .. }) => "configure",
        None => "ado-aw",
    };

    // Initialize file-based logging to $HOME/.ado-aw/logs/{command}.log
    let _log_path = logging::init_logging(command_name, args.debug, args.verbose);

    if let Some(command) = args.command {
        match command {
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
                enabled_tools,
            } => {
                let filter = if enabled_tools.is_empty() { None } else { Some(enabled_tools) };
                mcp::run(&output_directory, &bounding_directory, filter.as_deref()).await?
            }
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

                // Load agent stats from OTel JSONL if available
                let otel_path = safe_output_dir.join(agent_stats::OTEL_FILENAME);
                if otel_path.exists() {
                    match agent_stats::AgentStats::from_otel_file(&otel_path, &front_matter.name)
                        .await
                    {
                        Ok(stats) => {
                            log::info!(
                                "Agent stats: {} input / {} output tokens, {}s duration, {} tool calls, {} turns",
                                stats.input_tokens, stats.output_tokens,
                                stats.duration_seconds as u64, stats.tool_calls, stats.turns
                            );
                            ctx.agent_stats = Some(stats);
                        }
                        Err(e) => {
                            log::warn!("Failed to parse OTel stats file: {}", e);
                        }
                    }
                } else {
                    log::debug!("No OTel stats file found at {}", otel_path.display());
                }

                let results = execute::execute_safe_outputs(&safe_output_dir, &ctx).await?;

                // Process agent memory if cache-memory tool is enabled
                let cache_memory = front_matter
                    .tools
                    .as_ref()
                    .and_then(|t| t.cache_memory.as_ref());
                if let Some(cm) = cache_memory {
                    if cm.is_enabled() {
                        let memory_config = execute::MemoryConfig {
                            allowed_extensions: cm.allowed_extensions().to_vec(),
                        };
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
                }

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

                if failure_count > 0 {
                    std::process::exit(1);
                } else if warning_count > 0 {
                    // Exit code 2 signals "succeeded with issues" — the pipeline
                    // step wraps this to emit ##vso[task.complete result=SucceededWithIssues;]
                    std::process::exit(2);
                }
            }
            Commands::McpHttp {
                port,
                api_key,
                output_directory,
                bounding_directory,
                enabled_tools,
            } => {
                let filter = if enabled_tools.is_empty() { None } else { Some(enabled_tools) };
                mcp::run_http(&output_directory, &bounding_directory, port, api_key.as_deref(), filter.as_deref())
                    .await?;
            }
            Commands::Init { path, force } => {
                init::run(path.as_deref(), force).await?;
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
