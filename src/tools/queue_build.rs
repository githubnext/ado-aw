//! Queue build safe output tool

use log::{debug, info};
use percent_encoding::utf8_percent_encode;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::PATH_SEGMENT;
use crate::sanitize::{Sanitize, sanitize as sanitize_text};
use crate::tool_result;
use crate::tools::{ExecutionContext, ExecutionResult, Executor, Validate};
use anyhow::{Context, ensure};

/// Parameters for queuing a build
#[derive(Deserialize, JsonSchema)]
pub struct QueueBuildParams {
    /// Pipeline definition ID to trigger (must be positive)
    pub pipeline_id: i32,

    /// Branch to build (optional, defaults to configured default or "main")
    pub branch: Option<String>,

    /// Template parameter key-value pairs (optional)
    pub parameters: Option<std::collections::HashMap<String, String>>,

    /// Human-readable reason for triggering the build; at least 5 characters if provided
    pub reason: Option<String>,
}

impl Validate for QueueBuildParams {
    fn validate(&self) -> anyhow::Result<()> {
        ensure!(self.pipeline_id > 0, "pipeline_id must be positive");
        if let Some(reason) = &self.reason {
            ensure!(
                reason.len() >= 5,
                "reason must be at least 5 characters"
            );
        }
        if let Some(branch) = &self.branch {
            ensure!(
                !branch.contains(".."),
                "branch name must not contain '..'"
            );
            ensure!(
                !branch.contains('\0'),
                "branch name must not contain null bytes"
            );
        }
        Ok(())
    }
}

tool_result! {
    name = "queue-build",
    params = QueueBuildParams,
    /// Result of queuing a build
    pub struct QueueBuildResult {
        pipeline_id: i32,
        branch: Option<String>,
        parameters: Option<std::collections::HashMap<String, String>>,
        reason: Option<String>,
    }
}

impl Sanitize for QueueBuildResult {
    fn sanitize_fields(&mut self) {
        if let Some(reason) = &self.reason {
            self.reason = Some(sanitize_text(reason));
        }
        if let Some(params) = &self.parameters {
            self.parameters = Some(
                params
                    .iter()
                    .map(|(k, v)| (sanitize_text(k), sanitize_text(v)))
                    .collect(),
            );
        }
    }
}

/// Configuration for the queue-build tool (specified in front matter)
///
/// Example front matter:
/// ```yaml
/// safe-outputs:
///   queue-build:
///     allowed-pipelines:
///       - 123
///       - 456
///     allowed-branches:
///       - main
///       - release/*
///     allowed-parameters:
///       - environment
///       - version
///     default-branch: main
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueueBuildConfig {
    /// Pipeline definition IDs that are allowed to be triggered (REQUIRED — empty rejects all)
    #[serde(default, rename = "allowed-pipelines")]
    pub allowed_pipelines: Vec<i32>,

    /// Branches that are allowed to be built (if empty, any branch is allowed)
    #[serde(default, rename = "allowed-branches")]
    pub allowed_branches: Vec<String>,

    /// Parameter keys that are allowed to be passed (if empty, any parameters are allowed)
    #[serde(default, rename = "allowed-parameters")]
    pub allowed_parameters: Vec<String>,

    /// Default branch to use when the agent does not specify one
    #[serde(default, rename = "default-branch")]
    pub default_branch: Option<String>,
}

impl Default for QueueBuildConfig {
    fn default() -> Self {
        Self {
            allowed_pipelines: Vec::new(),
            allowed_branches: Vec::new(),
            allowed_parameters: Vec::new(),
            default_branch: None,
        }
    }
}

#[async_trait::async_trait]
impl Executor for QueueBuildResult {
    async fn execute_impl(&self, ctx: &ExecutionContext) -> anyhow::Result<ExecutionResult> {
        info!("Queuing build for pipeline definition {}", self.pipeline_id);

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
        let config: QueueBuildConfig = ctx.get_tool_config("queue-build");
        debug!("Allowed pipelines: {:?}", config.allowed_pipelines);
        debug!("Allowed branches: {:?}", config.allowed_branches);
        debug!("Allowed parameters: {:?}", config.allowed_parameters);

        // Validate pipeline_id against allowed-pipelines (REQUIRED)
        if config.allowed_pipelines.is_empty() {
            return Ok(ExecutionResult::failure(
                "queue-build allowed-pipelines is not configured. \
                 This is required to scope which pipelines the agent can trigger."
                    .to_string(),
            ));
        }
        if !config.allowed_pipelines.contains(&self.pipeline_id) {
            return Ok(ExecutionResult::failure(format!(
                "Pipeline definition {} is not in the allowed-pipelines list",
                self.pipeline_id
            )));
        }

        // Resolve the effective branch
        let effective_branch = self
            .branch
            .as_deref()
            .or(config.default_branch.as_deref())
            .unwrap_or("main");
        debug!("Effective branch: {}", effective_branch);

        // Validate branch against allowed-branches (if configured)
        if !config.allowed_branches.is_empty() {
            let branch_allowed = config.allowed_branches.iter().any(|pattern| {
                if pattern.ends_with("/*") {
                    let prefix = &pattern[..pattern.len() - 2];
                    effective_branch.starts_with(prefix)
                        && (effective_branch.len() == prefix.len()
                            || effective_branch[prefix.len()..].starts_with('/'))
                } else {
                    pattern == effective_branch
                }
            });
            if !branch_allowed {
                return Ok(ExecutionResult::failure(format!(
                    "Branch '{}' is not in the allowed-branches list",
                    effective_branch
                )));
            }
        }

        // Validate parameter keys against allowed-parameters (if configured)
        if let Some(params) = &self.parameters {
            if !config.allowed_parameters.is_empty() {
                for key in params.keys() {
                    if !config.allowed_parameters.contains(key) {
                        return Ok(ExecutionResult::failure(format!(
                            "Parameter '{}' is not in the allowed-parameters list",
                            key
                        )));
                    }
                }
            }
        }

        // Build the source branch ref
        let source_branch = if effective_branch.starts_with("refs/") {
            effective_branch.to_string()
        } else {
            format!("refs/heads/{}", effective_branch)
        };
        debug!("Source branch ref: {}", source_branch);

        // Build the request body
        let mut body = serde_json::json!({
            "definition": { "id": self.pipeline_id },
            "sourceBranch": source_branch,
            "reason": "userCreated",
        });

        // Add template parameters as a JSON string if provided
        if let Some(params) = &self.parameters {
            if !params.is_empty() {
                let params_json = serde_json::to_string(params)
                    .context("Failed to serialize template parameters")?;
                body["parameters"] = serde_json::Value::String(params_json);
            }
        }

        // Build the API URL
        let url = format!(
            "{}/{}/_apis/build/builds?api-version=7.1",
            org_url.trim_end_matches('/'),
            utf8_percent_encode(project, PATH_SEGMENT),
        );
        debug!("API URL: {}", url);

        // Make the API call
        let client = reqwest::Client::new();
        info!(
            "Sending queue build request for pipeline {} on branch {}",
            self.pipeline_id, source_branch
        );
        let response = client
            .post(&url)
            .header("Content-Type", "application/json")
            .basic_auth("", Some(token))
            .json(&body)
            .send()
            .await
            .context("Failed to send request to Azure DevOps")?;

        if response.status().is_success() {
            let resp_body: serde_json::Value = response
                .json()
                .await
                .context("Failed to parse response JSON")?;

            let build_id = resp_body.get("id").and_then(|v| v.as_i64()).unwrap_or(0);
            let build_url = resp_body
                .get("_links")
                .and_then(|l| l.get("web"))
                .and_then(|h| h.get("href"))
                .and_then(|h| h.as_str())
                .unwrap_or("");

            info!("Build queued: #{} - {}", build_id, build_url);

            Ok(ExecutionResult::success_with_data(
                format!(
                    "Queued build #{} for pipeline {} on branch {}",
                    build_id, self.pipeline_id, effective_branch
                ),
                serde_json::json!({
                    "build_id": build_id,
                    "pipeline_id": self.pipeline_id,
                    "branch": effective_branch,
                    "url": build_url,
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
                "Failed to queue build for pipeline {} (HTTP {}): {}",
                self.pipeline_id, status, error_body
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
        assert_eq!(QueueBuildResult::NAME, "queue-build");
    }

    #[test]
    fn test_params_deserializes() {
        let json = r#"{"pipeline_id": 123, "branch": "main", "reason": "Nightly rebuild"}"#;
        let params: QueueBuildParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.pipeline_id, 123);
        assert_eq!(params.branch, Some("main".to_string()));
        assert_eq!(params.reason, Some("Nightly rebuild".to_string()));
        assert!(params.parameters.is_none());
    }

    #[test]
    fn test_params_converts_to_result() {
        let params = QueueBuildParams {
            pipeline_id: 42,
            branch: Some("develop".to_string()),
            parameters: None,
            reason: Some("Trigger nightly build".to_string()),
        };
        let result: QueueBuildResult = params.try_into().unwrap();
        assert_eq!(result.name, "queue-build");
        assert_eq!(result.pipeline_id, 42);
        assert_eq!(result.branch, Some("develop".to_string()));
        assert_eq!(result.reason, Some("Trigger nightly build".to_string()));
    }

    #[test]
    fn test_validation_rejects_zero_pipeline_id() {
        let params = QueueBuildParams {
            pipeline_id: 0,
            branch: None,
            parameters: None,
            reason: None,
        };
        let result: Result<QueueBuildResult, _> = params.try_into();
        assert!(result.is_err());
    }

    #[test]
    fn test_validation_rejects_negative_pipeline_id() {
        let params = QueueBuildParams {
            pipeline_id: -1,
            branch: None,
            parameters: None,
            reason: None,
        };
        let result: Result<QueueBuildResult, _> = params.try_into();
        assert!(result.is_err());
    }

    #[test]
    fn test_validation_rejects_short_reason() {
        let params = QueueBuildParams {
            pipeline_id: 1,
            branch: None,
            parameters: None,
            reason: Some("Hi".to_string()),
        };
        let result: Result<QueueBuildResult, _> = params.try_into();
        assert!(result.is_err());
    }

    #[test]
    fn test_validation_rejects_branch_with_dotdot() {
        let params = QueueBuildParams {
            pipeline_id: 1,
            branch: Some("../../etc/passwd".to_string()),
            parameters: None,
            reason: None,
        };
        let result: Result<QueueBuildResult, _> = params.try_into();
        assert!(result.is_err());
    }

    #[test]
    fn test_validation_rejects_branch_with_null_byte() {
        let params = QueueBuildParams {
            pipeline_id: 1,
            branch: Some("main\0evil".to_string()),
            parameters: None,
            reason: None,
        };
        let result: Result<QueueBuildResult, _> = params.try_into();
        assert!(result.is_err());
    }

    #[test]
    fn test_validation_accepts_valid_params() {
        let params = QueueBuildParams {
            pipeline_id: 123,
            branch: Some("main".to_string()),
            parameters: None,
            reason: Some("Scheduled nightly build".to_string()),
        };
        let result: Result<QueueBuildResult, _> = params.try_into();
        assert!(result.is_ok());
    }

    #[test]
    fn test_validation_accepts_minimal_params() {
        let params = QueueBuildParams {
            pipeline_id: 1,
            branch: None,
            parameters: None,
            reason: None,
        };
        let result: Result<QueueBuildResult, _> = params.try_into();
        assert!(result.is_ok());
    }

    #[test]
    fn test_result_serializes_correctly() {
        let params = QueueBuildParams {
            pipeline_id: 99,
            branch: Some("release/v1".to_string()),
            parameters: None,
            reason: Some("Release build trigger".to_string()),
        };
        let result: QueueBuildResult = params.try_into().unwrap();
        let json = serde_json::to_string(&result).unwrap();

        assert!(json.contains(r#""name":"queue-build""#));
        assert!(json.contains(r#""pipeline_id":99"#));
    }

    #[test]
    fn test_config_defaults() {
        let config = QueueBuildConfig::default();
        assert!(config.allowed_pipelines.is_empty());
        assert!(config.allowed_branches.is_empty());
        assert!(config.allowed_parameters.is_empty());
        assert!(config.default_branch.is_none());
    }

    #[test]
    fn test_config_deserializes_from_yaml() {
        let yaml = r#"
allowed-pipelines:
  - 123
  - 456
allowed-branches:
  - main
  - release/*
allowed-parameters:
  - environment
  - version
default-branch: main
"#;
        let config: QueueBuildConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.allowed_pipelines, vec![123, 456]);
        assert_eq!(config.allowed_branches, vec!["main", "release/*"]);
        assert_eq!(config.allowed_parameters, vec!["environment", "version"]);
        assert_eq!(config.default_branch, Some("main".to_string()));
    }

    #[test]
    fn test_config_partial_deserialize_uses_defaults() {
        let yaml = r#"
allowed-pipelines:
  - 42
"#;
        let config: QueueBuildConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.allowed_pipelines, vec![42]);
        assert!(config.allowed_branches.is_empty());
        assert!(config.allowed_parameters.is_empty());
        assert!(config.default_branch.is_none());
    }

    #[test]
    fn test_params_deserializes_with_parameters() {
        let json = r#"{"pipeline_id": 10, "parameters": {"env": "prod", "version": "2.0"}}"#;
        let params: QueueBuildParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.pipeline_id, 10);
        let map = params.parameters.unwrap();
        assert_eq!(map.get("env"), Some(&"prod".to_string()));
        assert_eq!(map.get("version"), Some(&"2.0".to_string()));
    }
}
