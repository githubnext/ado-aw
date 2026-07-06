//! PR-checks extension of the PR contributor (Stage 6 of the
//! exec-context contributor build-out — see plan.md).
//!
//! NOT a standalone contributor — it's logically part of the PR
//! contributor but operationally implemented as a separate prepare
//! step so the YAML emit is clean and the activation gate can stay
//! tight. Activates iff:
//!   1. The PR contributor activates, AND
//!   2. `execution-context.pr.checks.enabled: true` is set
//!      explicitly (opt-in, default OFF).
//!
//! Stages under `aw-context/pr/checks/`:
//!   - `failing.json`   — Build Validation runs whose result was not
//!     Succeeded (failed / partiallySucceeded / canceled)
//!   - `succeeded.json` — runs whose result was Succeeded
//!   - `error.txt`      — REST failure
//!
//! Runtime gate: same as the PR contributor's gate
//! (eq(Build.Reason, 'PullRequest')); for synthetic-from-CI runs,
//! same `AW_PR_ID`-empty-check gate.

use crate::compile::ado_bundle::{Bundle, TokenSource, apply_bundle_auth};
use crate::compile::extensions::CompileContext;
use crate::compile::extensions::ado_script::EXEC_CONTEXT_PR_CHECKS_PATH;
use crate::compile::ir::condition::{Condition, Expr};
use crate::compile::ir::env::EnvValue;
use crate::compile::ir::step::{BashStep, Step};
use crate::compile::types::PrChecksContextConfig;

use super::contributor::ContextContributor;

pub(super) struct PrChecksContextContributor {
    config: PrChecksContextConfig,
    /// `mode: synthetic` flag — drives env-var selection like the PR
    /// contributor.
    synthetic_pr_active: bool,
    /// PR-contributor-enabled flag (false when `pr.enabled: false`
    /// has explicitly opted out of PR context).
    pr_contributor_enabled: bool,
}

impl PrChecksContextContributor {
    pub(super) fn new(
        config: PrChecksContextConfig,
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

impl ContextContributor for PrChecksContextContributor {
    fn name(&self) -> &str {
        "pr.checks"
    }

    fn should_activate(&self, ctx: &CompileContext) -> bool {
        if ctx.front_matter.pr_trigger().is_none() {
            return false;
        }
        if !self.pr_contributor_enabled {
            return false;
        }
        self.config.is_enabled()
    }

    fn prepare_step_typed(&self, ctx: &CompileContext) -> anyhow::Result<Option<Step>> {
        // Defensive: mirror the manual.rs pattern — `declarations()`
        // already gates on `should_activate`, but this guard catches
        // direct callers (tests / future tooling). Returning `Ok(None)`
        // ensures no live step (with an active bearer) is emitted
        // when the contributor is inactive.
        if !self.should_activate(ctx) {
            return Ok(None);
        }
        // In synth mode the PR id is overridden from the hoisted AW_PR_ID job
        // variable and the step always runs (guarded by a runtime prelude);
        // in real-PR mode the auto-injected SYSTEM_PULLREQUEST_PULLREQUESTID is
        // read directly and the step gates on Build.Reason == PullRequest.
        let (condition, prelude) = if self.synthetic_pr_active {
            (
                Condition::Succeeded,
                "    if [ -z \"$SYSTEM_PULLREQUEST_PULLREQUESTID\" ]; then\n      echo \"[aw-context] No PR identifier resolved; skipping exec-context-pr-checks.\"\n      exit 0\n    fi\n",
            )
        } else {
            (
                Condition::Eq(
                    Expr::Variable("Build.Reason".to_string()),
                    Expr::Literal("PullRequest".to_string()),
                ),
                "",
            )
        };

        let script = format!("set -euo pipefail\n{prelude}node '{EXEC_CONTEXT_PR_CHECKS_PATH}'\n");
        // ADO auto-injects predefined System.*/Build.* context vars, so only
        // SYSTEM_ACCESSTOKEN (bearer, not auto-injected) and the mode-dependent
        // PR id are projected. In synth mode the id comes from the hoisted
        // AW_PR_ID job variable; in real-PR mode the auto-injected value is
        // used directly.
        let mut step = apply_bundle_auth(
            BashStep::new(
                "Stage PR-checks execution context (aw-context/pr/checks/*)",
                script,
            )
            .with_condition(condition),
            Bundle::ExecContextPrChecks,
            TokenSource::SystemAccessToken,
        );
        if self.synthetic_pr_active {
            step = step.with_env(
                "SYSTEM_PULLREQUEST_PULLREQUESTID",
                EnvValue::pipeline_var("AW_PR_ID"),
            );
        }
        Ok(Some(Step::Bash(step)))
    }

    fn bash_commands(&self) -> Vec<String> {
        // No new bash commands — staged JSON files are read with cat.
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
    fn defaults_to_disabled_even_on_pr_builds() {
        let fm = pr_fm();
        let c = PrChecksContextContributor::new(PrChecksContextConfig::default(), false, true);
        let ctx = CompileContext::for_test(&fm);
        assert!(!c.should_activate(&ctx));
    }

    #[test]
    fn activates_when_enabled_on_pr_with_pr_contributor_active() {
        let fm = pr_fm();
        let c = PrChecksContextContributor::new(
            PrChecksContextConfig {
                enabled: Some(true),
            },
            false,
            true,
        );
        let ctx = CompileContext::for_test(&fm);
        assert!(c.should_activate(&ctx));
    }

    #[test]
    fn does_not_activate_without_on_pr() {
        let fm = no_trigger_fm();
        let c = PrChecksContextContributor::new(
            PrChecksContextConfig {
                enabled: Some(true),
            },
            false,
            true,
        );
        let ctx = CompileContext::for_test(&fm);
        assert!(!c.should_activate(&ctx));
    }

    #[test]
    fn does_not_activate_when_pr_contributor_disabled() {
        let fm = pr_fm();
        let c = PrChecksContextContributor::new(
            PrChecksContextConfig {
                enabled: Some(true),
            },
            false,
            false, // pr_contributor_enabled = false
        );
        let ctx = CompileContext::for_test(&fm);
        assert!(!c.should_activate(&ctx));
    }

    #[test]
    fn prepare_step_carries_bearer_condition_and_pr_id() {
        // Non-synth path: condition must be eq(Build.Reason, 'PullRequest'),
        // PR ID env must be the plain ADO macro (not a pipeline var).
        let c = PrChecksContextContributor::new(
            PrChecksContextConfig {
                enabled: Some(true),
            },
            false, // synthetic_pr_active = false
            true,
        );
        let fm = pr_fm();
        let ctx = CompileContext::for_test(&fm);
        let step = c.prepare_step_typed(&ctx).unwrap().unwrap();
        let bash = match &step {
            Step::Bash(b) => b,
            _ => panic!(),
        };
        // Trust boundary: bearer must be present (secret via applier).
        assert!(matches!(
            bash.env.get("SYSTEM_ACCESSTOKEN"),
            Some(EnvValue::Secret(v)) if v == "System.AccessToken"
        ));
        // Runtime gate: step must only fire on PR builds.
        match bash.condition.as_ref().expect("condition required") {
            Condition::Eq(Expr::Variable(v), Expr::Literal(l)) => {
                assert_eq!(v, "Build.Reason");
                assert_eq!(l, "PullRequest");
            }
            other => panic!(
                "expected Condition::Eq(Variable(Build.Reason), Literal(PullRequest)), got {other:?}"
            ),
        }
        // Non-synth path: the bundle reads the auto-injected
        // SYSTEM_PULLREQUEST_PULLREQUESTID (and other context vars) directly,
        // so the step re-projects none of them.
        for stripped in [
            "SYSTEM_PULLREQUEST_PULLREQUESTID",
            "SYSTEM_COLLECTIONURI",
            "SYSTEM_TEAMPROJECT",
            "BUILD_BUILDID",
            "BUILD_SOURCESDIRECTORY",
        ] {
            assert!(
                bash.env.get(stripped).is_none(),
                "{stripped} is auto-injected and must not be re-projected"
            );
        }
    }
}
