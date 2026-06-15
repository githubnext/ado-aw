//! CI-push execution-context contributor (Stage 3 of the exec-context
//! contributor build-out — see plan.md).
//!
//! Stages "since last green build on this branch" diff context for
//! non-PR push builds. Activates only when
//! `execution-context.ci-push.enabled: true` (opt-in, default OFF) —
//! the contributor does ADO REST + git fetch deepening work that
//! adds startup latency, so most agents shouldn't pay for it.
//!
//! Runtime gate: `or(eq(Build.Reason, 'IndividualCI'),
//! eq(Build.Reason, 'BatchedCI'))`. Skips PRs, scheduled runs, and
//! resource triggers at zero cost.
//!
//! ## Artefacts (staged by the bundle on success)
//!
//! - `aw-context/ci-push/current-sha`       — `Build.SourceVersion`
//! - `aw-context/ci-push/previous-sha`      — SHA of the last
//!   successful build of this pipeline on this branch (resolved via
//!   `shared/build.ts::listLastSuccessfulBuildOnBranch`)
//! - `aw-context/ci-push/base.sha`          — `git merge-base`
//!   between previous and current (usually `previous` itself; differs
//!   if intervening rebases or non-linear history are involved)
//! - `aw-context/ci-push/commits.txt`       — `git log previous..current --oneline`
//! - `aw-context/ci-push/changed-files.txt` — `git diff --name-status previous..current`
//! - `aw-context/ci-push/error.txt`         — present only on failure
//!
//! ## Trust boundary
//!
//! Bearer required for both:
//!   - the ADO Build REST API lookup ("last successful build")
//!   - the git fetch deepening (to reach the previous SHA if the
//!     workspace's shallow clone doesn't already include it)
//!
//! `SYSTEM_ACCESSTOKEN` is mapped only into this step's `env:` block;
//! never to the agent step's env. Same posture as the PR contributor.
//!
//! ## Failure modes
//!
//! - **No previous successful build** — first ever push for this
//!   branch, or all previous builds failed, or last green build was
//!   pruned by ADO retention. The bundle stages `error.txt` and
//!   appends a failure fragment telling the agent NOT to claim
//!   "diff is empty, ship it" when the diff couldn't be resolved.
//! - **Depth-budget exhausted** — `previous` SHA is older than the
//!   deepening budget can reach. Same failure fragment.
//! - **REST failure** — Build API returned an error. Same.

use crate::compile::extensions::CompileContext;
use crate::compile::extensions::ado_script::EXEC_CONTEXT_CI_PUSH_PATH;
use crate::compile::ir::condition::{Condition, Expr};
use crate::compile::ir::env::EnvValue;
use crate::compile::ir::step::{BashStep, Step};
use crate::compile::types::CiPushContextConfig;

use super::contributor::ContextContributor;

/// CI-push-context contributor.
pub(super) struct CiPushContextContributor {
    config: CiPushContextConfig,
}

impl CiPushContextContributor {
    pub(super) fn new(config: CiPushContextConfig) -> Self {
        Self { config }
    }
}

impl ContextContributor for CiPushContextContributor {
    fn name(&self) -> &str {
        "ci-push"
    }

    fn should_activate(&self, _ctx: &CompileContext) -> bool {
        // No trigger predicate — ci-push activation is purely
        // config-driven (opt-in). Runtime gate (eq(Build.Reason,
        // IndividualCI/BatchedCI)) means the step is a no-op on
        // non-CI builds even when activated.
        //
        // MAINTENANCE: this MUST stay in lock-step with
        // `super::ci_push_contributor_will_activate`.
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
        let script = format!("set -euo pipefail\nnode '{EXEC_CONTEXT_CI_PUSH_PATH}'\n");
        let step = BashStep::new(
            "Stage ci-push execution context (aw-context/ci-push/*)",
            script,
        )
        .with_condition(Condition::Or(vec![
            Condition::Eq(
                Expr::Variable("Build.Reason".to_string()),
                Expr::Literal("IndividualCI".to_string()),
            ),
            Condition::Eq(
                Expr::Variable("Build.Reason".to_string()),
                Expr::Literal("BatchedCI".to_string()),
            ),
        ]))
        .with_env(
            "SYSTEM_ACCESSTOKEN",
            EnvValue::ado_macro("System.AccessToken")?,
        )
        .with_env(
            "SYSTEM_COLLECTIONURI",
            EnvValue::ado_macro("System.CollectionUri")?,
        )
        .with_env(
            "SYSTEM_TEAMPROJECT",
            EnvValue::ado_macro("System.TeamProject")?,
        )
        .with_env(
            "SYSTEM_DEFINITIONID",
            EnvValue::ado_macro("System.DefinitionId")?,
        )
        .with_env("BUILD_BUILDID", EnvValue::ado_macro("Build.BuildId")?)
        .with_env(
            "BUILD_SOURCESDIRECTORY",
            EnvValue::ado_macro("Build.SourcesDirectory")?,
        )
        .with_env(
            "BUILD_SOURCEVERSION",
            EnvValue::ado_macro("Build.SourceVersion")?,
        )
        .with_env(
            "BUILD_SOURCEBRANCH",
            EnvValue::ado_macro("Build.SourceBranch")?,
        );
        Ok(Some(Step::Bash(step)))
    }

    fn bash_commands(&self) -> Vec<String> {
        // Same seven read-only git commands as the PR contributor —
        // the agent runs `git diff $BASE..$HEAD` and friends to
        // inspect the staged commit range.
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
    use crate::compile::extensions::CompileContext;
    use crate::compile::types::FrontMatter;

    fn parse_fm(src: &str) -> FrontMatter {
        let (fm, _) = crate::compile::common::parse_markdown(src).unwrap();
        fm
    }

    fn minimal_fm() -> FrontMatter {
        parse_fm("---\nname: test\ndescription: test\n---\n")
    }

    #[test]
    fn defaults_to_disabled() {
        let fm = minimal_fm();
        let c = CiPushContextContributor::new(CiPushContextConfig::default());
        let ctx = CompileContext::for_test(&fm);
        assert!(!c.should_activate(&ctx), "ci-push must default to OFF");
    }

    #[test]
    fn activates_when_explicitly_enabled() {
        let fm = minimal_fm();
        let c = CiPushContextContributor::new(CiPushContextConfig {
            enabled: Some(true),
        });
        let ctx = CompileContext::for_test(&fm);
        assert!(c.should_activate(&ctx));
    }

    #[test]
    fn prepare_step_emits_or_individualci_batchedci_condition() {
        let fm = minimal_fm();
        let c = CiPushContextContributor::new(CiPushContextConfig {
            enabled: Some(true),
        });
        let ctx = CompileContext::for_test(&fm);
        let step = c.prepare_step_typed(&ctx).unwrap().unwrap();
        let bash = match &step {
            Step::Bash(b) => b,
            other => panic!("expected Bash, got {other:?}"),
        };
        // Condition: or(eq(Build.Reason, IndividualCI),
        //              eq(Build.Reason, BatchedCI))
        match &bash.condition {
            Some(Condition::Or(clauses)) => {
                assert_eq!(clauses.len(), 2);
                let reasons: Vec<&str> = clauses
                    .iter()
                    .map(|c| match c {
                        Condition::Eq(Expr::Variable(_), Expr::Literal(l)) => l.as_str(),
                        _ => panic!("expected Eq clause, got {c:?}"),
                    })
                    .collect();
                assert!(reasons.contains(&"IndividualCI"));
                assert!(reasons.contains(&"BatchedCI"));
            }
            other => panic!("expected Or condition, got {other:?}"),
        }

        // Trust boundary: bearer present.
        assert!(matches!(
            bash.env.get("SYSTEM_ACCESSTOKEN"),
            Some(EnvValue::AdoMacro("System.AccessToken"))
        ));
        // Identifiers needed for the REST + git fetch.
        assert!(matches!(
            bash.env.get("SYSTEM_DEFINITIONID"),
            Some(EnvValue::AdoMacro("System.DefinitionId"))
        ));
        assert!(matches!(
            bash.env.get("BUILD_SOURCEVERSION"),
            Some(EnvValue::AdoMacro("Build.SourceVersion"))
        ));
        assert!(matches!(
            bash.env.get("BUILD_SOURCEBRANCH"),
            Some(EnvValue::AdoMacro("Build.SourceBranch"))
        ));
    }

    #[test]
    fn bash_commands_includes_read_only_git_set() {
        let c = CiPushContextContributor::new(CiPushContextConfig {
            enabled: Some(true),
        });
        let cmds = c.bash_commands();
        assert!(cmds.contains(&"git diff".to_string()));
        assert!(cmds.contains(&"git log".to_string()));
        assert!(cmds.contains(&"git show".to_string()));
        assert!(cmds.contains(&"git status".to_string()));
        assert!(cmds.contains(&"git rev-parse".to_string()));
        assert!(cmds.contains(&"git symbolic-ref".to_string()));
    }
}
