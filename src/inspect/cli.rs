//! CLI dispatchers for the `inspect` family of subcommands.
//!
//! Each `dispatch_*` is the single entry point invoked from
//! `src/main.rs`. Public option structs are by-reference / `Copy`
//! where convenient so call sites stay terse.

use std::path::Path;

use anyhow::{Context, Result};

use crate::audit::model::AuditData;
use crate::compile::{
    build_pipeline_ir,
    ir::summary::{GraphSummary, PipelineSummary},
};

use super::{catalog, graph_deps, graph_outputs, graph_query, lint, trace, whatif};

/// Options for `ado-aw inspect <source>`.
#[derive(Debug)]
pub struct InspectOptions<'a> {
    /// Path to the agent `.md` to inspect.
    pub source: &'a Path,
    /// Emit machine-readable JSON to stdout when `true`; otherwise
    /// render a terse human summary.
    pub json: bool,
}

/// Emit the public [`PipelineSummary`] for an agent source file.
///
/// In text mode prints a compact, scannable summary suitable for
/// terminals (counts + a few cross-cutting facts). In JSON mode
/// writes the full summary to stdout.
pub async fn dispatch_inspect(opts: InspectOptions<'_>) -> Result<()> {
    let summary = build_inspect(opts.source).await?;

    if opts.json {
        let json = serde_json::to_string_pretty(&summary)?;
        println!("{}", json);
    } else {
        print_text_inspect(&summary);
    }
    Ok(())
}

/// Build the public [`PipelineSummary`] for an agent source file.
pub async fn build_inspect(source: &Path) -> Result<PipelineSummary> {
    let (_fm, pipeline) = build_pipeline_ir(source)
        .await
        .with_context(|| format!("Failed to build IR for {}", source.display()))?;
    PipelineSummary::from_pipeline(&pipeline)
}

/// Output format selector for `ado-aw graph`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
#[clap(rename_all = "lower")]
pub enum GraphFormat {
    Text,
    Json,
    Dot,
}

/// Options for `ado-aw graph <source>`.
#[derive(Debug)]
pub struct GraphOptions<'a> {
    pub source: &'a Path,
    pub format: GraphFormat,
}

/// Dump the resolved dependency graph for `source` in the selected
/// format. Delegates the rendering to [`graph_query`].
pub async fn dispatch_graph(opts: GraphOptions<'_>) -> Result<()> {
    let output = build_graph_dump(opts.source, opts.format).await?;
    println!("{}", output);
    Ok(())
}

/// Build the resolved dependency graph summary for an agent source file.
pub async fn build_graph_summary(source: &Path) -> Result<GraphSummary> {
    Ok(build_inspect(source).await?.graph)
}

/// Render the resolved dependency graph for an agent source file.
pub async fn build_graph_dump(source: &Path, format: GraphFormat) -> Result<String> {
    let summary = build_inspect(source).await?;
    match format {
        GraphFormat::Text => Ok(graph_query::render_text(&summary)),
        GraphFormat::Json => serde_json::to_string_pretty(&summary.graph).map_err(Into::into),
        GraphFormat::Dot => Ok(graph_query::render_dot(&summary)),
    }
}

/// Options for `ado-aw graph deps <source> <step-id>`.
#[derive(Debug)]
pub struct GraphDepsOptions<'a> {
    /// Path to the agent markdown source.
    pub source: &'a Path,
    /// Step id to traverse from.
    pub step: &'a str,
    /// Traversal direction.
    pub direction: graph_deps::GraphDepsDirection,
    /// Emit machine-readable JSON instead of text.
    pub json: bool,
}

/// Traverse graph dependencies for one named step.
pub async fn dispatch_graph_deps(opts: GraphDepsOptions<'_>) -> Result<()> {
    let report = build_graph_deps(opts.source, opts.step, opts.direction).await?;

    if opts.json {
        let json = serde_json::to_string_pretty(&report)?;
        println!("{}", json);
    } else {
        println!("{}", graph_deps::render_text(&report));
    }
    Ok(())
}

/// Build a dependency traversal report for one named step.
pub async fn build_graph_deps(
    source: &Path,
    step: &str,
    direction: graph_deps::GraphDepsDirection,
) -> Result<graph_deps::GraphDepsReport> {
    let summary = build_inspect(source).await?;
    graph_deps::analyze(&summary, step, direction)
}

/// Options for `ado-aw graph outputs <source>`.
#[derive(Debug)]
pub struct GraphOutputsOptions<'a> {
    /// Path to the agent markdown source.
    pub source: &'a Path,
    /// Optional producer step id filter.
    pub producer: Option<&'a str>,
    /// Optional consumer step id filter.
    pub consumer: Option<&'a str>,
    /// Emit machine-readable JSON instead of text.
    pub json: bool,
}

/// Print the declared output ↔ consumer reference table.
pub async fn dispatch_graph_outputs(opts: GraphOutputsOptions<'_>) -> Result<()> {
    let edges = build_graph_outputs(opts.source, opts.producer, opts.consumer).await?;

    if opts.json {
        let json = serde_json::to_string_pretty(&edges)?;
        println!("{}", json);
    } else {
        println!("{}", graph_outputs::render_text(&edges));
    }
    Ok(())
}

/// Build the declared-output table, optionally filtered by producer/consumer.
pub async fn build_graph_outputs(
    source: &Path,
    producer: Option<&str>,
    consumer: Option<&str>,
) -> Result<Vec<graph_outputs::OutputEdge>> {
    let summary = build_inspect(source).await?;
    Ok(graph_outputs::output_edges(&summary, producer, consumer))
}

/// Options for `ado-aw trace <build-id-or-url>`.
#[derive(Debug)]
pub struct TraceOptions<'a> {
    pub build_id_or_url: &'a str,
    pub step: Option<&'a str>,
    pub json: bool,
    pub org: Option<&'a str>,
    pub project: Option<&'a str>,
    pub pat: Option<&'a str>,
    /// Cache root for downloaded build artifacts. When `None`,
    /// [`build_trace`] anchors writes under
    /// [`crate::audit::default_cache_root`]
    /// (`${TEMP}/ado-aw/audit`) so CLI invocations, the mcp-author
    /// `trace_failure` tool, and `ado-aw audit` all share a single
    /// cache root — preventing `./logs/` directories from being
    /// scattered under arbitrary IDE working directories.
    pub output: Option<&'a Path>,
}

/// Trace a build by joining audit telemetry with the local typed-IR graph.
pub async fn dispatch_trace(opts: TraceOptions<'_>) -> Result<()> {
    let (audit, report) = build_trace(&opts).await?;

    if audit.pipeline_graph.is_none() {
        eprintln!("warning: source markdown was not available locally; trace is runtime-only");
    }

    if opts.step.is_some() && report.step.is_none() {
        eprintln!("warning: requested step was not found in the local IR graph");
    }

    if opts.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", trace::render_text(&audit, &report, opts.step));
    }
    Ok(())
}

/// Build trace audit data and the derived trace report.
pub async fn build_trace(opts: &TraceOptions<'_>) -> Result<(AuditData, trace::TraceReport)> {
    // Default to the canonical audit cache root shared with every
    // other entry point (CLI `audit`, mcp-author `audit_build` /
    // `trace_failure`). Callers may pass `opts.output = Some(&Path)`
    // to override (e.g. for tests).
    let default_output = crate::audit::default_cache_root();
    let output = opts.output.unwrap_or(default_output.as_path());
    let audit = crate::audit::fetch_audit_data(crate::audit::AuditOptions {
        build_id_or_url: opts.build_id_or_url,
        output,
        json: true,
        org: opts.org,
        project: opts.project,
        pat: opts.pat,
        artifacts: None,
        no_cache: false,
    })
    .await?;
    let report = trace::build_trace_report(&audit, opts.step);
    Ok((audit, report))
}

/// Options for `ado-aw whatif <source> --fail <id>`.
#[derive(Debug)]
pub struct WhatIfOptions<'a> {
    /// Path to the agent markdown source.
    pub source: &'a Path,
    /// Step id or job id that should be treated as failing.
    pub fail: &'a str,
    /// Emit machine-readable JSON instead of text.
    pub json: bool,
}

/// Classify downstream jobs that would skip if a step or job failed.
pub async fn dispatch_whatif(opts: WhatIfOptions<'_>) -> Result<()> {
    let report = build_whatif(opts.source, opts.fail).await?;

    if opts.json {
        let json = serde_json::to_string_pretty(&report)?;
        println!("{}", json);
    } else {
        println!("{}", whatif::render_text(&report));
    }
    Ok(())
}

/// Build a static reachability report for a failing step/job id.
pub async fn build_whatif(source: &Path, fail: &str) -> Result<whatif::WhatIfReport> {
    let summary = build_inspect(source).await?;
    whatif::analyze(&summary, fail)
}

/// Options for `ado-aw lint <source>`.
#[derive(Debug)]
pub struct LintOptions<'a> {
    pub source: &'a Path,
    pub json: bool,
}

/// Run structural lint checks over an agent source file.
///
/// Returns `true` when at least one error-severity finding was emitted so the
/// CLI can translate that into exit code 1 without treating warnings/infos as
/// hard failures.
pub async fn dispatch_lint(opts: LintOptions<'_>) -> Result<bool> {
    let report = build_lint(opts.source).await?;
    let had_errors = report.summary.errors > 0;

    if opts.json {
        let json = serde_json::to_string_pretty(&report)?;
        println!("{}", json);
    } else {
        println!("{}", lint::render_text(&report));
    }

    Ok(had_errors)
}

/// Build the structural lint report for an agent source file.
pub async fn build_lint(source: &Path) -> Result<lint::LintReport> {
    let summary = build_inspect(source).await?;
    Ok(lint::report(&summary))
}

/// Options for `ado-aw catalog`.
#[derive(Debug)]
pub struct CatalogOptions<'a> {
    pub kind: Option<&'a str>,
    pub json: bool,
}

/// Emit the in-tree registry catalog.
pub fn dispatch_catalog(opts: CatalogOptions<'_>) -> Result<()> {
    let catalog = build_catalog(opts.kind)?;

    if opts.json {
        let json = serde_json::to_string_pretty(&catalog)?;
        println!("{}", json);
    } else {
        println!("{}", catalog::render_text(&catalog));
    }
    Ok(())
}

/// Build the in-tree registry catalog, optionally filtered by kind.
pub fn build_catalog(kind: Option<&str>) -> Result<catalog::Catalog> {
    Ok(match kind {
        Some(kind) => catalog::catalog_kind(kind)?,
        None => catalog::catalog(),
    })
}

fn print_text_inspect(s: &PipelineSummary) {
    use crate::compile::ir::summary::PipelineBodySummary;

    println!("Pipeline:        {}", s.name);
    println!("Target shape:    {}", s.shape);
    println!("Schema version:  {}", s.schema_version);
    println!();
    match &s.body {
        PipelineBodySummary::Jobs { jobs } => {
            println!("Jobs ({}):", jobs.len());
            for j in jobs {
                print_job_summary_line(j);
            }
        }
        PipelineBodySummary::Stages { stages } => {
            println!("Stages ({}):", stages.len());
            for st in stages {
                let dep = format_depends(&st.depends_on);
                println!("- {} ({}){}", st.id, st.display_name, dep);
                for j in &st.jobs {
                    print!("  ");
                    print_job_summary_line(j);
                }
            }
        }
    }
    println!();
    println!("Graph:");
    println!(
        "  step locations:           {}",
        s.graph.step_locations.len()
    );
    println!("  derived job edges:        {}", s.graph.job_edges.len());
    println!("  derived stage edges:      {}", s.graph.stage_edges.len());
    let need_io: usize = s
        .graph
        .outputs_needing_is_output
        .iter()
        .map(|e| e.outputs.len())
        .sum();
    println!("  outputs needing isOutput: {}", need_io);
}

fn print_job_summary_line(j: &crate::compile::ir::summary::JobSummary) {
    let dep = format_depends(&j.depends_on);
    let stage = j
        .stage
        .as_deref()
        .map(|s| format!(" [{}]", s))
        .unwrap_or_default();
    let step_count = j.steps.len();
    let id_step_count: usize = j.steps.iter().filter(|s| s.id.is_some()).count();
    println!(
        "- {}{} steps: {} ({} named){}",
        j.id, stage, step_count, id_step_count, dep
    );
}

fn format_depends(deps: &[String]) -> String {
    if deps.is_empty() {
        String::new()
    } else {
        format!("  depends on: {}", deps.join(", "))
    }
}
