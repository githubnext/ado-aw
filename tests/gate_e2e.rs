use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_yaml::{Mapping, Value};

fn ado_aw_bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_ado-aw"))
}

fn string_field<'a>(mapping: &'a Mapping, key: &str) -> Option<&'a str> {
    let key = Value::String(key.to_owned());
    mapping.get(&key).and_then(Value::as_str)
}

fn value_field<'a>(mapping: &'a Mapping, key: &str) -> Option<&'a Value> {
    let key = Value::String(key.to_owned());
    mapping.get(&key)
}

fn find_gate_spec(value: &Value) -> Option<String> {
    match value {
        Value::Mapping(mapping) => {
            let script = string_field(mapping, "bash").or_else(|| string_field(mapping, "script"));
            if script.is_some_and(|script| script.contains("node '/tmp/ado-aw-scripts/gate.js'")) {
                let env = value_field(mapping, "env")?.as_mapping()?;
                return string_field(env, "GATE_SPEC").map(str::to_owned);
            }

            mapping.values().find_map(find_gate_spec)
        }
        Value::Sequence(values) => values.iter().find_map(find_gate_spec),
        _ => None,
    }
}

fn run_gate(gate_js: &Path, gate_spec: &str, pr_title: &str) -> Output {
    let path = std::env::var_os("PATH").unwrap_or_default();

    Command::new("node")
        .arg(gate_js)
        .env_clear()
        .env("PATH", path)
        .env("GATE_SPEC", gate_spec)
        .env("ADO_BUILD_REASON", "PullRequest")
        .env("ADO_PR_TITLE", pr_title)
        .env("SYSTEM_ACCESSTOKEN", "dummy")
        .env("ADO_COLLECTION_URI", "https://example.invalid/")
        .env("ADO_PROJECT", "p")
        .env("ADO_BUILD_ID", "1")
        .output()
        .expect("failed to spawn node scripts/gate.js")
}

fn assert_gate_output(gate_js: &Path, gate_spec: &str, pr_title: &str, expected: &str) {
    let output = run_gate(gate_js, gate_spec, pr_title);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "gate.js should exit 0 for PR title {pr_title:?}; stdout:\n{stdout}\nstderr:\n{stderr}"
    );

    let expected_output =
        format!("##vso[task.setvariable variable=SHOULD_RUN;isOutput=true]{expected}");
    assert!(
        stdout.contains(&expected_output),
        "expected stdout to contain {expected_output:?} for PR title {pr_title:?}; stdout:\n{stdout}\nstderr:\n{stderr}"
    );
}

/// Ignored because it requires the bundled gate evaluator to be built first.
/// Run with: `cargo test --test gate_e2e -- --ignored`.
#[test]
#[ignore]
fn gate_js_runs_against_compiled_pipeline() {
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let gate_js = repo_root.join("scripts/gate.js");
    if !gate_js.exists() {
        panic!("gate.js not built; run: cd scripts/ado-script && npm run build");
    }

    match Command::new("node").arg("--version").output() {
        Ok(output) if output.status.success() => {}
        Ok(output) => {
            eprintln!(
                "skipping gate_js_runs_against_compiled_pipeline: node --version failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        Err(err) => {
            eprintln!(
                "skipping gate_js_runs_against_compiled_pipeline: node is not on PATH: {err}"
            );
            return;
        }
    }

    let temp_root = repo_root.join("target/gate-e2e");
    fs::create_dir_all(&temp_root).expect("failed to create gate e2e temp root");
    let temp_dir = tempfile::Builder::new()
        .prefix("gate-js-compiled-pipeline-")
        .tempdir_in(&temp_root)
        .expect("failed to create temp dir under target/gate-e2e");

    let agent_path = temp_dir.path().join("e2e-gate-test.md");
    fs::write(
        &agent_path,
        r#"---
name: e2e-gate-test
description: e2e test
on:
  pr:
    filters:
      title: "foo*"
---
# E2E
run echo hi
"#,
    )
    .expect("failed to write agent markdown");

    let output_path = temp_dir.path().join("e2e-gate-test.yml");
    let compile_output = ado_aw_bin()
        .args(["compile"])
        .arg(&agent_path)
        .args(["-o"])
        .arg(&output_path)
        .output()
        .expect("failed to run ado-aw compile");

    assert!(
        compile_output.status.success(),
        "ado-aw compile failed; stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&compile_output.stdout),
        String::from_utf8_lossy(&compile_output.stderr)
    );

    let compiled = fs::read_to_string(&output_path).expect("failed to read compiled YAML");
    let yaml: Value = serde_yaml::from_str(&compiled).expect("compiled output is not valid YAML");
    let gate_spec = find_gate_spec(&yaml).expect("compiled YAML did not contain prGate GATE_SPEC");

    assert_gate_output(&gate_js, &gate_spec, "fooBar", "true");
    assert_gate_output(&gate_js, &gate_spec, "barBar", "false");
}
