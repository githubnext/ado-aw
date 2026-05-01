//! Trigger filters compiler extension.
//!
//! Activates when any `filters:` configuration is present under `on.pr`
//! or `on.pipeline`. Injects into the Setup job: (1) a download step for
//! the gate evaluator scripts bundle and (2) the gate step that evaluates
//! the filter spec via the Python evaluator.
//!
//! All filter types (simple and complex) are evaluated by the Python
//! evaluator — there is no inline bash codegen path.

use anyhow::Result;

use super::{CompileContext, CompilerExtension, ExtensionPhase};
use crate::compile::filter_ir::{
    compile_gate_step_external, lower_pipeline_filters, lower_pr_filters,
    validate_pipeline_filters, validate_pr_filters, GateContext, Severity,
};
use crate::compile::types::{PipelineFilters, PrFilters};

/// The path where the gate evaluator is downloaded at pipeline runtime.
const GATE_EVAL_PATH: &str = "/tmp/ado-aw-scripts/gate-eval.py";

/// Base URL for ado-aw release artifacts.
const RELEASE_BASE_URL: &str = "https://github.com/githubnext/ado-aw/releases/download";

/// Compiler extension that delivers and runs the gate evaluator for
/// complex trigger filters.
pub struct TriggerFiltersExtension {
    pr_filters: Option<PrFilters>,
    pipeline_filters: Option<PipelineFilters>,
}

impl TriggerFiltersExtension {
    pub fn new(
        pr_filters: Option<PrFilters>,
        pipeline_filters: Option<PipelineFilters>,
    ) -> Self {
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
        let version = env!("CARGO_PKG_VERSION");
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

        // Only download scripts when we actually have gate steps
        if gate_steps.is_empty() {
            return Ok(vec![]);
        }

        let mut steps = Vec::new();
        steps.push(format!(
            r#"- bash: |
    mkdir -p /tmp/ado-aw-scripts
    curl -fsSL "{RELEASE_BASE_URL}/v{version}/scripts.zip" -o /tmp/ado-aw-scripts/scripts.zip
    cd /tmp/ado-aw-scripts && unzip -o scripts.zip
  displayName: "Download ado-aw scripts (v{version})"
  condition: succeeded()"#,
        ));
        steps.extend(gate_steps);

        Ok(steps)
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
    use crate::compile::types::*;
    use crate::compile::extensions::CompileContext;

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
    fn test_setup_steps_includes_download_and_gate() {
        let filters = PrFilters {
            labels: Some(LabelFilter {
                any_of: vec!["run-agent".into()],
                ..Default::default()
            }),
            ..Default::default()
        };
        let ext = TriggerFiltersExtension::new(
            Some(filters),
            None,
        );
        let yaml = "name: test\ndescription: test";
        let fm: FrontMatter = serde_yaml::from_str(yaml).unwrap();
        let ctx = CompileContext::for_test(&fm);
        let steps = ext.setup_steps(&ctx).unwrap();
        assert_eq!(steps.len(), 2, "should have download + gate step");
        assert!(steps[0].contains("curl"), "first step should download");
        assert!(
            steps[0].contains("scripts.zip"),
            "should download scripts.zip"
        );
        assert!(steps[1].contains("prGate"), "second step should be PR gate");
        assert!(
            steps[1].contains("python3 '/tmp/ado-aw-scripts/gate-eval.py'"),
            "gate step should reference external script"
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
        let ext = TriggerFiltersExtension::new(
            Some(filters),
            None,
        );
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
