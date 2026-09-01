//! Integration tests for `ado-aw enable`.
//!
//! These tests run the compiled binary in `--dry-run` mode against a
//! fake org/project so no real HTTP traffic is generated. We assert
//! that:
//!
//! - The help text advertises the documented surface.
//! - `--token` without `--also-set-token` is a clap-level error.
//!
//! The decision logic (`decide_action`, `sanitize_ado_display_name`,
//! `build_create_body`) is covered by `#[cfg(test)] mod tests` inside
//! `src/enable.rs`, since wire-stubbing the full ADO REST surface from
//! an integration test would add more infrastructure than it pays off
//! for in Phase 1.

use std::path::PathBuf;

fn binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_ado-aw"))
}

#[test]
fn enable_help_describes_command() {
    let output = std::process::Command::new(binary())
        .args(["enable", "--help"])
        .output()
        .expect("Failed to run ado-aw enable --help");
    assert!(output.status.success(), "--help should exit 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Register an ADO build definition"),
        "Help text should describe the enable command, got:\n{stdout}"
    );
    for flag in [
        "--org",
        "--project",
        "--pat",
        "--folder",
        "--default-branch",
        "--dry-run",
        "--also-set-token",
        "--token",
        "--service-connection",
        "--repository-name",
    ] {
        assert!(
            stdout.contains(flag),
            "Expected --help to advertise {flag}, got:\n{stdout}"
        );
    }
}

#[test]
fn enable_rejects_token_without_also_set_token() {
    // clap should reject this at parse time via `requires = "also_set_token"`.
    let output = std::process::Command::new(binary())
        .args(["enable", "--token", "secret", "--dry-run"])
        .output()
        .expect("Failed to run ado-aw enable");
    assert!(
        !output.status.success(),
        "Expected non-zero exit when --token used without --also-set-token"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--also-set-token") || stderr.contains("also_set_token"),
        "stderr should reference the requires-constraint, got:\n{stderr}"
    );
}

#[test]
fn enable_help_describes_github_source_support() {
    // The --service-connection flag description must begin with "GitHub
    // service-connection …" so operators know what kind of service
    // connection is expected without leaving the terminal.
    let output = std::process::Command::new(binary())
        .args(["enable", "--help"])
        .output()
        .expect("Failed to run ado-aw enable --help");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("GitHub service-connection"),
        "Help text for --service-connection should say 'GitHub service-connection …', got:\n{stdout}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn enable_dry_run_against_subdirectory_uses_repo_root_relative_yaml_path() {
    // Regression: previously `enable PATH` joined `pipeline.source`
    // against the scan root rather than the repo root, producing
    // doubled paths like
    //   C:\repo\tests\fixtures\tests\fixtures\job-agent.md
    // for every fixture, and posted a yamlFilename of
    // `/job-agent.lock.yml` (relative to scan root) instead of the
    // real repo-relative `/tests/fixtures/job-agent.lock.yml`.
    //
    // The scenario is staged in a throwaway git repo rather than run
    // against this checkout: no in-repo directory holds committed
    // `ado-aw` `*.lock.yml` files any more (both smoke lanes and the
    // fixtures recompile at run time), and the ambient git remote of
    // the checkout is not something a test may depend on. Compiling a
    // fixture into `pipelines/` inside the temp repo gives us a
    // subdirectory whose repo-relative path is known exactly.
    //
    // `enable` always calls `list_definitions` (to know which
    // pipelines already exist) even in --dry-run, so we point at a
    // wiremock that returns an empty list. The dry-run path then
    // prints the would-be POST body without ever making a real
    // network call.
    use wiremock::matchers::{method, path_regex};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let repo = tempfile::TempDir::new().expect("create temp repo");
    let repo_path = repo.path();
    std::process::Command::new("git")
        .args(["init", "-q", "."])
        .current_dir(repo_path)
        .status()
        .expect("git init");
    let pipelines_dir = repo_path.join("pipelines");
    std::fs::create_dir(&pipelines_dir).expect("create pipelines dir");
    std::fs::copy(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/minimal-agent.md"),
        pipelines_dir.join("minimal-agent.md"),
    )
    .expect("copy fixture");

    let compile = std::process::Command::new(binary())
        .args(["compile", "pipelines/minimal-agent.md"])
        .current_dir(repo_path)
        .output()
        .expect("Failed to run ado-aw compile");
    assert!(
        compile.status.success(),
        "fixture must compile; stdout=\n{}\nstderr=\n{}",
        String::from_utf8_lossy(&compile.stdout),
        String::from_utf8_lossy(&compile.stderr)
    );

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path_regex(r"/AgentPlayground/_apis/build/definitions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "count": 0,
            "value": []
        })))
        .mount(&server)
        .await;

    let output = std::process::Command::new(binary())
        .args([
            "enable",
            "--service-connection",
            "00000000-0000-0000-0000-000000000000",
            // The temp repo has no git remote, so the GitHub source
            // identity has to be supplied explicitly. This also keeps
            // the test independent of the remote of whatever checkout
            // it runs in.
            "--repository-name",
            "githubnext/ado-aw",
            "--project",
            "AgentPlayground",
            "--org",
            "msazuresphere",
            "--pat",
            "dummy-pat-for-dry-run",
            "--dry-run",
            "pipelines",
        ])
        // Redirect ADO REST calls at the wiremock; explicit dummy
        // PAT keeps `resolve_auth` off the Azure-CLI / interactive-
        // prompt fallback which CI doesn't support.
        .env("ADO_AW_TEST_ORG_URL", server.uri())
        .env_remove("AZURE_DEVOPS_EXT_PAT")
        .current_dir(repo_path)
        .output()
        .expect("Failed to run ado-aw enable");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "expected --dry-run exit 0; stdout=\n{stdout}\nstderr=\n{stderr}"
    );
    assert!(
        stdout.contains("Found ") && stdout.contains(" agentic pipeline(s)."),
        "expected pipeline-discovery line, got:\n{stdout}"
    );
    assert!(
        stdout.contains("\"yamlFilename\": \"/pipelines/minimal-agent.lock.yml\""),
        "yamlFilename must be repo-root-relative, got:\n{stdout}"
    );
    assert!(
        !stdout.contains("Failed to read source"),
        "no pipeline should fail to read; got:\n{stdout}"
    );
}
