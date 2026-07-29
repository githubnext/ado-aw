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
            content.contains("DEFAULT_COPILOT_MODEL"),
            "{rel} should anchor model defaults to src/engine.rs"
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
