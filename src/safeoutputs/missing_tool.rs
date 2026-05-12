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

/// Configuration for the missing-tool tool (specified in front matter).
///
/// When `work-item` is configured, the executor will file a new Azure DevOps work item
/// or append a comment to an existing one with the same title.
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
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MissingToolConfig {
    /// Optional work item to file (or append to) when a tool is reported missing.
    /// If absent, the executor only logs a message.
    #[serde(default, rename = "work-item")]
    pub work_item: Option<WorkItemReportConfig>,
}

impl SanitizeConfig for MissingToolConfig {
    fn sanitize_config_fields(&mut self) {
        if let Some(wi) = &mut self.work_item {
            wi.sanitize_config_fields();
        }
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

        if let Some(wi_config) = &config.work_item {
            return file_or_append_work_item(wi_config, &message, ctx).await;
        }

        Ok(ExecutionResult::success(message))
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
    fn test_config_defaults_to_no_work_item() {
        let config = MissingToolConfig::default();
        assert!(config.work_item.is_none());
    }

    #[test]
    fn test_config_deserializes_with_work_item() {
        let yaml = r#"
work-item:
  title: "Agent encountered missing tool"
  work-item-type: Bug
  area-path: "MyProject\\MyTeam"
  tags:
    - agent-missing-tool
"#;
        let config: MissingToolConfig = serde_yaml::from_str(yaml).unwrap();
        let wi = config.work_item.unwrap();
        assert_eq!(wi.title, "Agent encountered missing tool");
        assert_eq!(wi.work_item_type, "Bug");
        assert_eq!(wi.area_path.as_deref(), Some("MyProject\\MyTeam"));
        assert_eq!(wi.tags, vec!["agent-missing-tool"]);
    }

    #[test]
    fn test_config_deserializes_without_work_item() {
        let yaml = r#"{}"#;
        let config: MissingToolConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(config.work_item.is_none());
    }

    #[test]
    fn test_work_item_config_default_type() {
        let yaml = r#"
work-item:
  title: "Missing tool"
"#;
        let config: MissingToolConfig = serde_yaml::from_str(yaml).unwrap();
        let wi = config.work_item.unwrap();
        assert_eq!(wi.work_item_type, "Task");
        assert!(wi.area_path.is_none());
        assert!(wi.iteration_path.is_none());
        assert!(wi.tags.is_empty());
        assert!(wi.include_stats);
    }

    #[tokio::test]
    async fn test_execute_impl_without_work_item_config() {
        let result: MissingToolResult = MissingToolParams {
            tool_name: "bash".to_string(),
            context: Some("needed for script execution".to_string()),
        }
        .try_into()
        .unwrap();

        let exec = result
            .execute_impl(&crate::safeoutputs::ExecutionContext::default())
            .await
            .unwrap();
        assert!(exec.success);
        assert!(exec.message.contains("Missing tool reported"));
        assert!(exec.message.contains("bash"));
    }
}
