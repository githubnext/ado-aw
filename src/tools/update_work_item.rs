//! Update work item tool implementation

use log::{debug, info};
use percent_encoding::utf8_percent_encode;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::PATH_SEGMENT;
use crate::tool_result;
use crate::tools::{ExecutionContext, ExecutionResult, Executor, Validate};
use crate::sanitize::{Sanitize, sanitize as sanitize_text};
use anyhow::{Context, ensure};

/// Parameters for updating a work item
#[derive(Deserialize, JsonSchema)]
pub struct UpdateWorkItemParams {
    /// ID of the work item to update
    pub id: u64,

    /// New title for the work item (only if enabled in the safe-outputs configuration)
    pub title: Option<String>,

    /// New description/body in markdown format (only if enabled in the safe-outputs configuration)
    pub body: Option<String>,

    /// New state/status (e.g., "Active", "Resolved", "Closed"); only if enabled in the safe-outputs configuration
    pub state: Option<String>,

    /// New area path (only if enabled in the safe-outputs configuration)
    pub area_path: Option<String>,

    /// New iteration path (only if enabled in the safe-outputs configuration)
    pub iteration_path: Option<String>,

    /// New assignee email or display name (only if enabled in the safe-outputs configuration)
    pub assignee: Option<String>,

    /// New tags (replaces all existing tags; only if enabled in the safe-outputs configuration)
    pub tags: Option<Vec<String>>,
}

impl Validate for UpdateWorkItemParams {
    fn validate(&self) -> anyhow::Result<()> {
        ensure!(self.id > 0, "Work item ID must be a positive integer");
        ensure!(
            self.title.is_some()
                || self.body.is_some()
                || self.state.is_some()
                || self.area_path.is_some()
                || self.iteration_path.is_some()
                || self.assignee.is_some()
                || self.tags.is_some(),
            "At least one field must be provided for update (title, body, state, area_path, iteration_path, assignee, or tags)"
        );
        if let Some(title) = &self.title {
            ensure!(!title.is_empty(), "Title cannot be empty");
            ensure!(title.len() <= 255, "Title must be 255 characters or fewer");
        }
        if let Some(tags) = &self.tags {
            for tag in tags {
                ensure!(
                    !tag.contains(';'),
                    "Tag '{}' contains a semicolon, which is not allowed \
                     (ADO uses semicolons as tag separators)",
                    tag
                );
            }
        }
        Ok(())
    }
}

tool_result! {
    name = "update-work-item",
    write = true,
    params = UpdateWorkItemParams,
    /// Result of updating a work item
    pub struct UpdateWorkItemResult {
        id: u64,
        title: Option<String>,
        body: Option<String>,
        state: Option<String>,
        area_path: Option<String>,
        iteration_path: Option<String>,
        assignee: Option<String>,
        tags: Option<Vec<String>>,
    }
}

impl Sanitize for UpdateWorkItemResult {
    fn sanitize_fields(&mut self) {
        self.title = self.title.as_deref().map(sanitize_text);
        self.body = self.body.as_deref().map(sanitize_text);
        self.state = self.state.as_deref().map(sanitize_text);
        self.area_path = self.area_path.as_deref().map(sanitize_text);
        self.iteration_path = self.iteration_path.as_deref().map(sanitize_text);
        self.assignee = self.assignee.as_deref().map(sanitize_text);
        self.tags = self
            .tags
            .as_ref()
            .map(|ts| ts.iter().map(|t| sanitize_text(t)).collect());
    }
}

/// Which work items can be targeted for update
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum TargetConfig {
    /// Specific work item ID (agent can only update this exact work item)
    Id(u64),
    /// String pattern: `"*"` means any work item the agent specifies
    Pattern(String),
}

/// Configuration for the update-work-item tool (specified in front matter).
///
/// Example front matter:
/// ```yaml
/// safe-outputs:
///   update-work-item:
///     status: true              # enable state/status updates
///     title: true               # enable title updates
///     body: true                # enable body/description updates
///     markdown-body: true       # store body as markdown (requires ADO Services or Server 2022+)
///     title-prefix: "[bot] "    # only update work items whose title starts with this prefix
///     tag-prefix: "agent-"      # only update work items that have at least one tag starting with this prefix
///     max: 3                    # max updates per run (default: 1)
///     target: "*"               # "*" or a specific work item ID number (required)
///     area-path: true           # enable area path updates
///     iteration-path: true      # enable iteration path updates
///     assignee: true            # enable assignee updates
///     tags: true                # enable tag updates
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UpdateWorkItemConfig {
    /// Enable state/status updates via the `state` agent parameter (default: false).
    /// The YAML key for this option is `status`.
    #[serde(default)]
    pub status: bool,

    /// Enable title updates (default: false)
    #[serde(default)]
    pub title: bool,

    /// Enable body/description updates (default: false)
    #[serde(default)]
    pub body: bool,

    /// When true, adds a `/multilineFieldsFormat/System.Description: "Markdown"` patch
    /// operation alongside the body update so that ADO renders the content as markdown.
    ///
    /// Only supported on Azure DevOps Services and Server 2022+. On older on-premises
    /// deployments this extra operation causes the entire PATCH to fail with HTTP 400,
    /// so the flag defaults to `false` for broad compatibility.
    #[serde(default, rename = "markdown-body")]
    pub markdown_body: bool,

    /// Only update work items whose current title starts with this prefix.
    /// Requires an extra GET request to fetch the current title before patching.
    #[serde(default, rename = "title-prefix")]
    pub title_prefix: Option<String>,

    /// Only update work items that have at least one tag starting with this prefix.
    /// ADO stores tags as a semicolon-separated string; each tag is trimmed before comparison.
    /// Requires an extra GET request to fetch the current tags before patching.
    #[serde(default, rename = "tag-prefix")]
    pub tag_prefix: Option<String>,

    /// Which work items can be updated (required):
    /// - `"*"`: any work item ID the agent specifies
    /// - An integer: only that specific work item ID
    pub target: Option<TargetConfig>,

    /// Enable area path updates (default: false)
    #[serde(default, rename = "area-path")]
    pub area_path: bool,

    /// Enable iteration path updates (default: false)
    #[serde(default, rename = "iteration-path")]
    pub iteration_path: bool,

    /// Enable assignee updates (default: false)
    #[serde(default)]
    pub assignee: bool,

    /// Enable tag updates (default: false)
    #[serde(default)]
    pub tags: bool,
}

/// Build a replace-field patch operation for work item updates
fn replace_field_op(field: &str, value: impl Into<String>) -> serde_json::Value {
    serde_json::json!({
        "op": "replace",
        "path": format!("/fields/{}", field),
        "value": value.into()
    })
}

/// Fetch the current work item from ADO and return the full response body
async fn fetch_work_item(
    client: &reqwest::Client,
    org_url: &str,
    project: &str,
    token: &str,
    id: u64,
) -> anyhow::Result<serde_json::Value> {
    let url = format!(
        "{}/{}/_apis/wit/workitems/{}?api-version=7.0",
        org_url.trim_end_matches('/'),
        utf8_percent_encode(project, PATH_SEGMENT),
        id,
    );

    let response = client
        .get(&url)
        .basic_auth("", Some(token))
        .send()
        .await
        .context("Failed to fetch work item")?;

    if response.status().is_success() {
        response
            .json()
            .await
            .context("Failed to parse work item response")
    } else {
        let status = response.status();
        let error_body = response
            .text()
            .await
            .unwrap_or_else(|_| "Unknown error".to_string());
        anyhow::bail!(
            "Failed to fetch work item #{} (HTTP {}): {}",
            id,
            status,
            error_body
        )
    }
}

#[async_trait::async_trait]
impl Executor for UpdateWorkItemResult {
    async fn execute_impl(&self, ctx: &ExecutionContext) -> anyhow::Result<ExecutionResult> {
        info!("Updating work item #{}", self.id);
        debug!(
            "Fields: title={:?}, body_len={:?}, state={:?}, area={:?}, iter={:?}, assignee={:?}, tags={:?}",
            self.title,
            self.body.as_ref().map(|b| b.len()),
            self.state,
            self.area_path,
            self.iteration_path,
            self.assignee,
            self.tags,
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

        let config: UpdateWorkItemConfig = ctx.get_tool_config("update-work-item");
        debug!(
            "Config: status={}, title={}, body={}, markdown_body={}, target={:?}, title_prefix={:?}, tag_prefix={:?}",
            config.status,
            config.title,
            config.body,
            config.markdown_body,
            config.target,
            config.title_prefix,
            config.tag_prefix,
        );

        // Validate the target constraint
        let target = match &config.target {
            Some(t) => t,
            None => {
                return Ok(ExecutionResult::failure(
                    "update-work-item target is not configured. \
                     This is required to scope which work items the agent can update."
                        .to_string(),
                ));
            }
        };
        let target_allowed = match target {
            TargetConfig::Pattern(p) if p == "*" => true,
            TargetConfig::Id(allowed_id) => *allowed_id == self.id,
            TargetConfig::Pattern(p) => {
                log::warn!(
                    "update-work-item: unrecognised target pattern '{}'; \
                     only \"*\" or an integer ID are valid — all updates are blocked",
                    p
                );
                false
            }
        };
        if !target_allowed {
            return Ok(ExecutionResult::failure(format!(
                "Work item #{} is not permitted by the update-work-item target configuration",
                self.id
            )));
        }

        // Validate that each provided field is enabled in the configuration
        if self.title.is_some() && !config.title {
            return Ok(ExecutionResult::failure(
                "Title updates are not enabled in the update-work-item configuration; set 'title: true' in safe-outputs",
            ));
        }
        if self.body.is_some() && !config.body {
            return Ok(ExecutionResult::failure(
                "Body/description updates are not enabled in the update-work-item configuration; set 'body: true' in safe-outputs",
            ));
        }
        if self.state.is_some() && !config.status {
            return Ok(ExecutionResult::failure(
                "State/status updates are not enabled in the update-work-item configuration; set 'status: true' in safe-outputs",
            ));
        }
        if self.area_path.is_some() && !config.area_path {
            return Ok(ExecutionResult::failure(
                "Area path updates are not enabled in the update-work-item configuration; set 'area-path: true' in safe-outputs",
            ));
        }
        if self.iteration_path.is_some() && !config.iteration_path {
            return Ok(ExecutionResult::failure(
                "Iteration path updates are not enabled in the update-work-item configuration; set 'iteration-path: true' in safe-outputs",
            ));
        }
        if self.assignee.is_some() && !config.assignee {
            return Ok(ExecutionResult::failure(
                "Assignee updates are not enabled in the update-work-item configuration; set 'assignee: true' in safe-outputs",
            ));
        }
        if self.tags.is_some() && !config.tags {
            return Ok(ExecutionResult::failure(
                "Tag updates are not enabled in the update-work-item configuration; set 'tags: true' in safe-outputs",
            ));
        }

        let client = reqwest::Client::new();

        // If either prefix guard is configured, fetch the current work item once and check both
        if config.title_prefix.is_some() || config.tag_prefix.is_some() {
            debug!(
                "Fetching work item #{} to check prefix guards (title_prefix={:?}, tag_prefix={:?})",
                self.id, config.title_prefix, config.tag_prefix
            );
            match fetch_work_item(&client, org_url, project, token, self.id).await {
                Ok(wi) => {
                    // title-prefix check
                    if let Some(prefix) = &config.title_prefix {
                        let current_title = wi
                            .get("fields")
                            .and_then(|f| f.get("System.Title"))
                            .and_then(|t| t.as_str())
                            .unwrap_or("");
                        if !current_title.starts_with(prefix.as_str()) {
                            return Ok(ExecutionResult::failure(format!(
                                "Work item #{} title '{}' does not start with the required prefix '{}' (configured in title-prefix)",
                                self.id, current_title, prefix
                            )));
                        }
                        debug!("Title-prefix check passed: '{}'", current_title);
                    }

                    // tag-prefix check: ADO stores tags as a semicolon-separated string
                    if let Some(prefix) = &config.tag_prefix {
                        let raw_tags = wi
                            .get("fields")
                            .and_then(|f| f.get("System.Tags"))
                            .and_then(|t| t.as_str())
                            .unwrap_or("");
                        let has_matching_tag = raw_tags
                            .split(';')
                            .map(str::trim)
                            .any(|tag| tag.starts_with(prefix.as_str()));
                        if !has_matching_tag {
                            return Ok(ExecutionResult::failure(format!(
                                "Work item #{} has no tag starting with '{}' (configured in tag-prefix). Current tags: '{}'",
                                self.id, prefix, raw_tags
                            )));
                        }
                        debug!("Tag-prefix check passed; matched in tags: '{}'", raw_tags);
                    }
                }
                Err(e) => {
                    return Ok(ExecutionResult::failure(format!(
                        "Failed to fetch work item #{} for prefix validation: {}",
                        self.id, e
                    )));
                }
            }
        }

        // Build the JSON Patch document for the update
        let mut patch_doc: Vec<serde_json::Value> = Vec::new();

        if let Some(title) = &self.title {
            patch_doc.push(replace_field_op("System.Title", title));
        }
        if let Some(body) = &self.body {
            patch_doc.push(replace_field_op("System.Description", body));
            // Only add the markdown format hint when explicitly opted in.
            // This op is only supported on ADO Services and Server 2022+; omitting it
            // keeps the body update working on older on-premises deployments.
            if config.markdown_body {
                patch_doc.push(serde_json::json!({
                    "op": "replace",
                    "path": "/multilineFieldsFormat/System.Description",
                    "value": "Markdown"
                }));
            }
        }
        if let Some(state) = &self.state {
            patch_doc.push(replace_field_op("System.State", state));
        }
        if let Some(area_path) = &self.area_path {
            patch_doc.push(replace_field_op("System.AreaPath", area_path));
        }
        if let Some(iteration_path) = &self.iteration_path {
            patch_doc.push(replace_field_op("System.IterationPath", iteration_path));
        }
        if let Some(assignee) = &self.assignee {
            patch_doc.push(replace_field_op("System.AssignedTo", assignee));
        }
        if let Some(tags) = &self.tags {
            patch_doc.push(replace_field_op("System.Tags", tags.join("; ")));
        }

        // Make the PATCH API call
        let url = format!(
            "{}/{}/_apis/wit/workitems/{}?api-version=7.0",
            org_url.trim_end_matches('/'),
            utf8_percent_encode(project, PATH_SEGMENT),
            self.id,
        );
        debug!("PATCH URL: {}", url);

        let response = client
            .patch(&url)
            .header("Content-Type", "application/json-patch+json")
            .basic_auth("", Some(token))
            .json(&patch_doc)
            .send()
            .await
            .context("Failed to send update request to Azure DevOps")?;

        if response.status().is_success() {
            let body: serde_json::Value = response
                .json()
                .await
                .context("Failed to parse response JSON")?;
            let work_item_url = body
                .get("_links")
                .and_then(|l| l.get("html"))
                .and_then(|h| h.get("href"))
                .and_then(|h| h.as_str())
                .unwrap_or("");

            info!("Work item #{} updated successfully", self.id);
            Ok(ExecutionResult::success_with_data(
                format!("Updated work item #{}", self.id),
                serde_json::json!({
                    "id": self.id,
                    "url": work_item_url,
                }),
            ))
        } else {
            let status = response.status();
            let error_body = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            Ok(ExecutionResult::failure(format!(
                "Failed to update work item #{} (HTTP {}): {}",
                self.id, status, error_body
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::ToolResult;

    #[test]
    fn test_result_has_correct_name() {
        assert_eq!(UpdateWorkItemResult::NAME, "update-work-item");
    }

    #[test]
    fn test_params_validates_requires_positive_id() {
        let params = UpdateWorkItemParams {
            id: 0,
            title: Some("New title".to_string()),
            body: None,
            state: None,
            area_path: None,
            iteration_path: None,
            assignee: None,
            tags: None,
        };
        let result: Result<UpdateWorkItemResult, _> = params.try_into();
        assert!(result.is_err());
    }

    #[test]
    fn test_params_validates_requires_at_least_one_field() {
        let params = UpdateWorkItemParams {
            id: 42,
            title: None,
            body: None,
            state: None,
            area_path: None,
            iteration_path: None,
            assignee: None,
            tags: None,
        };
        let result: Result<UpdateWorkItemResult, _> = params.try_into();
        assert!(result.is_err());
    }

    #[test]
    fn test_params_validates_title_length() {
        let params = UpdateWorkItemParams {
            id: 42,
            title: Some("x".repeat(256)),
            body: None,
            state: None,
            area_path: None,
            iteration_path: None,
            assignee: None,
            tags: None,
        };
        let result: Result<UpdateWorkItemResult, _> = params.try_into();
        assert!(result.is_err());
    }

    #[test]
    fn test_params_valid_title_only() {
        let params = UpdateWorkItemParams {
            id: 42,
            title: Some("Valid new title".to_string()),
            body: None,
            state: None,
            area_path: None,
            iteration_path: None,
            assignee: None,
            tags: None,
        };
        let result: Result<UpdateWorkItemResult, _> = params.try_into();
        assert!(result.is_ok());
        let result = result.unwrap();
        assert_eq!(result.id, 42);
        assert_eq!(result.title, Some("Valid new title".to_string()));
    }

    #[test]
    fn test_params_valid_state_only() {
        let params = UpdateWorkItemParams {
            id: 123,
            title: None,
            body: None,
            state: Some("Resolved".to_string()),
            area_path: None,
            iteration_path: None,
            assignee: None,
            tags: None,
        };
        let result: Result<UpdateWorkItemResult, _> = params.try_into();
        assert!(result.is_ok());
    }

    #[test]
    fn test_params_valid_tags() {
        let params = UpdateWorkItemParams {
            id: 42,
            title: None,
            body: None,
            state: None,
            area_path: None,
            iteration_path: None,
            assignee: None,
            tags: Some(vec!["automated".to_string(), "agent".to_string()]),
        };
        let result: Result<UpdateWorkItemResult, _> = params.try_into();
        assert!(result.is_ok());
        let result = result.unwrap();
        assert_eq!(
            result.tags,
            Some(vec!["automated".to_string(), "agent".to_string()])
        );
    }

    #[test]
    fn test_result_serializes_correctly() {
        let params = UpdateWorkItemParams {
            id: 99,
            title: Some("Test title".to_string()),
            body: None,
            state: Some("Active".to_string()),
            area_path: None,
            iteration_path: None,
            assignee: None,
            tags: None,
        };
        let result: UpdateWorkItemResult = params.try_into().unwrap();
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains(r#""name":"update-work-item""#));
        assert!(json.contains(r#""id":99"#));
        assert!(json.contains(r#""title":"Test title""#));
        assert!(json.contains(r#""state":"Active""#));
    }

    #[test]
    fn test_config_defaults() {
        let config = UpdateWorkItemConfig::default();
        assert!(!config.status);
        assert!(!config.title);
        assert!(!config.body);
        assert!(!config.markdown_body);
        assert!(!config.area_path);
        assert!(!config.iteration_path);
        assert!(!config.assignee);
        assert!(!config.tags);
        assert!(config.target.is_none());
        assert!(config.title_prefix.is_none());
        assert!(config.tag_prefix.is_none());
    }

    #[test]
    fn test_config_deserializes_from_yaml() {
        let yaml = r#"
status: true
title: true
body: true
markdown-body: true
title-prefix: "[bot] "
tag-prefix: "agent-"
max: 3
target: "*"
area-path: true
iteration-path: true
assignee: true
tags: true
"#;
        let config: UpdateWorkItemConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(config.status);
        assert!(config.title);
        assert!(config.body);
        assert!(config.markdown_body);
        assert_eq!(config.title_prefix, Some("[bot] ".to_string()));
        assert_eq!(config.tag_prefix, Some("agent-".to_string()));
        assert_eq!(config.target, Some(TargetConfig::Pattern("*".to_string())));
        assert!(config.area_path);
        assert!(config.iteration_path);
        assert!(config.assignee);
        assert!(config.tags);
    }

    #[test]
    fn test_config_target_specific_id() {
        let yaml = r#"
title: true
target: 42
"#;
        let config: UpdateWorkItemConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.target, Some(TargetConfig::Id(42)));
    }

    #[test]
    fn test_config_partial_uses_defaults() {
        let yaml = "status: true";
        let config: UpdateWorkItemConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(config.status);
        assert!(!config.title);
        assert!(config.target.is_none());
    }

    #[tokio::test]
    async fn test_execute_requires_ado_context() {
        use crate::tools::Executor;
        use std::collections::HashMap;
        use std::path::PathBuf;

        let params = UpdateWorkItemParams {
            id: 42,
            title: Some("New title".to_string()),
            body: None,
            state: None,
            area_path: None,
            iteration_path: None,
            assignee: None,
            tags: None,
        };
        let mut result: UpdateWorkItemResult = params.try_into().unwrap();
        let ctx = ExecutionContext {
            ado_org_url: None,
            ado_organization: None,
            ado_project: None,
            access_token: None,
            working_directory: PathBuf::from("."),
            source_directory: PathBuf::from("."),
            tool_configs: HashMap::new(),
            repository_id: None,
            repository_name: None,
            allowed_repositories: HashMap::new(),
        };

        let exec_result = result.execute_sanitized(&ctx).await;
        assert!(exec_result.is_err());
        assert!(
            exec_result
                .unwrap_err()
                .to_string()
                .contains("AZURE_DEVOPS_ORG_URL")
        );
    }

    #[tokio::test]
    async fn test_execute_rejects_disabled_title_update() {
        use std::collections::HashMap;
        use std::path::PathBuf;

        let params = UpdateWorkItemParams {
            id: 42,
            title: Some("New title".to_string()),
            body: None,
            state: None,
            area_path: None,
            iteration_path: None,
            assignee: None,
            tags: None,
        };
        let mut result: UpdateWorkItemResult = params.try_into().unwrap();

        // Config with title updates disabled (default), but target set so target check passes
        let config = UpdateWorkItemConfig {
            target: Some(TargetConfig::Pattern("*".to_string())),
            ..UpdateWorkItemConfig::default()
        };
        let config_value = serde_json::to_value(config).unwrap();
        let mut tool_configs = HashMap::new();
        tool_configs.insert("update-work-item".to_string(), config_value);

        let ctx = ExecutionContext {
            ado_org_url: Some("https://dev.azure.com/myorg".to_string()),
            ado_organization: Some("myorg".to_string()),
            ado_project: Some("MyProject".to_string()),
            access_token: Some("fake-token".to_string()),
            working_directory: PathBuf::from("."),
            source_directory: PathBuf::from("."),
            tool_configs,
            repository_id: None,
            repository_name: None,
            allowed_repositories: HashMap::new(),
        };

        let exec_result = result.execute_sanitized(&ctx).await.unwrap();
        assert!(!exec_result.success);
        assert!(exec_result.message.contains("Title updates are not enabled"));
    }

    #[tokio::test]
    async fn test_execute_rejects_disallowed_target() {
        use std::collections::HashMap;
        use std::path::PathBuf;

        let params = UpdateWorkItemParams {
            id: 42,
            title: Some("New title".to_string()),
            body: None,
            state: None,
            area_path: None,
            iteration_path: None,
            assignee: None,
            tags: None,
        };
        let mut result: UpdateWorkItemResult = params.try_into().unwrap();

        // Config that only allows work item ID 99, not 42
        let config = UpdateWorkItemConfig {
            title: true,
            target: Some(TargetConfig::Id(99)),
            ..UpdateWorkItemConfig::default()
        };
        let config_value = serde_json::to_value(config).unwrap();
        let mut tool_configs = HashMap::new();
        tool_configs.insert("update-work-item".to_string(), config_value);

        let ctx = ExecutionContext {
            ado_org_url: Some("https://dev.azure.com/myorg".to_string()),
            ado_organization: Some("myorg".to_string()),
            ado_project: Some("MyProject".to_string()),
            access_token: Some("fake-token".to_string()),
            working_directory: PathBuf::from("."),
            source_directory: PathBuf::from("."),
            tool_configs,
            repository_id: None,
            repository_name: None,
            allowed_repositories: HashMap::new(),
        };

        let exec_result = result.execute_sanitized(&ctx).await.unwrap();
        assert!(!exec_result.success);
        assert!(
            exec_result
                .message
                .contains("not permitted by the update-work-item target configuration")
        );
    }

    #[tokio::test]
    async fn test_execute_rejects_disabled_status_update() {
        use std::collections::HashMap;
        use std::path::PathBuf;

        let params = UpdateWorkItemParams {
            id: 42,
            title: None,
            body: None,
            state: Some("Resolved".to_string()),
            area_path: None,
            iteration_path: None,
            assignee: None,
            tags: None,
        };
        let mut result: UpdateWorkItemResult = params.try_into().unwrap();

        let config = UpdateWorkItemConfig {
            target: Some(TargetConfig::Pattern("*".to_string())),
            ..UpdateWorkItemConfig::default()
        }; // status: false
        let config_value = serde_json::to_value(config).unwrap();
        let mut tool_configs = HashMap::new();
        tool_configs.insert("update-work-item".to_string(), config_value);

        let ctx = ExecutionContext {
            ado_org_url: Some("https://dev.azure.com/myorg".to_string()),
            ado_organization: Some("myorg".to_string()),
            ado_project: Some("MyProject".to_string()),
            access_token: Some("fake-token".to_string()),
            working_directory: PathBuf::from("."),
            source_directory: PathBuf::from("."),
            tool_configs,
            repository_id: None,
            repository_name: None,
            allowed_repositories: HashMap::new(),
        };

        let exec_result = result.execute_sanitized(&ctx).await.unwrap();
        assert!(!exec_result.success);
        assert!(
            exec_result
                .message
                .contains("State/status updates are not enabled")
        );
    }

    #[test]
    fn test_sanitize_fields() {
        let params = UpdateWorkItemParams {
            id: 1,
            title: Some("Hello @user".to_string()),
            body: Some("Description with <script>alert(1)</script>".to_string()),
            state: None,
            area_path: None,
            iteration_path: None,
            assignee: None,
            tags: Some(vec!["tag-one".to_string(), "tag @two".to_string()]),
        };
        let mut result: UpdateWorkItemResult = params.try_into().unwrap();
        result.sanitize_fields();

        // @mentions should be neutralized
        assert!(result.title.as_deref().unwrap().contains("`@user`"));
        // tags should be sanitized
        let tags = result.tags.as_ref().unwrap();
        assert!(tags[1].contains("`@two`"));
    }

    // -------------------------------------------------------------------------
    // tag-prefix parsing / logic tests (no network calls needed)
    // -------------------------------------------------------------------------

    #[test]
    fn test_config_tag_prefix_deserializes() {
        let yaml = r#"
title: true
tag-prefix: "agent-"
"#;
        let config: UpdateWorkItemConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.tag_prefix, Some("agent-".to_string()));
    }

    #[test]
    fn test_config_tag_prefix_absent_is_none() {
        let yaml = "title: true";
        let config: UpdateWorkItemConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(config.tag_prefix.is_none());
    }

    /// Helper: simulate the tag-prefix logic in isolation so it can be unit-tested
    /// without spinning up an HTTP server.
    fn tag_prefix_matches(raw_tags: &str, prefix: &str) -> bool {
        raw_tags
            .split(';')
            .map(str::trim)
            .any(|tag| tag.starts_with(prefix))
    }

    #[test]
    fn test_tag_prefix_matches_single_tag() {
        assert!(tag_prefix_matches("agent-run", "agent-"));
    }

    #[test]
    fn test_tag_prefix_matches_one_of_several_tags() {
        assert!(tag_prefix_matches("bug; agent-2026; automated", "agent-"));
    }

    #[test]
    fn test_tag_prefix_matches_with_extra_spaces() {
        // ADO can emit tags with surrounding spaces
        assert!(tag_prefix_matches("  agent-run  ; other  ", "agent-"));
    }

    #[test]
    fn test_tag_prefix_no_match() {
        assert!(!tag_prefix_matches("bug; automated", "agent-"));
    }

    #[test]
    fn test_tag_prefix_empty_tags() {
        assert!(!tag_prefix_matches("", "agent-"));
    }

    #[test]
    fn test_tag_prefix_exact_match_still_passes() {
        // A tag that exactly equals the prefix (no trailing chars) should match
        assert!(tag_prefix_matches("agent-", "agent-"));
    }

    #[test]
    fn test_params_rejects_tag_with_semicolon() {
        // A tag containing a semicolon would inject additional ADO tags — reject it
        let params = UpdateWorkItemParams {
            id: 42,
            title: None,
            body: None,
            state: None,
            area_path: None,
            iteration_path: None,
            assignee: None,
            tags: Some(vec!["valid".to_string(), "injected; extra-tag".to_string()]),
        };
        let result: Result<UpdateWorkItemResult, _> = params.try_into();
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("semicolon"), "Expected semicolon error, got: {err}");
    }

    #[test]
    fn test_params_accepts_tags_without_semicolons() {
        let params = UpdateWorkItemParams {
            id: 42,
            title: None,
            body: None,
            state: None,
            area_path: None,
            iteration_path: None,
            assignee: None,
            tags: Some(vec!["agent-run".to_string(), "automated".to_string()]),
        };
        let result: Result<UpdateWorkItemResult, _> = params.try_into();
        assert!(result.is_ok());
    }
}
