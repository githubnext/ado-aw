//! Author-facing MCP server for local IDE integrations.
//!
//! This server exposes read-only compiler inspection, graph, lint, what-if,
//! trace, and audit queries over stdio. It intentionally has no workspace
//! bounding directory: callers run it locally as the invoking user.

use std::path::PathBuf;

use anyhow::Result;
use log::{error, info};
use rmcp::{
    ErrorData as McpError, ServerHandler, ServiceExt, handler::server::tool::ToolRouter,
    handler::server::wrapper::Parameters, model::*, tool, tool_handler, tool_router,
    transport::stdio,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::inspect::{self, GraphDepsDirection, GraphFormat};

#[cfg(test)]
mod tests;

/// AuthorMcp is safe to clone for concurrent use: it only contains the
/// immutable rmcp tool router.
#[derive(Clone, Debug)]
pub struct AuthorMcp {
    #[allow(dead_code)]
    tool_router: ToolRouter<Self>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct SourcePathParams {
    /// Path to the source markdown workflow file to inspect.
    source_path: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct GraphDumpParams {
    /// Path to the source markdown workflow file.
    source_path: String,
    /// Render format: "text" (default) or "dot".
    format: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct StepDependenciesParams {
    /// Path to the source markdown workflow file.
    source_path: String,
    /// Step id, or job id fallback, to traverse from.
    step_id: String,
    /// Traversal direction: "upstream" or "downstream".
    direction: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct StepOutputsParams {
    /// Path to the source markdown workflow file.
    source_path: String,
    /// Optional producer step id filter.
    producer: Option<String>,
    /// Optional consumer step id filter.
    consumer: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct TraceFailureParams {
    /// Build ID, or full Azure DevOps build URL.
    build_id_or_url: String,
    /// Optional typed-IR step id to focus on.
    step: Option<String>,
    /// Azure DevOps organization URL or name override.
    org: Option<String>,
    /// Azure DevOps project name override.
    project: Option<String>,
    /// Azure DevOps PAT override. If omitted, normal ado-aw auth resolution is used.
    pat: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct WhatIfParams {
    /// Path to the source markdown workflow file.
    source_path: String,
    /// Step id or job id to treat as failing.
    failing_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct CatalogParams {
    /// Optional category: safe-outputs, runtimes, tools, engines, or models.
    kind: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct AuditBuildParams {
    /// Build ID, or full Azure DevOps build URL.
    build_id_or_url: String,
    /// Azure DevOps organization URL or name override.
    org: Option<String>,
    /// Azure DevOps project name override.
    project: Option<String>,
    /// Azure DevOps PAT override. If omitted, normal ado-aw auth resolution is used.
    pat: Option<String>,
    /// Artifact sets to download. Valid values: agent, detection, safe-outputs.
    artifacts: Option<Vec<String>>,
    /// Force re-processing even if a cached run-summary.json exists.
    no_cache: Option<bool>,
}

#[derive(Debug, Serialize)]
struct GraphDumpResult {
    text_or_dot: String,
}

#[tool_router]
impl AuthorMcp {
    pub fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        name = "inspect_workflow",
        description = "Build and return the public PipelineSummary for a markdown workflow."
    )]
    async fn inspect_workflow(
        &self,
        params: Parameters<SourcePathParams>,
    ) -> Result<CallToolResult, McpError> {
        let source = source_path(&params.0.source_path);
        let summary = inspect::build_inspect(&source)
            .await
            .map_err(to_mcp_error)?;
        structured_result(summary)
    }

    #[tool(
        name = "graph_summary",
        description = "Return the resolved GraphSummary for a markdown workflow."
    )]
    async fn graph_summary(
        &self,
        params: Parameters<SourcePathParams>,
    ) -> Result<CallToolResult, McpError> {
        let source = source_path(&params.0.source_path);
        let graph = inspect::build_graph_summary(&source)
            .await
            .map_err(to_mcp_error)?;
        structured_result(graph)
    }

    #[tool(
        name = "graph_dump",
        description = "Render the resolved workflow graph as text or Graphviz DOT."
    )]
    async fn graph_dump(
        &self,
        params: Parameters<GraphDumpParams>,
    ) -> Result<CallToolResult, McpError> {
        let format = parse_graph_dump_format(params.0.format.as_deref())?;
        let source = source_path(&params.0.source_path);
        let text_or_dot = inspect::build_graph_dump(&source, format)
            .await
            .map_err(to_mcp_error)?;
        structured_result(GraphDumpResult { text_or_dot })
    }

    #[tool(
        name = "step_dependencies",
        description = "Traverse upstream or downstream dependencies for a step id."
    )]
    async fn step_dependencies(
        &self,
        params: Parameters<StepDependenciesParams>,
    ) -> Result<CallToolResult, McpError> {
        let direction = parse_graph_deps_direction(&params.0.direction)?;
        let source = source_path(&params.0.source_path);
        let report = inspect::build_graph_deps(&source, &params.0.step_id, direction)
            .await
            .map_err(to_mcp_error)?;
        structured_result(report)
    }

    #[tool(
        name = "step_outputs",
        description = "Return declared step outputs and their consumers, with optional filters."
    )]
    async fn step_outputs(
        &self,
        params: Parameters<StepOutputsParams>,
    ) -> Result<CallToolResult, McpError> {
        let source = source_path(&params.0.source_path);
        let edges = inspect::build_graph_outputs(
            &source,
            params.0.producer.as_deref(),
            params.0.consumer.as_deref(),
        )
        .await
        .map_err(to_mcp_error)?;
        structured_result(edges)
    }

    #[tool(
        name = "trace_failure",
        description = "Trace a build's failing-job chain using audit data plus the local IR graph."
    )]
    async fn trace_failure(
        &self,
        params: Parameters<TraceFailureParams>,
    ) -> Result<CallToolResult, McpError> {
        let opts = inspect::TraceOptions {
            build_id_or_url: &params.0.build_id_or_url,
            step: params.0.step.as_deref(),
            json: true,
            org: params.0.org.as_deref(),
            project: params.0.project.as_deref(),
            pat: params.0.pat.as_deref(),
        };
        let (_audit, report) = inspect::build_trace(&opts).await.map_err(to_mcp_error)?;
        structured_result(report)
    }

    #[tool(
        name = "whatif",
        description = "Classify downstream jobs that would skip if a step or job failed."
    )]
    async fn whatif(&self, params: Parameters<WhatIfParams>) -> Result<CallToolResult, McpError> {
        let source = source_path(&params.0.source_path);
        let report = inspect::build_whatif(&source, &params.0.failing_id)
            .await
            .map_err(to_mcp_error)?;
        structured_result(report)
    }

    #[tool(
        name = "lint_workflow",
        description = "Run structural lint checks over a markdown workflow."
    )]
    async fn lint_workflow(
        &self,
        params: Parameters<SourcePathParams>,
    ) -> Result<CallToolResult, McpError> {
        let source = source_path(&params.0.source_path);
        let report = inspect::build_lint(&source).await.map_err(to_mcp_error)?;
        structured_result(report)
    }

    #[tool(
        name = "catalog",
        description = "List supported safe-outputs, runtimes, tools, engines, and models."
    )]
    async fn catalog(&self, params: Parameters<CatalogParams>) -> Result<CallToolResult, McpError> {
        let catalog = inspect::build_catalog(params.0.kind.as_deref()).map_err(to_mcp_error)?;
        structured_result(catalog)
    }

    #[tool(
        name = "audit_build",
        description = "Download and analyze a single Azure DevOps build; same JSON shape as `ado-aw audit --json`."
    )]
    async fn audit_build(
        &self,
        params: Parameters<AuditBuildParams>,
    ) -> Result<CallToolResult, McpError> {
        let artifacts = params.0.artifacts.as_deref();
        let output = std::env::temp_dir().join("ado-aw").join("audit");
        let audit = crate::audit::fetch_audit_data(crate::audit::AuditOptions {
            build_id_or_url: &params.0.build_id_or_url,
            output: &output,
            json: true,
            org: params.0.org.as_deref(),
            project: params.0.project.as_deref(),
            pat: params.0.pat.as_deref(),
            artifacts,
            no_cache: params.0.no_cache.unwrap_or(false),
        })
        .await
        .map_err(to_mcp_error)?;
        structured_result(audit)
    }
}

impl Default for AuthorMcp {
    fn default() -> Self {
        Self::new()
    }
}

#[tool_handler]
impl ServerHandler for AuthorMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions("Read-only ado-aw authoring and debugging tools.")
    }
}

pub async fn run_stdio() -> Result<()> {
    info!("Starting author-facing MCP server over stdio");
    let service = AuthorMcp::new().serve(stdio()).await.inspect_err(|e| {
        error!("Error starting author MCP server: {}", e);
    })?;
    service
        .waiting()
        .await
        .map_err(|e| anyhow::anyhow!("Author MCP exited with error: {:?}", e))?;
    Ok(())
}

fn source_path(path: &str) -> PathBuf {
    PathBuf::from(path)
}

fn parse_graph_dump_format(format: Option<&str>) -> Result<GraphFormat, McpError> {
    match format.unwrap_or("text") {
        "text" => Ok(GraphFormat::Text),
        "dot" => Ok(GraphFormat::Dot),
        other => Err(McpError::invalid_params(
            format!("unknown format '{other}' (expected 'text' or 'dot')"),
            None,
        )),
    }
}

fn parse_graph_deps_direction(direction: &str) -> Result<GraphDepsDirection, McpError> {
    match direction {
        "upstream" => Ok(GraphDepsDirection::Upstream),
        "downstream" => Ok(GraphDepsDirection::Downstream),
        other => Err(McpError::invalid_params(
            format!("unknown direction '{other}' (expected 'upstream' or 'downstream')"),
            None,
        )),
    }
}

fn structured_result<T: Serialize>(value: T) -> Result<CallToolResult, McpError> {
    let value = serde_json::to_value(value).map_err(|e| {
        McpError::internal_error(format!("failed to serialize tool result: {e}"), None)
    })?;
    Ok(CallToolResult::structured(value))
}

fn to_mcp_error(error: anyhow::Error) -> McpError {
    McpError::internal_error(format!("{error:#}"), None)
}
