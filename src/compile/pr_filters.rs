//! PR trigger filter logic.
//!
//! This module handles the generation of:
//! - Native ADO PR trigger blocks (branches/paths)
//! - Pre-activation gate steps that evaluate runtime PR filters
//! - Self-cancellation via ADO REST API when filters don't match
//!
//! Gate steps are injected into the Setup job. Non-PR builds bypass the gate
//! entirely. Cancelled builds are invisible to `DownloadPipelineArtifact@2`,
//! naturally preserving the cache-memory artifact chain.

use super::types::PrTriggerConfig;

// ─── Native ADO PR trigger ──────────────────────────────────────────────────

/// Generate native ADO PR trigger block from PrTriggerConfig.
pub(super) fn generate_native_pr_trigger(pr: &PrTriggerConfig) -> String {
    let has_branches = pr
        .branches
        .as_ref()
        .is_some_and(|b| !b.include.is_empty() || !b.exclude.is_empty());
    let has_paths = pr
        .paths
        .as_ref()
        .is_some_and(|p| !p.include.is_empty() || !p.exclude.is_empty());

    if !has_branches && !has_paths {
        return String::new();
    }

    let mut yaml = String::from("pr:\n");

    if let Some(branches) = &pr.branches {
        if !branches.include.is_empty() || !branches.exclude.is_empty() {
            yaml.push_str("  branches:\n");
            if !branches.include.is_empty() {
                yaml.push_str("    include:\n");
                for b in &branches.include {
                    yaml.push_str(&format!("      - '{}'\n", b.replace('\'', "''")));
                }
            }
            if !branches.exclude.is_empty() {
                yaml.push_str("    exclude:\n");
                for b in &branches.exclude {
                    yaml.push_str(&format!("      - '{}'\n", b.replace('\'', "''")));
                }
            }
        }
    }

    if let Some(paths) = &pr.paths {
        if !paths.include.is_empty() || !paths.exclude.is_empty() {
            yaml.push_str("  paths:\n");
            if !paths.include.is_empty() {
                yaml.push_str("    include:\n");
                for p in &paths.include {
                    yaml.push_str(&format!("      - '{}'\n", p.replace('\'', "''")));
                }
            }
            if !paths.exclude.is_empty() {
                yaml.push_str("    exclude:\n");
                for p in &paths.exclude {
                    yaml.push_str(&format!("      - '{}'\n", p.replace('\'', "''")));
                }
            }
        }
    }

    yaml.trim_end().to_string()
}

// ─── Gate step generation ───────────────────────────────────────────────────

// Gate step generation is now handled entirely by TriggerFiltersExtension.
// See src/compile/extensions/trigger_filters.rs.

/// Add a `condition:` to each step in a list of serde_yaml::Value steps.
pub(super) fn add_condition_to_steps(
    steps: &[serde_yaml::Value],
    condition: &str,
) -> Vec<serde_yaml::Value> {
    steps
        .iter()
        .map(|step| {
            let mut step = step.clone();
            if let serde_yaml::Value::Mapping(ref mut map) = step {
                map.insert(
                    serde_yaml::Value::String("condition".into()),
                    serde_yaml::Value::String(condition.into()),
                );
            }
            step
        })
        .collect()
}

// ─── Helpers ────────────────────────────────────────────────────────────────


// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::common::{generate_agentic_depends_on, generate_pr_trigger, generate_setup_job};
    use crate::compile::extensions::CompileContext;
    use crate::compile::types::*;

    fn make_ctx(fm: &FrontMatter) -> CompileContext<'_> {
        CompileContext::for_test(fm)
    }

    fn test_fm() -> FrontMatter {
        serde_yaml::from_str("name: test\ndescription: test").unwrap()
    }

    #[test]
    fn test_generate_pr_trigger_with_explicit_pr_trigger_overrides_schedule() {
        let triggers = Some(OnConfig {
            pipeline: None,
            pr: Some(PrTriggerConfig::default()),
        schedule: None,
        });
        let result = generate_pr_trigger(&triggers, true);
        assert!(!result.contains("pr: none"), "triggers.pr should override schedule suppression");
    }

    #[test]
    fn test_generate_pr_trigger_with_pr_trigger_and_pipeline_trigger() {
        let triggers = Some(OnConfig {
            pipeline: Some(PipelineTrigger {
                name: "Build".into(),
                project: None,
                branches: vec![],
            filters: None,
            }),
            pr: Some(PrTriggerConfig::default()),
        schedule: None,
        });
        let result = generate_pr_trigger(&triggers, false);
        assert!(!result.contains("pr: none"), "triggers.pr should override pipeline trigger suppression");
    }

    #[test]
    fn test_generate_pr_trigger_with_branches() {
        let triggers = Some(OnConfig {
            pipeline: None,
            pr: Some(PrTriggerConfig {
                branches: Some(BranchFilter {
                    include: vec!["main".into(), "release/*".into()],
                    exclude: vec!["test/*".into()],
                }),
                paths: None,
                filters: None,
            }),
        schedule: None,
        });
        let result = generate_pr_trigger(&triggers, false);
        assert!(result.contains("pr:"), "should emit pr: block");
        assert!(result.contains("branches:"), "should include branches");
        assert!(result.contains("main"), "should include main branch");
        assert!(result.contains("release/*"), "should include release/* branch");
        assert!(result.contains("exclude:"), "should include exclude");
        assert!(result.contains("test/*"), "should include test/* exclusion");
    }

    #[test]
    fn test_generate_pr_trigger_with_paths() {
        let triggers = Some(OnConfig {
            pipeline: None,
            pr: Some(PrTriggerConfig {
                branches: None,
                paths: Some(PathFilter {
                    include: vec!["src/*".into()],
                    exclude: vec!["docs/*".into()],
                }),
                filters: None,
            }),
        schedule: None,
        });
        let result = generate_pr_trigger(&triggers, false);
        assert!(result.contains("pr:"), "should emit pr: block");
        assert!(result.contains("paths:"), "should include paths");
        assert!(result.contains("src/*"), "should include src/* path");
        assert!(result.contains("docs/*"), "should include docs/* exclusion");
    }

    #[test]
    fn test_generate_pr_trigger_with_filters_only_no_pr_block() {
        let triggers = Some(OnConfig {
            pipeline: None,
            pr: Some(PrTriggerConfig {
                branches: None,
                paths: None,
                filters: Some(PrFilters {
                    title: Some(PatternFilter { pattern: "*[agent]*".into() }),
                    ..Default::default()
                }),
            }),
        schedule: None,
        });
        let result = generate_pr_trigger(&triggers, false);
        // When only runtime filters are configured (no branches/paths), no native
        // pr: block is emitted. ADO interprets this as "trigger on all PRs" — the
        // runtime gate step handles the actual filtering. Do NOT change this to
        // emit "pr: none" or the gate will never run.
        assert!(result.is_empty(), "filters-only should not emit a pr: block (use default trigger)");
    }

    // Gate step tests now use the spec/extension directly since generate_setup_job
    // delegates to TriggerFiltersExtension for all filter gate generation.

    #[test]
    fn test_generate_setup_job_with_filters_no_extension_creates_empty() {
        // Without the TriggerFiltersExtension, filters don't produce a gate step
        let fm = test_fm();
        let ctx = make_ctx(&fm);
        let filters = PrFilters {
            title: Some(PatternFilter { pattern: "*[review]*".into() }),
            ..Default::default()
        };
        let result = generate_setup_job(&[], "MyPool", Some(&filters), None, &[], &ctx).unwrap();
        // No extension → no gate step → setup job has no steps → empty
        assert!(result.is_empty(), "filters without extension should produce empty setup job");
    }

    #[test]
    fn test_generate_setup_job_with_user_steps_and_filters() {
        let fm = test_fm();
        let ctx = make_ctx(&fm);
        let step: serde_yaml::Value = serde_yaml::from_str("bash: echo hello\ndisplayName: User step").unwrap();
        let filters = PrFilters {
            title: Some(PatternFilter { pattern: "test".into() }),
            ..Default::default()
        };
        let result = generate_setup_job(&[step], "MyPool", Some(&filters), None, &[], &ctx).unwrap();
        // User steps are conditioned on gate output even without extension
        assert!(result.contains("User step"), "should include user step");
        assert!(result.contains("prGate.SHOULD_RUN"), "user steps should reference gate output");
    }

    #[test]
    fn test_generate_setup_job_without_filters_unchanged() {
        let fm = test_fm();
        let ctx = make_ctx(&fm);
        let result = generate_setup_job(&[], "MyPool", None, None, &[], &ctx).unwrap();
        assert!(result.is_empty(), "no setup steps and no filters should produce empty string");
    }

    #[test]
    fn test_generate_agentic_depends_on_with_pr_filters() {
        let result = generate_agentic_depends_on(&[], true, false, &[]);
        assert!(result.contains("dependsOn: Setup"), "should depend on Setup");
        assert!(result.contains("condition:"), "should have condition");
        assert!(result.contains("Build.Reason"), "should check Build.Reason");
        assert!(result.contains("prGate.SHOULD_RUN"), "should check gate output");
    }

    #[test]
    fn test_generate_agentic_depends_on_setup_only_no_condition() {
        let step: serde_yaml::Value = serde_yaml::from_str("bash: echo hello").unwrap();
        let result = generate_agentic_depends_on(&[step], false, false, &[]);
        assert_eq!(result, "dependsOn: Setup");
        assert!(!result.contains("condition:"), "no condition without PR filters");
    }

    #[test]
    fn test_generate_agentic_depends_on_nothing() {
        let result = generate_agentic_depends_on(&[], false, false, &[]);
        assert!(result.is_empty());
    }

    #[test]
    fn test_generate_setup_job_gate_spec_via_extension() {
        // Filter content is now tested via build_gate_spec, not generate_setup_job
        use crate::compile::filter_ir::{build_gate_spec, lower_pr_filters, GateContext};
        let filters = PrFilters {
            author: Some(IncludeExcludeFilter {
                include: vec!["alice@corp.com".into()],
                exclude: vec!["bot@noreply.com".into()],
            }),
            source_branch: Some(PatternFilter { pattern: "feature/*".into() }),
            target_branch: Some(PatternFilter { pattern: "main".into() }),
            ..Default::default()
        };
        let checks = lower_pr_filters(&filters);
        let spec = build_gate_spec(GateContext::PullRequest, &checks).unwrap();
        // Author include + exclude = 2 checks + source + target = 4
        assert_eq!(spec.checks.len(), 4);
        assert!(spec.facts.iter().any(|f| f.kind == "author_email"));
        assert!(spec.facts.iter().any(|f| f.kind == "source_branch"));
        assert!(spec.facts.iter().any(|f| f.kind == "target_branch"));
    }

    #[test]
    fn test_generate_setup_job_gate_non_pr_bypass_in_spec() {
        use crate::compile::filter_ir::{build_gate_spec, lower_pr_filters, GateContext};
        let filters = PrFilters {
            title: Some(PatternFilter { pattern: "test".into() }),
            ..Default::default()
        };
        let checks = lower_pr_filters(&filters);
        let spec = build_gate_spec(GateContext::PullRequest, &checks).unwrap();
        assert_eq!(spec.context.build_reason, "PullRequest");
        assert_eq!(spec.context.bypass_label, "PR");
    }

    #[test]
    fn test_generate_setup_job_gate_build_tags() {
        let filters = PrFilters {
            title: Some(PatternFilter { pattern: "test".into() }),
            ..Default::default()
        };
        // Build tags are now in the evaluator, driven by spec. Verify spec content.
        use crate::compile::filter_ir::{build_gate_spec, lower_pr_filters, GateContext};
        let checks = lower_pr_filters(&filters);
        let spec = build_gate_spec(GateContext::PullRequest, &checks).unwrap();
        assert_eq!(spec.context.tag_prefix, "pr-gate");
        assert_eq!(spec.checks[0].tag_suffix, "title-mismatch");
    }


    #[test]
    fn test_gate_step_includes_api_facts_for_tier2() {
        use crate::compile::filter_ir::{build_gate_spec, lower_pr_filters, GateContext};
        let filters = PrFilters {
            labels: Some(LabelFilter {
                any_of: vec!["run-agent".into()],
                ..Default::default()
            }),
            ..Default::default()
        };
        let checks = lower_pr_filters(&filters);
        let spec = build_gate_spec(GateContext::PullRequest, &checks).unwrap();
        assert!(spec.facts.iter().any(|f| f.kind == "pr_metadata"), "should require pr_metadata fact");
        assert!(spec.facts.iter().any(|f| f.kind == "pr_labels"), "should require pr_labels fact");
    }

    #[test]
    fn test_gate_step_no_api_facts_for_tier1_only() {
        use crate::compile::filter_ir::{build_gate_spec, lower_pr_filters, GateContext};
        let filters = PrFilters {
            title: Some(PatternFilter { pattern: "test".into() }),
            ..Default::default()
        };
        let checks = lower_pr_filters(&filters);
        let spec = build_gate_spec(GateContext::PullRequest, &checks).unwrap();
        assert!(!spec.facts.iter().any(|f| f.kind == "pr_metadata"), "should not require pr_metadata for title-only");
    }

    #[test]
    fn test_gate_step_labels_any_of() {
        use crate::compile::filter_ir::{build_gate_spec, lower_pr_filters, GateContext, PredicateSpec};
        let filters = PrFilters {
            labels: Some(LabelFilter {
                any_of: vec!["run-agent".into(), "needs-review".into()],
                ..Default::default()
            }),
            ..Default::default()
        };
        let checks = lower_pr_filters(&filters);
        let spec = build_gate_spec(GateContext::PullRequest, &checks).unwrap();
        let check = &spec.checks[0];
        assert_eq!(check.name, "labels");
        match &check.predicate {
            PredicateSpec::LabelSetMatch { any_of, .. } => {
                assert!(any_of.contains(&"run-agent".to_string()));
                assert!(any_of.contains(&"needs-review".to_string()));
            }
            other => panic!("expected LabelSetMatch, got {:?}", other),
        }
    }

    #[test]
    fn test_gate_step_labels_none_of() {
        use crate::compile::filter_ir::{build_gate_spec, lower_pr_filters, GateContext, PredicateSpec};
        let filters = PrFilters {
            labels: Some(LabelFilter {
                none_of: vec!["do-not-run".into()],
                ..Default::default()
            }),
            ..Default::default()
        };
        let checks = lower_pr_filters(&filters);
        let spec = build_gate_spec(GateContext::PullRequest, &checks).unwrap();
        match &spec.checks[0].predicate {
            PredicateSpec::LabelSetMatch { none_of, .. } => {
                assert!(none_of.contains(&"do-not-run".to_string()));
            }
            other => panic!("expected LabelSetMatch, got {:?}", other),
        }
    }

    #[test]
    fn test_gate_step_draft_false() {
        use crate::compile::filter_ir::{build_gate_spec, lower_pr_filters, GateContext, PredicateSpec};
        let filters = PrFilters {
            draft: Some(false),
            ..Default::default()
        };
        let checks = lower_pr_filters(&filters);
        let spec = build_gate_spec(GateContext::PullRequest, &checks).unwrap();
        match &spec.checks[0].predicate {
            PredicateSpec::Equals { fact, value } => {
                assert_eq!(fact, "pr_is_draft");
                assert_eq!(value, "false");
            }
            other => panic!("expected Equals, got {:?}", other),
        }
        assert!(spec.facts.iter().any(|f| f.kind == "pr_is_draft"), "should include pr_is_draft fact");
        assert!(spec.facts.iter().any(|f| f.kind == "pr_metadata"), "should include pr_metadata dependency");
    }

    #[test]
    fn test_gate_step_changed_files() {
        use crate::compile::filter_ir::{build_gate_spec, lower_pr_filters, GateContext, PredicateSpec};
        let filters = PrFilters {
            changed_files: Some(IncludeExcludeFilter {
                include: vec!["src/**/*.rs".into()],
                exclude: vec!["docs/**".into()],
            }),
            ..Default::default()
        };
        let checks = lower_pr_filters(&filters);
        let spec = build_gate_spec(GateContext::PullRequest, &checks).unwrap();
        match &spec.checks[0].predicate {
            PredicateSpec::FileGlobMatch { include, exclude, .. } => {
                assert!(include.contains(&"src/**/*.rs".to_string()));
                assert!(exclude.contains(&"docs/**".to_string()));
            }
            other => panic!("expected FileGlobMatch, got {:?}", other),
        }
        assert!(spec.facts.iter().any(|f| f.kind == "changed_files"), "should include changed_files fact");
    }

    #[test]
    fn test_gate_step_combined_tier1_and_tier2() {
        use crate::compile::filter_ir::{build_gate_spec, lower_pr_filters, GateContext};
        let filters = PrFilters {
            title: Some(PatternFilter { pattern: "\\[review\\]".into() }),
            draft: Some(false),
            labels: Some(LabelFilter {
                any_of: vec!["run-agent".into()],
                ..Default::default()
            }),
            ..Default::default()
        };
        let checks = lower_pr_filters(&filters);
        let spec = build_gate_spec(GateContext::PullRequest, &checks).unwrap();
        // Tier 1 fact
        assert!(spec.facts.iter().any(|f| f.kind == "pr_title"), "should include pr_title");
        // Tier 2 facts
        assert!(spec.facts.iter().any(|f| f.kind == "pr_metadata"), "should include pr_metadata");
        assert!(spec.facts.iter().any(|f| f.kind == "pr_is_draft"), "should include pr_is_draft");
        assert!(spec.facts.iter().any(|f| f.kind == "pr_labels"), "should include pr_labels");
        // Checks
        assert_eq!(spec.checks.len(), 3, "should have 3 checks (title, draft, labels)");
    }

    // ─── Tier 3 filter tests ────────────────────────────────────────────────

    #[test]
    fn test_gate_step_time_window() {
        use crate::compile::filter_ir::{build_gate_spec, lower_pr_filters, GateContext, PredicateSpec};
        let filters = PrFilters {
            time_window: Some(super::super::types::TimeWindowFilter {
                start: "09:00".into(),
                end: "17:00".into(),
            }),
            ..Default::default()
        };
        let checks = lower_pr_filters(&filters);
        let spec = build_gate_spec(GateContext::PullRequest, &checks).unwrap();
        match &spec.checks[0].predicate {
            PredicateSpec::TimeWindow { start, end } => {
                assert_eq!(start, "09:00");
                assert_eq!(end, "17:00");
            }
            other => panic!("expected TimeWindow, got {:?}", other),
        }
        assert_eq!(spec.checks[0].tag_suffix, "time-window-mismatch");
    }

    #[test]
    fn test_gate_step_min_changes() {
        use crate::compile::filter_ir::{build_gate_spec, lower_pr_filters, GateContext, PredicateSpec};
        let filters = PrFilters {
            min_changes: Some(5),
            ..Default::default()
        };
        let checks = lower_pr_filters(&filters);
        let spec = build_gate_spec(GateContext::PullRequest, &checks).unwrap();
        match &spec.checks[0].predicate {
            PredicateSpec::NumericRange { min, max, .. } => {
                assert_eq!(*min, Some(5));
                assert_eq!(*max, None);
            }
            other => panic!("expected NumericRange, got {:?}", other),
        }
    }

    #[test]
    fn test_gate_step_max_changes() {
        use crate::compile::filter_ir::{build_gate_spec, lower_pr_filters, GateContext, PredicateSpec};
        let filters = PrFilters {
            max_changes: Some(50),
            ..Default::default()
        };
        let checks = lower_pr_filters(&filters);
        let spec = build_gate_spec(GateContext::PullRequest, &checks).unwrap();
        match &spec.checks[0].predicate {
            PredicateSpec::NumericRange { min, max, .. } => {
                assert_eq!(*min, None);
                assert_eq!(*max, Some(50));
            }
            other => panic!("expected NumericRange, got {:?}", other),
        }
    }

    #[test]
    fn test_gate_step_min_and_max_changes() {
        use crate::compile::filter_ir::{build_gate_spec, lower_pr_filters, GateContext, PredicateSpec};
        let filters = PrFilters {
            min_changes: Some(2),
            max_changes: Some(100),
            ..Default::default()
        };
        let checks = lower_pr_filters(&filters);
        let spec = build_gate_spec(GateContext::PullRequest, &checks).unwrap();
        match &spec.checks[0].predicate {
            PredicateSpec::NumericRange { min, max, .. } => {
                assert_eq!(*min, Some(2));
                assert_eq!(*max, Some(100));
            }
            other => panic!("expected NumericRange, got {:?}", other),
        }
    }

    #[test]
    fn test_gate_step_build_reason_include() {
        use crate::compile::filter_ir::{build_gate_spec, lower_pr_filters, GateContext, PredicateSpec};
        let filters = PrFilters {
            build_reason: Some(IncludeExcludeFilter {
                include: vec!["PullRequest".into(), "Manual".into()],
                exclude: vec![],
            }),
            ..Default::default()
        };
        let checks = lower_pr_filters(&filters);
        let spec = build_gate_spec(GateContext::PullRequest, &checks).unwrap();
        match &spec.checks[0].predicate {
            PredicateSpec::ValueInSet { values, .. } => {
                assert!(values.contains(&"PullRequest".to_string()));
                assert!(values.contains(&"Manual".to_string()));
            }
            other => panic!("expected ValueInSet, got {:?}", other),
        }
        assert_eq!(spec.checks[0].tag_suffix, "build-reason-mismatch");
    }

    #[test]
    fn test_gate_step_build_reason_exclude() {
        use crate::compile::filter_ir::{build_gate_spec, lower_pr_filters, GateContext, PredicateSpec};
        let filters = PrFilters {
            build_reason: Some(IncludeExcludeFilter {
                include: vec![],
                exclude: vec!["Schedule".into()],
            }),
            ..Default::default()
        };
        let checks = lower_pr_filters(&filters);
        let spec = build_gate_spec(GateContext::PullRequest, &checks).unwrap();
        match &spec.checks[0].predicate {
            PredicateSpec::ValueNotInSet { values, .. } => {
                assert!(values.contains(&"Schedule".to_string()));
            }
            other => panic!("expected ValueNotInSet, got {:?}", other),
        }
        assert_eq!(spec.checks[0].tag_suffix, "build-reason-excluded");
    }

    #[test]
    fn test_agentic_depends_on_with_expression() {
        let result = generate_agentic_depends_on(
            &[],
            false,
            false,
            &["eq(variables['Custom.ShouldRun'], 'true')"],
        );
        assert!(result.contains("condition:"), "should have condition");
        assert!(result.contains("Custom.ShouldRun"), "should include expression");
        assert!(result.contains("succeeded()"), "should still require succeeded");
    }

    #[test]
    fn test_agentic_depends_on_with_pr_filters_and_expression() {
        let result = generate_agentic_depends_on(
            &[],
            true,
            false,
            &["eq(variables['Custom.Flag'], 'yes')"],
        );
        assert!(result.contains("prGate.SHOULD_RUN"), "should check gate output");
        assert!(result.contains("Custom.Flag"), "should include expression");
        assert!(result.contains("Build.Reason"), "should check build reason");
    }

    #[test]
    fn test_agentic_depends_on_expression_only_no_depends() {
        let result = generate_agentic_depends_on(
            &[],
            false,
            false,
            &["eq(variables['Run'], 'true')"],
        );
        // No setup steps, no PR filters — no dependsOn, but still a condition
        assert!(!result.contains("dependsOn"), "no dependsOn without setup/filters");
        assert!(result.contains("condition:"), "should have condition from expression");
    }

    #[test]
    fn test_gate_step_change_count_includes_changed_files_fact() {
        use crate::compile::filter_ir::{build_gate_spec, lower_pr_filters, GateContext};
        let filters = PrFilters {
            changed_files: Some(IncludeExcludeFilter {
                include: vec!["src/**".into()],
                ..Default::default()
            }),
            min_changes: Some(3),
            ..Default::default()
        };
        let checks = lower_pr_filters(&filters);
        let spec = build_gate_spec(GateContext::PullRequest, &checks).unwrap();
        // Both changed_files and changed_file_count facts should be present
        assert!(spec.facts.iter().any(|f| f.kind == "changed_files"));
        assert!(spec.facts.iter().any(|f| f.kind == "changed_file_count"));
    }

    #[test]
    fn test_pr_trigger_type_deserialization_tier3() {
        let yaml = r#"
triggers:
  pr:
    filters:
      time-window:
        start: "09:00"
        end: "17:00"
      min-changes: 5
      max-changes: 100
      build-reason:
        include: [PullRequest, Manual]
      expression: "eq(variables['Custom.Flag'], 'true')"
"#;
        let val: serde_yaml::Value = serde_yaml::from_str(yaml).unwrap();
        let tc: OnConfig = serde_yaml::from_value(val["triggers"].clone()).unwrap();
        let filters = tc.pr.unwrap().filters.unwrap();
        assert_eq!(filters.time_window.as_ref().unwrap().start, "09:00");
        assert_eq!(filters.time_window.as_ref().unwrap().end, "17:00");
        assert_eq!(filters.min_changes, Some(5));
        assert_eq!(filters.max_changes, Some(100));
        assert_eq!(filters.build_reason.as_ref().unwrap().include, vec!["PullRequest", "Manual"]);
        assert_eq!(filters.expression.as_ref().unwrap(), "eq(variables['Custom.Flag'], 'true')");
    }

    #[test]
    fn test_gate_step_commit_message() {
        use crate::compile::filter_ir::{build_gate_spec, lower_pr_filters, GateContext, PredicateSpec};
        let filters = PrFilters {
            commit_message: Some(PatternFilter { pattern: "*[skip-agent]*".into() }),
            ..Default::default()
        };
        let checks = lower_pr_filters(&filters);
        let spec = build_gate_spec(GateContext::PullRequest, &checks).unwrap();
        assert!(spec.facts.iter().any(|f| f.kind == "commit_message"), "should include commit_message fact");
        match &spec.checks[0].predicate {
            PredicateSpec::GlobMatch { fact, pattern } => {
                assert_eq!(fact, "commit_message");
                assert!(pattern.contains("skip-agent"));
            }
            other => panic!("expected GlobMatch, got {:?}", other),
        }
        assert_eq!(spec.checks[0].tag_suffix, "commit-message-mismatch");
    }

    #[test]
    fn test_on_config_deserialization_with_schedule() {
        let yaml = r#"
on:
  schedule: daily around 14:00
  pr:
    filters:
      title: "*[review]*"
"#;
        let val: serde_yaml::Value = serde_yaml::from_str(yaml).unwrap();
        let oc: OnConfig = serde_yaml::from_value(val["on"].clone()).unwrap();
        assert!(oc.schedule.is_some(), "should have schedule");
        assert!(oc.pr.is_some(), "should have pr");
        assert!(oc.pipeline.is_none(), "should not have pipeline");
    }

    #[test]
    fn test_on_config_deserialization_full() {
        let yaml = r#"
on:
  schedule:
    run: weekly on monday
    branches: [main]
  pipeline:
    name: "Build Pipeline"
    project: "OtherProject"
    branches: [main]
  pr:
    branches:
      include: [main]
    filters:
      title: "*[agent]*"
      commit-message: "*[skip-agent]*"
"#;
        let val: serde_yaml::Value = serde_yaml::from_str(yaml).unwrap();
        let oc: OnConfig = serde_yaml::from_value(val["on"].clone()).unwrap();
        let schedule = oc.schedule.unwrap();
        assert_eq!(schedule.expression(), "weekly on monday");
        let pipeline = oc.pipeline.unwrap();
        assert_eq!(pipeline.name, "Build Pipeline");
        let pr = oc.pr.unwrap();
        let filters = pr.filters.unwrap();
        assert_eq!(filters.title.unwrap().pattern, "*[agent]*");
        assert_eq!(filters.commit_message.unwrap().pattern, "*[skip-agent]*");
    }
}

