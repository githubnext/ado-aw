//! Repo execution-context contributor (Stage 7 of the exec-context
//! contributor build-out — see plan.md).
//!
//! Always-on capability: stages repository identity info (branch,
//! SHA, last release tag, commits-since-tag). Defaults to OFF to
//! avoid prompt-clutter regression for agents that already get
//! sufficient repo identity from PR / ci-push / pipeline
//! contributors.
//!
//! Runtime gate: none (the contributor's content is useful on any
//! build reason). Activation is purely config-driven (opt-in,
//! default OFF).
//!
//! No bearer, no network — pure `git` against the local workspace.

use crate::compile::extensions::CompileContext;
use crate::compile::extensions::ado_script::EXEC_CONTEXT_REPO_PATH;
use crate::compile::ir::condition::Condition;
use crate::compile::ir::env::EnvValue;
use crate::compile::ir::step::{BashStep, Step};
use crate::compile::types::RepoContextConfig;

use super::contributor::ContextContributor;

pub(super) struct RepoContextContributor {
    config: RepoContextConfig,
}

impl RepoContextContributor {
    pub(super) fn new(config: RepoContextConfig) -> Self {
        Self { config }
    }
}

impl ContextContributor for RepoContextContributor {
    fn name(&self) -> &str {
        "repo"
    }

    fn should_activate(&self, _ctx: &CompileContext) -> bool {
        self.config.is_enabled()
    }

    fn prepare_step_typed(&self, _ctx: &CompileContext) -> anyhow::Result<Option<Step>> {
        let script = format!("set -euo pipefail\nnode '{EXEC_CONTEXT_REPO_PATH}'\n");
        let step = BashStep::new(
            "Stage repo execution context (aw-context/repo/*)",
            script,
        )
        // Always-on (no Build.Reason gate). The compile-time
        // activation flag is the only gate.
        .with_condition(Condition::Succeeded)
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
        )
        .with_env(
            "AW_REPO_CONVENTIONS",
            EnvValue::literal(self.config.conventions_enabled().to_string()),
        );
        Ok(Some(Step::Bash(step)))
    }

    fn bash_commands(&self) -> Vec<String> {
        // git describe / git log / git rev-parse for the staging
        // step; agent reads the staged files via cat.
        vec![
            "git".to_string(),
            "git log".to_string(),
            "git rev-parse".to_string(),
            "git describe".to_string(),
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
        let c = RepoContextContributor::new(RepoContextConfig::default());
        let ctx = CompileContext::for_test(&fm);
        assert!(!c.should_activate(&ctx));
    }

    #[test]
    fn activates_when_enabled() {
        let fm = minimal_fm();
        let c = RepoContextContributor::new(RepoContextConfig {
            enabled: Some(true),
            conventions: None,
        });
        let ctx = CompileContext::for_test(&fm);
        assert!(c.should_activate(&ctx));
    }

    #[test]
    fn prepare_step_carries_no_bearer_and_passes_conventions_flag() {
        let fm = minimal_fm();
        let c = RepoContextContributor::new(RepoContextConfig {
            enabled: Some(true),
            conventions: Some(true),
        });
        let ctx = CompileContext::for_test(&fm);
        let step = c.prepare_step_typed(&ctx).unwrap().unwrap();
        let bash = match &step {
            Step::Bash(b) => b,
            _ => panic!(),
        };
        // No bearer — repo contributor is pure git, no REST.
        assert!(
            !bash.env.contains_key("SYSTEM_ACCESSTOKEN"),
            "repo contributor MUST NOT project SYSTEM_ACCESSTOKEN"
        );
        // Conventions flag plumbed through.
        match bash.env.get("AW_REPO_CONVENTIONS") {
            Some(EnvValue::Literal(s)) => assert_eq!(s, "true"),
            other => panic!("expected literal 'true', got {other:?}"),
        }
    }

    #[test]
    fn bash_commands_lists_git_describe() {
        let c = RepoContextContributor::new(RepoContextConfig::default());
        assert!(c.bash_commands().contains(&"git describe".to_string()));
    }
}
