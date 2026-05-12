//! Missing tool reporting schemas

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::sanitize::{SanitizeConfig, SanitizeContent, sanitize as sanitize_text};
use crate::tool_result;
use crate::safeoutputs::{ExecutionContext, ExecutionResult, Executor, Validate, WorkItemReportConfig, file_or_append_work_item};

/// Parameters for reporting a missing tool
#[derive(Deserialize, JsonSchema)]
pub struct MissingToolParams {
    /// Name of the tool that was expected but not found
    pub tool_name: String,
    /// Optional context about why the tool was needed
    #[serde(default)]
    pub context: Option<String>,
}

impl Validate for MissingToolParams {}

tool_result! {
    name = "missing-tool",
    params = MissingToolParams,
    /// Result of reporting a missing tool
    pub struct MissingToolResult {
        tool_name: String,
        #[serde(default)]
        context: Option<String>,
    }
}

impl SanitizeContent for MissingToolResult {
    fn sanitize_content_fields(&mut self) {
        self.tool_name = sanitize_text(&self.tool_name);
        self.context = self.context.as_deref().map(sanitize_text);
    }
}

fn missing_tool_default_work_item_title() -> String {
    "Agent encountered missing tool".to_string()
}

fn missing_tool_default_work_item() -> WorkItemReportConfig {
    WorkItemReportConfig {
        title: missing_tool_default_work_item_title(),
        work_item_type: "Task".to_string(),
        area_path: None,
        iteration_path: None,
        tags: Vec::new(),
        include_stats: true,
    }
}

/// Configuration for the missing-tool tool (specified in front matter).
///
/// The executor always files a new Azure DevOps work item or appends a comment to an
/// existing one with the same title. Override the defaults to customise the work item.
///
/// If ADO credentials are not available (e.g. the pipeline has no write service
/// connection), the executor succeeds with a warning rather than failing hard.
///
/// Example front matter:
/// ```yaml
/// safe-outputs:
///   missing-tool:
///     work-item:
///       title: "Agent encountered missing tool"
///       work-item-type: Bug
///       area-path: "MyProject\\MyTeam"
///       tags:
///         - agent-missing-tool
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MissingToolConfig {
    /// Work item to file (or append to) when a tool is reported missing.
    /// Defaults to a Task titled "Agent encountered missing tool".
    #[serde(default = "missing_tool_default_work_item", rename = "work-item")]
    pub work_item: WorkItemReportConfig,
}

impl Default for MissingToolConfig {
    fn default() -> Self {
        Self {
            work_item: missing_tool_default_work_item(),
        }
    }
}

impl SanitizeConfig for MissingToolConfig {
    fn sanitize_config_fields(&mut self) {
        self.work_item.sanitize_config_fields();
    }
}

#[async_trait::async_trait]
impl Executor for MissingToolResult {
    fn dry_run_summary(&self) -> String {
        format!("report missing tool '{}'", self.tool_name)
    }

    async fn execute_impl(&self, ctx: &ExecutionContext) -> anyhow::Result<ExecutionResult> {
        let message = match &self.context {
            Some(context) => format!("Missing tool reported: {} ({context})", self.tool_name),
            None => format!("Missing tool reported: {}", self.tool_name),
        };

        let config: MissingToolConfig = ctx.get_tool_config("missing-tool");
        file_or_append_work_item(&config.work_item, &message, ctx).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::safeoutputs::ToolResult;

    #[test]
    fn test_result_has_correct_name() {
        assert_eq!(MissingToolResult::NAME, "missing-tool");
    }

    #[test]
    fn test_result_serializes_correctly() {
        let result: MissingToolResult = MissingToolParams {
            tool_name: "some_tool".to_string(),
            context: None,
        }
        .try_into()
        .unwrap();
        let json = serde_json::to_string(&result).unwrap();

        assert!(json.contains(r#""name":"missing-tool""#));
        assert!(json.contains(r#""tool_name":"some_tool""#));
    }

    #[test]
    fn test_params_deserializes() {
        let json = r#"{"tool_name": "my_tool", "context": "why"}"#;
        let params: MissingToolParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.tool_name, "my_tool");
        assert_eq!(params.context, Some("why".to_string()));
    }

    #[test]
    fn test_params_converts_to_result() {
        let params = MissingToolParams {
            tool_name: "my_tool".to_string(),
            context: Some("context".to_string()),
        };
        let result: MissingToolResult = params.try_into().unwrap();
        assert_eq!(result.name, "missing-tool");
        assert_eq!(result.tool_name, "my_tool");
        assert_eq!(result.context, Some("context".to_string()));
    }

    #[test]
    fn test_params_requires_tool_name() {
        let json = r#"{"context": "why"}"#;
        let result: Result<MissingToolParams, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    #[test]
    fn test_config_default_has_sensible_work_item() {
        let config = MissingToolConfig::default();
        assert_eq!(config.work_item.title, "Agent encountered missing tool");
        assert_eq!(config.work_item.work_item_type, "Task");
        assert!(config.work_item.area_path.is_none());
        assert!(config.work_item.iteration_path.is_none());
        assert!(config.work_item.tags.is_empty());
        assert!(config.work_item.include_stats);
    }

    #[test]
    fn test_config_deserializes_with_work_item_overrides() {
        let yaml = r#"
work-item:
  title: "Custom missing tool title"
  work-item-type: Bug
  area-path: "MyProject\\MyTeam"
  tags:
    - agent-missing-tool
"#;
        let config: MissingToolConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.work_item.title, "Custom missing tool title");
        assert_eq!(config.work_item.work_item_type, "Bug");
        assert_eq!(config.work_item.area_path.as_deref(), Some("MyProject\\MyTeam"));
        assert_eq!(config.work_item.tags, vec!["agent-missing-tool"]);
    }

    #[test]
    fn test_config_deserializes_empty_uses_defaults() {
        let yaml = r#"{}"#;
        let config: MissingToolConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.work_item.title, "Agent encountered missing tool");
        assert_eq!(config.work_item.work_item_type, "Task");
    }

    #[tokio::test]
    async fn test_execute_impl_without_ado_credentials_returns_warning() {
        let result: MissingToolResult = MissingToolParams {
            tool_name: "bash".to_string(),
            context: Some("needed for script execution".to_string()),
        }
        .try_into()
        .unwrap();

        // Default ExecutionContext has no ADO credentials — should warn, not fail
        let exec = result
            .execute_impl(&crate::safeoutputs::ExecutionContext::default())
            .await
            .unwrap();
        assert!(exec.success);
        assert!(exec.is_warning());
    }
}
