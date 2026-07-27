use std::fs;
use std::path::PathBuf;

fn repo_path(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel)
}

fn read(rel: &str) -> String {
    fs::read_to_string(repo_path(rel)).expect("prompt file should be readable")
}

#[test]
fn prompts_reference_shared_contract_v2() {
    let shared = repo_path("prompts/prompt-contract-v2.md");
    assert!(shared.exists(), "shared prompt contract must exist");

    for rel in [
        "prompts/create-ado-agentic-workflow.md",
        "prompts/update-ado-agentic-workflow.md",
        "prompts/debug-ado-agentic-workflow.md",
    ] {
        let content = read(rel);
        assert!(
            content.contains("prompts/prompt-contract-v2.md"),
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
            content.contains("DEFAULT_COPILOT_MODEL"),
            "{rel} should anchor model defaults to src/engine.rs"
        );
    }
}
