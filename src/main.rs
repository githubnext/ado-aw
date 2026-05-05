mod agent_stats;
mod allowed_hosts;
mod compile;
mod configure;
mod detect;
mod ecosystem_domains;
mod engine;
mod execute;
mod fuzzy_schedule;
mod hash;
mod init;
mod logging;
mod mcp;
mod ndjson;
pub mod runtimes;
pub mod sanitize;
mod safeoutputs;
mod tools;
pub mod validate;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Subcommand, Debug)]
enum Commands {
    /// Compile markdown to pipeline definition (or recompile all detected pipelines)
    Compile {
        /// Path to the input markdown file. If omitted, auto-discovers and
        /// recompiles all existing agentic pipelines in the current directory.
        path: Option<String>,
        /// Optional output path for the generated YAML file. If the path
        /// refers to an existing directory, the compiled YAML is written
        /// inside that directory using the default filename derived from
        /// the input markdown (e.g. `foo.md` -> `<dir>/foo.lock.yml`).
        #[arg(short, long)]
        output: Option<String>,
        /// Omit the "Verify pipeline integrity" step from the generated pipeline.
        /// Only available in debug builds.
        #[cfg(debug_assertions)]
        #[arg(long)]
        skip_integrity: bool,
        /// Include MCPG debug diagnostics in the generated pipeline (debug
        /// logging, stderr streaming, backend probe step).
        /// Only available in debug builds.
        #[cfg(debug_assertions)]
        #[arg(long)]
        debug_pipeline: bool,
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
    /// Execute safe outputs from Stage 1 (Stage 3 of the pipeline)
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
        /// Dry run: validate inputs but skip ADO API calls
        #[arg(long)]
        dry_run: bool,
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

async fn run_compile(
    path: Option<String>,
    output: Option<String>,
    skip_integrity: bool,
    debug_pipeline: bool,
) -> Result<()> {
    if skip_integrity {
        eprintln!("Warning: pipeline integrity check step omitted (--skip-integrity)");
    }
    if debug_pipeline {
        eprintln!("Warning: debug diagnostics enabled in generated pipeline (--debug-pipeline)");
    }

    match path {
        Some(p) => compile::compile_pipeline(&p, output.as_deref(), skip_integrity, debug_pipeline).await,
        None => {
            if output.is_some() {
                anyhow::bail!(
                    "--output cannot be used with auto-discovery mode. \
                     Specify a path to compile a single file with a custom output."
                );
            }
            compile::compile_all_pipelines(skip_integrity, debug_pipeline).await
        }
    }
}

async fn run_execute(
    source: PathBuf,
    safe_output_dir: PathBuf,
    output_dir: Option<PathBuf>,
    ado_org_url: Option<String>,
    ado_project: Option<String>,
    dry_run: bool,
) -> Result<()> {
    // Read and parse source markdown to get tool configs
    let content = tokio::fs::read_to_string(&source)
        .await
        .with_context(|| format!("Failed to read source file: {}", source.display()))?;

    let (front_matter, _) = compile::parse_markdown(&content)
        .with_context(|| format!("Failed to parse source file: {}", source.display()))?;

    println!("Loaded tool configs from: {}", source.display());

    // Extract tools config before moving front_matter into build_execution_context
    let tools = front_matter.tools.clone();

    // Build execution context from front matter, CLI args, and environment
    let ctx = build_execution_context(
        front_matter,
        &safe_output_dir,
        ado_org_url,
        ado_project,
        dry_run,
    )
    .await;

    let results = execute::execute_safe_outputs(&safe_output_dir, &ctx).await?;

    // Process agent memory if cache-memory tool is enabled
    process_cache_memory(tools.as_ref(), &safe_output_dir, output_dir).await?;

    print_execution_summary(&results);

    let failure_count = results.iter().filter(|r| !r.success).count();
    let warning_count = results.iter().filter(|r| r.is_warning()).count();
    if failure_count > 0 {
        std::process::exit(1);
    } else if warning_count > 0 {
        // Exit code 2 signals "succeeded with issues" — the pipeline
        // step wraps this to emit ##vso[task.complete result=SucceededWithIssues;]
        std::process::exit(2);
    }
    Ok(())
}

async fn build_execution_context(
    front_matter: compile::FrontMatter,
    safe_output_dir: &PathBuf,
    ado_org_url: Option<String>,
    ado_project: Option<String>,
    dry_run: bool,
) -> crate::safeoutputs::ExecutionContext {
    // Map checkout aliases to ADO repo names from the repositories list
    let allowed_repositories = front_matter
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

    let mut ctx = crate::safeoutputs::ExecutionContext::default();
    ctx.ado_org_url = ado_org_url;
    ctx.ado_project = ado_project;
    ctx.working_directory = safe_output_dir.clone();
    ctx.tool_configs = front_matter.safe_outputs.clone();
    ctx.allowed_repositories = allowed_repositories;
    ctx.dry_run = dry_run;

    // Load agent stats from OTel JSONL if available
    let otel_path = safe_output_dir.join(agent_stats::OTEL_FILENAME);
    if otel_path.exists() {
        match agent_stats::AgentStats::from_otel_file(&otel_path, &front_matter.name).await {
            Ok(stats) => {
                log::info!(
                    "Agent stats: {} input / {} output tokens, {}s duration, {} tool calls, {} turns",
                    stats.input_tokens, stats.output_tokens,
                    stats.duration_seconds as u64, stats.tool_calls, stats.turns
                );
                ctx.agent_stats = Some(stats);
            }
            Err(e) => log::warn!("Failed to parse OTel stats file: {}", e),
        }
    } else {
        log::debug!("No OTel stats file found at {}", otel_path.display());
    }

    ctx
}

async fn process_cache_memory(
    tools: Option<&compile::types::ToolsConfig>,
    safe_output_dir: &PathBuf,
    output_dir: Option<PathBuf>,
) -> Result<()> {
    let Some(cm) = tools.and_then(|t| t.cache_memory.as_ref()) else {
        return Ok(());
    };
    if !cm.is_enabled() {
        return Ok(());
    }
    let memory_config = execute::MemoryConfig {
        allowed_extensions: cm.allowed_extensions().to_vec(),
    };
    let memory_output = output_dir.unwrap_or_else(|| safe_output_dir.clone());
    let result =
        execute::process_agent_memory(safe_output_dir, &memory_output, &memory_config).await?;
    println!(
        "Memory: {} - {}",
        if result.success { "✓" } else { "✗" },
        result.message
    );
    Ok(())
}

fn print_execution_summary(results: &[crate::safeoutputs::ExecutionResult]) {
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

    let Some(command) = args.command else {
        println!("No subcommand was used. Try `compile <path>`");
        return Ok(());
    };

    match command {
        Commands::Compile {
            path,
            output,
            #[cfg(debug_assertions)]
            skip_integrity,
            #[cfg(debug_assertions)]
            debug_pipeline,
        } => {
            #[cfg(not(debug_assertions))]
            let skip_integrity = false;
            #[cfg(not(debug_assertions))]
            let debug_pipeline = false;

            run_compile(path, output, skip_integrity, debug_pipeline).await?;
        }
        Commands::Check { pipeline } => {
            compile::check_pipeline(&pipeline).await?;
        }
        Commands::Mcp {
            output_directory,
            bounding_directory,
            enabled_tools,
        } => {
            let filter = if enabled_tools.is_empty() { None } else { Some(enabled_tools) };
            mcp::run(&output_directory, &bounding_directory, filter.as_deref()).await?;
        }
        Commands::Execute {
            source,
            safe_output_dir,
            output_dir,
            ado_org_url,
            ado_project,
            dry_run,
        } => {
            run_execute(source, safe_output_dir, output_dir, ado_org_url, ado_project, dry_run)
                .await?;
        }
        Commands::McpHttp {
            port,
            api_key,
            output_directory,
            bounding_directory,
            enabled_tools,
        } => {
            let filter = if enabled_tools.is_empty() { None } else { Some(enabled_tools) };
            mcp::run_http(
                &output_directory,
                &bounding_directory,
                port,
                api_key.as_deref(),
                filter.as_deref(),
            )
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
    Ok(())
}
