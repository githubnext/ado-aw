use std::collections::{BTreeMap, HashSet};

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use super::types::FrontMatter;

pub const LEGACY_PR_CONFIG: &str = "legacy-update-pr";
pub const PR_TOOL_RENAMES: &[(&str, &str)] = &[
    ("add-pr-comment", "add-pull-request-comment"),
    ("reply-to-pr-comment", "reply-to-pull-request-comment"),
    ("resolve-pr-thread", "resolve-pull-request-thread"),
    ("submit-pr-review", "submit-pull-request-review"),
    ("add-pr-reviewers", "add-pull-request-reviewers"),
    ("add-pr-labels", "add-pull-request-labels"),
    ("set-pr-auto-complete", "set-pull-request-auto-complete"),
];
pub const PR_OPERATIONS: &[(&str, &str)] = &[
    ("add-reviewers", "add-pull-request-reviewers"),
    ("add-labels", "add-pull-request-labels"),
    ("set-auto-complete", "set-pull-request-auto-complete"),
    ("vote", "submit-pull-request-review"),
    ("update-description", "update-pull-request"),
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BudgetGroup {
    pub max: usize,
    pub tools: Vec<String>,
}

pub type BudgetGroups = BTreeMap<String, BudgetGroup>;

pub fn focused_pr_tool(operation: &str) -> Option<&'static str> {
    PR_OPERATIONS
        .iter()
        .find_map(|(old, new)| (*old == operation).then_some(*new))
}

/// Split the legacy front-matter declaration without widening its policy.
pub fn migrate_safe_outputs(outputs: &mut Map<String, Value>) -> Result<bool> {
    let Some(raw) = outputs.get("update-pr") else {
        return Ok(false);
    };
    let original = match raw {
        Value::Null | Value::Bool(true) => Map::new(),
        Value::Object(config) => config.clone(),
        _ => bail!("safe-outputs.update-pr must be an object or null before migration"),
    };
    let operations: Vec<String> = original
        .get("allowed-operations")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .context("update-pr.allowed-operations must be a list of operation names")?
        .unwrap_or_default();
    for operation in &operations {
        ensure!(
            focused_pr_tool(operation).is_some(),
            "cannot migrate unknown update-pr operation '{operation}'"
        );
    }
    let selected: Vec<_> = PR_OPERATIONS
        .iter()
        .filter(|(operation, _)| {
            operations.is_empty() || operations.iter().any(|allowed| allowed == operation)
        })
        .copied()
        .collect();
    let max = original
        .get("max")
        .cloned()
        .map(serde_json::from_value::<usize>)
        .transpose()
        .context("update-pr.max must be a non-negative integer fitting usize")?
        .unwrap_or(1);
    let votes: Vec<String> = original
        .get("allowed-votes")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .context("update-pr.allowed-votes must be a list")?
        .unwrap_or_default();

    let mut migrated = outputs.clone();
    let mut groups: BudgetGroups = outputs
        .get("budget-groups")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .context("safe-outputs.budget-groups has invalid configuration")?
        .unwrap_or_default();
    ensure!(
        !groups.contains_key("update-pr"),
        "manual migration required: update-pr budget group already exists"
    );
    for (operation, tool) in &selected {
        ensure!(
            !migrated.contains_key(*tool),
            "manual migration required: both update-pr and {tool} are configured; \
             their permissions, budgets and approval policies cannot be silently combined"
        );
        let mut config = Map::new();
        for key in ["allowed-repositories", "max", "require-approval", "staged"] {
            if let Some(value) = original.get(key) {
                config.insert(key.to_string(), value.clone());
            }
        }
        match *operation {
            "add-reviewers" => {
                for key in ["allowed-reviewers", "max-reviewers"] {
                    if let Some(value) = original.get(key) {
                        config.insert(key.to_string(), value.clone());
                    }
                }
            }
            "set-auto-complete" => {
                for key in ["delete-source-branch", "merge-strategy"] {
                    if let Some(value) = original.get(key) {
                        config.insert(key.to_string(), value.clone());
                    }
                }
            }
            "vote" => {
                config.insert("allowed-events".to_string(), json!(votes));
                config.insert("allow-temporary-ids".to_string(), Value::Bool(true));
            }
            "update-description" => {
                config.insert("title".to_string(), Value::Bool(false));
                config.insert("body".to_string(), Value::Bool(true));
                config.insert("target".to_string(), json!("*"));
                config.insert("operation".to_string(), json!("replace"));
                config.insert("include-stats".to_string(), Value::Bool(false));
            }
            _ => {}
        }
        config.insert(
            LEGACY_PR_CONFIG.to_string(),
            Value::Object(original.clone()),
        );
        migrated.insert((*tool).to_string(), Value::Object(config));
    }
    groups.insert(
        "update-pr".to_string(),
        BudgetGroup {
            max,
            tools: selected
                .iter()
                .map(|(_, tool)| (*tool).to_string())
                .collect(),
        },
    );
    migrated.insert("budget-groups".to_string(), serde_json::to_value(groups)?);
    migrated.remove("update-pr");
    *outputs = migrated;
    Ok(true)
}

pub fn budget_groups(front_matter: &FrontMatter) -> Result<BudgetGroups> {
    front_matter
        .safe_outputs
        .get("budget-groups")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .context("safe-outputs.budget-groups has invalid configuration")
        .map(Option::unwrap_or_default)
}

pub fn validate_budget_groups(front_matter: &FrontMatter) -> Result<()> {
    let mut members = HashSet::new();
    for (name, group) in budget_groups(front_matter)? {
        ensure!(
            !name.trim().is_empty(),
            "budget group name must not be empty"
        );
        ensure!(
            !group.tools.is_empty(),
            "budget group '{name}' must contain tools"
        );
        crate::validate::reject_pipeline_injection(&name, "budget group")?;
        let first = &group.tools[0];
        for tool in &group.tools {
            ensure!(
                front_matter
                    .safe_output_tool_names()
                    .any(|configured| configured == tool),
                "budget group '{name}' references unconfigured tool '{tool}'"
            );
            ensure!(
                PR_OPERATIONS
                    .iter()
                    .any(|(_, candidate)| *candidate == tool),
                "budget group '{name}' may only contain focused PR tools"
            );
            ensure!(
                members.insert(tool.clone()),
                "tool '{tool}' belongs to multiple budget groups"
            );
            ensure!(
                front_matter.tool_requires_approval(tool).is_some()
                    == front_matter.tool_requires_approval(first).is_some()
                    && front_matter.tool_is_staged(tool) == front_matter.tool_is_staged(first),
                "budget group '{name}' must share the same effective require-approval and staged settings"
            );
        }
    }
    Ok(())
}

pub fn validate_execution_budget_groups(ctx: &crate::safe_outputs::ExecutionContext) -> Result<()> {
    let outputs = &ctx.tool_configs;
    let mut seen = HashSet::new();
    for (name, group) in &ctx.budget_groups {
        ensure!(
            !name.trim().is_empty(),
            "budget group name must not be empty"
        );
        crate::validate::reject_pipeline_injection(name, "budget group")?;
        ensure!(
            !group.tools.is_empty(),
            "budget group '{name}' must contain tools"
        );
        let first_staged = outputs
            .get(&group.tools[0])
            .and_then(|config| config.get("staged"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        for tool in &group.tools {
            ensure!(
                outputs.contains_key(tool),
                "budget group '{name}' references unconfigured tool '{tool}'"
            );
            ensure!(
                PR_OPERATIONS.iter().any(|(_, member)| *member == tool),
                "budget group '{name}' contains unsupported tool '{tool}'"
            );
            ensure!(
                seen.insert(tool),
                "tool '{tool}' belongs to multiple budget groups"
            );
            ensure!(
                outputs
                    .get(tool)
                    .and_then(|config| config.get("staged"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                    == first_staged,
                "budget group '{name}' must share the same staged setting"
            );
        }
    }
    Ok(())
}

pub fn rename_pr_tools(outputs: &mut Map<String, Value>) -> Result<bool> {
    let mut renamed = outputs.clone();
    let mut changed = false;
    for (old, new) in PR_TOOL_RENAMES {
        if let Some(config) = renamed.get(*old).cloned() {
            ensure!(
                !renamed.contains_key(*new),
                "manual migration required: both {old} and {new} are configured"
            );
            renamed.remove(*old);
            renamed.insert((*new).to_string(), config);
            changed = true;
        }
    }
    if let Some(raw_groups) = renamed.get("budget-groups") {
        let mut groups: BudgetGroups = serde_json::from_value(raw_groups.clone())
            .context("safe-outputs.budget-groups has invalid configuration")?;
        let mut groups_changed = false;
        for group in groups.values_mut() {
            for tool in &mut group.tools {
                if let Some((_, new)) = PR_TOOL_RENAMES.iter().find(|(old, _)| *old == tool) {
                    *tool = (*new).to_string();
                    groups_changed = true;
                }
            }
        }
        if groups_changed {
            renamed.insert("budget-groups".to_string(), serde_json::to_value(groups)?);
            changed = true;
        }
    }
    if changed {
        *outputs = renamed;
    }
    Ok(changed)
}

pub fn is_deprecated_pr_tool(name: &str) -> bool {
    matches!(name, "update-pr" | "update_pr")
        || PR_TOOL_RENAMES
            .iter()
            .any(|(old, _)| name == *old || name == old.replace('-', "_"))
}

pub fn deprecated_pr_prompt_lines(body: &str) -> Vec<usize> {
    body.lines()
        .enumerate()
        .filter_map(|(line, text)| {
            text.split(|c: char| {
                c.is_whitespace() || (!c.is_alphanumeric() && c != '-' && c != '_')
            })
            .any(is_deprecated_pr_tool)
            .then_some(line + 1)
        })
        .collect()
}

pub const PR_PROMPT_GUIDANCE: &str = "PR tools now use pull-request, not pr. Use \
     add-pull-request-comment, reply-to-pull-request-comment and resolve-pull-request-thread. \
     For former update-pr operations use update-pull-request for content, \
     add-pull-request-reviewers for reviewers, add-pull-request-labels for labels, set-pull-request-auto-complete \
     for auto-complete, and submit-pull-request-review for votes. Update the prompt manually; \
     its text has not been rewritten.";

pub fn warn_prompt_references(source: &std::path::Path, content: &str, body: &str) {
    let prefix = content.len().saturating_sub(body.len());
    let offset = content
        .get(..prefix)
        .map_or(0, |text| text.lines().count().saturating_sub(1));
    for line in deprecated_pr_prompt_lines(body) {
        eprintln!(
            "warning: {}:{}: deprecated-tool-reference: {PR_PROMPT_GUIDANCE}",
            crate::sanitize::neutralize_pipeline_commands(&source.display().to_string()),
            offset + line
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renamed_tools_preserve_configuration_and_are_idempotent() {
        for (old, new) in PR_TOOL_RENAMES {
            let config = json!({
                "max": 2, "require-approval": {"approvers": ["reviewers"]},
                "staged": false, "allowed-repositories": ["self"]
            });
            let mut outputs = Map::from_iter([((*old).to_string(), config.clone())]);
            assert!(rename_pr_tools(&mut outputs).unwrap());
            assert_eq!(outputs.get(*new), Some(&config));
            assert!(!outputs.contains_key(*old));
            let snapshot = outputs.clone();
            assert!(!rename_pr_tools(&mut outputs).unwrap());
            assert_eq!(outputs, snapshot);
        }
    }

    #[test]
    fn renamed_budget_members_keep_the_same_limit() {
        let mut outputs = json!({
            "add-pr-reviewers": {"max": 2, "legacy-update-pr": {"allowed-operations":["add-reviewers"]}},
            "add-pr-labels": {"max": 2},
            "budget-groups": {"update-pr": {"max": 2, "tools": ["add-pr-reviewers", "add-pr-labels"]}}
        }).as_object().unwrap().clone();
        assert!(rename_pr_tools(&mut outputs).unwrap());
        assert_eq!(
            outputs["budget-groups"]["update-pr"],
            json!({
                "max": 2, "tools": ["add-pull-request-reviewers", "add-pull-request-labels"]
            })
        );
        assert_eq!(
            outputs["add-pull-request-reviewers"]["legacy-update-pr"],
            json!({
                "allowed-operations": ["add-reviewers"]
            })
        );
        assert!(!rename_pr_tools(&mut outputs).unwrap());
    }

    #[test]
    fn name_conflicts_do_not_partially_rename_other_tools() {
        let mut outputs = json!({
            "add-pr-comment": {"max": 2},
            "add-pr-labels": {"max": 1},
            "add-pull-request-labels": {"max": 5}
        })
        .as_object()
        .unwrap()
        .clone();
        let original = outputs.clone();
        let error = rename_pr_tools(&mut outputs).unwrap_err().to_string();
        assert!(error.contains("manual migration required"));
        assert!(error.contains("add-pr-labels"));
        assert!(error.contains("add-pull-request-labels"));
        assert_eq!(outputs, original);
    }

    #[test]
    fn every_abbreviated_prompt_name_is_detected_but_canonical_names_are_not() {
        for (old, new) in PR_TOOL_RENAMES {
            assert_eq!(
                deprecated_pr_prompt_lines(&format!("Call `{old}`.")),
                vec![1]
            );
            assert_eq!(
                deprecated_pr_prompt_lines(&format!("Call `{}`.", old.replace('-', "_"))),
                vec![1]
            );
            assert!(
                deprecated_pr_prompt_lines(&format!(
                    "Call `{new}`; not prefix_{old} or {old}-helper."
                ))
                .is_empty()
            );
        }
    }

    #[test]
    fn migration_preserves_policy_and_shared_budget() {
        let mut outputs = json!({
            "update-pr": {
                "allowed-operations": ["add-reviewers", "update-description"],
                "allowed-reviewers": ["owner@example.test"],
                "max-reviewers": 2,
                "max": 1,
                "require-approval": true,
                "staged": true
            }
        })
        .as_object()
        .unwrap()
        .clone();
        assert!(migrate_safe_outputs(&mut outputs).unwrap());
        assert!(!outputs.contains_key("update-pr"));
        assert_eq!(outputs["add-pull-request-reviewers"]["max-reviewers"], 2);
        assert_eq!(outputs["update-pull-request"]["title"], false);
        assert_eq!(outputs["update-pull-request"]["include-stats"], false);
        assert_eq!(outputs["update-pull-request"]["target"], "*");
        assert_eq!(outputs["budget-groups"]["update-pr"]["max"], 1);
        assert_eq!(
            outputs["budget-groups"]["update-pr"]["tools"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        let snapshot = outputs.clone();
        assert!(!migrate_safe_outputs(&mut outputs).unwrap());
        assert_eq!(outputs, snapshot);
    }

    #[test]
    fn migration_conflict_is_atomic() {
        let mut outputs = json!({
            "update-pr": {"allowed-operations": ["vote"], "allowed-votes": ["reset"]},
            "submit-pull-request-review": {"allowed-events": ["approve"]}
        })
        .as_object()
        .unwrap()
        .clone();
        let before = outputs.clone();
        assert!(
            migrate_safe_outputs(&mut outputs)
                .unwrap_err()
                .to_string()
                .contains("manual migration")
        );
        assert_eq!(outputs, before);
    }

    #[test]
    fn prompt_detection_includes_code_but_not_other_identifiers() {
        assert_eq!(
            deprecated_pr_prompt_lines(
                "Call `update-pr`.\r\n```json\n{\"name\":\"update_pr\"}\n```\nupdate-pull-request update-pr-other prefix_update_pr"
            ),
            vec![1, 3]
        );
    }

    #[test]
    fn codemod_preserves_body_and_original_review_contract() {
        let source = "---\nname: test\ndescription: test\nsafe-outputs:\n  update-pr:\n    allowed-operations: [vote]\n    allowed-votes: [wait-for-author, reject, reset]\n    max: 2\n---\r\nCall `update-pr` with #aw_created.\r\n";
        let parsed = crate::compile::parse_markdown_detailed(source).unwrap();
        assert!(parsed.codemods.changed());
        assert_eq!(
            parsed.body_raw,
            "\r\nCall `update-pr` with #aw_created.\r\n"
        );
        let config = &parsed.front_matter.safe_outputs["submit-pull-request-review"];
        assert_eq!(
            config["allowed-events"],
            json!(["wait-for-author", "reject", "reset"])
        );
        assert_eq!(config["allow-temporary-ids"], true);
        assert_eq!(config[LEGACY_PR_CONFIG]["max"], 2);
    }

    #[test]
    fn budget_group_survives_resolved_config_and_rejects_split_lanes() {
        let source = "---\nname: test\ndescription: test\nsafe-outputs:\n  update-pr:\n    allowed-operations: [add-labels, update-description]\n    max: 0\n---\nbody\n";
        let parsed = crate::compile::parse_markdown_detailed(source).unwrap();
        let mut fm = parsed.front_matter;
        validate_budget_groups(&fm).unwrap();
        let raw = crate::compile::custom_tools::resolved_execution_config_json(&fm, &[]).unwrap();
        let config: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(config["budgetGroups"]["update-pr"]["max"], 0);
        fm.safe_outputs.get_mut("add-pull-request-labels").unwrap()["require-approval"] =
            json!(true);
        assert!(
            validate_budget_groups(&fm)
                .unwrap_err()
                .to_string()
                .contains("require-approval")
        );
    }

    #[test]
    fn focused_config_validators_run_during_compilation() {
        for (tool, config, expected) in [
            (
                "add-pull-request-reviewers",
                json!({"allowed-reviewers":[""]}),
                "allowed-reviewers",
            ),
            (
                "add-pull-request-labels",
                json!({"allowed-repositories":[""]}),
                "allowed-repositories",
            ),
            (
                "set-pull-request-auto-complete",
                json!({"merge-strategy":"invalid"}),
                "merge-strategy",
            ),
            (
                "submit-pull-request-review",
                json!({"allowed-events":["invalid"]}),
                "event",
            ),
        ] {
            let source = format!(
                "---\nname: test\ndescription: test\nsafe-outputs:\n  {tool}: {config}\n---\nbody\n"
            );
            let parsed = crate::compile::parse_markdown_detailed(&source).unwrap();
            let error =
                crate::compile::common::validate_pull_request_outputs_config(&parsed.front_matter)
                    .unwrap_err();
            assert!(error.to_string().contains(expected), "{tool}: {error}");
        }
    }
}
