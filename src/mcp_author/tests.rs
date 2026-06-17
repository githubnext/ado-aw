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
        "validate_steps",
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

#[tokio::test]
async fn source_path_rejects_non_markdown_extension() {
    // Prompt-injected request would otherwise reach build_pipeline_ir
    // and read the file; source_path must refuse before that happens.
    let err = source_path("/etc/passwd")
        .await
        .expect_err("non-md path must be rejected");
    assert!(
        format!("{err}").contains("only `.md`"),
        "expected non-md rejection message, got: {err}"
    );
}

#[tokio::test]
async fn source_path_rejects_parent_traversal() {
    let err = source_path("../../.ssh/authorized_keys.md")
        .await
        .expect_err("parent traversal must be rejected");
    assert!(
        format!("{err}").contains("parent-directory components"),
        "expected traversal rejection message, got: {err}"
    );
}

#[tokio::test]
async fn source_path_rejects_backslash_parent_traversal() {
    // Regression for the linux-side `..\\workflow.md` bypass.
    let err = source_path("..\\..\\authorized_keys.md")
        .await
        .expect_err("backslash-encoded `..` must be rejected");
    assert!(
        format!("{err}").contains("parent-directory components"),
        "expected traversal rejection message, got: {err}"
    );
}

#[tokio::test]
async fn source_path_rejects_tilde_prefix() {
    let err = source_path("~/private.md")
        .await
        .expect_err("tilde prefix must be rejected");
    assert!(
        format!("{err}").contains("parent-directory components"),
        "expected tilde rejection message, got: {err}"
    );
}

#[tokio::test]
async fn source_path_accepts_legitimate_relative_md() {
    source_path("workflows/foo.md")
        .await
        .expect("plain relative .md path must be accepted");
}

#[tokio::test]
async fn graph_dump_json_returns_structured_graph_not_escaped_string() {
    // Regression: previously the json format went through
    // build_graph_dump(Json) which returns a serialized string;
    // wrapping that in GraphDumpResult { text_or_dot: String }
    // forced callers to parse the inner JSON twice. Now the json
    // format short-circuits to the structured GraphSummary.
    let server = AuthorMcp::new();
    let result = server
        .graph_dump(Parameters(GraphDumpParams {
            source_path: fixture_path(),
            format: Some("json".to_string()),
        }))
        .await
        .expect("graph_dump(json) succeeds");

    let graph = result
        .into_typed::<GraphSummary>()
        .expect("graph_dump(json) must return GraphSummary, not GraphDumpResult");
    assert!(!graph.step_locations.is_empty());
}


// ── validate_steps ──────────────────────────────────────────────────────

#[tokio::test]
async fn validate_steps_accepts_valid_bash_in_full_mode() {
    let server = AuthorMcp::new();
    let result = server
        .validate_steps(Parameters(ValidateStepsParams {
            steps: serde_json::json!([{"bash": "echo hi", "displayName": "Greet"}]),
            allow_list: None,
        }))
        .await
        .expect("validate_steps succeeds");

    let value = result
        .structured_content
        .expect("validate_steps returns structured content");
    let ok = value
        .get("ok")
        .and_then(serde_json::Value::as_str)
        .expect("response has ok flag");
    assert_eq!(ok, "true", "expected success response, got: {value:?}");
    let kinds = value
        .get("kinds")
        .and_then(serde_json::Value::as_array)
        .expect("response has kinds array");
    assert_eq!(kinds.len(), 1);
    assert_eq!(kinds[0]["kind"], "bash");
}

#[tokio::test]
async fn validate_steps_curated_mode_rejects_arbitrary_task() {
    let server = AuthorMcp::new();
    let result = server
        .validate_steps(Parameters(ValidateStepsParams {
            steps: serde_json::json!([
                {"task": "AzureCLI@2", "displayName": "shouldnt-pass-curated"}
            ]),
            allow_list: Some("curated".into()),
        }))
        .await
        .expect("validate_steps succeeds");

    let value = result
        .structured_content
        .expect("validate_steps returns structured content");
    let ok = value
        .get("ok")
        .and_then(serde_json::Value::as_str)
        .expect("response has ok flag");
    assert_eq!(
        ok, "false",
        "curated mode must reject AzureCLI@2; got: {value:?}"
    );
    let errors = value
        .get("errors")
        .and_then(serde_json::Value::as_array)
        .expect("response has errors array");
    assert!(errors.iter().any(|e| e["message"]
        .as_str()
        .is_some_and(|m| m.contains("curated allow-list"))));
}

#[tokio::test]
async fn validate_steps_rejects_invalid_allow_list_value() {
    let server = AuthorMcp::new();
    let err = server
        .validate_steps(Parameters(ValidateStepsParams {
            steps: serde_json::json!([{"bash": "echo hi"}]),
            allow_list: Some("permissive".into()),
        }))
        .await
        .expect_err("invalid allow_list must be rejected");
    assert!(
        format!("{err}").contains("full")
            && format!("{err}").contains("curated"),
        "expected error message to mention valid values; got: {err}"
    );
}
