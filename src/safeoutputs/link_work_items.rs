//! Link work items safe output tool

use log::{debug, info};
use percent_encoding::utf8_percent_encode;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::PATH_SEGMENT;
use crate::sanitize::{Sanitize, sanitize as sanitize_text};
use crate::tool_result;
use crate::safeoutputs::{ExecutionContext, ExecutionResult, Executor, Validate};
use crate::safeoutputs::comment_on_work_item::CommentTarget;
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
    /// The source work item ID (the item the link is added to)
    pub source_id: i64,

    /// The target work item ID (the item being linked to)
    pub target_id: i64,

    /// Link type: parent, child, related, predecessor, successor, duplicate, duplicate-of
    pub link_type: String,

    /// Optional comment describing the relationship
    pub comment: Option<String>,
}

impl Validate for LinkWorkItemsParams {
    fn validate(&self) -> anyhow::Result<()> {
        ensure!(self.source_id > 0, "source_id must be positive");
        ensure!(self.target_id > 0, "target_id must be positive");
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
            ensure!(
                comment.len() >= 5,
                "comment must be at least 5 characters"
            );
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
        source_id: i64,
        target_id: i64,
        link_type: String,
        comment: Option<String>,
    }
}

impl Sanitize for LinkWorkItemsResult {
    fn sanitize_fields(&mut self) {
        self.link_type = sanitize_text(&self.link_type);
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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

impl Default for LinkWorkItemsConfig {
    fn default() -> Self {
        Self {
            allowed_link_types: Vec::new(),
            target: None,
        }
    }
}

#[async_trait::async_trait]
impl Executor for LinkWorkItemsResult {
    async fn execute_impl(&self, ctx: &ExecutionContext) -> anyhow::Result<ExecutionResult> {
        info!(
            "Linking work item #{} -> #{} ({})",
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

        let config: LinkWorkItemsConfig = ctx.get_tool_config("link-work-items");
        debug!("Allowed link types: {:?}", config.allowed_link_types);

        // Validate work item IDs against target scope
        match &config.target {
            None => {
                return Ok(ExecutionResult::failure(
                    "link-work-items requires a 'target' field in safe-outputs configuration \
                     to scope which work items can be linked. Example:\n  safe-outputs:\n    \
                     link-work-items:\n      target: \"*\""
                        .to_string(),
                ));
            }
            Some(target) => {
                // Check source_id
                if let Some(false) = target.allows_id(self.source_id) {
                    return Ok(ExecutionResult::failure(format!(
                        "Source work item #{} is not allowed by the configured target scope",
                        self.source_id
                    )));
                }
                // Check target_id
                if let Some(false) = target.allows_id(self.target_id) {
                    return Ok(ExecutionResult::failure(format!(
                        "Target work item #{} is not allowed by the configured target scope",
                        self.target_id
                    )));
                }
                // Area path validation is deferred — would need API calls for both IDs.
                // For now, ID-based and wildcard scoping is enforced.
            }
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
            self.target_id,
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
            self.source_id,
        );
        debug!("API URL: {}", url);

        let client = reqwest::Client::new();

        info!(
            "Sending link request: #{} -[{}]-> #{}",
            self.source_id, self.link_type, self.target_id
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
                self.source_id, self.target_id, self.link_type
            );

            Ok(ExecutionResult::success_with_data(
                format!(
                    "Linked work item #{} -> #{} ({})",
                    self.source_id, self.target_id, self.link_type
                ),
                serde_json::json!({
                    "source_id": self.source_id,
                    "target_id": self.target_id,
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
                self.source_id, self.target_id, status, error_body
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::safeoutputs::ToolResult;

    #[test]
    fn test_result_has_correct_name() {
        assert_eq!(LinkWorkItemsResult::NAME, "link-work-items");
    }

    #[test]
    fn test_params_deserializes() {
        let json =
            r#"{"source_id": 100, "target_id": 200, "link_type": "parent", "comment": "test linking"}"#;
        let params: LinkWorkItemsParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.source_id, 100);
        assert_eq!(params.target_id, 200);
        assert_eq!(params.link_type, "parent");
        assert_eq!(params.comment.as_deref(), Some("test linking"));
    }

    #[test]
    fn test_params_converts_to_result() {
        let params = LinkWorkItemsParams {
            source_id: 100,
            target_id: 200,
            link_type: "child".to_string(),
            comment: Some("Links parent to child".to_string()),
        };
        let result: LinkWorkItemsResult = params.try_into().unwrap();
        assert_eq!(result.name, "link-work-items");
        assert_eq!(result.source_id, 100);
        assert_eq!(result.target_id, 200);
        assert_eq!(result.link_type, "child");
        assert_eq!(result.comment.as_deref(), Some("Links parent to child"));
    }

    #[test]
    fn test_validation_rejects_zero_source_id() {
        let params = LinkWorkItemsParams {
            source_id: 0,
            target_id: 200,
            link_type: "related".to_string(),
            comment: None,
        };
        let result: Result<LinkWorkItemsResult, _> = params.try_into();
        assert!(result.is_err());
    }

    #[test]
    fn test_validation_rejects_zero_target_id() {
        let params = LinkWorkItemsParams {
            source_id: 100,
            target_id: 0,
            link_type: "related".to_string(),
            comment: None,
        };
        let result: Result<LinkWorkItemsResult, _> = params.try_into();
        assert!(result.is_err());
    }

    #[test]
    fn test_validation_rejects_same_ids() {
        let params = LinkWorkItemsParams {
            source_id: 100,
            target_id: 100,
            link_type: "related".to_string(),
            comment: None,
        };
        let result: Result<LinkWorkItemsResult, _> = params.try_into();
        assert!(result.is_err());
    }

    #[test]
    fn test_validation_rejects_invalid_link_type() {
        let params = LinkWorkItemsParams {
            source_id: 100,
            target_id: 200,
            link_type: "unknown".to_string(),
            comment: None,
        };
        let result: Result<LinkWorkItemsResult, _> = params.try_into();
        assert!(result.is_err());
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
        assert_eq!(config.allowed_link_types.len(), 3);
        assert!(config.allowed_link_types.contains(&"parent".to_string()));
        assert!(config.allowed_link_types.contains(&"child".to_string()));
        assert!(config.allowed_link_types.contains(&"related".to_string()));
    }

    #[test]
    fn test_result_serializes_correctly() {
        let params = LinkWorkItemsParams {
            source_id: 100,
            target_id: 200,
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
