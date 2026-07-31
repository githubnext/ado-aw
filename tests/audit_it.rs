//! End-to-end integration tests for `ado-aw audit` against a fake ADO server.

use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::json;
use tempfile::TempDir;
use tokio::fs;
use tokio::process::Command;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_ado-aw"))
}

#[derive(Debug, Deserialize)]
struct CachedRunSummary {
    ado_aw_version: String,
    build_id: u64,
    audit_data: CachedAuditData,
}

#[derive(Debug, Deserialize)]
struct CachedAuditData {
    overview: CachedOverviewData,
    #[serde(default)]
    jobs: Vec<CachedJobData>,
}

#[derive(Debug, Deserialize)]
struct CachedOverviewData {
    build_id: u64,
    pipeline_name: String,
}

#[derive(Debug, Deserialize)]
struct CachedJobData {
    name: String,
}

fn run_summary_path(output_dir: &Path, build_id: u64) -> PathBuf {
    output_dir
        .join(format!("build-{build_id}"))
        .join("run-summary.json")
}

async fn read_run_summary(path: &Path) -> CachedRunSummary {
    let bytes = fs::read(path)
        .await
        .unwrap_or_else(|e| panic!("read run summary {}: {e}", path.display()));
    serde_json::from_slice(&bytes)
        .unwrap_or_else(|e| panic!("parse run summary {}: {e}", path.display()))
}

async fn run_audit(
    workspace: &Path,
    output_dir: &Path,
    build_id_or_url: &str,
    server: Option<&MockServer>,
) -> std::process::Output {
    let mut command = Command::new(binary());
    command.current_dir(workspace).env("CI", "1").args([
        "audit",
        build_id_or_url,
        "--output",
        output_dir
            .to_str()
            .expect("output path should be valid UTF-8"),
        "--org",
        "test-org",
        "--project",
        "test-project",
        "--pat",
        "test-pat",
    ]);

    if let Some(server) = server {
        command.env("ADO_AW_TEST_ORG_URL", server.uri());
    }

    command.output().await.expect("run ado-aw audit")
}

#[tokio::test]
async fn audit_happy_path_against_fake_ado() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/test-project/_apis/build/builds/12345"))
        .and(query_param("api-version", "7.1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": 12345,
            "status": "completed",
            "result": "succeeded",
            "definition": { "name": "mocked-pipeline" },
            "sourceBranch": "refs/heads/main",
            "sourceVersion": "deadbeef1234",
            "queueTime": "2026-05-21T12:00:00Z",
            "startTime": "2026-05-21T12:00:30Z",
            "finishTime": "2026-05-21T12:05:30Z"
        })))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/test-project/_apis/build/builds/12345/artifacts"))
        .and(query_param("api-version", "7.1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "value": []
        })))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/test-project/_apis/build/builds/12345/timeline"))
        .and(query_param("api-version", "7.1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "records": [
                {
                    "id": "job-agent",
                    "type": "Job",
                    "name": "Agent",
                    "state": "completed",
                    "result": "succeeded",
                    "startTime": "2026-05-21T12:00:30Z",
                    "finishTime": "2026-05-21T12:03:00Z"
                },
                {
                    "id": "job-detection",
                    "type": "Job",
                    "name": "Detection",
                    "state": "completed",
                    "result": "succeeded",
                    "startTime": "2026-05-21T12:03:01Z",
                    "finishTime": "2026-05-21T12:04:00Z"
                },
                {
                    "id": "job-safe-outputs",
                    "type": "Job",
                    "name": "SafeOutputs",
                    "state": "completed",
                    "result": "succeeded",
                    "startTime": "2026-05-21T12:04:01Z",
                    "finishTime": "2026-05-21T12:05:00Z"
                }
            ]
        })))
        .mount(&server)
        .await;

    let workspace = TempDir::new().expect("create workspace temp dir");
    let output_dir = TempDir::new().expect("create output temp dir");

    let output = run_audit(workspace.path(), output_dir.path(), "12345", Some(&server)).await;

    assert!(
        output.status.success(),
        "audit should succeed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let summary_path = run_summary_path(output_dir.path(), 12345);
    assert!(
        summary_path.exists(),
        "expected run summary at {}",
        summary_path.display()
    );

    let summary = read_run_summary(&summary_path).await;
    assert_eq!(summary.build_id, 12345);
    assert_eq!(summary.audit_data.overview.build_id, 12345);
    assert_eq!(summary.audit_data.overview.pipeline_name, "mocked-pipeline");
    assert_eq!(summary.audit_data.jobs.len(), 3);
    assert_eq!(
        summary
            .audit_data
            .jobs
            .iter()
            .map(|job| job.name.as_str())
            .collect::<Vec<_>>(),
        vec!["Agent", "Detection", "SafeOutputs"]
    );
}

#[tokio::test]
async fn audit_permission_denied_returns_structured_error() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/test-project/_apis/build/builds/12345"))
        .and(query_param("api-version", "7.1"))
        .respond_with(ResponseTemplate::new(403).set_body_json(json!({
            "message": "TF400813: Permission denied"
        })))
        .mount(&server)
        .await;

    let workspace = TempDir::new().expect("create workspace temp dir");
    let output_dir = TempDir::new().expect("create output temp dir");

    let output = run_audit(workspace.path(), output_dir.path(), "12345", Some(&server)).await;

    assert!(
        !output.status.success(),
        "audit should fail on permission denied: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("TF400813: Permission denied") || stderr.contains("403"),
        "expected permission-denied error details, got:\n{stderr}"
    );

    let summary_path = run_summary_path(output_dir.path(), 12345);
    assert!(
        !summary_path.exists(),
        "run summary should not be created on build metadata failure"
    );
}

#[tokio::test]
async fn audit_uses_cached_run_summary_when_present() {
    let server = MockServer::start().await;
    let workspace = TempDir::new().expect("create workspace temp dir");
    let output_dir = TempDir::new().expect("create output temp dir");
    let summary_path = run_summary_path(output_dir.path(), 12345);

    fs::create_dir_all(
        summary_path
            .parent()
            .expect("run summary should have a parent"),
    )
    .await
    .expect("create cached summary directory");
    fs::write(
        &summary_path,
        serde_json::to_vec_pretty(&json!({
            "ado_aw_version": env!("CARGO_PKG_VERSION"),
            "build_id": 12345,
            "processed_at": "2026-05-21T12:00:00Z",
            "audit_data": {
                "overview": {
                    "build_id": 12345,
                    "pipeline_name": "cached-pipeline"
                },
                "jobs": [
                    { "name": "CachedJob" }
                ]
            }
        }))
        .expect("serialize cached summary"),
    )
    .await
    .expect("write cached summary");

    let output = run_audit(workspace.path(), output_dir.path(), "12345", Some(&server)).await;

    assert!(
        output.status.success(),
        "audit should succeed from cache: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let requests = server
        .received_requests()
        .await
        .expect("wiremock request history should be available");
    assert!(
        requests.is_empty(),
        "cache hit should avoid all HTTP requests, saw {}",
        requests.len()
    );

    let summary = read_run_summary(&summary_path).await;
    assert_eq!(summary.ado_aw_version, env!("CARGO_PKG_VERSION"));
    assert_eq!(summary.audit_data.overview.pipeline_name, "cached-pipeline");
}

#[tokio::test]
async fn cached_audit_correlates_custom_safe_output_at_job_level() {
    let server = MockServer::start().await;
    let workspace = TempDir::new().expect("create workspace temp dir");
    let output_dir = TempDir::new().expect("create output temp dir");
    let summary_path = run_summary_path(output_dir.path(), 12345);
    let run_dir = summary_path.parent().expect("run summary parent");
    let staging = run_dir.join("agent_outputs_12345").join("staging");

    fs::create_dir_all(&staging)
        .await
        .expect("create cached agent artifacts");
    fs::write(
        staging.join("safe_outputs.ndjson"),
        b"{\"name\":\"notify-team\",\"message\":\"hello\"}\n{\"name\":\"missing-tool\"}\n",
    )
    .await
    .expect("write custom proposal");
    fs::write(
        staging.join("custom-tools.json"),
        serde_json::to_vec(&json!([
            {
                "name": "missing-tool",
                "description": "Missing",
                "inputSchema": {"type": "object"},
                "max": 1
            },
            {
                "name": "no-proposal",
                "description": "Unexpected",
                "inputSchema": {"type": "object"},
                "max": 1
            },
            {
                "name": "notify-team",
                "description": "Notify",
                "inputSchema": {"type": "object"},
                "max": 1,
                "output": "Notification proposal accepted."
            }
        ]))
        .expect("serialize custom tools"),
    )
    .await
    .expect("write custom tools");
    fs::write(
        &summary_path,
        serde_json::to_vec_pretty(&json!({
            "ado_aw_version": env!("CARGO_PKG_VERSION"),
            "build_id": 12345,
            "processed_at": "2026-05-21T12:00:00Z",
            "audit_data": {
                "overview": {
                    "build_id": 12345,
                    "pipeline_name": "cached-custom-pipeline",
                    "aw_info": {
                        "custom_components": [{
                            "tool": "notify-team",
                            "source": "org/repo/components/notify",
                            "sha": "0123456789abcdef0123456789abcdef01234567",
                            "manifest_digest": "manifest-digest",
                            "schema_digest": "mismatched-schema-digest"
                        }],
                        "custom_jobs": [
                            {
                                "tool": "missing-tool",
                                "job_id": "Custom_missing_tool",
                                "approval_path": "automatic",
                                "staged_requested": false
                            },
                            {
                                "tool": "no-proposal",
                                "job_id": "Custom_no_proposal",
                                "approval_path": "automatic",
                                "staged_requested": false
                            },
                            {
                                "tool": "notify-team",
                                "job_id": "Custom_notify_team",
                                "approval_path": "automatic",
                                "staged_requested": true
                            }
                        ]
                    }
                },
                "metrics": {},
                "detection_analysis": {
                    "threats": {},
                    "safe_to_process": true
                },
                "jobs": [
                    {
                        "name": "Custom_no_proposal",
                        "status": "completed",
                        "result": "succeeded",
                        "started_at": "2026-05-21T12:03:00Z",
                        "finished_at": "2026-05-21T12:03:01Z"
                    },
                    {
                        "name": "Custom_notify_team",
                        "status": "completed",
                        "result": "succeeded",
                        "started_at": "2026-05-21T12:04:00Z",
                        "finished_at": "2026-05-21T12:04:01Z"
                    }
                ],
                "downloaded_files": []
            }
        }))
        .expect("serialize cached summary"),
    )
    .await
    .expect("write cached summary");

    let output = run_audit(workspace.path(), output_dir.path(), "12345", Some(&server)).await;
    assert!(
        output.status.success(),
        "audit should correlate cached custom job: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("## Custom Safe-Output Jobs"));
    assert!(stdout.contains("proposal_time_acknowledgement: Notification proposal accepted."));

    let value: serde_json::Value = serde_json::from_slice(
        &fs::read(&summary_path)
            .await
            .expect("read refreshed run summary"),
    )
    .expect("parse refreshed run summary");
    let custom_jobs = value["audit_data"]["custom_safe_output_jobs"]
        .as_array()
        .expect("custom job array");
    let custom = custom_jobs
        .iter()
        .find(|entry| entry["tool"] == "notify-team")
        .expect("notify-team audit row");
    assert!(
        value["audit_data"].get("safe_output_execution").is_none(),
        "custom proposals must not synthesize per-item execution rows"
    );
    assert_eq!(custom["tool"], "notify-team");
    assert_eq!(custom["proposed_count"], 1);
    assert_eq!(custom["expected_job_id"], "Custom_notify_team");
    assert_eq!(custom["staged_requested"], true);
    assert_eq!(
        custom["proposal_time_acknowledgement"],
        "Notification proposal accepted."
    );
    assert_eq!(custom["ado_job"]["name"], "Custom_notify_team");
    assert_eq!(custom["ado_job"]["result"], "succeeded");
    assert!(
        custom.get("result").is_none(),
        "custom audit rows must not expose a per-item execution result"
    );
    let finding_titles = value["audit_data"]["key_findings"]
        .as_array()
        .expect("key findings")
        .iter()
        .filter_map(|finding| finding["title"].as_str())
        .collect::<Vec<_>>();
    assert!(
        finding_titles.contains(&"Expected custom job missing for missing-tool"),
        "missing custom jobs should produce a finding: {finding_titles:?}"
    );
    assert!(
        finding_titles.contains(&"Custom job ran without proposals for no-proposal"),
        "impossible custom job states should produce a finding: {finding_titles:?}"
    );
    assert!(
        finding_titles.contains(&"Custom component provenance mismatch for notify-team"),
        "provenance mismatches should produce a finding: {finding_titles:?}"
    );
}
