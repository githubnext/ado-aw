use std::fs;
use std::path::PathBuf;

fn repo_path(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel)
}

fn read(rel: &str) -> String {
    fs::read_to_string(repo_path(rel)).expect("prompt file should be readable")
}

#[test]
fn prompts_reference_shared_contract() {
    let shared = repo_path("prompts/prompt-contract.md");
    assert!(shared.exists(), "shared prompt contract must exist");

    for rel in [
        "prompts/create-ado-agentic-workflow.md",
        "prompts/update-ado-agentic-workflow.md",
        "prompts/debug-ado-agentic-workflow.md",
    ] {
        let content = read(rel);
        assert!(
            content.contains("prompts/prompt-contract.md"),
            "{rel} must reference shared contract"
        );
    }
}

#[test]
fn debug_prompt_regression_report_only_without_consent() {
    let content = read("prompts/debug-ado-agentic-workflow.md");

    assert!(
        content.contains("Dry-run report only"),
        "debug prompt must default to report-only"
    );
    assert!(
        content.contains("If and only if the user explicitly asks to file now"),
        "debug prompt must gate filing on explicit consent"
    );
    assert!(
        content.contains("Any case without explicit approval")
            && content.contains("Return dry-run report/draft only"),
        "decision table must preserve report-only behavior without approval"
    );
}

#[test]
fn debug_prompt_regression_issue_filing_only_after_gate() {
    let content = read("prompts/debug-ado-agentic-workflow.md");

    assert!(
        content.contains("Consent-Gated Filing")
            && content.contains("Confirm approval")
            && content.contains("File and return URL"),
        "debug prompt must enforce approval gate before filing"
    );
}

#[test]
fn prompts_ban_unconditional_issue_filing_language() {
    let banned = [
        "The session is not complete until the issue is filed",
        "File directly; do not ask for confirmation first",
    ];

    for rel in [
        "prompts/create-ado-agentic-workflow.md",
        "prompts/update-ado-agentic-workflow.md",
        "prompts/debug-ado-agentic-workflow.md",
    ] {
        let content = read(rel);
        for phrase in banned {
            assert!(
                !content.contains(phrase),
                "{rel} contains banned unconditional side-effect phrase: {phrase}"
            );
        }
    }
}

#[test]
fn prompts_align_model_default_to_code_truth() {
    for rel in [
        "prompts/create-ado-agentic-workflow.md",
        "prompts/update-ado-agentic-workflow.md",
    ] {
        let content = read(rel);
        assert!(
            content.contains("no compiler-selected default")
                && content.contains("omits `--model` unless `engine.model` is configured"),
            "{rel} should anchor model defaults to src/engine.rs behavior"
        );
    }
}

#[test]
fn prompts_define_explicit_done_criteria() {
    for rel in [
        "prompts/create-ado-agentic-workflow.md",
        "prompts/update-ado-agentic-workflow.md",
        "prompts/debug-ado-agentic-workflow.md",
    ] {
        let content = read(rel);
        assert!(
            content.contains("## Done Criteria"),
            "{rel} must define explicit done criteria"
        );
    }
}

#[test]
fn authoring_prompts_keep_expected_output_contracts() {
    let create = read("prompts/create-ado-agentic-workflow.md");
    assert!(
        create.contains("complete `.md` content")
            && create.contains("assumptions and unresolved questions")
            && create.contains("ado-aw compile"),
        "create prompt must preserve its workflow artifact and compile guidance"
    );

    let update = read("prompts/update-ado-agentic-workflow.md");
    assert!(
        update.contains("concise diff summary")
            && update.contains("whether compile is required")
            && update.contains("front matter changed -> `ado-aw compile` required"),
        "update prompt must preserve targeted-diff and recompilation guidance"
    );

    let debug = read("prompts/debug-ado-agentic-workflow.md");
    for section in [
        "## Diagnostic Summary",
        "## Evidence",
        "## Analysis",
        "## Root Cause",
        "## Recommended Next Action",
    ] {
        assert!(
            debug.contains(section),
            "debug prompt must require section {section}"
        );
    }
}

#[test]
fn authoring_prompts_separate_pr_conversations_reviews_and_votes() {
    for rel in [
        "prompts/create-ado-agentic-workflow.md",
        "prompts/update-ado-agentic-workflow.md",
    ] {
        let content = read(rel);
        for required in [
            "`add-pull-request-comment`",
            "`submit-pull-request-review`",
            "`comment` is non-voting",
            "`reset` explicitly clears",
            "never buffered",
            "`max-comments`",
            "`expected_head_sha`",
            "`update-pull-request-comment`",
        ] {
            assert!(
                content.contains(required),
                "{rel} is missing PR intent contract {required}"
            );
        }
    }
}

#[test]
fn repository_workflow_subagents_inherit_model_selection() {
    let mut subagents = Vec::new();
    for entry in fs::read_dir(repo_path(".github/workflows")).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("md") {
            continue;
        }

        let content = fs::read_to_string(&path).unwrap().replace("\r\n", "\n");
        for block in content.split("\n## agent: ").skip(1) {
            let (name, body) = block.split_once('\n').expect("inline agent heading");
            let front_matter = body
                .strip_prefix("---\n")
                .and_then(|body| {
                    body.split_once("\n---")
                        .map(|(front_matter, _)| front_matter)
                })
                .expect("inline agent front matter");
            let config: serde_yaml::Mapping = serde_yaml::from_str(front_matter).unwrap();
            assert!(
                !config.contains_key(serde_yaml::Value::String("model".into())),
                "{}: {name} must inherit runtime model selection, not override it",
                path.display()
            );
            assert!(
                content.contains("Do not specify a model or model alias when launching"),
                "{} must prevent launch-time model overrides",
                path.display()
            );
            subagents.push(name.to_string());
        }
    }
    for name in ["`rust-critic`", "`ts-critic`", "`pr-processor`"] {
        assert!(
            subagents.iter().any(|agent| agent == name),
            "missing inline agent {name}"
        );
    }
}

#[test]
fn reviewers_pin_inline_and_summary_targets_to_trusted_dispatch_context() {
    let text = read(".github/workflows/shared/pr-review-base.md").replace("\r\n", "\n");
    let front = text
        .strip_prefix("---\n")
        .unwrap()
        .split_once("\n---")
        .unwrap()
        .0;
    let config: serde_yaml::Value = serde_yaml::from_str(front).unwrap();
    let inline = &config["safe-outputs"]["create-pull-request-review-comment"];
    let summary = &config["safe-outputs"]["submit-pull-request-review"];
    let target = inline["target"].as_str().unwrap();
    assert_eq!(summary["target"], inline["target"]);
    assert_eq!(
        target,
        "${{ github.event.pull_request.number || github.event.issue.number || fromJSON(github.event.inputs.aw_context || github.event.client_payload.aw_context || '{}').item_number || '0' }}"
    );
    let commit = "${{ github.event.pull_request.head.sha || github.sha }}";
    assert_eq!(inline["commit-id"].as_str().unwrap(), commit);
    assert_eq!(summary["commit-id"].as_str().unwrap(), commit);
    let guard = "github.event.pull_request.number || github.event.issue.pull_request || fromJSON(github.event.inputs.aw_context || github.event.client_payload.aw_context || '{}').item_type == 'pull_request'";
    for workflow in [
        "review-rust",
        "review-typescript",
        "review-tests",
        "review-security",
        "review-compiler-contract",
    ] {
        let source = read(&format!(".github/workflows/{workflow}.md")).replace("\r\n", "\n");
        let front = source
            .strip_prefix("---\n")
            .unwrap()
            .split_once("\n---")
            .unwrap()
            .0;
        let source: serde_yaml::Value = serde_yaml::from_str(front).unwrap();
        assert_eq!(
            source["if"].as_str(),
            Some(guard),
            "{workflow}: root PR-only guard"
        );
        let lock: serde_yaml::Value =
            serde_yaml::from_str(&read(&format!(".github/workflows/{workflow}.lock.yml"))).unwrap();
        assert!(
            lock["jobs"]["activation"]["if"]
                .as_str()
                .unwrap()
                .contains(guard),
            "{workflow}: compiled PR-only guard"
        );
        assert_eq!(
            lock["jobs"]["agent"]["permissions"]["pull-requests"].as_str(),
            Some("read")
        );
        let handlers = lock["jobs"]["safe_outputs"]["steps"]
            .as_sequence()
            .unwrap()
            .iter()
            .find_map(|step| step["env"]["GH_AW_SAFE_OUTPUTS_HANDLER_CONFIG"].as_str())
            .unwrap();
        assert!(
            !handlers.contains("\\u0026"),
            "JSON escaping must not corrupt GitHub expression operators"
        );
        let handlers: serde_json::Value = serde_json::from_str(handlers).unwrap();
        for handler in [
            "create_pull_request_review_comment",
            "submit_pull_request_review",
        ] {
            assert_eq!(
                handlers[handler]["target"].as_str(),
                Some(target),
                "{workflow}: {handler}"
            );
            assert_eq!(
                handlers[handler]["commit_id"].as_str(),
                Some(commit),
                "{workflow}: {handler}"
            );
            assert!(
                handlers[handler].get("allowed_repos").is_none(),
                "{workflow} must not widen repository scope"
            );
        }
    }
}

#[test]
fn rust_reviewer_reserves_time_for_a_bounded_final_review() {
    let prompt = read(".github/workflows/review-rust.md");
    for required in [
        "12-minute agent-work budget",
        "by minute 12 submit",
        "at most a 60-second",
        "Never poll repeatedly",
        "at most three high-risk changed Rust",
        "six-minute investigation budget",
        "Do not build the repository or run full test",
        "unreviewed areas",
        "Submit exactly once even if the critic was unavailable",
    ] {
        assert!(
            prompt.contains(required),
            "Rust reviewer is missing its bounded contract: {required}"
        );
    }
}
