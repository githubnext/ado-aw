//! Trigger filters compiler extension.
//!
//! Activates when Tier 2/3 filters are configured (labels, draft,
//! changed-files, time-window, min/max-changes). Injects into the Setup
//! job: (1) a download step for the gate evaluator script and (2) the
//! gate step that evaluates the filter spec.
//!
//! Tier 1 filters (title, author, branch, commit-message, build-reason)
//! are handled inline without this extension.

use anyhow::Result;

use super::{CompileContext, CompilerExtension, ExtensionPhase};
use crate::compile::filter_ir::{
    compile_gate_step_external, lower_pipeline_filters, lower_pr_filters, needs_evaluator,
    validate_pipeline_filters, validate_pr_filters, GateContext, Severity,
};
use crate::compile::types::{PipelineFilters, PrFilters};

/// The path where the gate evaluator is downloaded at pipeline runtime.
const GATE_EVAL_PATH: &str = "/tmp/ado-aw-scripts/gate-eval.py";

/// Compiler extension that delivers and runs the gate evaluator for
/// complex trigger filters.
pub struct TriggerFiltersExtension {
    pr_filters: Option<PrFilters>,
    pipeline_filters: Option<PipelineFilters>,
    version: String,
}

impl TriggerFiltersExtension {
    pub fn new(
        pr_filters: Option<PrFilters>,
        pipeline_filters: Option<PipelineFilters>,
        version: String,
    ) -> Self {
        Self {
            pr_filters,
            pipeline_filters,
            version,
        }
    }

    /// Returns true if any configured filter requires the evaluator (Tier 2/3).
    pub fn is_needed(
        pr_filters: Option<&PrFilters>,
        pipeline_filters: Option<&PipelineFilters>,
    ) -> bool {
        if let Some(f) = pr_filters {
            let checks = lower_pr_filters(f);
            if needs_evaluator(&checks) {
                return true;
            }
        }
        if let Some(f) = pipeline_filters {
            let checks = lower_pipeline_filters(f);
            if needs_evaluator(&checks) {
                return true;
            }
        }
        false
    }

    fn download_url(&self) -> String {
        format!(
            "https://github.com/githubnext/ado-aw/releases/download/v{}/gate-eval.py",
            self.version
        )
    }
}

impl CompilerExtension for TriggerFiltersExtension {
    fn name(&self) -> &str {
        "trigger-filters"
    }

    fn phase(&self) -> ExtensionPhase {
        ExtensionPhase::Tool
    }

    fn setup_steps(&self) -> Vec<String> {
        let mut steps = Vec::new();

        // Download the gate evaluator script
        steps.push(format!(
            r#"- bash: |
    mkdir -p /tmp/ado-aw-scripts
    curl -sL "{}" -o {}
    chmod +x {}
  displayName: "Download gate evaluator (v{})"
  condition: succeeded()"#,
            self.download_url(),
            GATE_EVAL_PATH,
            GATE_EVAL_PATH,
            self.version,
        ));

        // PR gate step
        if let Some(filters) = &self.pr_filters {
            let checks = lower_pr_filters(filters);
            if !checks.is_empty() {
                steps.push(compile_gate_step_external(
                    GateContext::PullRequest,
                    &checks,
                    GATE_EVAL_PATH,
                ));
            }
        }

        // Pipeline gate step
        if let Some(filters) = &self.pipeline_filters {
            let checks = lower_pipeline_filters(filters);
            if !checks.is_empty() {
                steps.push(compile_gate_step_external(
                    GateContext::PipelineCompletion,
                    &checks,
                    GATE_EVAL_PATH,
                ));
            }
        }

        steps
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
    fn test_is_needed_tier1_only() {
        let filters = PrFilters {
            title: Some(PatternFilter {
                pattern: "test".into(),
            }),
            ..Default::default()
        };
        assert!(
            !TriggerFiltersExtension::is_needed(Some(&filters), None),
            "Tier 1 only should not need evaluator"
        );
    }

    #[test]
    fn test_is_needed_tier2() {
        let filters = PrFilters {
            labels: Some(LabelFilter {
                any_of: vec!["run-agent".into()],
                ..Default::default()
            }),
            ..Default::default()
        };
        assert!(
            TriggerFiltersExtension::is_needed(Some(&filters), None),
            "Labels filter should need evaluator"
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
            "1.0.0".into(),
        );
        let steps = ext.setup_steps();
        assert_eq!(steps.len(), 2, "should have download + gate step");
        assert!(steps[0].contains("curl"), "first step should download");
        assert!(
            steps[0].contains("gate-eval.py"),
            "should download gate-eval.py"
        );
        assert!(steps[1].contains("prGate"), "second step should be PR gate");
        assert!(
            steps[1].contains("python3 /tmp/ado-aw-scripts/gate-eval.py"),
            "gate step should reference external script"
        );
    }

    #[test]
    fn test_extension_name_and_phase() {
        let ext = TriggerFiltersExtension::new(None, None, "1.0.0".into());
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
            "1.0.0".into(),
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
