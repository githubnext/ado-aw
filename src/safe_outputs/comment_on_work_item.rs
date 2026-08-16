//! Comment on work item safe output tool

use log::{debug, info};
use percent_encoding::utf8_percent_encode;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::PATH_SEGMENT;
use crate::safe_outputs::{
    ExecutionContext, ExecutionResult, Executor, Validate, WorkItemReference, WorkItemResolution,
};
use crate::sanitize::{SanitizeContent, sanitize as sanitize_text};
use crate::tool_result;
use ado_aw_derive::SanitizeConfig;
use anyhow::{Context, ensure};

/// Parameters for commenting on a work item
#[derive(Deserialize, JsonSchema)]
pub struct CommentOnWorkItemParams {
    /// Positive Azure DevOps work-item ID to comment on, or a temporary ID
    /// from an earlier `create-work-item` call in the same run.
    pub work_item_id: WorkItemReference,

    /// Comment text in markdown format. Ensure adequate content > 10 characters.
    pub body: String,
}

impl Validate for CommentOnWorkItemParams {
    fn validate(&self) -> anyhow::Result<()> {
        if let WorkItemReference::Number(id) = self.work_item_id {
            ensure!(id > 0, "work_item_id must be positive");
        }
        ensure!(self.body.len() >= 10, "body must be at least 10 characters");
        Ok(())
    }
}

tool_result! {
    name = "comment-on-work-item",
    write = true,
    params = CommentOnWorkItemParams,
    /// Result of commenting on a work item
    pub struct CommentOnWorkItemResult {
        work_item_id: WorkItemReference,
        body: String,
    }
}

impl SanitizeContent for CommentOnWorkItemResult {
    fn sanitize_content_fields(&mut self) {
        self.body = sanitize_text(&self.body);
    }
}

/// Target scope for which work items can be commented on.
///
/// Deserialized from the `target` field in front matter:
/// - `"*"` → wildcard (any work item)
/// - `12345` → single work item ID
/// - `[12345, 67890]` → set of work item IDs
/// - `"Some\\Path"` → area path prefix (any string that isn't `"*"`)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CommentTarget {
    /// A single work item ID
    SingleId(i64),
    /// A list of work item IDs
    IdList(Vec<i64>),
    /// A string target: "*" for wildcard, anything else is an area path
    StringTarget(String),
}

impl CommentTarget {
    /// Check whether the given work item ID is allowed by this target.
    /// For area path targets, returns `None` — the caller must validate via API.
    pub fn allows_id(&self, work_item_id: i64) -> Option<bool> {
        match self {
            CommentTarget::SingleId(id) => Some(*id == work_item_id),
            CommentTarget::IdList(ids) => Some(ids.contains(&work_item_id)),
            CommentTarget::StringTarget(s) if s == "*" => Some(true),
            CommentTarget::StringTarget(_) => None, // area path — needs API check
        }
    }

    /// Get the area path prefix if this is an area-path target.
    pub fn area_path_prefix(&self) -> Option<&str> {
        match self {
            CommentTarget::StringTarget(s) if s != "*" => Some(s.as_str()),
            _ => None,
        }
    }
}

/// Configuration for the comment-on-work-item tool (specified in front matter)
///
/// Example front matter:
/// ```yaml
/// safe-outputs:
///   comment-on-work-item:
///     max: 5
///     target: "*"
/// ```
#[derive(Debug, Clone, SanitizeConfig, Serialize, Deserialize)]
pub struct CommentOnWorkItemConfig {
    /// Target scope — which work items can be commented on.
    /// `None` means no target was configured; execution must reject this.
    pub target: Option<CommentTarget>,

    /// Whether to include agent execution stats in the comment (default: true).
    #[serde(
        default = "crate::agent_stats::default_include_stats",
        rename = "include-stats"
    )]
    pub include_stats: bool,
}

impl Default for CommentOnWorkItemConfig {
    fn default() -> Self {
        Self {
            target: None,
            include_stats: true,
        }
    }
}

/// Validate that a work item is allowed by the configured target policy.
///
/// Returns `Ok(None)` when the work item is allowed, `Ok(Some(msg))` when it is
/// rejected by policy (the caller should return `ExecutionResult::failure(msg)`),
/// or `Err(…)` on an unexpected infrastructure failure.
async fn validate_target_policy(
    target: &CommentTarget,
    client: &reqwest::Client,
    org_url: &str,
    project: &str,
    token: &str,
    work_item_id: i64,
) -> anyhow::Result<Option<String>> {
    match target.allows_id(work_item_id) {
        Some(true) => {
            debug!("Work item #{} allowed by target policy", work_item_id);
            Ok(None)
        }
        Some(false) => Ok(Some(format!(
            "Work item #{} is not in the allowed target set",
            work_item_id
        ))),
        None => {
            // Area path validation — `allows_id` returns `None` only for
            // `StringTarget(s != "*")`, so `area_path_prefix` is always `Some` here.
            let prefix = target
                .area_path_prefix()
                .expect("allows_id returned None but area_path_prefix is also None");
            debug!(
                "Validating area path for work item #{} against prefix '{}'",
                work_item_id, prefix
            );
            match get_work_item_area_path(client, org_url, project, token, work_item_id).await {
                Ok(area_path) => {
                    // ADO area paths are case-insensitive and use backslash separators.
                    // Require the match to land on a path boundary so that prefix "4x4"
                    // doesn't accidentally match "4x4Production".
                    let ap = area_path.to_lowercase();
                    let pf = prefix.to_lowercase();
                    let is_match =
                        ap == pf || (ap.starts_with(&*pf) && ap[pf.len()..].starts_with('\\'));
                    if is_match {
                        debug!("Area path '{}' validated against '{}'", area_path, prefix);
                        Ok(None)
                    } else {
                        Ok(Some(format!(
                            "Work item #{} has area path '{}' which is not under allowed prefix '{}'",
                            work_item_id, area_path, prefix
                        )))
                    }
                }
                Err(e) => Ok(Some(format!(
                    "Failed to validate area path for work item #{}: {}",
                    work_item_id, e
                ))),
            }
        }
    }
}

/// Fetch a work item's area path from the ADO API
async fn get_work_item_area_path(
    client: &reqwest::Client,
    org_url: &str,
    project: &str,
    token: &str,
    work_item_id: i64,
) -> anyhow::Result<String> {
    let url = format!(
        "{}/{}/_apis/wit/workitems/{}?$fields=System.AreaPath&api-version=7.0",
        org_url.trim_end_matches('/'),
        utf8_percent_encode(project, PATH_SEGMENT),
        work_item_id,
    );

    let response = client
        .get(&url)
        .basic_auth("", Some(token))
        .send()
        .await
        .context("Failed to query work item")?;

    if response.status().is_success() {
        let body: serde_json::Value = response
            .json()
            .await
            .context("Failed to parse work item response")?;

        body.get("fields")
            .and_then(|f| f.get("System.AreaPath"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .context("Work item response missing 'System.AreaPath' field")
    } else {
        let status = response.status();
        let error_body = response
            .text()
            .await
            .unwrap_or_else(|_| "Unknown error".to_string());

        anyhow::bail!(
            "Failed to fetch work item {} (HTTP {}): {}",
            work_item_id,
            status,
            error_body
        )
    }
}

#[async_trait::async_trait]
impl Executor for CommentOnWorkItemResult {
    fn dry_run_summary(&self) -> String {
        format!("comment on work item {}", self.work_item_id)
    }

    async fn execute_impl(&self, ctx: &ExecutionContext) -> anyhow::Result<ExecutionResult> {
        info!(
            "Commenting on work item {}: {} chars",
            self.work_item_id,
            self.body.len()
        );
        debug!(
            "comment-on-work-item: work_item_id={}, body length={}",
            self.work_item_id,
            self.body.len()
        );

        let config: CommentOnWorkItemConfig = ctx.get_tool_config("comment-on-work-item")?;
        debug!("Target: {:?}", config.target);

        // Temporary IDs are resolved against the creates that already ran in
        // this SafeOutputs job. An ID that cannot be traced to such a create is
        // rejected before any HTTP call so the comment never lands on an
        // unrelated work item.
        let (work_item_id, target_policy_applies) = match self.work_item_id.resolve(ctx)? {
            WorkItemResolution::Unresolved(message) => {
                return Ok(ExecutionResult::failure(message));
            }
            WorkItemResolution::SameRun(id) => (id, false),
            WorkItemResolution::Numeric(id) => (id, true),
        };
        let Ok(work_item_id) = i64::try_from(work_item_id) else {
            return Ok(ExecutionResult::failure(format!(
                "work item ID {work_item_id} is out of range"
            )));
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
        debug!("ADO org: {}, project: {}", org_url, project);

        let client = reqwest::Client::new();

        // Work items created in this run are already scoped by the
        // create-work-item configuration, so they do not need `target`.
        if target_policy_applies {
            let Some(target) = &config.target else {
                return Ok(ExecutionResult::failure(
                    "comment-on-work-item target is not configured. \
                     This is required to scope which work items the agent can comment on."
                        .to_string(),
                ));
            };

            // Validate work item ID against target policy
            if let Some(rejection_msg) =
                validate_target_policy(target, &client, org_url, project, token, work_item_id)
                    .await?
            {
                return Ok(ExecutionResult::failure(rejection_msg));
            }
        }

        // Build the Azure DevOps REST API URL for adding a comment
        // POST https://dev.azure.com/{org}/{project}/_apis/wit/workItems/{id}/comments?api-version=7.1-preview.4
        let url = format!(
            "{}/{}/_apis/wit/workItems/{}/comments?api-version=7.1-preview.4",
            org_url.trim_end_matches('/'),
            utf8_percent_encode(project, PATH_SEGMENT),
            work_item_id,
        );
        debug!("API URL: {}", url);

        let body_with_stats =
            crate::agent_stats::append_stats_to_body(&self.body, ctx, config.include_stats);
        let comment_body = serde_json::json!({
            "text": body_with_stats,
        });

        info!("Sending comment to work item #{}", work_item_id);
        let response = client
            .post(&url)
            .header("Content-Type", "application/json")
            .basic_auth("", Some(token))
            .json(&comment_body)
            .send()
            .await
            .context("Failed to send request to Azure DevOps")?;

        if response.status().is_success() {
            let body: serde_json::Value = response
                .json()
                .await
                .context("Failed to parse response JSON")?;

            let comment_id = body.get("id").and_then(|v| v.as_i64()).unwrap_or(0);
            let comment_url = body.get("url").and_then(|h| h.as_str()).unwrap_or("");

            info!(
                "Comment added to work item #{}: comment #{}",
                work_item_id, comment_id
            );

            Ok(ExecutionResult::success_with_data(
                format!(
                    "Added comment #{} to work item #{}",
                    comment_id, work_item_id
                ),
                serde_json::json!({
                    "comment_id": comment_id,
                    "work_item_id": work_item_id,
                    "url": comment_url,
                    "project": project,
                }),
            ))
        } else {
            let status = response.status();
            let error_body = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());

            Ok(ExecutionResult::failure(format!(
                "Failed to add comment to work item #{} (HTTP {}): {}",
                work_item_id, status, error_body
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
    use crate::secure::WorkItemTemporaryId;

    #[test]
    fn test_result_has_correct_name() {
        assert_eq!(CommentOnWorkItemResult::NAME, "comment-on-work-item");
    }

    #[test]
    fn test_params_deserializes() {
        let json = r#"{"work_item_id": 12345, "body": "This is a comment on the work item."}"#;
        let params: CommentOnWorkItemParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.work_item_id, WorkItemReference::Number(12345));
        assert_eq!(params.body, "This is a comment on the work item.");
    }

    #[test]
    fn test_params_converts_to_result() {
        let params = CommentOnWorkItemParams {
            work_item_id: WorkItemReference::Number(42),
            body: "This is a test comment with enough characters.".to_string(),
        };
        let result: CommentOnWorkItemResult = params.try_into().unwrap();
        assert_eq!(result.name, "comment-on-work-item");
        assert_eq!(result.work_item_id, WorkItemReference::Number(42));
        assert!(result.body.contains("test comment"));
    }

    #[test]
    fn test_validation_rejects_zero_work_item_id() {
        let params = CommentOnWorkItemParams {
            work_item_id: WorkItemReference::Number(0),
            body: "This is a valid comment body text.".to_string(),
        };
        let result: Result<CommentOnWorkItemResult, _> = params.try_into();
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("work_item_id must be positive"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_negative_work_item_id_is_rejected_at_deserialization() {
        let error = serde_json::from_value::<CommentOnWorkItemParams>(serde_json::json!({
            "work_item_id": -5,
            "body": "This is a valid comment body text."
        }))
        .err()
        .expect("negative work_item_id must be rejected")
        .to_string();
        assert!(
            error.contains("work_item_id must be positive"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn test_temporary_work_item_id_deserializes() {
        let params: CommentOnWorkItemParams = serde_json::from_value(serde_json::json!({
            "work_item_id": "#aw_acd847a0",
            "body": "This is a comment on the created work item."
        }))
        .unwrap();
        assert_eq!(
            params.work_item_id,
            WorkItemReference::Temporary(WorkItemTemporaryId::parse("#aw_acd847a0").unwrap())
        );
    }

    #[test]
    fn test_quoted_numeric_work_item_id_deserializes_as_number() {
        let params: CommentOnWorkItemParams = serde_json::from_value(serde_json::json!({
            "work_item_id": "42",
            "body": "This is a comment on the work item."
        }))
        .unwrap();
        assert_eq!(params.work_item_id, WorkItemReference::Number(42));
    }

    #[tokio::test]
    async fn unresolved_temporary_id_fails_before_http() {
        let mut tool_configs = std::collections::HashMap::new();
        tool_configs.insert(
            "comment-on-work-item".to_string(),
            serde_json::json!({"target": "*"}),
        );
        let ctx = ExecutionContext {
            tool_configs,
            ..Default::default()
        };
        let mut result: CommentOnWorkItemResult = CommentOnWorkItemParams {
            work_item_id: WorkItemReference::Temporary(
                WorkItemTemporaryId::parse("#aw_missing").unwrap(),
            ),
            body: "This is a comment on the created work item.".to_string(),
        }
        .try_into()
        .unwrap();
        let execution = result.execute_sanitized(&ctx).await.unwrap();
        assert!(!execution.success);
        assert!(
            execution.message.contains("has not been resolved"),
            "unexpected message: {}",
            execution.message
        );
    }

    #[tokio::test]
    async fn resolved_temporary_id_does_not_require_target() {
        let mut tool_configs = std::collections::HashMap::new();
        tool_configs.insert("comment-on-work-item".to_string(), serde_json::json!({}));
        let ctx = ExecutionContext {
            tool_configs,
            ..Default::default()
        };
        let temporary_id = WorkItemTemporaryId::parse("#aw_created1").unwrap();
        ctx.register_resolved_work_item(
            &temporary_id,
            ResolvedWorkItem {
                id: 4242,
                url: "https://example.invalid/4242".to_string(),
            },
        )
        .unwrap();
        let mut result: CommentOnWorkItemResult = CommentOnWorkItemParams {
            work_item_id: WorkItemReference::Temporary(temporary_id),
            body: "This is a comment on the created work item.".to_string(),
        }
        .try_into()
        .unwrap();
        // Resolution succeeds, so execution proceeds past policy and fails only
        // because no ADO environment is configured in the test.
        let error = result
            .execute_sanitized(&ctx)
            .await
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("AZURE_DEVOPS_ORG_URL not set"),
            "unexpected error: {error}"
        );
    }

    #[tokio::test]
    async fn create_then_comment_resolves_temporary_id() {
        use wiremock::matchers::{body_json, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/Project/_apis/wit/workitems/$Task"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": 42,
                "_links": { "html": { "href": "https://example.test/items/42" } }
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/Project/_apis/wit/workItems/42/comments"))
            .and(body_json(serde_json::json!({
                "text": "This is a comment on the created work item."
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": 7,
                "url": "https://example.test/items/42/comments/7"
            })))
            .expect(1)
            .mount(&server)
            .await;

        let mut tool_configs = std::collections::HashMap::new();
        tool_configs.insert(
            "create-work-item".to_string(),
            serde_json::json!({"include-stats": false}),
        );
        // No `target` configured: a temporary ID resolved from this run's create
        // must still be accepted, because the create is scoped by its own config.
        tool_configs.insert(
            "comment-on-work-item".to_string(),
            serde_json::json!({"include-stats": false}),
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

        let mut comment: CommentOnWorkItemResult = CommentOnWorkItemParams {
            work_item_id: WorkItemReference::Temporary(temporary_id),
            body: "This is a comment on the created work item.".to_string(),
        }
        .try_into()
        .unwrap();
        let commented = comment.execute_sanitized(&ctx).await.unwrap();
        assert!(commented.success, "comment failed: {}", commented.message);
        assert_eq!(
            commented
                .data
                .as_ref()
                .and_then(|data| data["work_item_id"].as_i64()),
            Some(42)
        );
        assert_eq!(
            commented
                .data
                .as_ref()
                .and_then(|data| data["comment_id"].as_i64()),
            Some(7)
        );
    }

    #[tokio::test]
    async fn out_of_range_numeric_id_fails_before_http() {
        let mut tool_configs = std::collections::HashMap::new();
        tool_configs.insert(
            "comment-on-work-item".to_string(),
            serde_json::json!({"target": "*"}),
        );
        let ctx = ExecutionContext {
            tool_configs,
            ..Default::default()
        };
        let mut result: CommentOnWorkItemResult = CommentOnWorkItemParams {
            work_item_id: WorkItemReference::Number(u64::MAX),
            body: "This is a comment on the work item.".to_string(),
        }
        .try_into()
        .unwrap();
        let execution = result.execute_sanitized(&ctx).await.unwrap();
        assert!(!execution.success);
        assert!(
            execution.message.contains("is out of range"),
            "unexpected message: {}",
            execution.message
        );
    }

    #[tokio::test]
    async fn out_of_range_resolved_temporary_id_fails_before_http() {
        let mut tool_configs = std::collections::HashMap::new();
        tool_configs.insert("comment-on-work-item".to_string(), serde_json::json!({}));
        let ctx = ExecutionContext {
            tool_configs,
            ..Default::default()
        };
        let temporary_id = WorkItemTemporaryId::parse("#aw_created2").unwrap();
        ctx.register_resolved_work_item(
            &temporary_id,
            ResolvedWorkItem {
                id: u64::MAX,
                url: "https://example.invalid/huge".to_string(),
            },
        )
        .unwrap();
        let mut result: CommentOnWorkItemResult = CommentOnWorkItemParams {
            work_item_id: WorkItemReference::Temporary(temporary_id),
            body: "This is a comment on the created work item.".to_string(),
        }
        .try_into()
        .unwrap();
        let execution = result.execute_sanitized(&ctx).await.unwrap();
        assert!(!execution.success);
        assert!(
            execution.message.contains("is out of range"),
            "unexpected message: {}",
            execution.message
        );
    }

    #[test]
    fn test_validation_rejects_short_body() {
        let params = CommentOnWorkItemParams {
            work_item_id: WorkItemReference::Number(42),
            body: "Too short".to_string(),
        };
        let result: Result<CommentOnWorkItemResult, _> = params.try_into();
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("body must be at least 10 characters"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_result_serializes_correctly() {
        let params = CommentOnWorkItemParams {
            work_item_id: WorkItemReference::Number(42),
            body: "A comment body that is definitely longer than ten characters.".to_string(),
        };
        let result: CommentOnWorkItemResult = params.try_into().unwrap();
        let json = serde_json::to_string(&result).unwrap();

        assert!(json.contains(r#""name":"comment-on-work-item""#));
        assert!(json.contains(r#""work_item_id":42"#));
    }

    #[test]
    fn test_config_defaults() {
        let config = CommentOnWorkItemConfig::default();
        assert!(config.target.is_none());
    }

    #[test]
    fn test_config_deserializes_from_yaml() {
        let yaml = r#"
target: "*"
"#;
        let config: CommentOnWorkItemConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(config.target.is_some());
    }

    #[test]
    fn test_config_single_id_target() {
        let yaml = r#"
max: 1
target: 12345
"#;
        let config: CommentOnWorkItemConfig = serde_yaml::from_str(yaml).unwrap();
        let target = config.target.unwrap();
        assert!(target.allows_id(12345) == Some(true));
        assert!(target.allows_id(99999) == Some(false));
    }

    #[test]
    fn test_config_id_list_target() {
        let yaml = r#"
max: 3
target:
  - 100
  - 200
  - 300
"#;
        let config: CommentOnWorkItemConfig = serde_yaml::from_str(yaml).unwrap();
        let target = config.target.unwrap();
        assert!(target.allows_id(100) == Some(true));
        assert!(target.allows_id(200) == Some(true));
        assert!(target.allows_id(999) == Some(false));
    }

    #[test]
    fn test_config_wildcard_target() {
        let yaml = r#"
target: "*"
"#;
        let config: CommentOnWorkItemConfig = serde_yaml::from_str(yaml).unwrap();
        let target = config.target.unwrap();
        assert!(target.allows_id(1) == Some(true));
        assert!(target.allows_id(99999) == Some(true));
    }

    #[test]
    fn test_config_area_path_target() {
        let yaml = r#"
target: "4x4\\QED"
"#;
        let config: CommentOnWorkItemConfig = serde_yaml::from_str(yaml).unwrap();
        let target = config.target.unwrap();
        assert!(target.allows_id(1).is_none());
        assert_eq!(target.area_path_prefix(), Some("4x4\\QED"));
    }

    #[test]
    fn test_config_missing_target_defaults_to_none() {
        let yaml = r#"
max: 3
"#;
        let config: CommentOnWorkItemConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(config.target.is_none());
    }
}
