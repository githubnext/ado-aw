//! End-to-end integration tests for `ado-aw audit` against a fake ADO server.

use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;

use serde::Deserialize;
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::fs;
use tokio::io::AsyncWriteExt;
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

fn artifact_zip(name: &str, repeated_root: bool, files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (path, contents) in files {
        let path = if repeated_root {
            format!("{name}/{path}")
        } else {
            (*path).to_string()
        };
        writer
            .start_file(path, zip::write::SimpleFileOptions::default())
            .expect("start fixture zip entry");
        writer.write_all(contents).expect("write fixture zip entry");
    }
    writer.finish().expect("finish fixture zip").into_inner()
}

async fn mount_complete_build(server: &MockServer, repeated_root: bool, malformed_aw_info: bool) {
    const BUILD_ID: u64 = 630125;
    let artifact_names = [
        format!("agent_outputs_{BUILD_ID}"),
        format!("analyzed_outputs_{BUILD_ID}"),
        String::from("safe_outputs"),
    ];
    let otel = include_bytes!("fixtures/copilot-otel.jsonl");
    let aw_info: &[u8] = if malformed_aw_info {
        b"{\"schema\":\"ado-aw/aw_info/999\",\"token\":\"do-not-leak\""
    } else {
        br#"{"schema":"ado-aw/aw_info/1","engine":"copilot","model":"claude-sonnet-4.5","threat_detection_enabled":true,"detection_engine":"copilot","detection_model":"gpt-5-mini","agent_name":"audit-fixture","target":"standalone","source":"agent.md","compiler_version":"0.48.0"}"#
    };
    let agent_files: &[(&str, &[u8])] = &[
        (
            "staging/aw_info.json",
            aw_info,
        ),
        ("staging/otel.jsonl", otel),
        (
            "staging/safe_outputs.ndjson",
            b"{\"name\":\"noop\",\"context\":\"noop-1\",\"reason\":\"already complete\"}\n{\"name\":\"missing-tool\",\"tool\":\"bash\",\"reason\":\"not configured\"}\n{\"name\":\"missing_data\",\"reason\":\"title unavailable\"}\n",
        ),
        (
            "logs/firewall/policy-manifest.json",
            br#"{"version":1,"rules":[{"pattern":"api.github.com","verdict":"allow"},{"pattern":"blocked.example","verdict":"deny"}]}"#,
        ),
        (
            "logs/firewall/audit.jsonl",
            b"{\"timestamp\":\"2026-05-21T12:01:00Z\",\"host\":\"api.github.com\",\"rule\":\"api.github.com\",\"verdict\":\"allowed\"}\n{\"timestamp\":\"2026-05-21T12:01:01Z\",\"host\":\"blocked.example\",\"rule\":\"blocked.example\",\"verdict\":\"denied\"}\nmalformed source line with https://secret.example/?token=do-not-leak\n",
        ),
        (
            "logs/mcpg/gateway.jsonl",
            b"{\"ts\":\"2026-05-21T12:02:00Z\",\"server\":\"github\",\"event\":\"server_start\"}\n{\"ts\":\"2026-05-21T12:02:01Z\",\"server\":\"github\",\"tool\":\"search_code\",\"event\":\"tool_call\",\"input_size\":10,\"output_size\":20}\n{\"ts\":\"2026-05-21T12:02:02Z\",\"server\":\"github\",\"tool\":\"create_issue\",\"event\":\"tool_error\",\"error\":\"sanitized failure\"}\n",
        ),
    ];
    let detection_files: &[(&str, &[u8])] = &[(
        "threat-analysis.json",
        br#"{"prompt_injection":true,"secret_leak":false,"malicious_patch":false,"reasons":["synthetic prompt injection"]}"#,
    )];
    let safe_output_files: &[(&str, &[u8])] = &[(
        "executed-safe-outputs.ndjson",
        b"{\"name\":\"noop\",\"status\":\"succeeded\",\"context\":\"noop-1\",\"result\":{\"status\":\"ok\"}}\n",
    )];

    let artifacts = artifact_names
        .iter()
        .enumerate()
        .map(|(index, name)| {
            json!({
                "id": index + 1,
                "name": name,
                "source": BUILD_ID.to_string(),
                "resource": {
                    "type": "PipelineArtifact",
                    "downloadUrl": format!("{}/download/{}", server.uri(), name)
                }
            })
        })
        .collect::<Vec<_>>();

    Mock::given(method("GET"))
        .and(path(format!("/test-project/_apis/build/builds/{BUILD_ID}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": BUILD_ID,
            "status": "completed",
            "result": "succeeded",
            "definition": { "name": "audit-discovery-630125" },
            "sourceBranch": "refs/heads/main",
            "sourceVersion": "deadbeef1234",
            "queueTime": "2026-05-21T12:00:00Z",
            "startTime": "2026-05-21T12:00:30Z",
            "finishTime": "2026-05-21T12:05:30Z"
        })))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/test-project/_apis/build/builds/{BUILD_ID}/artifacts"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "value": artifacts })))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/test-project/_apis/build/builds/{BUILD_ID}/timeline"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "records": [
                {"id":"agent","type":"Job","name":"Agent","state":"completed","result":"succeeded","startTime":"2026-05-21T12:00:30Z","finishTime":"2026-05-21T12:03:00Z"},
                {"id":"detection","type":"Job","name":"Detection","state":"completed","result":"succeeded","startTime":"2026-05-21T12:03:01Z","finishTime":"2026-05-21T12:04:00Z"},
                {"id":"safe","type":"Job","name":"SafeOutputs","state":"completed","result":"succeeded","startTime":"2026-05-21T12:04:01Z","finishTime":"2026-05-21T12:05:00Z"}
            ]
        })))
        .mount(server)
        .await;

    for (name, files) in [
        (&artifact_names[0], agent_files),
        (&artifact_names[1], detection_files),
        (&artifact_names[2], safe_output_files),
    ] {
        Mock::given(method("GET"))
            .and(path(format!("/download/{name}")))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(artifact_zip(
                name,
                repeated_root,
                files,
            )))
            .mount(server)
            .await;
    }
}

async fn run_audit_json(
    workspace: &Path,
    output_dir: &Path,
    server: &MockServer,
    extra_args: &[&str],
) -> Value {
    let mut command = Command::new(binary());
    command
        .current_dir(workspace)
        .env("CI", "1")
        .env("ADO_AW_TEST_ORG_URL", server.uri())
        .args(["audit", "630125", "--json", "--output"])
        .arg(output_dir)
        .args([
            "--org",
            "test-org",
            "--project",
            "test-project",
            "--pat",
            "test-pat",
        ])
        .args(extra_args);
    let output = command.output().await.expect("run JSON audit");
    assert!(
        output.status.success(),
        "audit should succeed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("audit stdout should be JSON")
}

fn normalize_server_url(value: &mut Value) {
    value["overview"]["url"] = Value::Null;
    value["overview"]["logs_path"] = Value::Null;
}

async fn run_mcp_author(workspace: &Path, cache_root: &Path, server: &MockServer) -> Vec<Value> {
    let mut child = Command::new(binary())
        .arg("mcp-author")
        .current_dir(workspace)
        .env("CI", "1")
        .env("TMPDIR", cache_root)
        .env("ADO_AW_TEST_ORG_URL", server.uri())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn mcp-author");
    let requests = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"audit-it","version":"1"}}}),
        json!({"jsonrpc":"2.0","method":"notifications/initialized","params":{}}),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"audit_build","arguments":{"build_id_or_url":"630125","org":"test-org","project":"test-project","pat":"test-pat","no_cache":true}}}),
        json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"trace_failure","arguments":{"build_id_or_url":"630125","org":"test-org","project":"test-project","pat":"test-pat"}}}),
    ];
    let mut stdin = child.stdin.take().expect("mcp stdin");
    for request in requests {
        stdin
            .write_all(format!("{request}\n").as_bytes())
            .await
            .expect("write MCP request");
    }
    drop(stdin);
    let output = child.wait_with_output().await.expect("wait for mcp-author");
    assert!(
        output.status.success(),
        "mcp-author should succeed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("MCP stdout is UTF-8")
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
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

#[tokio::test]
async fn audit_pipeline_artifact_layouts_are_equivalent_end_to_end() {
    let flat_server = MockServer::start().await;
    let repeated_server = MockServer::start().await;
    mount_complete_build(&flat_server, false, false).await;
    mount_complete_build(&repeated_server, true, false).await;

    let workspace = TempDir::new().expect("create workspace");
    fs::write(
        workspace.path().join("agent.md"),
        "---\nname: audit fixture\ndescription: audit fixture\nsafe-outputs:\n  noop:\n---\n\nAudit fixture.\n",
    )
    .await
    .expect("write workflow source");
    let flat_output = TempDir::new().expect("create flat output");
    let repeated_output = TempDir::new().expect("create repeated output");

    let mut flat = run_audit_json(workspace.path(), flat_output.path(), &flat_server, &[]).await;
    let mut repeated = run_audit_json(
        workspace.path(),
        repeated_output.path(),
        &repeated_server,
        &[],
    )
    .await;
    normalize_server_url(&mut flat);
    normalize_server_url(&mut repeated);
    assert_eq!(
        flat, repeated,
        "flat and repeated artifact-name roots must produce identical AuditData"
    );

    for section in [
        "firewall_analysis",
        "policy_analysis",
        "mcp_server_health",
        "mcp_tool_usage",
        "engine_config",
        "detection_analysis",
        "safe_output_summary",
        "safe_output_execution",
        "pipeline_graph",
    ] {
        assert!(
            !flat[section].is_null(),
            "expected analyzer section '{section}' to be populated"
        );
    }
    assert!(flat["metrics"]["token_usage"].as_u64().unwrap_or(0) > 0);
    assert!(
        !flat["mcp_failures"]
            .as_array()
            .expect("MCP failures")
            .is_empty()
    );
    assert!(
        !flat["missing_tools"]
            .as_array()
            .expect("missing tools")
            .is_empty()
    );
    assert!(
        !flat["missing_data"]
            .as_array()
            .expect("missing data")
            .is_empty()
    );
    assert!(!flat["noops"].as_array().expect("noops").is_empty());
    assert_eq!(flat["jobs"].as_array().expect("jobs").len(), 3);

    let rendered = serde_json::to_string(&flat).expect("serialize normalized audit");
    for secret in [
        "do-not-leak",
        "secret.example",
        "test-pat",
        "malformed source line",
    ] {
        assert!(
            !rendered.contains(secret),
            "audit output leaked fixture secret marker '{secret}'"
        );
    }

    let cached = run_audit_json(workspace.path(), flat_output.path(), &flat_server, &[]).await;
    let refreshed = run_audit_json(
        workspace.path(),
        flat_output.path(),
        &flat_server,
        &["--no-cache"],
    )
    .await;
    assert_eq!(cached, refreshed, "cached and --no-cache audits must agree");

    for (filter, included, excluded) in [
        (
            "agent",
            vec!["firewall_analysis", "mcp_tool_usage", "engine_config"],
            vec!["detection_analysis", "safe_output_execution"],
        ),
        (
            "detection",
            vec!["detection_analysis"],
            vec![
                "firewall_analysis",
                "engine_config",
                "safe_output_execution",
            ],
        ),
        (
            "safe-outputs",
            vec![],
            vec!["firewall_analysis", "engine_config", "detection_analysis"],
        ),
    ] {
        let output = TempDir::new().expect("create filtered output");
        let audit = run_audit_json(
            workspace.path(),
            output.path(),
            &flat_server,
            &["--artifacts", filter],
        )
        .await;
        for section in included {
            assert!(
                !audit[section].is_null(),
                "{filter} audit should populate {section}"
            );
        }
        for section in excluded {
            assert!(
                audit[section].is_null(),
                "{filter} audit must not populate excluded section {section}"
            );
        }
        let files = audit["downloaded_files"]
            .as_array()
            .expect("downloaded files");
        assert!(
            files.iter().all(
                |file| file["path"].as_str().is_some_and(|path| match filter {
                    "agent" => path.starts_with("agent_outputs_"),
                    "detection" => path.starts_with("analyzed_outputs_"),
                    "safe-outputs" => path.starts_with("safe_outputs/"),
                    _ => false,
                })
            ),
            "{filter} audit included an excluded artifact family: {files:?}"
        );
    }
    let filter_cache = TempDir::new().expect("create filter cache");
    let _ = run_audit_json(workspace.path(), filter_cache.path(), &flat_server, &[]).await;
    let filtered_after_full = run_audit_json(
        workspace.path(),
        filter_cache.path(),
        &flat_server,
        &["--artifacts", "agent"],
    )
    .await;
    assert!(
        filtered_after_full["detection_analysis"].is_null(),
        "an artifact filter must not reuse an incompatible full-audit cache"
    );
    let full_after_filtered =
        run_audit_json(workspace.path(), filter_cache.path(), &flat_server, &[]).await;
    assert!(
        !full_after_filtered["detection_analysis"].is_null(),
        "a filtered audit must not poison a later unfiltered cache"
    );

    let console = run_audit(
        workspace.path(),
        flat_output.path(),
        "630125",
        Some(&flat_server),
    )
    .await;
    assert!(console.status.success(), "console audit should succeed");
    let console = String::from_utf8_lossy(&console.stdout);
    for heading in [
        "## Overview",
        "## Safe Output Summary",
        "## MCP Server Health",
        "## Firewall Analysis",
        "## Detection Analysis",
        "## MCP Failures",
        "## MCP Tool Usage",
    ] {
        assert!(
            console.contains(heading),
            "console audit omitted '{heading}'"
        );
    }

    let trace_cache = TempDir::new().expect("create trace cache");
    let trace = Command::new(binary())
        .current_dir(workspace.path())
        .env("CI", "1")
        .env("TMPDIR", trace_cache.path())
        .env("ADO_AW_TEST_ORG_URL", flat_server.uri())
        .args([
            "trace",
            "630125",
            "--json",
            "--org",
            "test-org",
            "--project",
            "test-project",
            "--pat",
            "test-pat",
        ])
        .output()
        .await
        .expect("run trace");
    assert!(
        trace.status.success(),
        "trace should succeed: stdout={} stderr={}",
        String::from_utf8_lossy(&trace.stdout),
        String::from_utf8_lossy(&trace.stderr)
    );
    let trace: Value = serde_json::from_slice(&trace.stdout).expect("trace JSON");
    assert_eq!(trace["build_id"], 630125);

    let mcp_cache = TempDir::new().expect("create MCP cache");
    let responses = run_mcp_author(workspace.path(), mcp_cache.path(), &flat_server).await;
    let audit_build = responses
        .iter()
        .find(|response| response["id"] == 2)
        .expect("audit_build MCP response");
    let trace_failure = responses
        .iter()
        .find(|response| response["id"] == 3)
        .expect("trace_failure MCP response");
    assert_eq!(
        audit_build["result"]["structuredContent"]["overview"]["build_id"],
        630125
    );
    assert_eq!(
        trace_failure["result"]["structuredContent"]["build_id"],
        trace["build_id"]
    );

    let malformed_server = MockServer::start().await;
    mount_complete_build(&malformed_server, true, true).await;
    let malformed_output = TempDir::new().expect("create malformed output");
    let malformed = run_audit_json(
        workspace.path(),
        malformed_output.path(),
        &malformed_server,
        &[],
    )
    .await;
    assert!(malformed["engine_config"].is_null());
    assert!(malformed["pipeline_graph"].is_null());
    assert!(malformed["metrics"]["token_usage"].as_u64().unwrap_or(0) > 0);
    assert!(!malformed["firewall_analysis"].is_null());
    assert!(!malformed["detection_analysis"].is_null());
    assert!(
        !malformed["safe_output_summary"].is_null(),
        "safe-output evidence should survive malformed aw_info: {malformed}"
    );
    let warnings = malformed["warnings"].as_array().expect("warnings");
    assert!(
        warnings
            .iter()
            .any(|warning| warning["source"] == "audit::aw_info"),
        "malformed optional metadata should emit one bounded warning: {warnings:?}"
    );
    let rendered = serde_json::to_string(&malformed).expect("serialize malformed audit");
    assert!(!rendered.contains("do-not-leak"));
}
