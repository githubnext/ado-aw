//! Schedule execution-context contributor (Stage 5 of the
//! exec-context contributor build-out — see plan.md).
//!
//! Stages "since last run of this pipeline on this branch" diff
//! context for scheduled builds. Default-OFF (opt-in via
//! `execution-context.schedule.enabled: true`).
//!
//! Activation: purely config-driven (default OFF) AND `on.schedule`
//! is configured. Runtime gate:
//! `eq(variables['Build.Reason'], 'Schedule')`.
//!
//! Reuses `shared/build.ts::listLastSuccessfulBuildOnBranch` (added
//! in Stage 2) plus `shared/git.ts` deepening (Stage 0) — so this
//! contributor's TS bundle is a thin variation on
//! `exec-context-ci-push`.

use crate::compile::extensions::CompileContext;
use crate::compile::ado_bundle::{Bundle, TokenSource, apply_bundle_auth};
use crate::compile::extensions::ado_script::EXEC_CONTEXT_SCHEDULE_PATH;
use crate::compile::ir::condition::{Condition, Expr};
use crate::compile::ir::step::{BashStep, Step};
use crate::compile::types::ScheduleContextConfig;

use super::contributor::ContextContributor;

pub(super) struct ScheduleContextContributor {
    config: ScheduleContextConfig,
}

impl ScheduleContextContributor {
    pub(super) fn new(config: ScheduleContextConfig) -> Self {
        Self { config }
    }
}

impl ContextContributor for ScheduleContextContributor {
    fn name(&self) -> &str {
        "schedule"
    }

    fn should_activate(&self, ctx: &CompileContext) -> bool {
        // Opt-in only AND requires `on.schedule` to be declared
        // (otherwise the runtime gate is dead and we waste a step
        // slot on every non-scheduled build).
        if ctx.front_matter.schedule().is_none() {
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
        let script = format!("set -euo pipefail\nnode '{EXEC_CONTEXT_SCHEDULE_PATH}'\n");
        // ADO auto-injects the predefined System.*/Build.* context variables
        // into the step env, so the bundle reads them directly; only the
        // non-auto-injected SYSTEM_ACCESSTOKEN bearer is projected here.
        let step = apply_bundle_auth(
            BashStep::new(
                "Stage schedule execution context (aw-context/schedule/*)",
                script,
            )
            .with_condition(Condition::Eq(
                Expr::Variable("Build.Reason".to_string()),
                Expr::Literal("Schedule".to_string()),
            )),
            Bundle::ExecContextSchedule,
            TokenSource::SystemAccessToken,
        );
        Ok(Some(Step::Bash(step)))
    }

    fn bash_commands(&self) -> Vec<String> {
        // Same seven read-only git commands as ci-push / PR — the
        // agent uses them to inspect the staged commit range.
        vec![
            "git".to_string(),
            "git diff".to_string(),
            "git log".to_string(),
            "git show".to_string(),
            "git status".to_string(),
            "git rev-parse".to_string(),
            "git symbolic-ref".to_string(),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::ir::env::EnvValue;
    use crate::compile::extensions::CompileContext;
    use crate::compile::types::FrontMatter;

    fn parse_fm(src: &str) -> FrontMatter {
        let (fm, _) = crate::compile::common::parse_markdown(src).unwrap();
        fm
    }

    fn schedule_fm() -> FrontMatter {
        parse_fm(
            "---\nname: test\ndescription: test\non:\n  schedule: 'daily around 09:00 UTC'\n---\n",
        )
    }

    fn no_trigger_fm() -> FrontMatter {
        parse_fm("---\nname: test\ndescription: test\n---\n")
    }

    #[test]
    fn defaults_to_disabled() {
        let fm = schedule_fm();
        let c = ScheduleContextContributor::new(ScheduleContextConfig::default());
        let ctx = CompileContext::for_test(&fm);
        assert!(!c.should_activate(&ctx));
    }

    #[test]
    fn does_not_activate_without_on_schedule() {
        let fm = no_trigger_fm();
        let c = ScheduleContextContributor::new(ScheduleContextConfig {
            enabled: Some(true),
        });
        let ctx = CompileContext::for_test(&fm);
        assert!(!c.should_activate(&ctx));
    }

    #[test]
    fn activates_when_enabled_and_on_schedule() {
        let fm = schedule_fm();
        let c = ScheduleContextContributor::new(ScheduleContextConfig {
            enabled: Some(true),
        });
        let ctx = CompileContext::for_test(&fm);
        assert!(c.should_activate(&ctx));
    }

    #[test]
    fn prepare_step_runtime_gates_on_build_reason_schedule() {
        let fm = schedule_fm();
        let c = ScheduleContextContributor::new(ScheduleContextConfig {
            enabled: Some(true),
        });
        let ctx = CompileContext::for_test(&fm);
        let step = c.prepare_step_typed(&ctx).unwrap().unwrap();
        let bash = match &step {
            Step::Bash(b) => b,
            other => panic!("expected Bash, got {other:?}"),
        };
        match &bash.condition {
            Some(Condition::Eq(Expr::Variable(v), Expr::Literal(l))) => {
                assert_eq!(v, "Build.Reason");
                assert_eq!(l, "Schedule");
            }
            other => panic!("expected eq(Build.Reason, 'Schedule'), got {other:?}"),
        }
        assert!(matches!(
            bash.env.get("SYSTEM_ACCESSTOKEN"),
            Some(EnvValue::Secret(v)) if v == "System.AccessToken"
        ));
        // Predefined System.*/Build.* context vars are auto-injected by ADO;
        // the step must not re-project them.
        for stripped in [
            "SYSTEM_COLLECTIONURI",
            "SYSTEM_TEAMPROJECT",
            "SYSTEM_DEFINITIONID",
            "BUILD_BUILDID",
            "BUILD_SOURCESDIRECTORY",
            "BUILD_SOURCEVERSION",
            "BUILD_SOURCEBRANCH",
        ] {
            assert!(
                bash.env.get(stripped).is_none(),
                "{stripped} is auto-injected and must not be re-projected"
            );
        }
    }

    #[test]
    fn bash_commands_includes_read_only_git_set() {
        let c = ScheduleContextContributor::new(ScheduleContextConfig::default());
        let cmds = c.bash_commands();
        // The schedule contributor advertises the same seven read-only git
        // commands as ci-push and PR contributors (per the code comment in
        // bash_commands()). Verify all seven are present.
        for expected in &[
            "git",
            "git diff",
            "git log",
            "git show",
            "git status",
            "git rev-parse",
            "git symbolic-ref",
        ] {
            assert!(
                cmds.contains(&expected.to_string()),
                "'{expected}' missing from bash_commands: {cmds:?}"
            );
        }
    }
}
