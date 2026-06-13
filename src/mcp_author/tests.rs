use std::collections::BTreeSet;
use std::path::PathBuf;

use rmcp::handler::server::wrapper::Parameters;

use super::*;
use crate::compile::ir::summary::{GraphSummary, PipelineSummary};
use crate::inspect::lint::LintReport;

fn fixture_path() -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("safe-outputs")
        .join("create-pull-request.md")
        .display()
        .to_string()
}

#[test]
fn list_tools_contains_expected_author_surface() {
    let server = AuthorMcp::new();
    let names: BTreeSet<String> = server
        .tool_router
        .list_all()
        .iter()
        .map(|tool| tool.name.to_string())
        .collect();

    for expected in [
        "inspect_workflow",
        "graph_summary",
        "graph_dump",
        "step_dependencies",
        "step_outputs",
        "trace_failure",
        "whatif",
        "lint_workflow",
        "catalog",
        "audit_build",
    ] {
        assert!(names.contains(expected), "missing MCP tool {expected}");
    }
}

#[tokio::test]
async fn inspect_workflow_returns_pipeline_summary_schema_version_one() {
    let server = AuthorMcp::new();
    let result = server
        .inspect_workflow(Parameters(SourcePathParams {
            source_path: fixture_path(),
        }))
        .await
        .expect("inspect_workflow succeeds");

    let summary = result
        .into_typed::<PipelineSummary>()
        .expect("inspect_workflow returns PipelineSummary");
    assert_eq!(summary.schema_version, 1);
}

#[tokio::test]
async fn graph_summary_and_lint_workflow_smoke_fixture() {
    let server = AuthorMcp::new();
    let source_path = fixture_path();

    let graph = server
        .graph_summary(Parameters(SourcePathParams {
            source_path: source_path.clone(),
        }))
        .await
        .expect("graph_summary succeeds")
        .into_typed::<GraphSummary>()
        .expect("graph_summary returns GraphSummary");
    assert!(!graph.step_locations.is_empty());

    let lint = server
        .lint_workflow(Parameters(SourcePathParams { source_path }))
        .await
        .expect("lint_workflow succeeds")
        .into_typed::<LintReport>()
        .expect("lint_workflow returns LintReport");
    assert_eq!(lint.summary.errors, 0);
}
