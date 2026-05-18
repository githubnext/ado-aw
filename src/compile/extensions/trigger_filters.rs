//! Trigger filters compiler extension.
//!
//! Activates when any `filters:` configuration is present under `on.pr`
//! or `on.pipeline`. Injects the gate step that evaluates the filter
//! spec via the Node evaluator into the Setup job, and declares a need
//! for the shared `ado-script.zip` bundle so the compiler emits a
//! single `NodeTool@0` + download/extract pair shared with any other
//! bundle consumers (e.g., the runtime prompt renderer).
//!
//! All filter types (simple and complex) are evaluated by the Node
//! evaluator — there is no inline bash codegen path.

use anyhow::Result;

use super::{CompileContext, CompilerExtension, ExtensionPhase};
use crate::compile::filter_ir::{
    GateContext, Severity, compile_gate_step_external, lower_pipeline_filters, lower_pr_filters,
    validate_pipeline_filters, validate_pr_filters,
};
use crate::compile::types::{PipelineFilters, PrFilters};

/// The path where the gate evaluator is invoked at pipeline runtime.
const GATE_EVAL_PATH: &str = "/tmp/ado-aw-scripts/gate.js";

/// Compiler extension that delivers and runs the gate evaluator for
/// complex trigger filters.
pub struct TriggerFiltersExtension {
    pr_filters: Option<PrFilters>,
    pipeline_filters: Option<PipelineFilters>,
}

impl TriggerFiltersExtension {
    pub fn new(pr_filters: Option<PrFilters>, pipeline_filters: Option<PipelineFilters>) -> Self {
        Self {
            pr_filters,
            pipeline_filters,
        }
    }

    /// Returns true if any filter configuration produces actual checks.
    pub fn is_needed(
        pr_filters: Option<&PrFilters>,
        pipeline_filters: Option<&PipelineFilters>,
    ) -> bool {
        let has_pr = pr_filters
            .map(|f| !lower_pr_filters(f).is_empty())
            .unwrap_or(false);
        let has_pipeline = pipeline_filters
            .map(|f| !lower_pipeline_filters(f).is_empty())
            .unwrap_or(false);
        has_pr || has_pipeline
    }
}

impl CompilerExtension for TriggerFiltersExtension {
    fn name(&self) -> &str {
        "trigger-filters"
    }

    fn phase(&self) -> ExtensionPhase {
        ExtensionPhase::Tool
    }

    fn setup_steps(&self, _ctx: &CompileContext) -> Result<Vec<String>> {
        let mut gate_steps = Vec::new();

        // PR gate step
        if let Some(filters) = &self.pr_filters {
            let checks = lower_pr_filters(filters);
            if !checks.is_empty() {
                gate_steps.push(compile_gate_step_external(
                    GateContext::PullRequest,
                    &checks,
                    GATE_EVAL_PATH,
                )?);
            }
        }

        // Pipeline gate step
        if let Some(filters) = &self.pipeline_filters {
            let checks = lower_pipeline_filters(filters);
            if !checks.is_empty() {
                gate_steps.push(compile_gate_step_external(
                    GateContext::PipelineCompletion,
                    &checks,
                    GATE_EVAL_PATH,
                )?);
            }
        }

        // The NodeTool@0 + scripts-download steps are NOT emitted here:
        // they are inserted once by the compiler when any extension
        // returns `needs_scripts_bundle() == true`. See
        // `super::scripts_install_steps_if_needed`.
        Ok(gate_steps)
    }

    fn needs_scripts_bundle(&self) -> bool {
        // The bundle is only needed if we actually emit a gate step.
        let has_pr = self
            .pr_filters
            .as_ref()
            .map(|f| !lower_pr_filters(f).is_empty())
            .unwrap_or(false);
        let has_pipeline = self
            .pipeline_filters
            .as_ref()
            .map(|f| !lower_pipeline_filters(f).is_empty())
            .unwrap_or(false);
        has_pr || has_pipeline
    }

    fn validate(&self, _ctx: &CompileContext) -> Result<Vec<String>> {
        let mut warnings = Vec::new();

        if let Some(f) = &self.pr_filters {
            for diag in validate_pr_filters(f) {
                match diag.severity {
                    Severity::Error => anyhow::bail!("{}", diag),
                    Severity::Warning | Severity::Info => {
                        warnings.push(format!("{}", diag));
                    }
                }
            }
        }

        if let Some(f) = &self.pipeline_filters {
            for diag in validate_pipeline_filters(f) {
                match diag.severity {
                    Severity::Error => anyhow::bail!("{}", diag),
                    Severity::Warning | Severity::Info => {
                        warnings.push(format!("{}", diag));
                    }
                }
            }
        }

        Ok(warnings)
    }

    fn required_hosts(&self) -> Vec<String> {
        vec!["github.com".to_string()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::extensions::CompileContext;
    use crate::compile::types::*;

    #[test]
    fn test_is_needed_any_filters() {
        // Any filters configuration activates the extension
        let filters = PrFilters {
            title: Some(PatternFilter {
                pattern: "test".into(),
            }),
            ..Default::default()
        };
        assert!(
            TriggerFiltersExtension::is_needed(Some(&filters), None),
            "Any filters should activate extension"
        );
    }

    #[test]
    fn test_is_not_needed_without_filters() {
        assert!(
            !TriggerFiltersExtension::is_needed(None, None),
            "No filters should not activate extension"
        );
    }

    #[test]
    fn test_is_needed_draft() {
        let filters = PrFilters {
            draft: Some(false),
            ..Default::default()
        };
        assert!(
            TriggerFiltersExtension::is_needed(Some(&filters), None),
            "Draft filter should need evaluator"
        );
    }

    #[test]
    fn test_is_needed_time_window() {
        let filters = PrFilters {
            time_window: Some(TimeWindowFilter {
                start: "09:00".into(),
                end: "17:00".into(),
            }),
            ..Default::default()
        };
        assert!(
            TriggerFiltersExtension::is_needed(Some(&filters), None),
            "Time window should need evaluator"
        );
    }

    #[test]
    fn test_setup_steps_emits_only_gate_step() {
        // After dedupe, setup_steps emits ONLY the gate step. The
        // NodeTool@0 + scripts-download pair is hoisted into a
        // compiler-level emission gated on needs_scripts_bundle().
        let filters = PrFilters {
            labels: Some(LabelFilter {
                any_of: vec!["run-agent".into()],
                ..Default::default()
            }),
            ..Default::default()
        };
        let ext = TriggerFiltersExtension::new(Some(filters), None);
        let yaml = "name: test\ndescription: test";
        let fm: FrontMatter = serde_yaml::from_str(yaml).unwrap();
        let ctx = CompileContext::for_test(&fm);
        let steps = ext.setup_steps(&ctx).unwrap();
        assert_eq!(
            steps.len(),
            1,
            "setup_steps should only emit the gate step now that install is dedup'd: {steps:?}"
        );
        assert!(
            steps[0].contains("prGate"),
            "step should be the PR gate"
        );
        assert!(
            steps[0].contains("node '/tmp/ado-aw-scripts/gate.js'"),
            "gate step should reference external script"
        );
        // No install/download leakage from this extension after dedupe.
        assert!(
            !steps[0].contains("NodeTool@0"),
            "extension should NOT emit NodeTool@0 (compiler emits it once)"
        );
        assert!(
            !steps[0].contains("ado-script.zip"),
            "extension should NOT emit the download (compiler emits it once)"
        );
        assert!(
            ext.needs_scripts_bundle(),
            "extension must declare bundle dependency so compiler emits install"
        );
    }

    #[test]
    fn test_needs_scripts_bundle_false_without_filters() {
        let ext = TriggerFiltersExtension::new(None, None);
        assert!(
            !ext.needs_scripts_bundle(),
            "no filters → no gate step → no bundle dependency"
        );
    }

    #[test]
    fn test_extension_name_and_phase() {
        let ext = TriggerFiltersExtension::new(None, None);
        assert_eq!(ext.name(), "trigger-filters");
        assert_eq!(ext.phase(), ExtensionPhase::Tool);
    }

    #[test]
    fn test_validate_catches_errors() {
        let filters = PrFilters {
            min_changes: Some(100),
            max_changes: Some(5),
            ..Default::default()
        };
        let ext = TriggerFiltersExtension::new(Some(filters), None);
        let yaml = r#"
name: test
description: test agent
"#;
        let fm: FrontMatter = serde_yaml::from_str(yaml).unwrap();
        let ctx = CompileContext::for_test(&fm);
        let result = ext.validate(&ctx);
        assert!(result.is_err(), "should error on min > max");
    }
}
