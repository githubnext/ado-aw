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

use super::types::{PrFilters, PrTriggerConfig};

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

/// Generate the bash gate step for PR filter evaluation.
///
/// Delegates to the filter IR pipeline: lower → validate → compile.
/// Returns an error string as a comment in the output if validation fails.
pub(super) fn generate_pr_gate_step(filters: &PrFilters) -> String {
    use super::filter_ir::{
        compile_gate_step, lower_pr_filters, validate_pr_filters, GateContext, Severity,
    };

    // Validate filters at compile time
    let diags = validate_pr_filters(filters);
    for diag in &diags {
        match diag.severity {
            Severity::Error => {
                eprintln!("error: {}", diag);
            }
            Severity::Warning => {
                eprintln!("warning: {}", diag);
            }
            Severity::Info => {
                eprintln!("info: {}", diag);
            }
        }
    }
    if diags.iter().any(|d| d.severity == Severity::Error) {
        // Return a commented-out error so compilation surfaces the problem
        let errors: Vec<String> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .map(|d| format!("# FILTER ERROR: {}", d))
            .collect();
        return errors.join("\n");
    }

    // Lower filters to IR and compile to bash
    let checks = lower_pr_filters(filters);
    compile_gate_step(GateContext::PullRequest, &checks)
}

/// Returns true if any Tier 2 filter (requiring REST API) is configured.
pub(super) fn has_tier2_filters(filters: &PrFilters) -> bool {
    filters.labels.is_some() || filters.draft.is_some() || filters.changed_files.is_some()
}

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

/// Shell-escape a string for use in a bash script.
/// Prevents shell injection from filter pattern values.
pub(super) fn shell_escape(s: &str) -> String {
    s.chars()
        .filter(|c| {
            c.is_alphanumeric()
                || matches!(
                    c,
                    '.' | '*'
                        | '+'
                        | '?'
                        | '^'
                        | '$'
                        | '|'
                        | '('
                        | ')'
                        | '['
                        | ']'
                        | '{'
                        | '}'
                        | '\\'
                        | '-'
                        | '_'
                        | '/'
                        | '@'
                        | ' '
                        | ':'
                )
        })
        .collect()
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::common::{generate_agentic_depends_on, generate_pr_trigger, generate_setup_job};
    use crate::compile::types::*;

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
                    title: Some(PatternFilter { pattern: "\\[agent\\]".into() }),
                    ..Default::default()
                }),
            }),
        schedule: None,
        });
        let result = generate_pr_trigger(&triggers, false);
        assert!(result.is_empty(), "filters-only should not emit a pr: block (use default trigger)");
    }

    #[test]
    fn test_generate_setup_job_with_pr_filters_creates_gate() {
        let filters = PrFilters {
            title: Some(PatternFilter { pattern: "\\[review\\]".into() }),
            ..Default::default()
        };
        let result = generate_setup_job(&[], "MyPool", Some(&filters), None);
        assert!(result.contains("- job: Setup"), "should create Setup job");
        assert!(result.contains("name: prGate"), "should include gate step");
        assert!(result.contains("Evaluate PR filters"), "should have gate displayName");
        assert!(result.contains("SHOULD_RUN"), "should set SHOULD_RUN variable");
        assert!(result.contains("\\[review\\]"), "should include title pattern");
        assert!(result.contains("SYSTEM_ACCESSTOKEN"), "should pass System.AccessToken");
        assert!(result.contains("cancelling"), "should include self-cancel API call");
    }

    #[test]
    fn test_generate_setup_job_with_filters_and_user_steps() {
        let step: serde_yaml::Value = serde_yaml::from_str("bash: echo hello\ndisplayName: User step").unwrap();
        let filters = PrFilters {
            title: Some(PatternFilter { pattern: "test".into() }),
            ..Default::default()
        };
        let result = generate_setup_job(&[step], "MyPool", Some(&filters), None);
        assert!(result.contains("name: prGate"), "should include gate step");
        assert!(result.contains("User step"), "should include user step");
        assert!(result.contains("prGate.SHOULD_RUN"), "user steps should reference gate output");
    }

    #[test]
    fn test_generate_setup_job_without_filters_unchanged() {
        let result = generate_setup_job(&[], "MyPool", None, None);
        assert!(result.is_empty(), "no setup steps and no filters should produce empty string");
    }

    #[test]
    fn test_generate_agentic_depends_on_with_pr_filters() {
        let result = generate_agentic_depends_on(&[], true, false, None);
        assert!(result.contains("dependsOn: Setup"), "should depend on Setup");
        assert!(result.contains("condition:"), "should have condition");
        assert!(result.contains("Build.Reason"), "should check Build.Reason");
        assert!(result.contains("prGate.SHOULD_RUN"), "should check gate output");
    }

    #[test]
    fn test_generate_agentic_depends_on_setup_only_no_condition() {
        let step: serde_yaml::Value = serde_yaml::from_str("bash: echo hello").unwrap();
        let result = generate_agentic_depends_on(&[step], false, false, None);
        assert_eq!(result, "dependsOn: Setup");
        assert!(!result.contains("condition:"), "no condition without PR filters");
    }

    #[test]
    fn test_generate_agentic_depends_on_nothing() {
        let result = generate_agentic_depends_on(&[], false, false, None);
        assert!(result.is_empty());
    }

    #[test]
    fn test_generate_setup_job_gate_author_filter() {
        let filters = PrFilters {
            author: Some(IncludeExcludeFilter {
                include: vec!["alice@corp.com".into()],
                exclude: vec!["bot@noreply.com".into()],
            }),
            ..Default::default()
        };
        let result = generate_setup_job(&[], "MyPool", Some(&filters), None);
        assert!(result.contains("alice@corp.com"), "should include author email");
        assert!(result.contains("bot@noreply.com"), "should include excluded email");
        assert!(result.contains("Build.RequestedForEmail"), "should check author variable");
    }

    #[test]
    fn test_generate_setup_job_gate_branch_filters() {
        let filters = PrFilters {
            source_branch: Some(PatternFilter { pattern: "^feature/.*".into() }),
            target_branch: Some(PatternFilter { pattern: "^main$".into() }),
            ..Default::default()
        };
        let result = generate_setup_job(&[], "MyPool", Some(&filters), None);
        assert!(result.contains("SourceBranch"), "should check source branch");
        assert!(result.contains("TargetBranch"), "should check target branch");
        assert!(result.contains("^feature/.*"), "should include source pattern");
        assert!(result.contains("^main$"), "should include target pattern");
    }

    #[test]
    fn test_generate_setup_job_gate_non_pr_passthrough() {
        let filters = PrFilters {
            title: Some(PatternFilter { pattern: "test".into() }),
            ..Default::default()
        };
        let result = generate_setup_job(&[], "MyPool", Some(&filters), None);
        assert!(result.contains("PullRequest"), "should check for PR build reason");
        assert!(result.contains("Not a PR build"), "should pass non-PR builds automatically");
    }

    #[test]
    fn test_generate_setup_job_gate_build_tags() {
        let filters = PrFilters {
            title: Some(PatternFilter { pattern: "test".into() }),
            ..Default::default()
        };
        let result = generate_setup_job(&[], "MyPool", Some(&filters), None);
        assert!(result.contains("pr-gate:passed"), "should tag passed builds");
        assert!(result.contains("pr-gate:skipped"), "should tag skipped builds");
        assert!(result.contains("pr-gate:title-mismatch"), "should tag specific filter failures");
    }

    #[test]
    fn test_shell_escape_removes_dangerous_chars() {
        assert_eq!(shell_escape("safe-pattern_123"), "safe-pattern_123");
        assert_eq!(shell_escape("test;echo pwned"), "testecho pwned");
        assert_eq!(shell_escape("test`echo`"), "testecho");
        assert_eq!(shell_escape("^feature/.*$"), "^feature/.*$");
        assert_eq!(shell_escape("\\[agent\\]"), "\\[agent\\]");
        assert_eq!(shell_escape("(a|b)"), "(a|b)");
    }

    // ─── Tier 2 filter tests ────────────────────────────────────────────────

    #[test]
    fn test_has_tier2_filters_none() {
        let filters = PrFilters::default();
        assert!(!has_tier2_filters(&filters));
    }

    #[test]
    fn test_has_tier2_filters_labels() {
        let filters = PrFilters {
            labels: Some(LabelFilter {
                any_of: vec!["run-agent".into()],
                ..Default::default()
            }),
            ..Default::default()
        };
        assert!(has_tier2_filters(&filters));
    }

    #[test]
    fn test_has_tier2_filters_draft() {
        let filters = PrFilters {
            draft: Some(false),
            ..Default::default()
        };
        assert!(has_tier2_filters(&filters));
    }

    #[test]
    fn test_has_tier2_filters_changed_files() {
        let filters = PrFilters {
            changed_files: Some(IncludeExcludeFilter {
                include: vec!["src/**".into()],
                ..Default::default()
            }),
            ..Default::default()
        };
        assert!(has_tier2_filters(&filters));
    }

    #[test]
    fn test_gate_step_includes_api_call_for_tier2() {
        let filters = PrFilters {
            labels: Some(LabelFilter {
                any_of: vec!["run-agent".into()],
                ..Default::default()
            }),
            ..Default::default()
        };
        let result = generate_pr_gate_step(&filters);
        assert!(result.contains("pullRequests"), "should include API call for labels filter");
        assert!(result.contains("PR_DATA"), "should store API response");
    }

    #[test]
    fn test_gate_step_no_api_call_for_tier1_only() {
        let filters = PrFilters {
            title: Some(PatternFilter { pattern: "test".into() }),
            ..Default::default()
        };
        let result = generate_pr_gate_step(&filters);
        assert!(!result.contains("PR_DATA"), "should not make API call for title-only filter");
    }

    #[test]
    fn test_gate_step_labels_any_of() {
        let filters = PrFilters {
            labels: Some(LabelFilter {
                any_of: vec!["run-agent".into(), "needs-review".into()],
                ..Default::default()
            }),
            ..Default::default()
        };
        let result = generate_pr_gate_step(&filters);
        assert!(result.contains("run-agent"), "should check for run-agent label");
        assert!(result.contains("needs-review"), "should check for needs-review label");
        assert!(result.contains("LABEL_MATCH"), "should use any-of matching");
    }

    #[test]
    fn test_gate_step_labels_none_of() {
        let filters = PrFilters {
            labels: Some(LabelFilter {
                none_of: vec!["do-not-run".into()],
                ..Default::default()
            }),
            ..Default::default()
        };
        let result = generate_pr_gate_step(&filters);
        assert!(result.contains("do-not-run"), "should check for blocked label");
        assert!(result.contains("BLOCKED_LABEL"), "should use none-of matching");
    }

    #[test]
    fn test_gate_step_draft_false() {
        let filters = PrFilters {
            draft: Some(false),
            ..Default::default()
        };
        let result = generate_pr_gate_step(&filters);
        assert!(result.contains("isDraft"), "should check isDraft field");
        assert!(result.contains("false"), "should expect draft=false");
    }

    #[test]
    fn test_gate_step_changed_files() {
        let filters = PrFilters {
            changed_files: Some(IncludeExcludeFilter {
                include: vec!["src/**/*.rs".into()],
                exclude: vec!["docs/**".into()],
            }),
            ..Default::default()
        };
        let result = generate_pr_gate_step(&filters);
        assert!(result.contains("iterations"), "should fetch iteration changes");
        assert!(result.contains("fnmatch"), "should use fnmatch for glob matching");
        assert!(result.contains("src/**/*.rs"), "should include the include pattern");
        assert!(result.contains("docs/**"), "should include the exclude pattern");
    }

    #[test]
    fn test_gate_step_combined_tier1_and_tier2() {
        let filters = PrFilters {
            title: Some(PatternFilter { pattern: "\\[review\\]".into() }),
            draft: Some(false),
            labels: Some(LabelFilter {
                any_of: vec!["run-agent".into()],
                ..Default::default()
            }),
            ..Default::default()
        };
        let result = generate_pr_gate_step(&filters);
        // Tier 1
        assert!(result.contains("System.PullRequest.Title"), "should check title");
        // Tier 2
        assert!(result.contains("PR_DATA"), "should make API call");
        assert!(result.contains("isDraft"), "should check draft");
        assert!(result.contains("run-agent"), "should check labels");
    }

    // ─── Tier 3 filter tests ────────────────────────────────────────────────

    #[test]
    fn test_gate_step_time_window() {
        let filters = PrFilters {
            time_window: Some(super::super::types::TimeWindowFilter {
                start: "09:00".into(),
                end: "17:00".into(),
            }),
            ..Default::default()
        };
        let result = generate_pr_gate_step(&filters);
        assert!(result.contains("CURRENT_HOUR"), "should get current UTC hour");
        assert!(result.contains("09:00"), "should include start time");
        assert!(result.contains("17:00"), "should include end time");
        assert!(result.contains("IN_WINDOW"), "should evaluate time window");
        assert!(result.contains("pr-gate:time-window-mismatch"), "should tag time-window failures");
    }

    #[test]
    fn test_gate_step_min_changes() {
        let filters = PrFilters {
            min_changes: Some(5),
            ..Default::default()
        };
        let result = generate_pr_gate_step(&filters);
        assert!(result.contains("FILE_COUNT"), "should count changed files");
        assert!(result.contains("-ge 5"), "should check minimum 5 files");
        assert!(result.contains("pr-gate:min-changes-mismatch"), "should tag min-changes failures");
    }

    #[test]
    fn test_gate_step_max_changes() {
        let filters = PrFilters {
            max_changes: Some(50),
            ..Default::default()
        };
        let result = generate_pr_gate_step(&filters);
        assert!(result.contains("FILE_COUNT"), "should count changed files");
        assert!(result.contains("-le 50"), "should check maximum 50 files");
        assert!(result.contains("pr-gate:max-changes-mismatch"), "should tag max-changes failures");
    }

    #[test]
    fn test_gate_step_min_and_max_changes() {
        let filters = PrFilters {
            min_changes: Some(2),
            max_changes: Some(100),
            ..Default::default()
        };
        let result = generate_pr_gate_step(&filters);
        assert!(result.contains("-ge 2"), "should check min");
        assert!(result.contains("-le 100"), "should check max");
    }

    #[test]
    fn test_gate_step_build_reason_include() {
        let filters = PrFilters {
            build_reason: Some(IncludeExcludeFilter {
                include: vec!["PullRequest".into(), "Manual".into()],
                exclude: vec![],
            }),
            ..Default::default()
        };
        let result = generate_pr_gate_step(&filters);
        assert!(result.contains("Build.Reason"), "should check build reason");
        assert!(result.contains("PullRequest"), "should include PullRequest");
        assert!(result.contains("Manual"), "should include Manual");
        assert!(result.contains("pr-gate:build-reason-mismatch"), "should tag build-reason failures");
    }

    #[test]
    fn test_gate_step_build_reason_exclude() {
        let filters = PrFilters {
            build_reason: Some(IncludeExcludeFilter {
                include: vec![],
                exclude: vec!["Schedule".into()],
            }),
            ..Default::default()
        };
        let result = generate_pr_gate_step(&filters);
        assert!(result.contains("Schedule"), "should check excluded reason");
        assert!(result.contains("pr-gate:build-reason-excluded"), "should tag excluded builds");
    }

    #[test]
    fn test_agentic_depends_on_with_expression() {
        let result = generate_agentic_depends_on(
            &[],
            false,
            false,
            Some("eq(variables['Custom.ShouldRun'], 'true')"),
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
            Some("eq(variables['Custom.Flag'], 'yes')"),
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
            Some("eq(variables['Run'], 'true')"),
        );
        // No setup steps, no PR filters — no dependsOn, but still a condition
        assert!(!result.contains("dependsOn"), "no dependsOn without setup/filters");
        assert!(result.contains("condition:"), "should have condition from expression");
    }

    #[test]
    fn test_gate_step_change_count_reuses_changed_files_data() {
        let filters = PrFilters {
            changed_files: Some(IncludeExcludeFilter {
                include: vec!["src/**".into()],
                ..Default::default()
            }),
            min_changes: Some(3),
            ..Default::default()
        };
        let result = generate_pr_gate_step(&filters);
        // Should use CHANGED_FILES from the changed-files filter, not make a new API call
        assert!(result.contains("grep -c ."), "should count from existing CHANGED_FILES");
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
        let filters = PrFilters {
            commit_message: Some(PatternFilter { pattern: "^(?!.*\\[skip-agent\\])".into() }),
            ..Default::default()
        };
        let result = generate_pr_gate_step(&filters);
        assert!(result.contains("Build.SourceVersionMessage"), "should check commit message variable");
        assert!(result.contains("skip-agent"), "should include the pattern");
        assert!(result.contains("pr-gate:commit-message-mismatch"), "should tag commit-message failures");
    }

    #[test]
    fn test_on_config_deserialization_with_schedule() {
        let yaml = r#"
on:
  schedule: daily around 14:00
  pr:
    filters:
      title:
        match: "\\[review\\]"
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
      title:
        match: "\\[agent\\]"
      commit-message:
        match: "^(?!.*\\[skip-agent\\])"
"#;
        let val: serde_yaml::Value = serde_yaml::from_str(yaml).unwrap();
        let oc: OnConfig = serde_yaml::from_value(val["on"].clone()).unwrap();
        let schedule = oc.schedule.unwrap();
        assert_eq!(schedule.expression(), "weekly on monday");
        let pipeline = oc.pipeline.unwrap();
        assert_eq!(pipeline.name, "Build Pipeline");
        let pr = oc.pr.unwrap();
        let filters = pr.filters.unwrap();
        assert_eq!(filters.title.unwrap().pattern, "\\[agent\\]");
        assert_eq!(filters.commit_message.unwrap().pattern, "^(?!.*\\[skip-agent\\])");
    }
}
