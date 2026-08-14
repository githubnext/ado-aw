//! Assign an Azure DevOps work item.

use anyhow::Context;
use log::{debug, info};
use percent_encoding::utf8_percent_encode;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{PATH_SEGMENT, TargetConfig, normalize_work_item_assignee};
use crate::safe_outputs::{ExecutionContext, ExecutionResult, Executor, Validate};
use crate::sanitize::{SanitizeContent, sanitize_config};
use crate::secure::WorkItemTemporaryId;
use crate::tool_result;
use ado_aw_derive::SanitizeConfig;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum WorkItemReference {
    Number(u64),
    Temporary(WorkItemTemporaryId),
}

impl<'de> Deserialize<'de> for WorkItemReference {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct WorkItemReferenceVisitor;

        impl serde::de::Visitor<'_> for WorkItemReferenceVisitor {
            type Value = WorkItemReference;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a positive work-item ID or #aw_ temporary ID")
            }

            fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
                Ok(WorkItemReference::Number(value))
            }

            fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                u64::try_from(value)
                    .map(WorkItemReference::Number)
                    .map_err(|_| E::custom("work_item_id must be positive"))
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                if value.chars().all(|c| c.is_ascii_digit()) {
                    return value
                        .parse::<u64>()
                        .map(WorkItemReference::Number)
                        .map_err(|_| E::custom("quoted work_item_id is outside the u64 range"));
                }
                WorkItemTemporaryId::parse(value)
                    .map(WorkItemReference::Temporary)
                    .map_err(E::custom)
            }
        }

        deserializer.deserialize_any(WorkItemReferenceVisitor)
    }
}

#[derive(Deserialize, JsonSchema)]
pub struct AssignWorkItemParams {
    /// Positive Azure DevOps work-item ID or a temporary ID from create-work-item.
    pub work_item_id: WorkItemReference,
    /// Azure DevOps identity to assign.
    pub assignee: String,
}

impl Validate for AssignWorkItemParams {
    fn validate(&self) -> anyhow::Result<()> {
        if let WorkItemReference::Number(id) = self.work_item_id {
            anyhow::ensure!(id > 0, "work_item_id must be positive");
        }
        normalize_work_item_assignee(&self.assignee, "assign-work-item.assignee")?;
        Ok(())
    }
}

tool_result! {
    name = "assign-work-item",
    write = true,
    params = AssignWorkItemParams,
    /// Result of assigning an Azure DevOps work item.
    pub struct AssignWorkItemResult {
        work_item_id: WorkItemReference,
        assignee: String,
    }
}

impl SanitizeContent for AssignWorkItemResult {
    fn sanitize_content_fields(&mut self) {
        self.assignee = sanitize_config(self.assignee.trim());
    }
}

#[derive(Debug, Clone, Default, SanitizeConfig, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssignWorkItemConfig {
    /// Numeric work items this tool may target. Temporary IDs created in the
    /// current run are always eligible.
    #[serde(default)]
    pub target: Option<TargetConfig>,
    /// Optional case-insensitive exact allowlist. Empty means unrestricted.
    #[serde(default)]
    pub allowed: Vec<String>,
    /// Optional case-insensitive wildcard blocklist.
    #[serde(default)]
    pub blocked: Vec<String>,
    /// Per-run assignment budget.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[sanitize_config(skip)]
    pub max: Option<u32>,
}

fn check_numeric_target(id: u64, target: Option<&TargetConfig>) -> Option<ExecutionResult> {
    match target {
        Some(TargetConfig::Pattern(pattern)) if pattern == "*" => None,
        Some(TargetConfig::Id(allowed_id)) if *allowed_id == id => None,
        Some(TargetConfig::Id(_)) | Some(TargetConfig::Pattern(_)) => {
            Some(ExecutionResult::failure(format!(
                "Work item #{id} is not permitted by the assign-work-item target configuration"
            )))
        }
        None => Some(ExecutionResult::failure(
            "assign-work-item requires `target: \"*\"` or an exact numeric target \
             when work_item_id refers to a pre-existing work item",
        )),
    }
}

fn check_identity_policy(assignee: &str, config: &AssignWorkItemConfig) -> anyhow::Result<String> {
    let assignee = normalize_work_item_assignee(assignee, "assign-work-item.assignee")?;
    if !config.allowed.is_empty()
        && !config
            .allowed
            .iter()
            .any(|allowed| allowed.trim().eq_ignore_ascii_case(&assignee))
    {
        anyhow::bail!("assignee '{assignee}' is not in assign-work-item.allowed");
    }
    if config
        .blocked
        .iter()
        .any(|pattern| super::tag_matches_pattern(&assignee, pattern.trim()))
    {
        anyhow::bail!("assignee '{assignee}' is blocked by assign-work-item.blocked");
    }
    Ok(assignee)
}

#[async_trait::async_trait]
impl Executor for AssignWorkItemResult {
    fn dry_run_summary(&self) -> String {
        let target = match &self.work_item_id {
            WorkItemReference::Number(id) => format!("#{id}"),
            WorkItemReference::Temporary(id) => id.canonical(),
        };
        format!("assign work item {target} to '{}'", self.assignee)
    }

    async fn execute_impl(&self, ctx: &ExecutionContext) -> anyhow::Result<ExecutionResult> {
        if !ctx.tool_configs.contains_key("assign-work-item") {
            return Ok(ExecutionResult::failure(
                "assign-work-item is not configured for this workflow",
            ));
        }

        let config: AssignWorkItemConfig = ctx.get_tool_config("assign-work-item")?;
        let assignee = match check_identity_policy(&self.assignee, &config) {
            Ok(assignee) => assignee,
            Err(error) => return Ok(ExecutionResult::failure(error.to_string())),
        };
        let id = match &self.work_item_id {
            WorkItemReference::Number(id) => {
                if let Some(failure) = check_numeric_target(*id, config.target.as_ref()) {
                    return Ok(failure);
                }
                *id
            }
            WorkItemReference::Temporary(temporary_id) => {
                let Some(work_item) = ctx.resolve_work_item(temporary_id)? else {
                    return Ok(ExecutionResult::failure(format!(
                        "temporary work-item ID '{}' has not been resolved; create-work-item must \
                         succeed earlier in the same SafeOutputs job",
                        temporary_id.canonical()
                    )));
                };
                work_item.id
            }
        };

        let org_url = ctx
            .ado_org_url
            .as_ref()
            .context("AZURE_DEVOPS_ORG_URL not set")?;
        let project = ctx
            .ado_project
            .as_ref()
            .context("SYSTEM_TEAMPROJECT not set")?;
        let token = ctx
            .access_token
            .as_ref()
            .context("No access token available (SYSTEM_ACCESSTOKEN or AZURE_DEVOPS_EXT_PAT)")?;
        let url = format!(
            "{}/{}/_apis/wit/workitems/{}?api-version=7.0",
            org_url.trim_end_matches('/'),
            utf8_percent_encode(project, PATH_SEGMENT),
            id
        );
        let patch = serde_json::json!([{
            "op": "replace",
            "path": "/fields/System.AssignedTo",
            "value": assignee,
        }]);
        debug!("Assigning work item #{id} to '{}'", assignee);
        let response = reqwest::Client::new()
            .patch(url)
            .header("Content-Type", "application/json-patch+json")
            .basic_auth("", Some(token))
            .json(&patch)
            .send()
            .await
            .context("Failed to send work-item assignment request to Azure DevOps")?;

        let status = response.status();
        if status.is_success() {
            info!("Assigned work item #{id} to '{}'", assignee);
            Ok(ExecutionResult::success_with_data(
                format!("Assigned work item #{id} to '{assignee}'"),
                serde_json::json!({
                    "id": id,
                    "assignee": assignee,
                }),
            ))
        } else {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "<unable to read response body>".to_string());
            Ok(ExecutionResult::failure(format!(
                "Failed to assign work item #{id} (HTTP {status}): {}",
                crate::sanitize::neutralize_pipeline_commands(&body)
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::safe_outputs::{
        CreateWorkItemParams, CreateWorkItemResult, ResolvedWorkItem, ToolResult,
    };

    fn assign_config() -> AssignWorkItemConfig {
        AssignWorkItemConfig {
            target: None,
            allowed: Vec::new(),
            blocked: Vec::new(),
            max: None,
        }
    }

    #[test]
    fn result_has_correct_name() {
        assert_eq!(AssignWorkItemResult::NAME, "assign-work-item");
    }

    #[test]
    fn quoted_numeric_work_item_id_deserializes_as_number() {
        let params: AssignWorkItemParams = serde_json::from_value(serde_json::json!({
            "work_item_id": "42",
            "assignee": "owner@example.com"
        }))
        .unwrap();
        assert_eq!(params.work_item_id, WorkItemReference::Number(42));
    }

    #[test]
    fn hard_denied_identities_cannot_be_allowed() {
        let mut config = assign_config();
        config.allowed = vec!["Agency".to_string(), "GitHub Copilot".to_string()];
        for identity in ["agency", " GITHUB COPILOT "] {
            let error = check_identity_policy(identity, &config).unwrap_err();
            assert!(error.to_string().contains("reserved identity"));
        }
    }

    #[test]
    fn allowed_and_blocked_policies_are_enforced() {
        let mut config = assign_config();
        config.allowed = vec!["owner@example.com".to_string()];
        assert!(check_identity_policy("OWNER@example.com", &config).is_ok());
        assert!(check_identity_policy("other@example.com", &config).is_err());

        config.allowed.clear();
        config.blocked = vec!["svc-*".to_string()];
        assert!(check_identity_policy("SVC-build", &config).is_err());
        assert!(check_identity_policy("person@example.com", &config).is_ok());
    }

    #[test]
    fn numeric_targets_require_explicit_scope() {
        assert!(check_numeric_target(42, None).is_some());
        assert!(check_numeric_target(42, Some(&TargetConfig::Pattern("*".to_string()))).is_none());
        assert!(check_numeric_target(42, Some(&TargetConfig::Id(42))).is_none());
        assert!(check_numeric_target(42, Some(&TargetConfig::Id(7))).is_some());
    }

    #[tokio::test]
    async fn unresolved_temporary_id_fails_before_http() {
        let mut tool_configs = std::collections::HashMap::new();
        tool_configs.insert("assign-work-item".to_string(), serde_json::json!({}));
        let ctx = ExecutionContext {
            tool_configs,
            ..Default::default()
        };
        let mut result: AssignWorkItemResult = AssignWorkItemParams {
            work_item_id: WorkItemReference::Temporary(
                WorkItemTemporaryId::parse("#aw_missing").unwrap(),
            ),
            assignee: "owner@example.com".to_string(),
        }
        .try_into()
        .unwrap();
        let execution = result.execute_sanitized(&ctx).await.unwrap();
        assert!(!execution.success);
        assert!(execution.message.contains("has not been resolved"));
    }

    #[tokio::test]
    async fn create_then_assign_resolves_temporary_id() {
        use wiremock::matchers::{body_json, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/Project/_apis/wit/workitems/$Task"))
            .and(body_json(serde_json::json!([
                {
                    "op": "add",
                    "path": "/fields/System.Title",
                    "value": "Create a real task"
                },
                {
                    "op": "add",
                    "path": "/fields/System.Description",
                    "value": "A detailed work-item description that is long enough."
                },
                {
                    "op": "add",
                    "path": "/multilineFieldsFormat/System.Description",
                    "value": "Markdown"
                }
            ])))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": 42,
                "_links": { "html": { "href": "https://example.test/items/42" } }
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(path("/Project/_apis/wit/workitems/42"))
            .and(body_json(serde_json::json!([{
                "op": "replace",
                "path": "/fields/System.AssignedTo",
                "value": "owner@example.com"
            }])))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
            .expect(1)
            .mount(&server)
            .await;

        let mut tool_configs = std::collections::HashMap::new();
        tool_configs.insert(
            "create-work-item".to_string(),
            serde_json::json!({
                "include-stats": false
            }),
        );
        tool_configs.insert(
            "assign-work-item".to_string(),
            serde_json::json!({
                "allowed": ["owner@example.com"]
            }),
        );
        let ctx = ExecutionContext {
            ado_org_url: Some(server.uri()),
            ado_project: Some("Project".to_string()),
            access_token: Some("token".to_string()),
            tool_configs,
            ..Default::default()
        };

        let temporary_id = WorkItemTemporaryId::parse("#aw_task1").unwrap();
        let mut create: CreateWorkItemResult = (
            CreateWorkItemParams {
                title: "Create a real task".to_string(),
                description: "A detailed work-item description that is long enough.".to_string(),
                tags: Vec::new(),
            },
            temporary_id.clone(),
        )
            .try_into()
            .unwrap();
        let created = create.execute_sanitized(&ctx).await.unwrap();
        assert!(created.success, "create failed: {}", created.message);
        assert_eq!(
            ctx.resolve_work_item(&temporary_id).unwrap(),
            Some(ResolvedWorkItem {
                id: 42,
                url: "https://example.test/items/42".to_string(),
            })
        );

        let mut assign: AssignWorkItemResult = AssignWorkItemParams {
            work_item_id: WorkItemReference::Temporary(temporary_id),
            assignee: "owner@example.com".to_string(),
        }
        .try_into()
        .unwrap();
        let assigned = assign.execute_sanitized(&ctx).await.unwrap();
        assert!(assigned.success, "assignment failed: {}", assigned.message);
        assert_eq!(
            assigned.data.as_ref().and_then(|data| data["id"].as_u64()),
            Some(42)
        );
    }
}
