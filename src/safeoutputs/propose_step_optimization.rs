//! `propose-step-optimization` safe-output tool — opt-in Flow B
//! surface for runtime self-optimization.
//!
//! When the agent's front matter sets `self-optimization.enabled:
//! true` (see [`crate::compile::types::SelfOptimizationConfig`]), the
//! Stage-1 agent gets access to this tool. The agent uses it to
//! propose lifting **deterministic** bash work it ran successfully —
//! clone, install, cache restore, artifact download — out of its
//! prompt body and into the front-matter `steps:` / `post-steps:`
//! (and, when explicitly opted in, `setup:` / `teardown:`) sections.
//!
//! Stage 2 (threat analysis) cross-checks `source_command_evidence`
//! against the agent's actual command history (recorded via
//! [`crate::audit::analyzers::mcp`]); any bash in the proposed steps
//! block that the agent didn't demonstrably execute is a strong
//! prompt-injection signal.
//!
//! Stage 3 (the executor) IR-validates the proposed steps block via
//! [`crate::compile::ir::validate_step_block`] with
//! [`crate::compile::ir::StepKindAllow::Curated`] — only Bash steps
//! and the typed-factory tasks (see
//! [`crate::compile::ir::tasks::CURATED_TASK_IDS`]) are accepted —
//! and then either renders a `🎭`-marked diff preview to the build
//! summary (`staged: true`, the default) or opens a PR against the
//! source `.md` adding the new step entries (`staged: false`). The
//! Stage 3 staged-preview and live-PR paths land in subsequent
//! commits; this commit ships the Stage-1 surface plus a
//! placeholder Stage-3 executor that records the proposal for audit
//! but does not yet apply it.
//!
//! Tool registration is gated by
//! [`crate::safeoutputs::OPT_IN_GATED_TOOLS`]: the MCP layer strips
//! the route unless the compiler explicitly enables it via
//! `--enabled-tools propose-step-optimization`, which only happens
//! when `self-optimization.enabled: true` is in the front matter.

use log::{debug, info, warn};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::safeoutputs::{ExecutionContext, ExecutionResult, Executor, Validate};
use crate::sanitize::{SanitizeContent, sanitize_config};
use crate::tool_result;
use anyhow::ensure;

// ── Wire-format Section enum ────────────────────────────────────────────

/// Front-matter section a proposal targets.
///
/// Mirrors [`crate::compile::types::StepSection`] (kebab-case wire
/// format). Defined locally so the safe-output's Stage-1 surface
/// doesn't drag schemars into `compile::types`. Stage 3 compares the
/// agent's claimed section against the front-matter's
/// `allowed_sections` directly from the deserialized string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ProposalSection {
    /// `steps:` — runs BEFORE the agent inside the agent job.
    Steps,
    /// `post-steps:` — runs AFTER the agent inside the agent job.
    PostSteps,
    /// `setup:` — separate job that runs before the agent job.
    Setup,
    /// `teardown:` — separate job that runs after safe-outputs.
    Teardown,
}

impl ProposalSection {
    /// Stable, kebab-case wire string for the section. Used by
    /// Stage 3 to look up the matching front-matter
    /// `allowed_sections` entry without depending on
    /// `compile::types::StepSection` Display semantics.
    pub fn as_wire_str(self) -> &'static str {
        match self {
            ProposalSection::Steps => "steps",
            ProposalSection::PostSteps => "post-steps",
            ProposalSection::Setup => "setup",
            ProposalSection::Teardown => "teardown",
        }
    }
}

// ── Stage 1: Params (agent-provided) ──────────────────────────────────────

/// Parameters the agent supplies when calling `propose-step-optimization`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ProposeStepOptimizationParams {
    /// Which front-matter section to propose into. Stage 3 rejects
    /// proposals that target a section not listed under
    /// `self-optimization.allowed-sections` in the front matter
    /// (default: `[steps, post-steps]` — opt-in required for
    /// `setup` / `teardown`).
    pub section: ProposalSection,
    /// Short, plain-prose explanation of why this hoist saves time
    /// or tokens. Sanitised at Stage 3; surfaced in the build
    /// summary preview and the live-mode PR body. Max 2 KB.
    pub rationale: String,
    /// Optional estimated token savings per build. Informational
    /// only; surfaced in the preview/PR body. Stage 3 does not
    /// trust this number — it's a hint the author can use to
    /// prioritise reviewing proposals.
    pub estimated_token_savings: Option<u64>,
    /// The proposed step block as a JSON array of step entries
    /// (one entry per step). Stage 3 IR-validates this with
    /// `StepKindAllow::Curated` — only Bash steps and the curated
    /// task allow-list (`CURATED_TASK_IDS`) are accepted.
    pub steps: serde_json::Value,
    /// Bash commands the agent actually executed during this run.
    /// Used by Stage 2 detection as the cross-check signal: any
    /// bash content in `steps` that does not have a matching entry
    /// in this list indicates the proposal is not grounded in the
    /// agent's observed behaviour and is treated as a
    /// prompt-injection candidate.
    pub source_command_evidence: Vec<String>,
}

/// Cap on the rationale string. Keeps the PR body / build-summary
/// preview compact and bounds the sanitiser's work.
const MAX_RATIONALE_BYTES: usize = 2_048;

/// Cap on the number of evidence entries the agent may submit per
/// proposal. The agent's full command history is already preserved
/// in the audit artefacts; this list is a cross-check on the
/// proposal, not a re-emission of every command.
const MAX_EVIDENCE_ENTRIES: usize = 64;

/// Cap on the size of an individual evidence entry. Mirrors the
/// `MAX_BASH_BODY_BYTES` cap inside the step-block validator so a
/// matching `steps` entry cannot exceed it.
const MAX_EVIDENCE_ENTRY_BYTES: usize = 10_000;

impl Validate for ProposeStepOptimizationParams {
    fn validate(&self) -> anyhow::Result<()> {
        ensure!(!self.rationale.trim().is_empty(), "rationale must not be empty");
        ensure!(
            self.rationale.len() <= MAX_RATIONALE_BYTES,
            "rationale must be at most {MAX_RATIONALE_BYTES} bytes (got {})",
            self.rationale.len()
        );
        // `steps` must be a JSON array; the deeper structural
        // validation runs in Stage 3 via the IR validator.
        ensure!(
            self.steps.is_array(),
            "steps must be a JSON array of ADO step entries"
        );
        ensure!(
            !self
                .steps
                .as_array()
                .expect("checked array above")
                .is_empty(),
            "steps must contain at least one entry"
        );
        ensure!(
            self.source_command_evidence.len() <= MAX_EVIDENCE_ENTRIES,
            "source_command_evidence must contain at most {MAX_EVIDENCE_ENTRIES} entries (got {})",
            self.source_command_evidence.len()
        );
        for (i, entry) in self.source_command_evidence.iter().enumerate() {
            ensure!(
                entry.len() <= MAX_EVIDENCE_ENTRY_BYTES,
                "source_command_evidence[{i}] exceeds {MAX_EVIDENCE_ENTRY_BYTES} bytes"
            );
        }
        Ok(())
    }
}

// ── Stage 1: Result (generated by macro) ──────────────────────────────────

tool_result! {
    name = "propose-step-optimization",
    write = true,
    params = ProposeStepOptimizationParams,
    /// Result of proposing a step-block optimisation. Stage 2
    /// cross-checks; Stage 3 IR-validates the steps block and
    /// either previews (staged) or opens a PR against the source
    /// `.md` (live).
    pub struct ProposeStepOptimizationResult {
        section: ProposalSection,
        rationale: String,
        estimated_token_savings: Option<u64>,
        steps: serde_json::Value,
        source_command_evidence: Vec<String>,
    }
}

// ── Stage 3: Sanitisation ────────────────────────────────────────────────

impl SanitizeContent for ProposeStepOptimizationResult {
    fn sanitize_content_fields(&mut self) {
        self.rationale = sanitize_config(&self.rationale);
        // `steps` and `source_command_evidence` are passed through
        // unchanged: `steps` is validated structurally by
        // `validate_step_block` in Stage 3 (which enforces tight
        // shape and length constraints), and the evidence list is
        // bounded by Validate above. Sanitising bash bodies would
        // mangle their literal content and break the Stage 2
        // command-history cross-check.
    }
}

// ── Stage 3: Execution (placeholder) ─────────────────────────────────────

#[async_trait::async_trait]
impl Executor for ProposeStepOptimizationResult {
    fn dry_run_summary(&self) -> String {
        let entry_count = self.steps.as_array().map(Vec::len).unwrap_or(0);
        format!(
            "propose hoisting {entry_count} step(s) into front-matter `{}` (rationale: {})",
            self.section.as_wire_str(),
            truncate(&self.rationale, 120),
        )
    }

    async fn execute_impl(&self, _ctx: &ExecutionContext) -> anyhow::Result<ExecutionResult> {
        info!(
            "propose-step-optimization: agent proposed {} entries for section `{}`",
            self.steps.as_array().map(Vec::len).unwrap_or(0),
            self.section.as_wire_str()
        );
        debug!(
            "propose-step-optimization payload: rationale={:?}, est_savings={:?}, evidence_count={}",
            self.rationale,
            self.estimated_token_savings,
            self.source_command_evidence.len()
        );

        // PLACEHOLDER — the Stage 3 staged-preview renderer and
        // live-mode PR opener land in subsequent commits (todos
        // stage3-staged-preview and stage3-live-pr-path). For now
        // we record the proposal in safe_outputs.ndjson so audit
        // tooling can see the proposed payload, and report success
        // with a message that the executor is not yet active.
        warn!(
            "propose-step-optimization: Stage 3 executor is not yet active; \
             the proposal was recorded in safe_outputs.ndjson but no \
             preview or PR has been emitted."
        );

        Ok(ExecutionResult::success(
            "Step-optimization proposal recorded. The Stage 3 executor \
             (staged preview / PR opener) lands in a follow-up commit; \
             this run only persists the proposal to safe_outputs.ndjson \
             for audit visibility."
                .to_string(),
        ))
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let mut out = s.chars().take(max).collect::<String>();
        out.push('…');
        out
    }
}

// No per-tool `Config` struct: this tool is configured exclusively via
// the top-level `self-optimization:` front-matter section (see
// `crate::compile::types::SelfOptimizationConfig`). Stage 3 reads
// `self_optimization.staged` and `allowed_sections` directly from
// the front matter, not via `ctx.get_tool_config(...)`. A stray
// `safe-outputs.propose-step-optimization:` block is independently
// rejected by `compile::common::validate_self_optimization_config`.

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::safeoutputs::ToolResult;

    fn minimal_params() -> ProposeStepOptimizationParams {
        ProposeStepOptimizationParams {
            section: ProposalSection::Steps,
            rationale: "Lift deterministic clone+install out of the agent".into(),
            estimated_token_savings: Some(4200),
            steps: serde_json::json!([
                {"bash": "git fetch --depth=1 origin main", "displayName": "Fetch main"}
            ]),
            source_command_evidence: vec![
                "git fetch --depth=1 origin main".into(),
                "git fetch --depth=1 origin main".into(),
            ],
        }
    }

    #[test]
    fn result_has_correct_name() {
        assert_eq!(
            ProposeStepOptimizationResult::NAME,
            "propose-step-optimization"
        );
    }

    #[test]
    fn requires_write_is_true() {
        assert!(
            ProposeStepOptimizationResult::REQUIRES_WRITE,
            "propose-step-optimization opens PRs in live mode; must require write"
        );
    }

    #[test]
    fn params_round_trip_through_json() {
        let p = minimal_params();
        // Round-trip via JSON to confirm the wire shape (the MCP
        // transport hands us JSON).
        let json = serde_json::to_string(&serde_json::json!({
            "section": "steps",
            "rationale": p.rationale,
            "estimated_token_savings": p.estimated_token_savings,
            "steps": p.steps,
            "source_command_evidence": p.source_command_evidence,
        }))
        .unwrap();
        let parsed: ProposeStepOptimizationParams = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.section, ProposalSection::Steps);
        assert_eq!(parsed.rationale, p.rationale);
        assert_eq!(parsed.estimated_token_savings, Some(4200));
        assert!(parsed.steps.is_array());
        assert_eq!(parsed.source_command_evidence.len(), 2);
    }

    #[test]
    fn section_kebab_case_round_trip() {
        for (variant, wire) in [
            (ProposalSection::Steps, "steps"),
            (ProposalSection::PostSteps, "post-steps"),
            (ProposalSection::Setup, "setup"),
            (ProposalSection::Teardown, "teardown"),
        ] {
            let serialised = serde_json::to_string(&variant).unwrap();
            assert!(
                serialised.contains(wire),
                "{variant:?} should serialise as {wire:?}; got {serialised}"
            );
            let parsed: ProposalSection =
                serde_json::from_str(&format!("\"{wire}\"")).unwrap();
            assert_eq!(parsed, variant);
            assert_eq!(variant.as_wire_str(), wire);
        }
    }

    #[test]
    fn validation_rejects_empty_rationale() {
        let mut p = minimal_params();
        p.rationale = "   ".into();
        let r: Result<ProposeStepOptimizationResult, _> = p.try_into();
        let err = r.expect_err("empty rationale must be rejected");
        assert!(format!("{err}").contains("rationale"));
    }

    #[test]
    fn validation_rejects_oversized_rationale() {
        let mut p = minimal_params();
        p.rationale = "x".repeat(MAX_RATIONALE_BYTES + 1);
        let r: Result<ProposeStepOptimizationResult, _> = p.try_into();
        assert!(r.is_err(), "oversized rationale must be rejected");
    }

    #[test]
    fn validation_rejects_non_array_steps() {
        let mut p = minimal_params();
        p.steps = serde_json::json!({ "bash": "echo hi" });
        let r: Result<ProposeStepOptimizationResult, _> = p.try_into();
        let err = r.expect_err("steps must be a JSON array");
        assert!(format!("{err}").contains("array"));
    }

    #[test]
    fn validation_rejects_empty_steps_array() {
        let mut p = minimal_params();
        p.steps = serde_json::json!([]);
        let r: Result<ProposeStepOptimizationResult, _> = p.try_into();
        assert!(r.is_err(), "empty steps array must be rejected");
    }

    #[test]
    fn validation_rejects_too_many_evidence_entries() {
        let mut p = minimal_params();
        p.source_command_evidence = (0..MAX_EVIDENCE_ENTRIES + 1)
            .map(|i| format!("echo {i}"))
            .collect();
        let r: Result<ProposeStepOptimizationResult, _> = p.try_into();
        assert!(r.is_err(), "over-cap evidence list must be rejected");
    }

    #[test]
    fn validation_rejects_oversized_evidence_entry() {
        let mut p = minimal_params();
        p.source_command_evidence = vec!["x".repeat(MAX_EVIDENCE_ENTRY_BYTES + 1)];
        let r: Result<ProposeStepOptimizationResult, _> = p.try_into();
        assert!(r.is_err(), "oversized evidence entry must be rejected");
    }

    #[test]
    fn dry_run_summary_includes_section_and_truncated_rationale() {
        let p = minimal_params();
        let r: ProposeStepOptimizationResult = p.try_into().unwrap();
        let summary = r.dry_run_summary();
        assert!(summary.contains("steps"));
        assert!(summary.contains("Lift deterministic"));
    }

    #[test]
    fn sanitize_preserves_steps_and_evidence_unchanged() {
        let p = minimal_params();
        let mut r: ProposeStepOptimizationResult = p.try_into().unwrap();
        let steps_before = r.steps.clone();
        let evidence_before = r.source_command_evidence.clone();
        r.sanitize_content_fields();
        assert_eq!(
            r.steps, steps_before,
            "steps must pass through sanitize unchanged — \
             Stage 3 IR validator enforces structure; mangling here \
             would break the Stage 2 command-history cross-check"
        );
        assert_eq!(
            r.source_command_evidence, evidence_before,
            "evidence list must pass through sanitize unchanged"
        );
    }
}
