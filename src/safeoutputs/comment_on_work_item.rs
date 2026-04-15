//! Comment on work item safe output tool

use log::{debug, info};
use percent_encoding::utf8_percent_encode;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::PATH_SEGMENT;
use ado_aw_derive::SanitizeConfig;
use crate::sanitize::{SanitizeContent, sanitize as sanitize_text};
use crate::tool_result;
use crate::safeoutputs::{ExecutionContext, ExecutionResult, Executor, Validate};
use anyhow::{Context, ensure};

/// Parameters for commenting on a work item
#[derive(Deserialize, JsonSchema)]
pub struct CommentOnWorkItemParams {
    /// The work item ID to comment on
    pub work_item_id: i64,

    /// Comment text in markdown format. Ensure adequate content > 10 characters.
    pub body: String,
}

impl Validate for CommentOnWorkItemParams {
    fn validate(&self) -> anyhow::Result<()> {
        ensure!(self.work_item_id > 0, "work_item_id must be positive");
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
        work_item_id: i64,
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
#[derive(Debug, Clone, SanitizeConfig, Default, Serialize, Deserialize)]
pub struct CommentOnWorkItemConfig {
    /// Target scope — which work items can be commented on.
    /// `None` means no target was configured; execution must reject this.
    pub target: Option<CommentTarget>,
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
    async fn execute_impl(&self, ctx: &ExecutionContext) -> anyhow::Result<ExecutionResult> {
        info!(
            "Commenting on work item #{}: {} chars",
            self.work_item_id,
            self.body.len()
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

        let config: CommentOnWorkItemConfig = ctx.get_tool_config("comment-on-work-item");
        debug!("Target: {:?}", config.target);

        let target = match &config.target {
            Some(t) => t,
            None => {
                return Ok(ExecutionResult::failure(
                    "comment-on-work-item target is not configured. \
                     This is required to scope which work items the agent can comment on."
                        .to_string(),
                ));
            }
        };

        let client = reqwest::Client::new();

        // Validate work item ID against target policy
        match target.allows_id(self.work_item_id) {
            Some(true) => {
                debug!(
                    "Work item #{} allowed by target policy",
                    self.work_item_id
                );
            }
            Some(false) => {
                return Ok(ExecutionResult::failure(format!(
                    "Work item #{} is not in the allowed target set",
                    self.work_item_id
                )));
            }
            None => {
                // Area path validation — need to fetch the work item.
                // Invariant: allows_id returns None only for StringTarget(s != "*"),
                // and area_path_prefix returns Some for exactly that case.
                let prefix = match target.area_path_prefix() {
                    Some(p) => p,
                    None => unreachable!(
                        "allows_id returned None but area_path_prefix is also None"
                    ),
                };
                debug!(
                    "Validating area path for work item #{} against prefix '{}'",
                    self.work_item_id, prefix
                );
                match get_work_item_area_path(&client, org_url, project, token, self.work_item_id)
                    .await
                {
                    Ok(area_path) => {
                        // ADO area paths are case-insensitive and use backslash separators.
                        // Require the match to land on a path boundary so that prefix "4x4"
                        // doesn't accidentally match "4x4Production".
                        let ap = area_path.to_lowercase();
                        let pf = prefix.to_lowercase();
                        let is_match = ap == pf
                            || (ap.starts_with(&*pf)
                                && ap[pf.len()..].starts_with('\\'));
                        if !is_match {
                            return Ok(ExecutionResult::failure(format!(
                                "Work item #{} has area path '{}' which is not under allowed prefix '{}'",
                                self.work_item_id, area_path, prefix
                            )));
                        }
                        debug!("Area path '{}' validated against '{}'", area_path, prefix);
                    }
                    Err(e) => {
                        return Ok(ExecutionResult::failure(format!(
                            "Failed to validate area path for work item #{}: {}",
                            self.work_item_id, e
                        )));
                    }
                }
            }
        }

        // Build the Azure DevOps REST API URL for adding a comment
        // POST https://dev.azure.com/{org}/{project}/_apis/wit/workItems/{id}/comments?api-version=7.1-preview.4
        let url = format!(
            "{}/{}/_apis/wit/workItems/{}/comments?api-version=7.1-preview.4",
            org_url.trim_end_matches('/'),
            utf8_percent_encode(project, PATH_SEGMENT),
            self.work_item_id,
        );
        debug!("API URL: {}", url);

        let comment_body = serde_json::json!({
            "text": self.body,
        });

        info!("Sending comment to work item #{}", self.work_item_id);
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
            let comment_url = body
                .get("url")
                .and_then(|h| h.as_str())
                .unwrap_or("");

            info!(
                "Comment added to work item #{}: comment #{}",
                self.work_item_id, comment_id
            );

            Ok(ExecutionResult::success_with_data(
                format!(
                    "Added comment #{} to work item #{}",
                    comment_id, self.work_item_id
                ),
                serde_json::json!({
                    "comment_id": comment_id,
                    "work_item_id": self.work_item_id,
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
                self.work_item_id, status, error_body
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
        assert_eq!(CommentOnWorkItemResult::NAME, "comment-on-work-item");
    }

    #[test]
    fn test_params_deserializes() {
        let json = r#"{"work_item_id": 12345, "body": "This is a comment on the work item."}"#;
        let params: CommentOnWorkItemParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.work_item_id, 12345);
        assert!(params.body.contains("comment"));
    }

    #[test]
    fn test_params_converts_to_result() {
        let params = CommentOnWorkItemParams {
            work_item_id: 42,
            body: "This is a test comment with enough characters.".to_string(),
        };
        let result: CommentOnWorkItemResult = params.try_into().unwrap();
        assert_eq!(result.name, "comment-on-work-item");
        assert_eq!(result.work_item_id, 42);
        assert!(result.body.contains("test comment"));
    }

    #[test]
    fn test_validation_rejects_zero_work_item_id() {
        let params = CommentOnWorkItemParams {
            work_item_id: 0,
            body: "This is a valid comment body text.".to_string(),
        };
        let result: Result<CommentOnWorkItemResult, _> = params.try_into();
        assert!(result.is_err());
    }

    #[test]
    fn test_validation_rejects_negative_work_item_id() {
        let params = CommentOnWorkItemParams {
            work_item_id: -5,
            body: "This is a valid comment body text.".to_string(),
        };
        let result: Result<CommentOnWorkItemResult, _> = params.try_into();
        assert!(result.is_err());
    }

    #[test]
    fn test_validation_rejects_short_body() {
        let params = CommentOnWorkItemParams {
            work_item_id: 42,
            body: "Too short".to_string(),
        };
        let result: Result<CommentOnWorkItemResult, _> = params.try_into();
        assert!(result.is_err());
    }

    #[test]
    fn test_result_serializes_correctly() {
        let params = CommentOnWorkItemParams {
            work_item_id: 42,
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

    #[test]
    fn test_config_partial_deserialize_uses_defaults() {
        let yaml = r#"
target: "*"
"#;
        let config: CommentOnWorkItemConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(config.target.is_some());
    }
}
