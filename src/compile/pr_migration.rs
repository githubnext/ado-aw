use std::collections::{BTreeMap, HashSet};

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use super::types::FrontMatter;

pub const LEGACY_PR_CONFIG: &str = "legacy-update-pr";
pub const PR_OPERATIONS: &[(&str, &str)] = &[
    ("add-reviewers", "add-pr-reviewers"),
    ("add-labels", "add-pr-labels"),
    ("set-auto-complete", "set-pr-auto-complete"),
    ("vote", "submit-pr-review"),
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

/// Pure, atomic normalization shared by the codemod and historical execution.
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

/// Keep historical proposals executable without advertising the old MCP tool.
pub fn normalize_execution_context(ctx: &mut crate::safe_outputs::ExecutionContext) -> Result<()> {
    let mut outputs: Map<String, Value> = ctx.tool_configs.clone().into_iter().collect();
    if !ctx.budget_groups.is_empty() {
        outputs.insert(
            "budget-groups".to_string(),
            serde_json::to_value(&ctx.budget_groups)?,
        );
    }
    migrate_safe_outputs(&mut outputs)?;
    ctx.budget_groups = outputs
        .remove("budget-groups")
        .map(serde_json::from_value)
        .transpose()?
        .unwrap_or_default();
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
    let legacy = outputs
        .values()
        .filter_map(|config| {
            config
                .get(LEGACY_PR_CONFIG)
                .map(|original| (original, config))
        })
        .collect::<Vec<_>>();
    if let Some((original, effective)) = legacy.first() {
        ensure!(
            legacy.iter().all(|(config, _)| config == original),
            "conflicting legacy update-pr execution policies"
        );
        let mut original = (*original).clone();
        if let (Some(object), Some(staged)) = (original.as_object_mut(), effective.get("staged")) {
            object.insert("staged".to_string(), staged.clone());
        }
        outputs.insert("update-pr".to_string(), original);
    }
    ctx.tool_configs = outputs.into_iter().collect();
    Ok(())
}

pub fn deprecated_pr_prompt_lines(body: &str) -> Vec<usize> {
    body.lines()
        .enumerate()
        .filter_map(|(line, text)| {
            text.split(|c: char| {
                c.is_whitespace() || (!c.is_alphanumeric() && c != '-' && c != '_')
            })
            .any(|word| matches!(word, "update-pr" | "update_pr"))
            .then_some(line + 1)
        })
        .collect()
}

pub const PR_PROMPT_GUIDANCE: &str = "update-pr is no longer an agent tool: use update-pull-request for content, \
     add-pr-reviewers for reviewers, add-pr-labels for labels, set-pr-auto-complete \
     for auto-complete, and submit-pr-review for votes. Update the prompt manually; \
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
        assert_eq!(outputs["add-pr-reviewers"]["max-reviewers"], 2);
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
            "submit-pr-review": {"allowed-events": ["approve"]}
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
        let config = &parsed.front_matter.safe_outputs["submit-pr-review"];
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
        fm.safe_outputs.get_mut("add-pr-labels").unwrap()["require-approval"] = json!(true);
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
                "add-pr-reviewers",
                json!({"allowed-reviewers":[""]}),
                "allowed-reviewers",
            ),
            (
                "add-pr-labels",
                json!({"allowed-repositories":[""]}),
                "allowed-repositories",
            ),
            (
                "set-pr-auto-complete",
                json!({"merge-strategy":"invalid"}),
                "merge-strategy",
            ),
            (
                "submit-pr-review",
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
