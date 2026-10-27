//! Non-authoritative early PR policy checks and durable queue budget accounting.

use anyhow::{Context, Result, ensure};
use rmcp::model::{CallToolResult, Content};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

use crate::compile::{FrontMatter, pr_migration::BudgetGroups};
use crate::safe_outputs::pr_policy;

const POLICY_FIELDS: &[&str] = &[
    "target",
    "default-repository",
    "allowed-repositories",
    "allowed-labels",
    "allowed-add",
    "allowed-remove",
    "blocked-labels",
    "allowed-transitions",
    "allowed-reviewers",
    "allowed-events",
    "allowed-statuses",
    "allowed-operations",
    "default-operation",
    "title",
    "title-prefix",
    "body",
    "max",
    "max-labels",
    "max-reviewers",
    "max-comments",
    "draft",
    "auto-complete",
    "merge-strategy",
    "delete-source-branch",
    "reviewers",
    "labels",
    "work-items",
    "required-labels",
    "required-title-prefix",
    "allowed-branches",
    "max-files",
    "max-patch-size",
    "protected-files",
    "excluded-files",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PrAdmissionPolicy {
    pub schema_version: u32,
    pub tools: BTreeMap<String, Value>,
    pub budget_groups: BudgetGroups,
    #[serde(default)]
    pub repositories: BTreeMap<String, String>,
}

impl PrAdmissionPolicy {
    pub(crate) fn from_front_matter(fm: &FrontMatter) -> Result<Option<Self>> {
        let mut tools = BTreeMap::new();
        for (tool, config) in &fm.safe_outputs {
            if !pr_policy::is_pr_tool(tool) {
                continue;
            }
            let mut projected = serde_json::Map::new();
            for field in POLICY_FIELDS {
                if let Some(value) = config.get(*field) {
                    if value.is_null()
                        && matches!(
                            *field,
                            "delete-source-branch"
                                | "merge-strategy"
                                | "default-operation"
                                | "default-repository"
                        )
                    {
                        continue;
                    }
                    projected.insert((*field).into(), value.clone());
                }
            }
            projected.insert(
                "require-approval".into(),
                json!(fm.tool_requires_approval(tool).is_some()),
            );
            projected.insert("staged".into(), json!(fm.tool_is_staged(tool)));
            tools.insert(tool.clone(), Value::Object(projected));
        }
        if tools.is_empty() {
            return Ok(None);
        }
        let mut repositories = BTreeMap::from([("self".into(), String::new())]);
        for repository in &fm.repositories {
            if fm.checkout.contains(&repository.repository) {
                repositories.insert(repository.repository.clone(), repository.name.clone());
            }
        }
        Ok(Some(Self {
            schema_version: 1,
            tools,
            repositories,
            budget_groups: crate::compile::pr_migration::budget_groups(fm)?,
        }))
    }

    pub(crate) fn load(path: &std::path::Path) -> Result<Option<Self>> {
        let document: Value = serde_json::from_slice(&std::fs::read(path)?)
            .context("Invalid MCP tool configuration")?;
        let Some(value) = document.get("prPolicy") else {
            return Ok(None);
        };
        let policy: Self =
            serde_json::from_value(value.clone()).context("Invalid PR admission policy")?;
        ensure!(
            policy.schema_version == 1,
            "Unsupported PR admission policy version"
        );
        ensure!(
            policy.tools.keys().all(|tool| pr_policy::is_pr_tool(tool)),
            "Admission policy contains an unknown PR tool"
        );
        for config in policy.tools.values() {
            let config = config
                .as_object()
                .context("PR admission tool policy must be an object")?;
            ensure!(
                config
                    .keys()
                    .all(|key| POLICY_FIELDS.contains(&key.as_str())
                        || ["require-approval", "staged"].contains(&key.as_str())),
                "PR admission policy contains an unsupported field"
            );
            for field in [
                "title",
                "body",
                "draft",
                "auto-complete",
                "delete-source-branch",
                "require-approval",
                "staged",
            ] {
                ensure!(
                    config.get(field).is_none_or(Value::is_boolean),
                    "Invalid boolean PR admission policy field"
                );
            }
            for field in [
                "max",
                "max-labels",
                "max-reviewers",
                "max-comments",
                "max-files",
                "max-patch-size",
            ] {
                ensure!(
                    config
                        .get(field)
                        .is_none_or(|value| value.is_null() || value.as_u64().is_some()),
                    "Invalid numeric PR admission policy field"
                );
            }
            for field in [
                "allowed-repositories",
                "allowed-labels",
                "allowed-reviewers",
                "allowed-add",
                "allowed-remove",
                "blocked-labels",
            ] {
                if let Some(value) = config.get(field) {
                    let names: Vec<String> = serde_json::from_value(value.clone())
                        .with_context(|| format!("Invalid name allowance {field}"))?;
                    pr_policy::validate_names(&names, field)?;
                }
            }
        }
        Ok(Some(policy))
    }

    pub(crate) fn usable(&self, tool: &str) -> bool {
        self.tools
            .get(tool)
            .is_some_and(|config| pr_policy::has_usable_action(tool, config))
            && !self
                .budget_groups
                .values()
                .any(|group| group.max == 0 && group.tools.iter().any(|member| member == tool))
    }

    pub(crate) fn validate(
        &self,
        tool: &str,
        proposal: &Value,
        queued: &[Value],
    ) -> Result<Vec<String>> {
        let config = self
            .tools
            .get(tool)
            .context("PR tool is not enabled by author configuration")?;
        ensure!(
            pr_policy::has_usable_action(tool, config),
            "PR tool has no usable effective grant"
        );
        if tool == "create-pull-request" {
            let title = proposal
                .get("title")
                .and_then(Value::as_str)
                .context("Creation proposal requires a title")?;
            let prefix = config
                .get("title-prefix")
                .map(|value| value.as_str().context("title-prefix must be a string"))
                .transpose()?
                .unwrap_or_default();
            crate::safe_outputs::pr_common::validate_title(&format!("{prefix}{title}"))?;
        }
        let reference = proposal
            .get("pull_request_id")
            .filter(|value| !value.is_null());
        let target = config.get("target").cloned().unwrap_or(json!("triggering"));
        if target == "created" {
            let reference =
                reference.context("target 'created' requires an explicit creation handle")?;
            ensure!(
                matches!(
                    serde_json::from_value::<crate::safe_outputs::pr_common::PullRequestReference>(
                        reference.clone()
                    )?,
                    crate::safe_outputs::pr_common::PullRequestReference::Temporary(_)
                ),
                "target 'created' accepts only the handle returned by creation"
            );
        } else if target == "*" {
            ensure!(
                reference.is_some(),
                "pull_request_id is required for target '*'"
            );
        } else if target != "triggering" {
            let expected: crate::safe_outputs::pr_common::PullRequestReference =
                serde_json::from_value(target).context("Invalid configured PR target")?;
            if let Some(reference) = reference {
                let requested = serde_json::from_value::<
                    crate::safe_outputs::pr_common::PullRequestReference,
                >(reference.clone())?;
                if matches!(
                    requested,
                    crate::safe_outputs::pr_common::PullRequestReference::Number(_)
                ) {
                    ensure!(
                        requested == expected,
                        "pull_request_id does not match the configured target"
                    );
                }
            }
        }
        let mut dependencies = Vec::new();
        let mut producer_repository = None;
        if let Some(reference) = reference {
            let reference: crate::safe_outputs::pr_common::PullRequestReference =
                serde_json::from_value(reference.clone())?;
            if let crate::safe_outputs::pr_common::PullRequestReference::Temporary(handle) =
                reference
            {
                let producer = queued
                    .iter()
                    .find(|entry| {
                        entry["name"] == "create-pull-request"
                            && entry["temporary_id"].as_str() == Some(handle.canonical().as_str())
                    })
                    .context("Creation handle has not been queued by create-pull-request")?;
                let creator = self
                    .tools
                    .get("create-pull-request")
                    .context("create-pull-request is not configured")?;
                ensure!(
                    config["staged"] == creator["staged"]
                        && config["require-approval"] == creator["require-approval"],
                    "Creation handle and consumer must use the same approval and staged lanes"
                );
                dependencies.push(
                    producer
                        .get("context")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                        .unwrap_or_else(|| handle.canonical()),
                );
                producer_repository = Some(
                    producer
                        .get("repository")
                        .and_then(Value::as_str)
                        .unwrap_or("self"),
                );
            }
        }
        let grant = |field: &str| -> Result<Vec<String>> {
            config
                .get(field)
                .cloned()
                .map(serde_json::from_value)
                .transpose()
                .with_context(|| format!("Invalid {field} grant"))
                .map(|value| value.unwrap_or_else(pr_policy::unrestricted_names))
        };
        let names = |field: &str, value: &Value| -> Result<()> {
            let values: Vec<String> =
                serde_json::from_value(value.clone()).context("Expected a list of names")?;
            let allowed = grant(field)?;
            pr_policy::validate_names(&allowed, field)?;
            for name in values {
                ensure!(
                    pr_policy::permits_name(&allowed, &name),
                    "Requested name is not permitted by {field}"
                );
            }
            Ok(())
        };
        let selector = proposal
            .get("repository")
            .and_then(Value::as_str)
            .or(producer_repository)
            .or_else(|| (tool == "create-pull-request").then_some("self"))
            .or_else(|| {
                (config
                    .get("target")
                    .is_some_and(|target| target != "triggering"))
                .then(|| config.get("default-repository").and_then(Value::as_str))
                .flatten()
            });
        if let Some(selector) = selector {
            ensure!(!selector.trim().is_empty(), "repository must not be blank");
            if let Some(alias) = self.repository_alias(selector) {
                let allowed = grant("allowed-repositories")?;
                ensure!(
                    allowed
                        .iter()
                        .any(|name| name == "*" || self.repository_alias(name) == Some(alias)),
                    "Repository is not permitted by allowed-repositories"
                );
                if let Some(producer) = producer_repository
                    && let Some(producer_alias) = self.repository_alias(producer)
                {
                    ensure!(
                        alias == producer_alias,
                        "Repository contradicts the creation handle destination"
                    );
                }
            }
        }
        if let Some(labels) = proposal
            .get("labels")
            .or_else(|| proposal.get("agent_labels"))
        {
            names("allowed-labels", labels)?;
            let values: Vec<String> = serde_json::from_value(labels.clone())?;
            let blocked: Vec<String> = config
                .get("blocked-labels")
                .cloned()
                .map(serde_json::from_value)
                .transpose()?
                .unwrap_or_default();
            ensure!(
                !values
                    .iter()
                    .any(|name| pr_policy::permits_name(&blocked, name)),
                "Requested label is blocked"
            );
            if tool != "create-pull-request" {
                let max = config
                    .get("max-labels")
                    .and_then(Value::as_u64)
                    .unwrap_or(10);
                let unique: std::collections::BTreeSet<_> = values
                    .iter()
                    .map(|name| name.trim().to_ascii_lowercase())
                    .collect();
                ensure!(
                    u64::try_from(unique.len())? <= max,
                    "Label batch exceeds max-labels"
                );
            }
        }
        if let Some(reviewers) = proposal.get("reviewers") {
            names("allowed-reviewers", reviewers)?;
            let values: Vec<String> = serde_json::from_value(reviewers.clone())?;
            let unique: std::collections::BTreeSet<_> = values
                .iter()
                .map(|name| name.trim().to_ascii_lowercase())
                .collect();
            ensure!(
                u64::try_from(unique.len())?
                    <= config
                        .get("max-reviewers")
                        .and_then(Value::as_u64)
                        .unwrap_or(3),
                "Reviewer batch exceeds max-reviewers"
            );
        }
        if tool == "replace-pull-request-label" {
            let from = proposal["from"]
                .as_str()
                .context("Replacement requires from")?;
            let to = proposal["to"].as_str().context("Replacement requires to")?;
            names("allowed-remove", &json!([from]))?;
            names("allowed-add", &json!([to]))?;
            let blocked: Vec<String> = config
                .get("blocked-labels")
                .cloned()
                .map(serde_json::from_value)
                .transpose()?
                .unwrap_or_default();
            ensure!(
                !pr_policy::permits_name(&blocked, from) && !pr_policy::permits_name(&blocked, to),
                "Replacement label is blocked"
            );
            if let Some(transitions) = config.get("allowed-transitions") {
                let transitions = transitions
                    .as_array()
                    .context("allowed-transitions must be a list")?;
                ensure!(
                    transitions.iter().any(|transition| {
                        transition["from"]
                            .as_str()
                            .is_some_and(|value| value.trim().eq_ignore_ascii_case(from.trim()))
                            && transition["to"]
                                .as_str()
                                .is_some_and(|value| value.trim().eq_ignore_ascii_case(to.trim()))
                    }),
                    "Requested transition is not permitted"
                );
            }
        }
        for (field, input, default) in [
            ("allowed-events", "event", "comment"),
            (
                "allowed-statuses",
                "status",
                if tool == "resolve-pull-request-thread" {
                    "fixed"
                } else {
                    "active"
                },
            ),
        ] {
            if let Some(value) = proposal.get(input).and_then(Value::as_str) {
                let permitted: Vec<String> = config
                    .get(field)
                    .cloned()
                    .map(serde_json::from_value)
                    .transpose()?
                    .unwrap_or_else(|| vec![default.into()]);
                ensure!(
                    permitted
                        .iter()
                        .any(|allowed| allowed.eq_ignore_ascii_case(value)),
                    "Requested {input} is not permitted by {field}"
                );
            }
        }
        if let Some(comments) = proposal.get("comments").and_then(Value::as_array) {
            ensure!(
                u64::try_from(comments.len())?
                    <= config
                        .get("max-comments")
                        .and_then(Value::as_u64)
                        .unwrap_or(0),
                "Inline comments exceed max-comments"
            );
        }
        if tool == "update-pull-request" {
            ensure!(
                !proposal["title"].is_null() || !proposal["body"].is_null(),
                "An update requires title or body"
            );
            for field in ["title", "body"] {
                ensure!(
                    proposal[field].is_null() || config[field] != false,
                    "Requested update field is disabled"
                );
            }
            if !proposal["body"].is_null() {
                let allowed: Vec<String> = config
                    .get("allowed-operations")
                    .cloned()
                    .map(serde_json::from_value)
                    .transpose()?
                    .unwrap_or_else(|| vec!["replace".into()]);
                let operation = proposal
                    .get("operation")
                    .and_then(Value::as_str)
                    .or_else(|| config.get("default-operation").and_then(Value::as_str))
                    .or_else(|| (allowed.len() == 1).then(|| allowed[0].as_str()));
                ensure!(
                    operation.is_some_and(|operation| allowed
                        .iter()
                        .any(|allowed| allowed == operation)),
                    "Requested body operation is not permitted by allowed-operations"
                );
            }
        }
        Ok(dependencies)
    }

    fn repository_alias(&self, selector: &str) -> Option<&str> {
        if let Some((alias, _)) = self.repositories.iter().find(|(alias, name)| {
            alias.eq_ignore_ascii_case(selector)
                || (!name.is_empty() && name.eq_ignore_ascii_case(selector))
        }) {
            return Some(alias);
        }
        let mut matches = self.repositories.iter().filter(|(_, name)| {
            !name.is_empty()
                && name
                    .rsplit('/')
                    .next()
                    .is_some_and(|name| name.eq_ignore_ascii_case(selector))
        });
        let alias = matches.next()?.0.as_str();
        matches.next().is_none().then_some(alias)
    }
}

pub(crate) fn shared_budget_remaining(
    groups: &BudgetGroups,
    tool: &str,
    entries: &[Value],
) -> Result<BTreeMap<String, usize>> {
    let mut remaining = BTreeMap::new();
    for (name, group) in groups {
        if !group.tools.iter().any(|member| member == tool) {
            continue;
        }
        let used = entries
            .iter()
            .filter(|entry| {
                entry
                    .get("name")
                    .and_then(Value::as_str)
                    .is_some_and(|tool| group.tools.iter().any(|member| member == tool))
            })
            .count();
        remaining.insert(name.clone(), group.max.saturating_sub(used));
    }
    Ok(remaining)
}

pub(crate) fn response(value: Value) -> CallToolResult {
    let content = vec![Content::text(value.to_string())];
    let mut response = if value["status"] == "queued" {
        CallToolResult::success(content)
    } else {
        CallToolResult::error(content)
    };
    response.structured_content = Some(value);
    response
}

pub(crate) fn rejected(
    reason: &str,
    message: impl Into<String>,
    remaining: Value,
) -> CallToolResult {
    response(
        json!({"status":"not-queued","reason":reason,"message":message.into(),
        "retryable":false,"remaining_budget":remaining}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> PrAdmissionPolicy {
        serde_json::from_value(json!({
            "schema_version":1,"budget_groups":{},
            "repositories":{"self":"","other":"Project/other"},
            "tools":{
                "create-pull-request":{"staged":false,"require-approval":false},
                "add-pull-request-labels":{"target":"created","allowed-repositories":["other"],
                    "staged":false,"require-approval":false,"allowed-labels":["ready"]},
                "update-pull-request":{"target":"*","allowed-operations":["replace-island"]}
            }
        }))
        .unwrap()
    }

    #[test]
    fn admission_checks_handle_lane_repository_and_operation_authority() {
        let mut policy = policy();
        let queued = vec![
            json!({"name":"create-pull-request","temporary_id":"#aw_new",
            "context":"proposal_creator","repository":"other"}),
        ];
        let entry = json!({"name":"add-pull-request-labels","pull_request_id":"#aw_new","labels":["ready"]});
        assert_eq!(
            policy
                .validate("add-pull-request-labels", &entry, &queued)
                .unwrap(),
            ["proposal_creator"]
        );
        let mut invalid = entry.clone();
        invalid["repository"] = json!("self");
        assert!(
            policy
                .validate("add-pull-request-labels", &invalid, &queued)
                .is_err()
        );
        invalid["repository"] = json!("Project/other");
        assert!(
            policy
                .validate("add-pull-request-labels", &invalid, &queued)
                .is_ok()
        );
        invalid["pull_request_id"] = json!(7);
        assert!(
            policy
                .validate("add-pull-request-labels", &invalid, &queued)
                .is_err()
        );
        policy.tools.get_mut("add-pull-request-labels").unwrap()["target"] = json!("*");
        policy.tools.get_mut("create-pull-request").unwrap()["require-approval"] = json!(true);
        assert!(
            policy
                .validate("add-pull-request-labels", &invalid, &queued)
                .is_ok()
        );
        assert!(
            policy
                .validate("add-pull-request-labels", &entry, &queued)
                .is_err()
        );
        assert!(
            policy
                .validate(
                    "update-pull-request",
                    &json!({
                        "pull_request_id":7,"body":"New","operation":"replace"
                    }),
                    &[]
                )
                .is_err()
        );
        assert!(
            policy
                .validate(
                    "update-pull-request",
                    &json!({
                        "pull_request_id":7,"body":"New","operation":"replace-island"
                    }),
                    &[]
                )
                .is_ok()
        );
    }

    #[test]
    fn creation_title_prefix_uses_the_same_bound_as_the_executor() {
        let mut policy = policy();
        let config = policy.tools.get_mut("create-pull-request").unwrap();
        config["title-prefix"] =
            json!("\u{1f600}".repeat(crate::safe_outputs::pr_common::MAX_TITLE_SCALARS - 1));
        let proposal =
            json!({"name":"create-pull-request","title":"A","body":"Body","repository":"self"});
        assert!(
            policy
                .validate("create-pull-request", &proposal, &[])
                .is_ok()
        );
        let mut oversized = proposal;
        oversized["title"] = json!("AB");
        let error = policy
            .validate("create-pull-request", &oversized, &[])
            .unwrap_err()
            .to_string();
        assert!(error.contains("Unicode scalar values"));
        assert!(error.contains("compiler resource limit"));
    }
}
