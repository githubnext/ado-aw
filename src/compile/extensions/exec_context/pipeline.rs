//! Pipeline execution-context contributor (Stage 2 of the
//! exec-context contributor build-out — see plan.md).
//!
//! Activates whenever the agent declares an `on.pipeline` resource
//! trigger (and the `execution-context.pipeline.enabled` switch is
//! not `false`). Runtime gate:
//! `eq(variables['Build.Reason'], 'ResourceTrigger')`.
//!
//! ## Artefacts (staged by the bundle on success)
//!
//! - `aw-context/pipeline/upstream-build-id`       — numeric Build ID
//!   of the triggering upstream run
//! - `aw-context/pipeline/upstream-source-sha`     — `Build.sourceVersion`
//!   of the upstream
//! - `aw-context/pipeline/upstream-source-branch`  — `Build.sourceBranch`
//!   of the upstream
//! - `aw-context/pipeline/upstream-status`         — `succeeded`,
//!   `failed`, `partiallySucceeded`, `canceled` (the result-translated
//!   string from `BuildResult`)
//! - `aw-context/pipeline/upstream-definition`     — upstream pipeline
//!   definition name
//! - `aw-context/pipeline/upstream-artifacts.json` — `getArtifacts`
//!   output (artifact INDEX only — names + URLs; bytes NOT downloaded)
//!
//! On failure the bundle writes `aw-context/pipeline/error.txt` and
//! appends a tailored failure-fragment to the agent prompt.
//!
//! ## Trust boundary
//!
//! - `SYSTEM_ACCESSTOKEN` is mapped only into THIS step's `env:`
//!   block; never the agent step's env. The bundle uses it as the
//!   bearer for the Build REST API. The token is never written to
//!   disk, never logged, never passed in argv.
//! - The step is gated by
//!   `condition: eq(variables['Build.Reason'], 'ResourceTrigger')`
//!   so it never runs on non-pipeline-completion builds.
//! - All staged artefacts are short, structured ADO REST output —
//!   no user-controlled HTML, no free-text fields (the upstream
//!   pipeline's name is auditable infrastructure metadata, not
//!   PR-author-controlled).

use crate::compile::extensions::CompileContext;
use crate::compile::extensions::ado_script::EXEC_CONTEXT_PIPELINE_PATH;
use crate::compile::ir::condition::{Condition, Expr};
use crate::compile::ir::env::EnvValue;
use crate::compile::ir::step::{BashStep, Step};
use crate::compile::types::PipelineContextConfig;

use super::contributor::ContextContributor;

/// Pipeline-context contributor.
pub(super) struct PipelineContextContributor {
    config: PipelineContextConfig,
}

impl PipelineContextContributor {
    pub(super) fn new(config: PipelineContextConfig) -> Self {
        Self { config }
    }
}

impl ContextContributor for PipelineContextContributor {
    fn name(&self) -> &str {
        "pipeline"
    }

    fn should_activate(&self, ctx: &CompileContext) -> bool {
        // MAINTENANCE: must stay in lock-step with
        // `super::pipeline_contributor_will_activate` (used by
        // `ExecContextExtension::new` to populate
        // `any_contributor_active`).
        if ctx.front_matter.pipeline_trigger().is_none() {
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
        // when the contributor is inactive.
        if !self.should_activate(ctx) {
            return Ok(None);
        }
        let script = format!("set -euo pipefail\nnode '{EXEC_CONTEXT_PIPELINE_PATH}'\n");
        let step = BashStep::new(
            "Stage pipeline execution context (aw-context/pipeline/*)",
            script,
        )
        .with_condition(Condition::Eq(
            Expr::Variable("Build.Reason".to_string()),
            Expr::Literal("ResourceTrigger".to_string()),
        ))
        .with_env(
            "SYSTEM_ACCESSTOKEN",
            EnvValue::ado_macro("System.AccessToken")?,
        )
        .with_env(
            "SYSTEM_COLLECTIONURI",
            EnvValue::ado_macro("System.CollectionUri")?,
        )
        .with_env(
            "BUILD_SOURCESDIRECTORY",
            EnvValue::ado_macro("Build.SourcesDirectory")?,
        )
        // Upstream-build identifiers populated by ADO when the build
        // was triggered via `resources.pipelines`. The bundle uses
        // these to look up the upstream Build via the REST API.
        // `Build.TriggeredBy.ProjectID` carries the project that owns
        // the upstream pipeline (may differ from the consumer's
        // project for cross-project triggers — ADO handles the
        // routing natively).
        .with_env(
            "BUILD_TRIGGEREDBY_BUILDID",
            EnvValue::ado_macro("Build.TriggeredBy.BuildId")?,
        )
        .with_env(
            "BUILD_TRIGGEREDBY_DEFINITIONID",
            EnvValue::ado_macro("Build.TriggeredBy.DefinitionId")?,
        )
        .with_env(
            "BUILD_TRIGGEREDBY_DEFINITIONNAME",
            EnvValue::ado_macro("Build.TriggeredBy.DefinitionName")?,
        )
        .with_env(
            "BUILD_TRIGGEREDBY_PROJECTID",
            EnvValue::ado_macro("Build.TriggeredBy.ProjectID")?,
        );
        Ok(Some(Step::Bash(step)))
    }

    fn bash_commands(&self) -> Vec<String> {
        // The agent reads the staged files with the already-permitted
        // `cat` / `jq` (if installed) — no git / no REST tooling
        // needed. The pipeline contributor adds nothing to the
        // agent's bash allow-list.
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

    fn pipeline_fm() -> FrontMatter {
        parse_fm(
            "---\n\
             name: test\n\
             description: test\n\
             on:\n  \
               pipeline:\n    \
                 name: upstream\n---\n",
        )
    }

    fn no_trigger_fm() -> FrontMatter {
        parse_fm("---\nname: test\ndescription: test\n---\n")
    }

    #[test]
    fn should_not_activate_without_on_pipeline() {
        let fm = no_trigger_fm();
        let c = PipelineContextContributor::new(PipelineContextConfig::default());
        let ctx = CompileContext::for_test(&fm);
        assert!(!c.should_activate(&ctx));
    }

    #[test]
    fn should_activate_when_on_pipeline_configured() {
        let fm = pipeline_fm();
        let c = PipelineContextContributor::new(PipelineContextConfig::default());
        let ctx = CompileContext::for_test(&fm);
        assert!(c.should_activate(&ctx));
    }

    #[test]
    fn should_not_activate_when_explicitly_disabled() {
        let fm = pipeline_fm();
        let c =
            PipelineContextContributor::new(PipelineContextConfig { enabled: Some(false) });
        let ctx = CompileContext::for_test(&fm);
        assert!(!c.should_activate(&ctx));
    }

    #[test]
    fn prepare_step_carries_bearer_and_triggered_by_envs() {
        let fm = pipeline_fm();
        let c = PipelineContextContributor::new(PipelineContextConfig::default());
        let ctx = CompileContext::for_test(&fm);
        let step = c.prepare_step_typed(&ctx).unwrap().unwrap();
        let bash = match &step {
            Step::Bash(b) => b,
            other => panic!("expected Step::Bash, got {other:?}"),
        };

        // Runtime gate.
        match &bash.condition {
            Some(Condition::Eq(Expr::Variable(v), Expr::Literal(l))) => {
                assert_eq!(v, "Build.Reason");
                assert_eq!(l, "ResourceTrigger");
            }
            other => panic!("expected eq(Build.Reason, 'ResourceTrigger'), got {other:?}"),
        }

        // Bearer present.
        assert!(matches!(
            bash.env.get("SYSTEM_ACCESSTOKEN"),
            Some(EnvValue::AdoMacro("System.AccessToken"))
        ));
        // Collection URI for REST endpoint construction.
        assert!(matches!(
            bash.env.get("SYSTEM_COLLECTIONURI"),
            Some(EnvValue::AdoMacro("System.CollectionUri"))
        ));
        // TriggeredBy quartet.
        for (env_key, ado_var) in [
            ("BUILD_TRIGGEREDBY_BUILDID", "Build.TriggeredBy.BuildId"),
            (
                "BUILD_TRIGGEREDBY_DEFINITIONID",
                "Build.TriggeredBy.DefinitionId",
            ),
            (
                "BUILD_TRIGGEREDBY_DEFINITIONNAME",
                "Build.TriggeredBy.DefinitionName",
            ),
            (
                "BUILD_TRIGGEREDBY_PROJECTID",
                "Build.TriggeredBy.ProjectID",
            ),
        ] {
            match bash.env.get(env_key) {
                Some(EnvValue::AdoMacro(name)) => assert_eq!(*name, ado_var),
                other => panic!("expected {env_key} -> AdoMacro({ado_var}), got {other:?}"),
            }
        }
    }

    #[test]
    fn bash_commands_is_empty() {
        let c = PipelineContextContributor::new(PipelineContextConfig::default());
        assert!(c.bash_commands().is_empty());
    }
}
