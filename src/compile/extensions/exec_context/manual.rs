//! Manual execution-context contributor (Stage 1 of the
//! exec-context contributor build-out — see plan.md).
//!
//! Activates whenever the agent declares any `parameters:` block (and
//! the `execution-context.manual.enabled` switch is not `false`).
//! Runtime gate: `eq(variables['Build.Reason'], 'Manual')`.
//!
//! ## Artefacts (staged by the bundle on success)
//!
//! - `aw-context/manual/requested-for`       — `$(Build.RequestedFor)`
//!   display name
//! - `aw-context/manual/requested-for-email` — `$(Build.RequestedForEmail)`
//!   address (only when `manual.include-email: true`; absent otherwise)
//! - `aw-context/manual/parameters.json`     — pretty-printed JSON
//!   object of user-declared parameter values (the auto-injected
//!   `clearMemory` parameter is NOT included because it isn't in
//!   `front_matter.parameters` — see `src/compile/common.rs::build_parameters`).
//!
//! ## Prompt injection
//!
//! The bundle appends a short fragment under `## Manual run context`
//! to `/tmp/awf-tools/agent-prompt.md` summarising who queued the
//! run and (when present) the JSON parameter snapshot. Identifiers
//! (requestor name, parameter names) are interpolated literally; the
//! parameter VALUES come from user input at queue time and could
//! contain arbitrary characters, so the bundle sanitises them before
//! interpolation (single-line truncation; same posture as
//! `shared/validate.ts::sanitizeForPrompt`).
//!
//! ## Trust boundary
//!
//! No bearer, no network, no REST. All inputs come from ADO
//! predefined variables and template-expanded parameter values. The
//! step's `env:` block contains:
//!
//! - `BUILD_REQUESTEDFOR` — typed `EnvValue::AdoMacro("Build.RequestedFor")`
//! - `BUILD_REQUESTEDFOREMAIL` — same, only when `manual.include-email: true`
//! - `BUILD_SOURCESDIRECTORY` — typed AdoMacro; used to anchor
//!   `aw-context/` under the workspace
//! - `PARAM_<name>` — one entry per user-declared parameter, value
//!   is the literal template expression `${{ parameters.<name> }}`.
//!   The names are validated against `crate::validate::is_valid_parameter_name`
//!   at front-matter parse time, so the interpolated `<name>` is
//!   always identifier-shaped.
//!
//! `SYSTEM_ACCESSTOKEN` is intentionally NOT projected — this
//! contributor never touches REST.

use crate::compile::extensions::CompileContext;
use crate::compile::extensions::ado_script::EXEC_CONTEXT_MANUAL_PATH;
use crate::compile::ir::condition::{Condition, Expr};
use crate::compile::ir::env::EnvValue;
use crate::compile::ir::step::{BashStep, Step};
use crate::compile::types::ManualContextConfig;

#[cfg(test)]
use crate::compile::types::FrontMatter;

use super::contributor::ContextContributor;

/// Manual-context contributor.
pub(super) struct ManualContextContributor {
    config: ManualContextConfig,
    /// Snapshot of user-declared parameter names captured at construction
    /// time. Cloned from `front_matter.parameters` so the contributor is
    /// `'static` for the duration of a compile.
    ///
    /// Empty means "no parameters declared → contributor does not
    /// activate" (see [`ManualContextContributor::should_activate`]).
    parameter_names: Vec<String>,
}

impl ManualContextContributor {
    /// Construct the contributor from already-extracted parameter
    /// names. Used by [`super::ExecContextExtension::contributors`]
    /// so the extension does not need to hold a reference to the
    /// front matter (which would force a lifetime parameter on the
    /// extension type).
    ///
    /// Tests construct directly via this entry point — see
    /// `ManualContextContributor::from_fm` below for a convenience
    /// wrapper that extracts the names from a `FrontMatter`.
    pub(super) fn new_from_parts(
        config: ManualContextConfig,
        parameter_names: Vec<String>,
    ) -> Self {
        Self {
            config,
            parameter_names,
        }
    }

    /// Test-only convenience: extract parameter names from a
    /// `FrontMatter` and delegate to [`Self::new_from_parts`].
    #[cfg(test)]
    fn from_fm(config: ManualContextConfig, front_matter: &FrontMatter) -> Self {
        let parameter_names = front_matter
            .parameters
            .iter()
            .map(|p| p.name.clone())
            .collect();
        Self::new_from_parts(config, parameter_names)
    }
}

impl ContextContributor for ManualContextContributor {
    fn name(&self) -> &str {
        "manual"
    }

    fn should_activate(&self, _ctx: &CompileContext) -> bool {
        // MAINTENANCE: this MUST agree with
        // `super::manual_contributor_will_activate` on the
        // contributor-local conditions — i.e. "parameters declared"
        // AND "per-contributor `enabled` flag not Some(false)". The
        // master switch (`execution-context.enabled`) is enforced by
        // the outer `ExecContextExtension::declarations()` guard
        // (which short-circuits when the master switch is off) AND
        // by `manual_contributor_will_activate_with_cfg`, but is
        // intentionally absent here because the contributor only
        // sees a `CompileContext`, not the resolved config.
        // Divergence-trap tests in `super::tests` exercise both
        // paths to keep them aligned on the conditions they share.
        if self.parameter_names.is_empty() {
            return false;
        }
        match self.config.explicit_enabled() {
            Some(false) => false,
            Some(true) | None => true,
        }
    }

    fn prepare_step_typed(&self, ctx: &CompileContext) -> anyhow::Result<Option<Step>> {
        // Defensive: mirror the same guard pattern as every other
        // contributor — `declarations()` already gates on
        // `should_activate`, but this guard catches direct callers
        // (tests / future tooling) that bypass the outer filter.
        // Using the full `should_activate` predicate (rather than
        // just the `parameter_names.is_empty()` sub-check) ensures
        // the explicit `enabled: false` case is also caught.
        if !self.should_activate(ctx) {
            return Ok(None);
        }

        let script = format!("set -euo pipefail\nnode '{EXEC_CONTEXT_MANUAL_PATH}'\n");
        let mut step = BashStep::new(
            "Stage manual execution context (aw-context/manual/*)",
            script,
        )
        .with_condition(Condition::Eq(
            Expr::Variable("Build.Reason".to_string()),
            Expr::Literal("Manual".to_string()),
        ))
        .with_env(
            "BUILD_REQUESTEDFOR",
            EnvValue::ado_macro("Build.RequestedFor")?,
        )
        .with_env(
            "BUILD_SOURCESDIRECTORY",
            EnvValue::ado_macro("Build.SourcesDirectory")?,
        );

        // Email is opt-in for hygiene — see ManualContextConfig docs.
        if self.config.include_email_resolved() {
            step = step.with_env(
                "BUILD_REQUESTEDFOREMAIL",
                EnvValue::ado_macro("Build.RequestedForEmail")?,
            );
        }

        // One env var per user-declared parameter. The bundle scans
        // `process.env` for the `PARAM_` prefix and assembles the JSON
        // snapshot, so adding/removing a parameter at runtime needs no
        // bundle change. Parameter names are validated as ADO
        // identifiers upstream (during pipeline build —
        // `crate::compile::agentic_pipeline` calls
        // `crate::validate::is_valid_parameter_name`), so by the time
        // the contributor runs the front matter is well-formed.
        //
        // DEFENCE-IN-DEPTH: validate the name again here at the
        // contributor boundary. The cost is one regex match per
        // parameter; the benefit is that a future refactor that
        // reorders pipeline-build passes (or constructs the
        // contributor directly with a hand-built parameter list, as
        // some tests do) cannot smuggle a hostile name into the YAML
        // template expression `${{ parameters.<name> }}` or the
        // shell env-var name `PARAM_<name>`. Both would be
        // injection vectors if a hostile name reached the emitter.
        //
        // TRUST: parameter VALUES come from user input at queue time
        // and could contain arbitrary characters. They cross the
        // template-expansion → YAML → env-var pipeline as opaque
        // strings; the bundle sanitises them before any prompt
        // interpolation (see exec-context-manual/index.ts).
        for name in &self.parameter_names {
            if !crate::validate::is_valid_parameter_name(name) {
                anyhow::bail!(
                    "manual execution-context contributor: parameter name '{name}' \
                     is not a valid ADO identifier (must match \
                     [A-Za-z_][A-Za-z0-9_]*); refusing to emit \
                     `PARAM_{name}: ${{{{ parameters.{name} }}}}` env var. \
                     This indicates an upstream validation bypass — see \
                     `crate::validate::is_valid_parameter_name` and \
                     `compile::agentic_pipeline`'s parameter-validation pass."
                );
            }
            let var_name = format!("PARAM_{name}");
            let template_expr = format!("${{{{ parameters.{name} }}}}");
            step = step.with_env(var_name, EnvValue::literal(template_expr));
        }

        Ok(Some(Step::Bash(step)))
    }

    fn bash_commands(&self) -> Vec<String> {
        // No bash allow-list contributions — the agent reads the
        // staged files with the always-allowed `cat` / `ls` commands,
        // and the manual contributor never invokes `git` or any
        // network tool.
        vec![]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::extensions::CompileContext;
    use crate::compile::types::{FrontMatter, PipelineParameter};

    fn parse_fm(src: &str) -> FrontMatter {
        let (fm, _) = crate::compile::common::parse_markdown(src).unwrap();
        fm
    }

    fn manual_fm_with_params() -> FrontMatter {
        parse_fm(
            "---\n\
             name: test\n\
             description: test\n\
             parameters:\n  \
               - name: topic\n    \
                 type: string\n    \
                 default: foo\n  \
               - name: dryRun\n    \
                 type: boolean\n    \
                 default: false\n---\n",
        )
    }

    fn manual_fm_no_params() -> FrontMatter {
        parse_fm("---\nname: test\ndescription: test\n---\n")
    }

    #[test]
    fn should_not_activate_when_no_parameters() {
        let fm = manual_fm_no_params();
        let c = ManualContextContributor::from_fm(ManualContextConfig::default(), &fm);
        let ctx = CompileContext::for_test(&fm);
        assert!(
            !c.should_activate(&ctx),
            "manual contributor must not activate when no parameters are declared"
        );
    }

    #[test]
    fn should_activate_when_parameters_present_default_enabled() {
        let fm = manual_fm_with_params();
        let c = ManualContextContributor::from_fm(ManualContextConfig::default(), &fm);
        let ctx = CompileContext::for_test(&fm);
        assert!(c.should_activate(&ctx));
    }

    #[test]
    fn should_not_activate_when_explicitly_disabled() {
        let fm = manual_fm_with_params();
        let cfg = ManualContextConfig {
            enabled: Some(false),
            include_email: None,
        };
        let c = ManualContextContributor::from_fm(cfg, &fm);
        let ctx = CompileContext::for_test(&fm);
        assert!(!c.should_activate(&ctx));
    }

    #[test]
    fn prepare_step_emits_param_env_vars() {
        let fm = manual_fm_with_params();
        let c = ManualContextContributor::from_fm(ManualContextConfig::default(), &fm);
        let ctx = CompileContext::for_test(&fm);
        let step = c
            .prepare_step_typed(&ctx)
            .expect("prepare_step succeeds")
            .expect("contributor activates");
        let bash = match &step {
            Step::Bash(b) => b,
            other => panic!("expected Step::Bash, got {other:?}"),
        };

        // Condition: gated on Build.Reason == 'Manual'.
        match &bash.condition {
            Some(Condition::Eq(Expr::Variable(v), Expr::Literal(l))) => {
                assert_eq!(v, "Build.Reason");
                assert_eq!(l, "Manual");
            }
            other => panic!("expected eq(Build.Reason, 'Manual'), got {other:?}"),
        }

        // Requestor identity env vars present.
        assert!(matches!(
            bash.env.get("BUILD_REQUESTEDFOR"),
            Some(EnvValue::AdoMacro("Build.RequestedFor"))
        ));
        // Email is opt-in; default should NOT include it.
        assert!(
            !bash.env.contains_key("BUILD_REQUESTEDFOREMAIL"),
            "default config must NOT project Build.RequestedForEmail"
        );

        // One PARAM_<name> per declared parameter.
        match bash.env.get("PARAM_topic") {
            Some(EnvValue::Literal(s)) => assert_eq!(s, "${{ parameters.topic }}"),
            other => panic!("expected PARAM_topic literal template expr, got {other:?}"),
        }
        match bash.env.get("PARAM_dryRun") {
            Some(EnvValue::Literal(s)) => assert_eq!(s, "${{ parameters.dryRun }}"),
            other => panic!("expected PARAM_dryRun literal template expr, got {other:?}"),
        }

        // Trust boundary: NO bearer, NO REST identifiers.
        assert!(
            !bash.env.contains_key("SYSTEM_ACCESSTOKEN"),
            "manual contributor must NOT project SYSTEM_ACCESSTOKEN (no bearer needed)"
        );
        assert!(
            !bash.env.contains_key("SYSTEM_TEAMPROJECT"),
            "manual contributor must NOT project SYSTEM_TEAMPROJECT (no REST)"
        );
    }

    #[test]
    fn prepare_step_includes_email_when_opted_in() {
        let fm = manual_fm_with_params();
        let cfg = ManualContextConfig {
            enabled: None,
            include_email: Some(true),
        };
        let c = ManualContextContributor::from_fm(cfg, &fm);
        let ctx = CompileContext::for_test(&fm);
        let step = c
            .prepare_step_typed(&ctx)
            .expect("prepare_step succeeds")
            .expect("contributor activates");
        let bash = match &step {
            Step::Bash(b) => b,
            other => panic!("expected Step::Bash, got {other:?}"),
        };
        assert!(matches!(
            bash.env.get("BUILD_REQUESTEDFOREMAIL"),
            Some(EnvValue::AdoMacro("Build.RequestedForEmail"))
        ));
    }

    #[test]
    fn prepare_step_none_when_no_parameters() {
        let fm = manual_fm_no_params();
        let c = ManualContextContributor::from_fm(ManualContextConfig::default(), &fm);
        let ctx = CompileContext::for_test(&fm);
        // Direct call returns None even though `should_activate`
        // would also return false — defensive guard against misuse.
        assert!(c.prepare_step_typed(&ctx).unwrap().is_none());
    }

    /// Defensive guard parity test: when `enabled: Some(false)` is set
    /// explicitly AND parameters are non-empty, direct calls to
    /// `prepare_step_typed` MUST still return `Ok(None)`. Mirrors
    /// `workitem::tests::prepare_step_returns_none_when_inactive` —
    /// the guard now uses `!should_activate(ctx)` rather than just
    /// the `parameter_names.is_empty()` sub-check, so this case is
    /// covered.
    #[test]
    fn prepare_step_none_when_explicitly_disabled() {
        let fm = manual_fm_with_params();
        let cfg = ManualContextConfig {
            enabled: Some(false),
            include_email: None,
        };
        let c = ManualContextContributor::from_fm(cfg, &fm);
        let ctx = CompileContext::for_test(&fm);
        assert!(
            c.prepare_step_typed(&ctx).unwrap().is_none(),
            "manual contributor with enabled: Some(false) and non-empty \
             parameters MUST return Ok(None) from prepare_step_typed; \
             without the full should_activate guard, the no-bearer step \
             would be emitted as a live step bypassing the explicit \
             opt-out."
        );
    }

    #[test]
    fn bash_commands_is_empty() {
        // The manual contributor never invokes git or any tool that
        // needs an allow-list entry. Future review: if this changes,
        // the divergence-trap tests in `super::tests` should also be
        // updated.
        let fm = manual_fm_with_params();
        let c = ManualContextContributor::from_fm(ManualContextConfig::default(), &fm);
        assert!(c.bash_commands().is_empty());
    }

    /// Defensive: ensure that the contributor's emitted env var name
    /// `PARAM_<name>` and template expression `${{ parameters.<name> }}`
    /// cannot contain shell-injection characters. Parameter NAME
    /// validation lives upstream (`crate::validate::is_valid_parameter_name`,
    /// called by `compile::agentic_pipeline` during pipeline build),
    /// so by the time the contributor runs the front matter has been
    /// validated. As defence-in-depth, this test exercises the
    /// contributor with a parameter list assembled directly with
    /// hostile names and asserts the contributor itself REJECTS them
    /// at `prepare_step_typed` time — never emitting a step with
    /// a hostile env-var name into the YAML.
    #[test]
    fn hostile_parameter_name_rejected_by_contributor() {
        let cfg = ManualContextConfig::default();
        let c =
            ManualContextContributor::new_from_parts(cfg, vec!["evil-name; rm -rf /".to_string()]);
        let fm = manual_fm_no_params();
        let ctx = CompileContext::for_test(&fm);
        let result = c.prepare_step_typed(&ctx);
        assert!(
            result.is_err(),
            "manual contributor must reject parameter names containing \
             non-identifier characters; got Ok({result:?})"
        );
    }

    /// Construct the contributor directly with a hand-built parameter
    /// list. This bypasses front-matter parsing so we can exercise
    /// the constructor's behaviour without depending on validate.rs.
    /// The parameter NAME used here is identifier-shaped so the
    /// generated template expression is syntactically valid.
    #[test]
    fn constructor_preserves_parameter_names_order() {
        let mut fm = manual_fm_no_params();
        fm.parameters = vec![
            PipelineParameter {
                name: "alpha".to_string(),
                display_name: None,
                param_type: Some("string".to_string()),
                default: None,
                values: None,
            },
            PipelineParameter {
                name: "beta".to_string(),
                display_name: None,
                param_type: Some("string".to_string()),
                default: None,
                values: None,
            },
        ];
        let c = ManualContextContributor::from_fm(ManualContextConfig::default(), &fm);
        assert_eq!(c.parameter_names, vec!["alpha", "beta"]);
    }
}
