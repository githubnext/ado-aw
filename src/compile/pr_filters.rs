//! PR trigger filter logic.
//!
//! This module historically housed:
//! - Native ADO PR trigger blocks (branches/paths)
//! - Pre-activation gate steps that evaluate runtime PR filters
//! - Self-cancellation via ADO REST API when filters don't match
//!
//! All YAML-string emission for these concerns is now owned by the typed IR
//! (see `src/compile/ir/` and `src/compile/agentic_pipeline.rs`). Gate steps are
//! produced by `AdoScriptExtension`'s `setup_steps()` hook
//! (`src/compile/extensions/ado_script.rs`). What remains here is the
//! cfg(test) coverage of the `filter_ir` lowering / spec layer that those
//! emitters consume.

// ─── Gate step generation ───────────────────────────────────────────────────

// Gate step generation is now handled entirely by AdoScriptExtension's
// `setup_steps()` hook. See src/compile/extensions/ado_script.rs.

// ─── Helpers ────────────────────────────────────────────────────────────────

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use crate::compile::types::*;

    // Gate step tests now use the spec/extension directly since generate_setup_job
    // delegates to AdoScriptExtension (in `src/compile/extensions/ado_script.rs`)
    // for all filter gate generation.

    #[test]
    fn test_gate_step_includes_api_facts_for_tier2() {
        use crate::compile::filter_ir::{GateContext, build_gate_spec, lower_pr_filters};
        let filters = PrFilters {
            labels: Some(LabelFilter {
                any_of: vec!["run-agent".into()],
                ..Default::default()
            }),
            ..Default::default()
        };
        let checks = lower_pr_filters(&filters);
        let spec = build_gate_spec(GateContext::PullRequest, &checks).unwrap();
        assert!(
            spec.facts.iter().any(|f| f.kind == "pr_metadata"),
            "should require pr_metadata fact"
        );
        assert!(
            spec.facts.iter().any(|f| f.kind == "pr_labels"),
            "should require pr_labels fact"
        );
    }

    #[test]
    fn test_gate_step_no_api_facts_for_tier1_only() {
        use crate::compile::filter_ir::{GateContext, build_gate_spec, lower_pr_filters};
        let filters = PrFilters {
            title: Some(PatternFilter {
                pattern: "test".into(),
            }),
            ..Default::default()
        };
        let checks = lower_pr_filters(&filters);
        let spec = build_gate_spec(GateContext::PullRequest, &checks).unwrap();
        assert!(
            spec.facts.iter().any(|f| f.kind == "pr_title"),
            "should require pr_title fact for title filter"
        );
        assert!(
            !spec.facts.iter().any(|f| f.kind == "pr_metadata"),
            "should not require pr_metadata for title-only"
        );
    }

    #[test]
    fn test_gate_step_labels_any_of() {
        use crate::compile::filter_ir::{
            GateContext, PredicateSpec, build_gate_spec, lower_pr_filters,
        };
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
        use crate::compile::filter_ir::{
            GateContext, PredicateSpec, build_gate_spec, lower_pr_filters,
        };
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
        use crate::compile::filter_ir::{
            GateContext, PredicateSpec, build_gate_spec, lower_pr_filters,
        };
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
        assert!(
            spec.facts.iter().any(|f| f.kind == "pr_is_draft"),
            "should include pr_is_draft fact"
        );
        assert!(
            spec.facts.iter().any(|f| f.kind == "pr_metadata"),
            "should include pr_metadata dependency"
        );
    }

    #[test]
    fn test_gate_step_changed_files() {
        use crate::compile::filter_ir::{
            GateContext, PredicateSpec, build_gate_spec, lower_pr_filters,
        };
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
            PredicateSpec::FileGlobMatch {
                include, exclude, ..
            } => {
                assert!(include.contains(&"src/**/*.rs".to_string()));
                assert!(exclude.contains(&"docs/**".to_string()));
            }
            other => panic!("expected FileGlobMatch, got {:?}", other),
        }
        assert!(
            spec.facts.iter().any(|f| f.kind == "changed_files"),
            "should include changed_files fact"
        );
    }

    #[test]
    fn test_gate_step_combined_tier1_and_tier2() {
        use crate::compile::filter_ir::{GateContext, build_gate_spec, lower_pr_filters};
        let filters = PrFilters {
            title: Some(PatternFilter {
                pattern: "\\[review\\]".into(),
            }),
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
        assert!(
            spec.facts.iter().any(|f| f.kind == "pr_title"),
            "should include pr_title"
        );
        // Tier 2 facts
        assert!(
            spec.facts.iter().any(|f| f.kind == "pr_metadata"),
            "should include pr_metadata"
        );
        assert!(
            spec.facts.iter().any(|f| f.kind == "pr_is_draft"),
            "should include pr_is_draft"
        );
        assert!(
            spec.facts.iter().any(|f| f.kind == "pr_labels"),
            "should include pr_labels"
        );
        // Checks
        assert_eq!(
            spec.checks.len(),
            3,
            "should have 3 checks (title, draft, labels)"
        );
    }

    // ─── Tier 3 filter tests ────────────────────────────────────────────────

    #[test]
    fn test_gate_step_time_window() {
        use crate::compile::filter_ir::{
            GateContext, PredicateSpec, build_gate_spec, lower_pr_filters,
        };
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
    fn test_gate_step_min_and_max_changes() {
        use crate::compile::filter_ir::{
            GateContext, PredicateSpec, build_gate_spec, lower_pr_filters,
        };
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
        assert!(spec.facts.iter().any(|f| f.kind == "changed_file_count"), "should include changed_file_count fact");
        assert_eq!(spec.checks[0].tag_suffix, "changes-mismatch");
    }

    #[test]
    fn test_gate_step_build_reason_include() {
        use crate::compile::filter_ir::{
            GateContext, PredicateSpec, build_gate_spec, lower_pr_filters,
        };
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
        use crate::compile::filter_ir::{
            GateContext, PredicateSpec, build_gate_spec, lower_pr_filters,
        };
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
    fn test_gate_step_change_count_includes_changed_files_fact() {
        use crate::compile::filter_ir::{GateContext, build_gate_spec, lower_pr_filters};
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
        assert_eq!(
            filters.build_reason.as_ref().unwrap().include,
            vec!["PullRequest", "Manual"]
        );
        assert_eq!(
            filters.expression.as_ref().unwrap(),
            "eq(variables['Custom.Flag'], 'true')"
        );
    }

    #[test]
    fn test_gate_step_commit_message() {
        use crate::compile::filter_ir::{
            GateContext, PredicateSpec, build_gate_spec, lower_pr_filters,
        };
        let filters = PrFilters {
            commit_message: Some(PatternFilter {
                pattern: "*[skip-agent]*".into(),
            }),
            ..Default::default()
        };
        let checks = lower_pr_filters(&filters);
        let spec = build_gate_spec(GateContext::PullRequest, &checks).unwrap();
        assert!(
            spec.facts.iter().any(|f| f.kind == "commit_message"),
            "should include commit_message fact"
        );
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
        assert_eq!(
            oc.schedule.unwrap().expression(),
            "daily around 14:00",
            "schedule expression should round-trip"
        );
        let pr = oc.pr.expect("should have pr");
        let filters = pr.filters.expect("pr should have filters");
        assert_eq!(
            filters.title.unwrap().pattern,
            "*[review]*",
            "title pattern should round-trip"
        );
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
        assert_eq!(schedule.branches(), &["main"], "schedule branches should round-trip");
        let pipeline = oc.pipeline.unwrap();
        assert_eq!(pipeline.name, "Build Pipeline");
        assert_eq!(pipeline.project.as_deref(), Some("OtherProject"), "pipeline project should round-trip");
        assert_eq!(pipeline.branches, vec!["main"], "pipeline branches should round-trip");
        let pr = oc.pr.unwrap();
        assert_eq!(pr.branches.as_ref().unwrap().include, vec!["main"], "pr branches should round-trip");
        let filters = pr.filters.unwrap();
        assert_eq!(filters.title.unwrap().pattern, "*[agent]*");
        assert_eq!(filters.commit_message.unwrap().pattern, "*[skip-agent]*");
    }
}
