//! Create work item reporting schemas

use log::{debug, info};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::tool_result;
use crate::tools::{ExecutionContext, ExecutionResult, Executor, Validate};
use crate::sanitize::{Sanitize, sanitize as sanitize_text};
use anyhow::{Context, ensure};

/// Parameters for creating a work item
#[derive(Deserialize, JsonSchema)]
pub struct CreateWorkItemParams {
    /// A concise title for the work item; less than 64 characters
    pub title: String,

    /// Work item description in markdown format. Use headings, lists, code blocks, and other markdown formatting. Ensure adequate content > 30 characters.
    pub description: String,
}

impl Validate for CreateWorkItemParams {
    fn validate(&self) -> anyhow::Result<()> {
        ensure!(self.title.len() > 5);
        ensure!(self.description.len() > 30);
        Ok(())
    }
}

tool_result! {
    name = "create-work-item",
    params = CreateWorkItemParams,
    /// Result of creating a work item
    pub struct CreateWorkItemResult {
        title: String,
        description: String,
    }
}

impl Sanitize for CreateWorkItemResult {
    fn sanitize_fields(&mut self) {
        self.title = sanitize_text(&self.title);
        self.description = sanitize_text(&self.description);
    }
}

/// Configuration for the create_work_item tool (specified in front matter)
///
/// Example front matter:
/// ```yaml
/// safe-outputs:
///   create_work_item:
///     work_item_type: Task
///     area_path: "MyProject\\MyTeam"
///     iteration_path: "MyProject\\Sprint 1"
///     assignee: "user@example.com"
///     tags:
///       - agent-created
///       - automated
///     artifact_link:
///       enabled: true
///       repository: "my-repo-name"  # optional, defaults to current repo
///       branch: "main"              # optional, defaults to "main"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateWorkItemConfig {
    /// Work item type (default: "Task")
    #[serde(default = "default_work_item_type", rename = "work-item-type")]
    pub work_item_type: String,

    /// Area path for the work item
    #[serde(default, rename = "area-path")]
    pub area_path: Option<String>,

    /// Iteration path for the work item
    #[serde(default, rename = "iteration-path")]
    pub iteration_path: Option<String>,

    /// User to assign the work item to (email or display name)
    #[serde(default)]
    pub assignee: Option<String>,

    /// Tags to apply to the work item
    #[serde(default)]
    pub tags: Vec<String>,

    /// Additional custom fields as key-value pairs
    /// Keys should be the full field reference name (e.g., "Custom.MyField")
    #[serde(default, rename = "custom-fields")]
    pub custom_fields: std::collections::HashMap<String, String>,

    /// Artifact link configuration for GitHub Copilot integration
    #[serde(default, rename = "artifact-link")]
    pub artifact_link: ArtifactLinkConfig,
}

/// Configuration for artifact links (repository linking for GitHub Copilot)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactLinkConfig {
    /// Whether to add an artifact link to the work item (default: false)
    #[serde(default)]
    pub enabled: bool,

    /// Override the repository name (defaults to BUILD_REPOSITORY_NAME from environment)
    /// The repository ID will be resolved automatically via Azure DevOps API
    #[serde(default)]
    pub repository: Option<String>,

    /// Branch name to link to (default: "main")
    #[serde(default = "default_branch")]
    pub branch: String,
}

fn default_branch() -> String {
    "main".to_string()
}

impl Default for ArtifactLinkConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            repository: None,
            branch: default_branch(),
        }
    }
}

impl Default for CreateWorkItemConfig {
    fn default() -> Self {
        Self {
            work_item_type: default_work_item_type(),
            area_path: None,
            iteration_path: None,
            assignee: None,
            tags: Vec::new(),
            custom_fields: std::collections::HashMap::new(),
            artifact_link: ArtifactLinkConfig::default(),
        }
    }
}

fn default_work_item_type() -> String {
    "Task".to_string()
}

/// Build a field patch operation for work item creation
fn field_op(field: &str, value: impl Into<String>) -> serde_json::Value {
    serde_json::json!({
        "op": "add",
        "path": format!("/fields/{}", field),
        "value": value.into()
    })
}

/// Build an artifact link relation patch operation
fn artifact_link_op(project: &str, repository_id: &str, branch: &str) -> serde_json::Value {
    // Azure DevOps artifact link URL format for Git branch refs:
    // vstfs:///Git/Ref/{projectId}%2F{repositoryId}%2FGB{branchName}
    // GB prefix indicates a branch (Git Branch)
    let artifact_uri = format!(
        "vstfs:///Git/Ref/{}%2F{}%2FGB{}",
        project, repository_id, branch
    );

    serde_json::json!({
        "op": "add",
        "path": "/relations/-",
        "value": {
            "rel": "ArtifactLink",
            "url": artifact_uri,
            "attributes": {
                "name": "Branch"
            }
        }
    })
}

/// Resolve a repository name to its ID via Azure DevOps API
async fn resolve_repository_id(
    client: &reqwest::Client,
    org_url: &str,
    project: &str,
    token: &str,
    repo_name: &str,
) -> anyhow::Result<String> {
    // GET https://dev.azure.com/{organization}/{project}/_apis/git/repositories/{repositoryId}?api-version=7.0
    let url = format!(
        "{}/{}/_apis/git/repositories/{}?api-version=7.0",
        org_url.trim_end_matches('/'),
        project,
        repo_name
    );

    let response = client
        .get(&url)
        .basic_auth("", Some(token))
        .send()
        .await
        .context("Failed to query repository")?;

    if response.status().is_success() {
        let body: serde_json::Value = response
            .json()
            .await
            .context("Failed to parse repository response")?;

        body.get("id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .context("Repository response missing 'id' field")
    } else {
        let status = response.status();
        let error_body = response
            .text()
            .await
            .unwrap_or_else(|_| "Unknown error".to_string());

        anyhow::bail!(
            "Failed to resolve repository '{}' (HTTP {}): {}",
            repo_name,
            status,
            error_body
        )
    }
}

#[async_trait::async_trait]
impl Executor for CreateWorkItemResult {
    async fn execute_impl(&self, ctx: &ExecutionContext) -> anyhow::Result<ExecutionResult> {
        info!("Creating work item: '{}'", self.title);
        debug!("Description length: {} chars", self.description.len());

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

        // Get tool-specific configuration
        let config: CreateWorkItemConfig = ctx.get_tool_config("create-work-item");
        debug!("Work item type: {}", config.work_item_type);
        debug!("Area path: {:?}", config.area_path);
        debug!("Iteration path: {:?}", config.iteration_path);
        debug!("Assignee: {:?}", config.assignee);

        // Build the Azure DevOps REST API URL for creating work items
        // POST https://dev.azure.com/{organization}/{project}/_apis/wit/workitems/${type}?api-version=7.0
        debug!("Building work item creation request");
        let url = format!(
            "{}/{}/_apis/wit/workitems/${}?api-version=7.0",
            org_url.trim_end_matches('/'),
            project,
            config.work_item_type,
        );
        debug!("API URL: {}", url);

        // Build the patch document for work item creation
        let mut patch_doc = vec![
            field_op("System.Title", &self.title),
            field_op("System.Description", &self.description),
            // Tell Azure DevOps the description is markdown
            serde_json::json!({
                "op": "add",
                "path": "/multilineFieldsFormat/System.Description",
                "value": "Markdown"
            }),
        ];

        // Add optional configured fields
        if let Some(area_path) = &config.area_path {
            patch_doc.push(field_op("System.AreaPath", area_path));
        }
        if let Some(iteration_path) = &config.iteration_path {
            patch_doc.push(field_op("System.IterationPath", iteration_path));
        }
        if let Some(assignee) = &config.assignee {
            patch_doc.push(field_op("System.AssignedTo", assignee));
        }
        if !config.tags.is_empty() {
            patch_doc.push(field_op("System.Tags", config.tags.join("; ")));
        }

        // Add any custom fields
        for (field, value) in &config.custom_fields {
            patch_doc.push(field_op(field, value));
        }

        // Create HTTP client (needed for both work item creation and optional repo lookup)
        let client = reqwest::Client::new();

        // Add artifact link if configured (included in creation request)
        let artifact_link_included = if config.artifact_link.enabled {
            // Get repository name from config override or context
            let repo_name = config
                .artifact_link
                .repository
                .as_ref()
                .or(ctx.repository_name.as_ref());

            if let Some(repo_name) = repo_name {
                // If we already have the repo ID from environment, use it (avoids extra API call)
                let repo_id = if config.artifact_link.repository.is_none() {
                    // Using default repo from environment - check if we have the ID already
                    ctx.repository_id.clone()
                } else {
                    None // Config overrides require lookup
                };

                let repo_id = match repo_id {
                    Some(id) => id,
                    None => {
                        // Resolve repo name to ID via API
                        match resolve_repository_id(&client, org_url, project, token, repo_name)
                            .await
                        {
                            Ok(id) => id,
                            Err(e) => {
                                return Ok(ExecutionResult::failure(format!(
                                    "Failed to resolve repository '{}': {}",
                                    repo_name, e
                                )));
                            }
                        }
                    }
                };

                patch_doc.push(artifact_link_op(
                    project,
                    &repo_id,
                    &config.artifact_link.branch,
                ));
                Some(format!(
                    "linked to {}:{}",
                    repo_name, config.artifact_link.branch
                ))
            } else {
                Some("skipped: no repository available".to_string())
            }
        } else {
            None
        };

        // Make the API call
        info!("Sending work item creation request to ADO");
        let response = client
            .post(&url)
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
            let work_item_url = body
                .get("_links")
                .and_then(|l| l.get("html"))
                .and_then(|h| h.get("href"))
                .and_then(|h| h.as_str())
                .unwrap_or("");

            info!("Work item created: #{} - {}", work_item_id, work_item_url);

            let message = match &artifact_link_included {
                Some(link_msg) => format!(
                    "Created work item #{}: {} (artifact link: {})",
                    work_item_id, self.title, link_msg
                ),
                None => format!("Created work item #{}: {}", work_item_id, self.title),
            };

            Ok(ExecutionResult::success_with_data(
                message,
                serde_json::json!({
                    "id": work_item_id,
                    "url": work_item_url,
                    "project": project,
                    "type": config.work_item_type,
                    "artifact_link": artifact_link_included,
                }),
            ))
        } else {
            let status = response.status();
            let error_body = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());

            Ok(ExecutionResult::failure(format!(
                "Failed to create work item (HTTP {}): {}",
                status, error_body
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
        assert_eq!(CreateWorkItemResult::NAME, "create-work-item");
    }

    #[test]
    fn test_params_deserializes() {
        let json = r#"{"title": "Fix bug", "description": "This is a detailed description of the work item to be created."}"#;
        let params: CreateWorkItemParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.title, "Fix bug");
        assert!(params.description.contains("detailed description"));
    }

    #[test]
    fn test_params_converts_to_result() {
        let params = CreateWorkItemParams {
            title: "Implement feature".to_string(),
            description: "This is a sufficiently long description for the work item.".to_string(),
        };
        let result: CreateWorkItemResult = params.try_into().unwrap();
        assert_eq!(result.name, "create-work-item");
        assert_eq!(result.title, "Implement feature");
        assert!(result.description.contains("sufficiently long"));
    }

    #[test]
    fn test_validation_rejects_short_title() {
        let params = CreateWorkItemParams {
            title: "Hi".to_string(),
            description: "This is a sufficiently long description for the work item.".to_string(),
        };
        let result: Result<CreateWorkItemResult, _> = params.try_into();
        assert!(result.is_err());
    }

    #[test]
    fn test_validation_rejects_short_description() {
        let params = CreateWorkItemParams {
            title: "Valid title here".to_string(),
            description: "Too short".to_string(),
        };
        let result: Result<CreateWorkItemResult, _> = params.try_into();
        assert!(result.is_err());
    }

    #[test]
    fn test_result_serializes_correctly() {
        let params = CreateWorkItemParams {
            title: "Test work item".to_string(),
            description: "A description that is definitely longer than thirty characters."
                .to_string(),
        };
        let result: CreateWorkItemResult = params.try_into().unwrap();
        let json = serde_json::to_string(&result).unwrap();

        assert!(json.contains(r#""name":"create-work-item""#));
        assert!(json.contains(r#""title":"Test work item""#));
    }

    #[test]
    fn test_config_defaults() {
        let config = CreateWorkItemConfig::default();
        assert_eq!(config.work_item_type, "Task");
        assert!(config.area_path.is_none());
        assert!(config.iteration_path.is_none());
        assert!(config.assignee.is_none());
        assert!(config.tags.is_empty());
        assert!(config.custom_fields.is_empty());
    }

    #[test]
    fn test_config_deserializes_from_yaml() {
        let yaml = r#"
work-item-type: Bug
area-path: "MyProject\\MyTeam"
assignee: "user@example.com"
tags:
  - agent-created
  - automated
custom-fields:
  Custom.Priority: "High"
"#;
        let config: CreateWorkItemConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.work_item_type, "Bug");
        assert_eq!(config.area_path, Some("MyProject\\MyTeam".to_string()));
        assert_eq!(config.assignee, Some("user@example.com".to_string()));
        assert_eq!(config.tags, vec!["agent-created", "automated"]);
        assert_eq!(
            config.custom_fields.get("Custom.Priority"),
            Some(&"High".to_string())
        );
    }

    #[test]
    fn test_config_partial_deserialize_uses_defaults() {
        let yaml = r#"
tags:
  - my-tag
"#;
        let config: CreateWorkItemConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.work_item_type, "Task"); // default
        assert!(config.area_path.is_none()); // default
        assert_eq!(config.tags, vec!["my-tag"]);
    }
}
