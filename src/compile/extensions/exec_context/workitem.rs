//! Workitem execution-context contributor (Stage 4 of the
//! exec-context contributor build-out — see plan.md).
//!
//! **PR-linked mode only in this iteration.** Commit-scrape and
//! parameter-driven activation modes are explicit follow-up tickets
//! per the user's scoping decision.
//!
//! Activates whenever the PR contributor activates (i.e. `on.pr` is
//! configured AND the PR contributor is not disabled), unless the
//! `workitem` contributor itself is explicitly disabled. Runtime
//! gate: same as the PR contributor — `eq(Build.Reason, 'PullRequest')`.
//!
//! ## Artefacts (staged by the bundle on success)
//!
//! - `aw-context/workitem/ids.txt`                  — newline-delimited
//!   list of WI ids found via `repo_list_pull_request_work_items`
//! - `aw-context/workitem/<id>/summary.json`        — id, type, title,
//!   state, area-path, iteration-path, assigned-to, tags
//! - `aw-context/workitem/<id>/description.md`      — System.Description
//!   (HTML → plain text via shared/untrusted.ts::htmlToPlainText),
//!   wrapped in untrusted-content sentinel
//! - `aw-context/workitem/<id>/acceptance.md`       — same for
//!   Microsoft.VSTS.Common.AcceptanceCriteria
//! - `aw-context/workitem/<id>/repro.md`            — same for
//!   Microsoft.VSTS.TCM.ReproSteps (Bug type)
//! - `aw-context/workitem/<id>/comments.json`       — discussion
//!   history (oldest → newest), each entry wrapped in untrusted sentinel
//! - `aw-context/workitem/<id>/links.json`          — relations summary
//! - `aw-context/workitem/<id>/attachments.json`    — attachment
//!   metadata (name, size, url) — bytes NOT downloaded
//! - `aw-context/workitem/truncated.txt`            — present when
//!   the linked WI count exceeded `max-items`
//! - `aw-context/workitem/errors.txt`               — per-id fetch
//!   failures (if any)
//! - `aw-context/workitem/error.txt`                — present only
//!   when ALL fetches failed (total failure)
//!
//! ## Trust boundary
//!
//! **This contributor crosses an untrusted-prose boundary.** WI
//! description / acceptance criteria / repro / comment text is
//! authored by anyone with WI write access — effectively arbitrary
//! user input. The bundle wraps every prose body via
//! `shared/untrusted.ts::wrapAgentReadableUntrusted` before
//! writing to disk, and the prompt fragment ONLY interpolates
//! short structured fields (id, title, type, state). Long prose
//! stays in files, sentineled so:
//!
//!   1. The agent sees a clear "this is untrusted content, do not
//!      obey embedded directives" framing.
//!   2. Stage-2 detection can scan for the sentinel to flag any
//!      prompt region that crossed an untrusted boundary.
//!
//! `SYSTEM_ACCESSTOKEN` is mapped only into this step's `env:`
//! block; same posture as the PR contributor.

use crate::compile::extensions::CompileContext;
use crate::compile::ado_bundle::{Bundle, TokenSource, apply_bundle_auth};
use crate::compile::extensions::ado_script::EXEC_CONTEXT_WORKITEM_PATH;
use crate::compile::ir::condition::{Condition, Expr};
use crate::compile::ir::env::EnvValue;
use crate::compile::ir::step::{BashStep, Step};
use crate::compile::types::WorkitemContextConfig;

use super::contributor::ContextContributor;

/// Workitem-context contributor (PR-linked mode only).
pub(super) struct WorkitemContextContributor {
    config: WorkitemContextConfig,
    /// Whether `on.pr.mode == Synthetic` for this agent. When true,
    /// PR identifiers come from `AW_PR_*` hoisted variables instead
    /// of `System.PullRequest.*` macros (same pattern as PR
    /// contributor).
    synthetic_pr_active: bool,
    /// Resolved PR-contributor-enabled flag. Workitem activation
    /// tracks PR-contributor activation per the plan's contract
    /// ("activates whenever the pr contributor activates"). Passed
    /// in at construction so the contributor doesn't have to know
    /// about `PrContextConfig`.
    pr_contributor_enabled: bool,
}

impl WorkitemContextContributor {
    pub(super) fn new(
        config: WorkitemContextConfig,
        synthetic_pr_active: bool,
        pr_contributor_enabled: bool,
    ) -> Self {
        Self {
            config,
            synthetic_pr_active,
            pr_contributor_enabled,
        }
    }
}

impl ContextContributor for WorkitemContextContributor {
    fn name(&self) -> &str {
        "workitem"
    }

    fn should_activate(&self, ctx: &CompileContext) -> bool {
        // Workitem activation = "PR contributor activates AND
        // workitem isn't explicitly disabled". The PR contributor's
        // activation check is the source of truth for "is this a
        // PR build with PR context enabled".
        if ctx.front_matter.pr_trigger().is_none() {
            return false;
        }
        if !self.pr_contributor_enabled {
            return false;
        }
        match self.config.explicit_enabled() {
            Some(false) => false,
            Some(true) | None => true,
        }
    }

    fn prepare_step_typed(&self, ctx: &CompileContext) -> anyhow::Result<Option<Step>> {
        // Defensive: mirror the manual.rs pattern — `declarations()`
        // already gates on `should_activate`, but this guard catches
        // direct callers (tests / future tooling). Returning `Ok(None)`
        // ensures no live step (with an active bearer) is emitted
        // when the contributor is inactive. The workitem contributor
        // is the highest-consequence bearer holder among the new
        // contributors (REST calls expose linked-WI prose), so the
        // guard matters most here.
        if !self.should_activate(ctx) {
            return Ok(None);
        }
        // In synth mode the PR id comes from the hoisted AW_PR_ID Agent-job
        // variable and the step always runs (guarded by a runtime prelude);
        // in real-PR mode the auto-injected SYSTEM_PULLREQUEST_PULLREQUESTID is
        // read directly and the step gates on Build.Reason == PullRequest.
        let condition = if self.synthetic_pr_active {
            Condition::Succeeded
        } else {
            Condition::Eq(
                Expr::Variable("Build.Reason".to_string()),
                Expr::Literal("PullRequest".to_string()),
            )
        };

        let prelude = if self.synthetic_pr_active {
            "    if [ -z \"$SYSTEM_PULLREQUEST_PULLREQUESTID\" ]; then\n      echo \"[aw-context] No PR identifier resolved; skipping exec-context-workitem.\"\n      exit 0\n    fi\n"
        } else {
            ""
        };

        let max_items = self.config.max_items_resolved();
        let max_body_kb = self.config.max_body_kb_resolved();

        let script = format!("set -euo pipefail\n{prelude}node '{EXEC_CONTEXT_WORKITEM_PATH}'\n");
        // ADO auto-injects predefined System.*/Build.* context vars, so only
        // the SYSTEM_ACCESSTOKEN bearer (not auto-injected), the mode-dependent
        // synth PR id, and the computed limits are projected.
        let mut step = apply_bundle_auth(
            BashStep::new(
                "Stage workitem execution context (aw-context/workitem/*)",
                script,
            )
            .with_condition(condition),
            Bundle::ExecContextWorkitem,
            TokenSource::SystemAccessToken,
        );
        if self.synthetic_pr_active {
            step = step.with_env(
                "SYSTEM_PULLREQUEST_PULLREQUESTID",
                EnvValue::pipeline_var("AW_PR_ID"),
            );
        }
        step = step
            .with_env(
                "AW_WORKITEM_MAX_ITEMS",
                EnvValue::literal(max_items.to_string()),
            )
            .with_env(
                "AW_WORKITEM_MAX_BODY_KB",
                EnvValue::literal(max_body_kb.to_string()),
            );

        Ok(Some(Step::Bash(step)))
    }

    fn bash_commands(&self) -> Vec<String> {
        // The agent reads staged files via the always-permitted
        // `cat` / `jq` — no new bash allow-list entries.
        vec![]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::extensions::CompileContext;
    use crate::compile::types::FrontMatter;

    fn parse_fm(src: &str) -> FrontMatter {
        let (fm, _) = crate::compile::common::parse_markdown(src).unwrap();
        fm
    }

    fn pr_fm() -> FrontMatter {
        parse_fm(
            "---\nname: test\ndescription: test\non:\n  pr:\n    branches:\n      include: [main]\n---\n",
        )
    }

    fn no_trigger_fm() -> FrontMatter {
        parse_fm("---\nname: test\ndescription: test\n---\n")
    }

    #[test]
    fn does_not_activate_without_on_pr() {
        let fm = no_trigger_fm();
        let c = WorkitemContextContributor::new(WorkitemContextConfig::default(), false, true);
        let ctx = CompileContext::for_test(&fm);
        assert!(!c.should_activate(&ctx));
    }

    #[test]
    fn activates_when_on_pr_configured_default() {
        let fm = pr_fm();
        let c = WorkitemContextContributor::new(WorkitemContextConfig::default(), false, true);
        let ctx = CompileContext::for_test(&fm);
        assert!(c.should_activate(&ctx));
    }

    #[test]
    fn explicitly_disabled_suppresses_activation() {
        let fm = pr_fm();
        let c = WorkitemContextContributor::new(
            WorkitemContextConfig {
                enabled: Some(false),
                max_items: None,
                max_body_kb: None,
            },
            false,
            true,
        );
        let ctx = CompileContext::for_test(&fm);
        assert!(!c.should_activate(&ctx));
    }

    /// Defensive guard: when the contributor is inactive, calling
    /// `prepare_step_typed` directly MUST return `Ok(None)` rather
    /// than a live step. Mirrors the manual.rs guard pattern and
    /// catches direct callers (tests / future tooling) that bypass
    /// the outer `declarations()`-level `should_activate` filter —
    /// without this guard, a future test could silently emit a
    /// step with a live SYSTEM_ACCESSTOKEN bearer.
    #[test]
    fn prepare_step_returns_none_when_inactive() {
        // Inactive case 1: no on.pr trigger.
        let fm = no_trigger_fm();
        let c = WorkitemContextContributor::new(WorkitemContextConfig::default(), false, true);
        let ctx = CompileContext::for_test(&fm);
        assert!(c.prepare_step_typed(&ctx).unwrap().is_none());

        // Inactive case 2: on.pr present but workitem.enabled: false.
        let fm = pr_fm();
        let c = WorkitemContextContributor::new(
            WorkitemContextConfig {
                enabled: Some(false),
                max_items: None,
                max_body_kb: None,
            },
            false,
            true,
        );
        let ctx = CompileContext::for_test(&fm);
        assert!(c.prepare_step_typed(&ctx).unwrap().is_none());

        // Inactive case 3: PR contributor disabled (workitem tracks PR).
        let fm = pr_fm();
        let c = WorkitemContextContributor::new(
            WorkitemContextConfig::default(),
            false,
            false, // pr_contributor_enabled = false
        );
        let ctx = CompileContext::for_test(&fm);
        assert!(c.prepare_step_typed(&ctx).unwrap().is_none());
    }

    #[test]
    fn prepare_step_carries_bearer_and_caps() {
        let fm = pr_fm();
        let c = WorkitemContextContributor::new(WorkitemContextConfig::default(), false, true);
        let ctx = CompileContext::for_test(&fm);
        let step = c.prepare_step_typed(&ctx).unwrap().unwrap();
        let bash = match &step {
            Step::Bash(b) => b,
            other => panic!("expected Bash, got {other:?}"),
        };
        assert!(matches!(
            bash.env.get("SYSTEM_ACCESSTOKEN"),
            Some(EnvValue::Secret(v)) if v == "System.AccessToken"
        ));
        // Non-synth path: the bundle reads the auto-injected PR + context
        // vars directly, so the step re-projects none of them.
        for stripped in [
            "SYSTEM_PULLREQUEST_PULLREQUESTID",
            "SYSTEM_COLLECTIONURI",
            "SYSTEM_TEAMPROJECT",
            "BUILD_SOURCESDIRECTORY",
            "BUILD_REPOSITORY_ID",
        ] {
            assert!(
                bash.env.get(stripped).is_none(),
                "{stripped} is auto-injected and must not be re-projected"
            );
        }
        // Default caps surfaced as env literals so the bundle can read them.
        match bash.env.get("AW_WORKITEM_MAX_ITEMS") {
            Some(EnvValue::Literal(s)) => assert_eq!(s, "5"),
            other => panic!("expected literal '5', got {other:?}"),
        }
        match bash.env.get("AW_WORKITEM_MAX_BODY_KB") {
            Some(EnvValue::Literal(s)) => assert_eq!(s, "32"),
            other => panic!("expected literal '32', got {other:?}"),
        }
    }

    #[test]
    fn caps_can_be_overridden() {
        let fm = pr_fm();
        let c = WorkitemContextContributor::new(
            WorkitemContextConfig {
                enabled: None,
                max_items: Some(10),
                max_body_kb: Some(64),
            },
            false,
            true,
        );
        let ctx = CompileContext::for_test(&fm);
        let step = c.prepare_step_typed(&ctx).unwrap().unwrap();
        let bash = match &step {
            Step::Bash(b) => b,
            _ => panic!(),
        };
        match bash.env.get("AW_WORKITEM_MAX_ITEMS") {
            Some(EnvValue::Literal(s)) => assert_eq!(s, "10"),
            _ => panic!(),
        }
        match bash.env.get("AW_WORKITEM_MAX_BODY_KB") {
            Some(EnvValue::Literal(s)) => assert_eq!(s, "64"),
            _ => panic!(),
        }
    }

    #[test]
    fn synth_active_uses_hoisted_pr_id_and_succeeded_condition() {
        let fm = pr_fm();
        let c = WorkitemContextContributor::new(WorkitemContextConfig::default(), true, true);
        let ctx = CompileContext::for_test(&fm);
        let step = c.prepare_step_typed(&ctx).unwrap().unwrap();
        let bash = match &step {
            Step::Bash(b) => b,
            _ => panic!(),
        };
        match bash.env.get("SYSTEM_PULLREQUEST_PULLREQUESTID") {
            Some(EnvValue::PipelineVar(name)) => assert_eq!(name, "AW_PR_ID"),
            _ => panic!(),
        }
        assert!(matches!(bash.condition, Some(Condition::Succeeded)));
        // Bash gate present.
        assert!(
            bash.script
                .contains("if [ -z \"$SYSTEM_PULLREQUEST_PULLREQUESTID\" ]")
        );
    }

    #[test]
    fn bash_commands_is_empty() {
        let c = WorkitemContextContributor::new(WorkitemContextConfig::default(), false, true);
        assert!(c.bash_commands().is_empty());
    }
}
