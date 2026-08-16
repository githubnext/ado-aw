//! Link work items safe output tool

use log::{debug, info};
use percent_encoding::utf8_percent_encode;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::PATH_SEGMENT;
use crate::safe_outputs::comment_on_work_item::CommentTarget;
use crate::safe_outputs::{
    ExecutionContext, ExecutionResult, Executor, Validate, WorkItemReference, WorkItemResolution,
};
use crate::sanitize::{SanitizeContent, sanitize as sanitize_text, sanitize_config};
use crate::tool_result;
use ado_aw_derive::SanitizeConfig;
use anyhow::{Context, ensure};

/// Resolve a human-friendly link type name to the ADO relation type string.
fn resolve_link_type(link_type: &str) -> Option<&'static str> {
    match link_type {
        "parent" => Some("System.LinkTypes.Hierarchy-Reverse"),
        "child" => Some("System.LinkTypes.Hierarchy-Forward"),
        "related" => Some("System.LinkTypes.Related"),
        "predecessor" => Some("System.LinkTypes.Dependency-Reverse"),
        "successor" => Some("System.LinkTypes.Dependency-Forward"),
        "duplicate" => Some("System.LinkTypes.Duplicate-Forward"),
        "duplicate-of" => Some("System.LinkTypes.Duplicate-Reverse"),
        _ => None,
    }
}

/// All valid link type names accepted by this tool.
const VALID_LINK_TYPES: &[&str] = &[
    "parent",
    "child",
    "related",
    "predecessor",
    "successor",
    "duplicate",
    "duplicate-of",
];

/// Parameters for linking two work items
#[derive(Deserialize, JsonSchema)]
pub struct LinkWorkItemsParams {
    /// The source work item (the item the link is added to): a positive ID, or
    /// a temporary ID from an earlier `create-work-item` call in the same run.
    pub source_id: WorkItemReference,

    /// The target work item (the item being linked to): a positive ID, or a
    /// temporary ID from an earlier `create-work-item` call in the same run.
    pub target_id: WorkItemReference,

    /// Link type: parent, child, related, predecessor, successor, duplicate, duplicate-of
    pub link_type: String,

    /// Optional comment describing the relationship
    pub comment: Option<String>,
}

impl Validate for LinkWorkItemsParams {
    fn validate(&self) -> anyhow::Result<()> {
        if let WorkItemReference::Number(source_id) = self.source_id {
            ensure!(source_id > 0, "source_id must be positive");
        }
        if let WorkItemReference::Number(target_id) = self.target_id {
            ensure!(target_id > 0, "target_id must be positive");
        }
        // Catches the literally identical case only. A temporary ID and a
        // numeric ID that name the same work item are indistinguishable until
        // Stage 3 resolution, so `execute_impl` repeats this check on the
        // resolved IDs.
        ensure!(
            self.source_id != self.target_id,
            "source_id and target_id must be different"
        );
        ensure!(
            resolve_link_type(&self.link_type).is_some(),
            "invalid link_type '{}'; must be one of: {}",
            self.link_type,
            VALID_LINK_TYPES.join(", ")
        );
        if let Some(ref comment) = self.comment {
            ensure!(comment.len() >= 5, "comment must be at least 5 characters");
        }
        Ok(())
    }
}

tool_result! {
    name = "link-work-items",
    write = true,
    params = LinkWorkItemsParams,
    default_max = 5,
    /// Result of linking two work items
    pub struct LinkWorkItemsResult {
        source_id: WorkItemReference,
        target_id: WorkItemReference,
        link_type: String,
        comment: Option<String>,
    }
}

impl SanitizeContent for LinkWorkItemsResult {
    fn sanitize_content_fields(&mut self) {
        self.link_type = sanitize_config(&self.link_type);
        self.comment = self.comment.as_deref().map(sanitize_text);
    }
}

/// Configuration for the link-work-items tool (specified in front matter)
///
/// Example front matter:
/// ```yaml
/// safe-outputs:
///   link-work-items:
///     target: "*"
///     allowed-link-types:
///       - parent
///       - child
///       - related
/// ```
#[derive(Debug, Clone, Default, SanitizeConfig, Serialize, Deserialize)]
pub struct LinkWorkItemsConfig {
    /// Restrict which link types the agent may use.
    /// An empty list (the default) means all link types are allowed.
    #[serde(default, rename = "allowed-link-types")]
    pub allowed_link_types: Vec<String>,

    /// Target scope — which work items can be linked.
    /// `None` means no target was configured; execution must reject this.
    /// Accepts the same values as comment-on-work-item: "*", a single ID,
    /// a list of IDs, or an area path string.
    pub target: Option<CommentTarget>,
}

/// Resolve one reference to `(id, needs_target_check)`, or the failure message
/// to surface when a temporary ID cannot be traced to a create in this run.
fn resolve_reference(
    reference: &WorkItemReference,
    ctx: &ExecutionContext,
) -> anyhow::Result<Result<(i64, bool), String>> {
    let (id, needs_target) = match reference.resolve(ctx)? {
        WorkItemResolution::Unresolved(message) => return Ok(Err(message)),
        WorkItemResolution::SameRun(id) => (id, false),
        WorkItemResolution::Numeric(id) => (id, true),
    };
    match i64::try_from(id) {
        Ok(id) => Ok(Ok((id, needs_target))),
        Err(_) => Ok(Err(format!("work item ID {id} is out of range"))),
    }
}

#[async_trait::async_trait]
impl Executor for LinkWorkItemsResult {
    fn dry_run_summary(&self) -> String {
        format!(
            "link work items {} -> {} ({})",
            self.source_id, self.target_id, self.link_type
        )
    }

    async fn execute_impl(&self, ctx: &ExecutionContext) -> anyhow::Result<ExecutionResult> {
        info!(
            "Linking work item {} -> {} ({})",
            self.source_id, self.target_id, self.link_type
        );
        debug!(
            "link-work-items: source={}, target={}, type={}",
            self.source_id, self.target_id, self.link_type
        );

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

        let config: LinkWorkItemsConfig = ctx.get_tool_config("link-work-items")?;
        debug!("Allowed link types: {:?}", config.allowed_link_types);

        // Resolve each reference. Temporary IDs resolve only against creates
        // that already succeeded in this SafeOutputs job, and are exempt from
        // `target` because that create is scoped by its own configuration.
        let (source_id, source_needs_target) = match resolve_reference(&self.source_id, ctx)? {
            Ok(resolved) => resolved,
            Err(message) => return Ok(ExecutionResult::failure(message)),
        };
        let (target_id, target_needs_target) = match resolve_reference(&self.target_id, ctx)? {
            Ok(resolved) => resolved,
            Err(message) => return Ok(ExecutionResult::failure(message)),
        };
        if source_id == target_id {
            return Ok(ExecutionResult::failure(
                "source_id and target_id resolve to the same work item".to_string(),
            ));
        }

        // Validate numeric work item IDs against target scope
        if source_needs_target || target_needs_target {
            let Some(target) = &config.target else {
                return Ok(ExecutionResult::failure(
                    "link-work-items requires a 'target' field in safe-outputs configuration \
                     to scope which work items can be linked. Example:\n  safe-outputs:\n    \
                     link-work-items:\n      target: \"*\""
                        .to_string(),
                ));
            };
            // Check source_id
            if source_needs_target && target.allows_id(source_id) == Some(false) {
                return Ok(ExecutionResult::failure(format!(
                    "Source work item #{source_id} is not allowed by the configured target scope"
                )));
            }
            // Check target_id
            if target_needs_target && target.allows_id(target_id) == Some(false) {
                return Ok(ExecutionResult::failure(format!(
                    "Target work item #{target_id} is not allowed by the configured target scope"
                )));
            }
            // Area path validation is deferred — would need API calls for both IDs.
            // For now, ID-based and wildcard scoping is enforced.
        }

        // Validate link type against configured allow-list
        if !config.allowed_link_types.is_empty()
            && !config.allowed_link_types.contains(&self.link_type)
        {
            return Ok(ExecutionResult::failure(format!(
                "Link type '{}' is not in the allowed set: {}",
                self.link_type,
                config.allowed_link_types.join(", ")
            )));
        }

        let relation_type = match resolve_link_type(&self.link_type) {
            Some(rt) => rt,
            None => {
                return Ok(ExecutionResult::failure(format!(
                    "Unknown link type '{}'; must be one of: {}",
                    self.link_type,
                    VALID_LINK_TYPES.join(", ")
                )));
            }
        };

        // Build the target work item URL for the relation
        let target_url = format!(
            "{}/{}/_apis/wit/workitems/{}",
            org_url.trim_end_matches('/'),
            utf8_percent_encode(project, PATH_SEGMENT),
            target_id,
        );

        // Build the JSON Patch body
        let mut relation_value = serde_json::json!({
            "rel": relation_type,
            "url": target_url,
        });

        if let Some(ref comment) = self.comment {
            relation_value["attributes"] = serde_json::json!({
                "comment": comment,
            });
        }

        let patch_doc = vec![serde_json::json!({
            "op": "add",
            "path": "/relations/-",
            "value": relation_value,
        })];

        // PATCH https://dev.azure.com/{org}/{project}/_apis/wit/workitems/{id}?api-version=7.1
        let url = format!(
            "{}/{}/_apis/wit/workitems/{}?api-version=7.1",
            org_url.trim_end_matches('/'),
            utf8_percent_encode(project, PATH_SEGMENT),
            source_id,
        );
        debug!("API URL: {}", url);

        let client = reqwest::Client::new();

        info!(
            "Sending link request: #{} -[{}]-> #{}",
            source_id, self.link_type, target_id
        );
        let response = client
            .patch(&url)
            .header("Content-Type", "application/json-patch+json")
            .basic_auth("", Some(token))
            .json(&patch_doc)
            .send()
            .await
            .context("Failed to send request to Azure DevOps")?;

        if response.status().is_success() {
            let body: serde_json::Value = response
                .json()
                .await
                .context("Failed to parse response JSON")?;

            let work_item_id = body.get("id").and_then(|v| v.as_i64()).unwrap_or(0);

            info!(
                "Linked work item #{} -> #{} ({})",
                source_id, target_id, self.link_type
            );

            Ok(ExecutionResult::success_with_data(
                format!(
                    "Linked work item #{} -> #{} ({})",
                    source_id, target_id, self.link_type
                ),
                serde_json::json!({
                    "source_id": source_id,
                    "target_id": target_id,
                    "link_type": self.link_type,
                    "relation_type": relation_type,
                    "work_item_id": work_item_id,
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
                "Failed to link work item #{} -> #{} (HTTP {}): {}",
                source_id, target_id, status, error_body
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::safe_outputs::ResolvedWorkItem;
    use crate::secure::WorkItemTemporaryId;

    #[test]
    fn temporary_ids_deserialize_for_both_ends() {
        let params: LinkWorkItemsParams = serde_json::from_value(serde_json::json!({
            "source_id": "#aw_created1",
            "target_id": 200,
            "link_type": "related"
        }))
        .unwrap();
        assert_eq!(
            params.source_id,
            WorkItemReference::Temporary(WorkItemTemporaryId::parse("#aw_created1").unwrap())
        );
        assert_eq!(params.target_id, WorkItemReference::Number(200));
    }

    #[tokio::test]
    async fn unresolved_temporary_id_fails_before_http() {
        let mut tool_configs = std::collections::HashMap::new();
        tool_configs.insert(
            "link-work-items".to_string(),
            serde_json::json!({"target": "*"}),
        );
        let ctx = ExecutionContext {
            tool_configs,
            ado_org_url: Some("https://dev.azure.com/org".to_string()),
            ado_project: Some("project".to_string()),
            access_token: Some("token".to_string()),
            ..Default::default()
        };
        let mut result: LinkWorkItemsResult = LinkWorkItemsParams {
            source_id: WorkItemReference::Temporary(
                WorkItemTemporaryId::parse("#aw_missing").unwrap(),
            ),
            target_id: WorkItemReference::Number(200),
            link_type: "related".to_string(),
            comment: None,
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
    async fn temporary_ids_resolving_to_the_same_work_item_are_rejected() {
        let mut tool_configs = std::collections::HashMap::new();
        tool_configs.insert(
            "link-work-items".to_string(),
            serde_json::json!({"target": "*"}),
        );
        let ctx = ExecutionContext {
            tool_configs,
            ado_org_url: Some("https://dev.azure.com/org".to_string()),
            ado_project: Some("project".to_string()),
            access_token: Some("token".to_string()),
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
        let mut result: LinkWorkItemsResult = LinkWorkItemsParams {
            source_id: WorkItemReference::Temporary(temporary_id),
            target_id: WorkItemReference::Number(4242),
            link_type: "related".to_string(),
            comment: None,
        }
        .try_into()
        .unwrap();
        let execution = result.execute_sanitized(&ctx).await.unwrap();
        assert!(!execution.success);
        assert!(
            execution.message.contains("same work item"),
            "unexpected message: {}",
            execution.message
        );
    }

    #[tokio::test]
    async fn resolved_temporary_ids_link_without_target() {
        use wiremock::matchers::{body_json, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let target_url = format!("{}/Project/_apis/wit/workitems/43", server.uri());
        Mock::given(method("PATCH"))
            .and(path("/Project/_apis/wit/workitems/42"))
            .and(body_json(serde_json::json!([{
                "op": "add",
                "path": "/relations/-",
                "value": {
                    "rel": "System.LinkTypes.Related",
                    "url": target_url,
                }
            }])))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({ "id": 42 })))
            .expect(1)
            .mount(&server)
            .await;

        let mut tool_configs = std::collections::HashMap::new();
        // No `target` configured: both ends resolve to work items created in
        // this run, so the target policy does not apply.
        tool_configs.insert("link-work-items".to_string(), serde_json::json!({}));
        let ctx = ExecutionContext {
            tool_configs,
            ado_org_url: Some(server.uri()),
            ado_project: Some("Project".to_string()),
            access_token: Some("token".to_string()),
            ..Default::default()
        };
        let source_temporary = WorkItemTemporaryId::parse("#aw_source1").unwrap();
        let target_temporary = WorkItemTemporaryId::parse("#aw_target1").unwrap();
        ctx.register_resolved_work_item(
            &source_temporary,
            ResolvedWorkItem {
                id: 42,
                url: "https://example.test/items/42".to_string(),
            },
        )
        .unwrap();
        ctx.register_resolved_work_item(
            &target_temporary,
            ResolvedWorkItem {
                id: 43,
                url: "https://example.test/items/43".to_string(),
            },
        )
        .unwrap();
        let mut result: LinkWorkItemsResult = LinkWorkItemsParams {
            source_id: WorkItemReference::Temporary(source_temporary),
            target_id: WorkItemReference::Temporary(target_temporary),
            link_type: "related".to_string(),
            comment: None,
        }
        .try_into()
        .unwrap();
        let execution = result.execute_sanitized(&ctx).await.unwrap();
        assert!(execution.success, "link failed: {}", execution.message);
        assert_eq!(
            execution
                .data
                .as_ref()
                .and_then(|data| data["source_id"].as_i64()),
            Some(42)
        );
        assert_eq!(
            execution
                .data
                .as_ref()
                .and_then(|data| data["target_id"].as_i64()),
            Some(43)
        );
    }

    #[tokio::test]
    async fn out_of_range_ids_fail_before_http() {
        let mut tool_configs = std::collections::HashMap::new();
        tool_configs.insert(
            "link-work-items".to_string(),
            serde_json::json!({"target": "*"}),
        );
        let ctx = ExecutionContext {
            tool_configs,
            ado_org_url: Some("https://dev.azure.com/org".to_string()),
            ado_project: Some("project".to_string()),
            access_token: Some("token".to_string()),
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
        // Resolved temporary ID out of range.
        let mut result: LinkWorkItemsResult = LinkWorkItemsParams {
            source_id: WorkItemReference::Temporary(temporary_id),
            target_id: WorkItemReference::Number(200),
            link_type: "related".to_string(),
            comment: None,
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

        // Numeric ID out of range.
        let mut result: LinkWorkItemsResult = LinkWorkItemsParams {
            source_id: WorkItemReference::Number(100),
            target_id: WorkItemReference::Number(u64::MAX),
            link_type: "related".to_string(),
            comment: None,
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
    use super::*;

    #[test]
    fn test_params_deserializes() {
        let json = r#"{"source_id": 100, "target_id": 200, "link_type": "parent", "comment": "test linking"}"#;
        let params: LinkWorkItemsParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.source_id, WorkItemReference::Number(100));
        assert_eq!(params.target_id, WorkItemReference::Number(200));
        assert_eq!(params.link_type, "parent");
        assert_eq!(params.comment.as_deref(), Some("test linking"));
    }

    #[test]
    fn test_params_converts_to_result() {
        let params = LinkWorkItemsParams {
            source_id: WorkItemReference::Number(100),
            target_id: WorkItemReference::Number(200),
            link_type: "child".to_string(),
            comment: Some("Links parent to child".to_string()),
        };
        let result: LinkWorkItemsResult = params.try_into().unwrap();
        assert_eq!(result.name, "link-work-items");
        assert_eq!(result.source_id, WorkItemReference::Number(100));
        assert_eq!(result.target_id, WorkItemReference::Number(200));
        assert_eq!(result.link_type, "child");
        assert_eq!(result.comment.as_deref(), Some("Links parent to child"));
    }

    #[test]
    fn test_validation_rejects_zero_source_id() {
        let params = LinkWorkItemsParams {
            source_id: WorkItemReference::Number(0),
            target_id: WorkItemReference::Number(200),
            link_type: "related".to_string(),
            comment: None,
        };
        let err = LinkWorkItemsResult::try_from(params).unwrap_err();
        assert!(
            err.to_string().contains("source_id must be positive"),
            "expected error about source_id, got: {err}"
        );
    }

    #[test]
    fn test_validation_rejects_zero_target_id() {
        let params = LinkWorkItemsParams {
            source_id: WorkItemReference::Number(100),
            target_id: WorkItemReference::Number(0),
            link_type: "related".to_string(),
            comment: None,
        };
        let err = LinkWorkItemsResult::try_from(params).unwrap_err();
        assert!(
            err.to_string().contains("target_id must be positive"),
            "expected error about target_id, got: {err}"
        );
    }

    #[test]
    fn test_validation_rejects_same_ids() {
        let params = LinkWorkItemsParams {
            source_id: WorkItemReference::Number(100),
            target_id: WorkItemReference::Number(100),
            link_type: "related".to_string(),
            comment: None,
        };
        let err = LinkWorkItemsResult::try_from(params).unwrap_err();
        assert!(
            err.to_string()
                .contains("source_id and target_id must be different"),
            "expected error about same ids, got: {err}"
        );
    }

    #[test]
    fn test_validation_rejects_invalid_link_type() {
        let params = LinkWorkItemsParams {
            source_id: WorkItemReference::Number(100),
            target_id: WorkItemReference::Number(200),
            link_type: "unknown".to_string(),
            comment: None,
        };
        let err = LinkWorkItemsResult::try_from(params).unwrap_err();
        assert!(
            err.to_string().contains("invalid link_type"),
            "expected error about invalid link_type, got: {err}"
        );
    }

    #[test]
    fn test_resolve_link_type() {
        assert_eq!(
            resolve_link_type("parent"),
            Some("System.LinkTypes.Hierarchy-Reverse")
        );
        assert_eq!(
            resolve_link_type("child"),
            Some("System.LinkTypes.Hierarchy-Forward")
        );
        assert_eq!(
            resolve_link_type("related"),
            Some("System.LinkTypes.Related")
        );
        assert_eq!(
            resolve_link_type("predecessor"),
            Some("System.LinkTypes.Dependency-Reverse")
        );
        assert_eq!(
            resolve_link_type("successor"),
            Some("System.LinkTypes.Dependency-Forward")
        );
        assert_eq!(
            resolve_link_type("duplicate"),
            Some("System.LinkTypes.Duplicate-Forward")
        );
        assert_eq!(
            resolve_link_type("duplicate-of"),
            Some("System.LinkTypes.Duplicate-Reverse")
        );
        assert_eq!(resolve_link_type("invalid"), None);
        assert_eq!(resolve_link_type(""), None);
    }

    #[test]
    fn test_config_defaults() {
        let config = LinkWorkItemsConfig::default();
        assert!(config.allowed_link_types.is_empty());
    }

    #[test]
    fn test_config_deserializes_from_yaml() {
        let yaml = r#"
allowed-link-types:
  - parent
  - child
  - related
"#;
        let config: LinkWorkItemsConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(
            config.allowed_link_types,
            vec!["parent", "child", "related"]
        );
    }

    #[test]
    fn test_result_serializes_correctly() {
        let params = LinkWorkItemsParams {
            source_id: WorkItemReference::Number(100),
            target_id: WorkItemReference::Number(200),
            link_type: "related".to_string(),
            comment: None,
        };
        let result: LinkWorkItemsResult = params.try_into().unwrap();
        let json = serde_json::to_string(&result).unwrap();

        assert!(json.contains(r#""name":"link-work-items""#));
        assert!(json.contains(r#""source_id":100"#));
        assert!(json.contains(r#""target_id":200"#));
        assert!(json.contains(r#""link_type":"related""#));
    }
}
