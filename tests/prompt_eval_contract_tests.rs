use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use serde_json::Value;

fn repo_path(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel)
}

fn read_json(path: &Path) -> Value {
    let content =
        fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_json::from_str(&content).unwrap_or_else(|e| panic!("parse JSON {}: {e}", path.display()))
}

fn string_array<'a>(value: &'a Value, field: &str, source: &Path) -> Vec<&'a str> {
    value[field]
        .as_array()
        .unwrap_or_else(|| panic!("{} field {field} must be an array", source.display()))
        .iter()
        .map(|entry| {
            entry.as_str().unwrap_or_else(|| {
                panic!("{} field {field} must contain strings", source.display())
            })
        })
        .collect()
}

fn safe_relative(root: &Path, relative: &str, label: &str) -> PathBuf {
    let relative_path = Path::new(relative);
    assert!(
        !relative_path.is_absolute(),
        "{label} must be relative: {relative}"
    );
    assert!(
        !relative_path
            .components()
            .any(|component| matches!(component, Component::ParentDir)),
        "{label} must not contain '..': {relative}"
    );
    let joined = root.join(relative_path);
    assert!(
        joined.exists(),
        "{label} does not exist: {}",
        joined.display()
    );
    joined
}

#[test]
fn prompt_eval_manifest_has_three_cases_per_prompt() {
    let root = repo_path("tests/prompt-evals");
    let manifest = read_json(&root.join("manifest.json"));
    assert_eq!(manifest["schema_version"], 1);
    assert!(
        manifest["fixture_set_version"]
            .as_u64()
            .is_some_and(|version| version > 0)
    );

    let case_paths = string_array(&manifest, "cases", &root.join("manifest.json"));
    assert_eq!(case_paths.len(), 9, "MVP corpus must contain nine cases");

    let mut ids = HashSet::new();
    let mut counts: HashMap<String, usize> = HashMap::new();
    for relative_case in case_paths {
        let case_path = safe_relative(&root, relative_case, "case path");
        let case = read_json(&case_path);
        assert_eq!(case["schema_version"], 1, "{}", case_path.display());
        let id = case["id"]
            .as_str()
            .unwrap_or_else(|| panic!("{} missing id", case_path.display()));
        assert!(ids.insert(id.to_string()), "duplicate case id {id}");
        let prompt = case["prompt"]
            .as_str()
            .unwrap_or_else(|| panic!("{} missing prompt", case_path.display()));
        assert!(
            matches!(prompt, "create" | "update" | "debug"),
            "{} has invalid prompt {prompt}",
            case_path.display()
        );
        *counts.entry(prompt.to_string()).or_default() += 1;
    }

    assert_eq!(counts.get("create"), Some(&3));
    assert_eq!(counts.get("update"), Some(&3));
    assert_eq!(counts.get("debug"), Some(&3));
}

#[test]
fn prompt_eval_cases_reference_safe_complete_inputs_and_rubrics() {
    let root = repo_path("tests/prompt-evals");
    let manifest_path = root.join("manifest.json");
    let manifest = read_json(&manifest_path);
    let required_common = [
        "task_completion",
        "grounding",
        "safety_and_consent",
        "clarity_and_done_criteria",
    ];

    for relative_case in string_array(&manifest, "cases", &manifest_path) {
        let case_path = safe_relative(&root, relative_case, "case path");
        let case_dir = case_path.parent().expect("case path has parent");
        let case = read_json(&case_path);
        let id = case["id"].as_str().expect("case id");

        let request_file = case["request_file"].as_str().expect("request_file");
        let request_path = safe_relative(case_dir, request_file, "request_file");
        let request = fs::read_to_string(&request_path).expect("request readable");
        assert!(!request.trim().is_empty(), "{id} request must not be empty");

        for context in string_array(&case, "context_files", &case_path) {
            let context_path = safe_relative(case_dir, context, "context file");
            assert!(
                fs::metadata(&context_path).expect("context metadata").len() > 0,
                "{id} context {} must not be empty",
                context_path.display()
            );
        }

        let rubric_paths = string_array(&case, "rubric_files", &case_path);
        assert!(
            rubric_paths.len() >= 2,
            "{id} must reference common and prompt-specific rubrics"
        );
        let mut criterion_ids = HashSet::new();
        for rubric in rubric_paths {
            let rubric_path = safe_relative(&root, rubric, "rubric file");
            let rubric = read_json(&rubric_path);
            assert_eq!(rubric["schema_version"], 1);
            for criterion in rubric["criteria"].as_array().expect("criteria array") {
                let criterion_id = criterion["id"].as_str().expect("criterion id");
                assert!(
                    criterion_ids.insert(criterion_id.to_string()),
                    "{id} repeats criterion {criterion_id}"
                );
                assert!(
                    criterion["weight"]
                        .as_f64()
                        .is_some_and(|weight| weight > 0.0),
                    "{id}.{criterion_id} must have positive weight"
                );
                for field in ["question", "score_0", "score_1", "score_2"] {
                    assert!(
                        criterion[field]
                            .as_str()
                            .is_some_and(|value| !value.trim().is_empty()),
                        "{id}.{criterion_id}.{field} must be non-empty"
                    );
                }
            }
        }
        for criterion in required_common {
            assert!(
                criterion_ids.contains(criterion),
                "{id} is missing common criterion {criterion}"
            );
        }

        assert!(case["expected"].is_object(), "{id} expected must be object");
        assert!(
            case["ground_truth"].is_object(),
            "{id} ground_truth must be object"
        );
        let required_sections = case["expected"]["required_sections"]
            .as_array()
            .unwrap_or_else(|| panic!("{id} required_sections must be array"));
        assert!(
            required_sections.iter().all(Value::is_string),
            "{id} required_sections must contain strings"
        );
    }
}

#[test]
fn diagnostic_cases_have_classification_ground_truth() {
    let root = repo_path("tests/prompt-evals");
    let manifest_path = root.join("manifest.json");
    let manifest = read_json(&manifest_path);

    for relative_case in string_array(&manifest, "cases", &manifest_path) {
        let case_path = safe_relative(&root, relative_case, "case path");
        let case = read_json(&case_path);
        if case["prompt"] != "debug" {
            continue;
        }
        let id = case["id"].as_str().expect("case id");
        assert_eq!(case["expected"]["outcome"], "diagnostic");
        assert!(
            case["expected"]["classification"]
                .as_str()
                .is_some_and(|classification| matches!(
                    classification,
                    "product-bug"
                        | "documentation-gap"
                        | "user-configuration"
                        | "infrastructure"
                        | "unknown"
                )),
            "{id} must have a supported diagnostic classification"
        );
        assert!(
            case["expected"]["confidence"]
                .as_array()
                .is_some_and(|values| !values.is_empty() && values.iter().all(Value::is_string)),
            "{id} must define allowed confidence values"
        );
    }
}

#[test]
fn prompt_eval_fixtures_are_synthetic_and_secret_free() {
    let root = repo_path("tests/prompt-evals");
    let mut stack = vec![root];
    let banned = [
        "github_pat_",
        "ghp_",
        "Authorization: Bearer",
        "dev.azure.com/",
        "visualstudio.com/",
    ];

    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).expect("fixture directory readable") {
            let entry = entry.expect("fixture entry readable");
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let content = fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("read fixture {}: {e}", path.display()));
            for marker in banned {
                assert!(
                    !content.contains(marker),
                    "{} contains banned live-data marker {marker}",
                    path.display()
                );
            }
        }
    }
}

#[test]
fn embedded_workflow_contexts_pass_ado_aw_lint() {
    let root = repo_path("tests/prompt-evals/cases");
    let mut stack = vec![root];
    let mut workflows = Vec::new();

    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).expect("fixture directory readable") {
            let entry = entry.expect("fixture entry readable");
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.file_name().is_some_and(|name| name == "workflow.md") {
                workflows.push(path);
            }
        }
    }

    assert!(!workflows.is_empty(), "expected embedded workflow contexts");
    let binary = PathBuf::from(env!("CARGO_BIN_EXE_ado-aw"));
    for workflow in workflows {
        let output = Command::new(&binary)
            .args([
                "lint",
                workflow.to_str().expect("UTF-8 fixture path"),
                "--json",
            ])
            .output()
            .unwrap_or_else(|e| panic!("run ado-aw lint for {}: {e}", workflow.display()));
        assert!(
            output.status.success(),
            "{} failed ado-aw lint:\nstdout:\n{}\nstderr:\n{}",
            workflow.display(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn prompt_evaluator_uses_only_the_gh_aw_managed_agent() {
    let workflow_path = repo_path(".github/workflows/prompt-evaluator.md");
    let workflow = fs::read_to_string(&workflow_path)
        .unwrap_or_else(|e| panic!("read {}: {e}", workflow_path.display()));

    for forbidden in [
        "scripts/prompt-evals",
        "--prompt-file",
        "subject_max_ai_credits",
        "judge_max_ai_credits",
        "judge_model",
    ] {
        assert!(
            !workflow.contains(forbidden),
            "prompt evaluator must not contain nested model harness marker {forbidden}"
        );
    }
    assert!(
        workflow.contains("gh-aw is already running you through the configured Copilot engine")
            && workflow.contains("Do not invoke `copilot`, another model, a judge, or a subagent"),
        "prompt evaluator must explicitly preserve the single gh-aw-managed agent architecture"
    );
}
