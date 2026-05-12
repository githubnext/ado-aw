use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::sanitize::{SanitizeConfig, SanitizeContent, sanitize as sanitize_text};
use crate::tool_result;
use crate::safeoutputs::{ExecutionContext, ExecutionResult, Executor, Validate, WorkItemReportConfig, file_or_append_work_item};

/// Parameters for describing a no operation. Use this if there is no work to do.
#[derive(Deserialize, JsonSchema)]
pub struct NoopParams {
    /// Optional context about why a no op was reached
    #[serde(default)]
    pub context: Option<String>,
}

impl Validate for NoopParams {}

tool_result! {
    name = "noop",
    params = NoopParams,
    /// Result of a no-op operation
    pub struct NoopResult {
        #[serde(default)]
        context: Option<String>,
    }
}

impl SanitizeContent for NoopResult {
    fn sanitize_content_fields(&mut self) {
        self.context = self.context.as_deref().map(sanitize_text);
    }
}

/// Configuration for the noop tool (specified in front matter).
///
/// When `work-item` is configured, the executor will file a new Azure DevOps work item
/// or append a comment to an existing one with the same title.
///
/// Example front matter:
/// ```yaml
/// safe-outputs:
///   noop:
///     work-item:
///       title: "Agent reported no operation"
///       work-item-type: Task
///       area-path: "MyProject\\MyTeam"
///       iteration-path: "MyProject\\Sprint 1"
///       tags:
///         - agent-noop
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NoopConfig {
    /// Optional work item to file (or append to) when a noop is reached.
    /// If absent, the executor only logs a message.
    #[serde(default, rename = "work-item")]
    pub work_item: Option<WorkItemReportConfig>,
}

impl SanitizeConfig for NoopConfig {
    fn sanitize_config_fields(&mut self) {
        if let Some(wi) = &mut self.work_item {
            wi.sanitize_config_fields();
        }
    }
}

#[async_trait::async_trait]
impl Executor for NoopResult {
    fn dry_run_summary(&self) -> String {
        "noop".to_string()
    }

    async fn execute_impl(&self, ctx: &ExecutionContext) -> anyhow::Result<ExecutionResult> {
        let message = match &self.context {
            Some(context) => format!("No operation needed: {context}"),
            None => "No operation needed".to_string(),
        };

        let config: NoopConfig = ctx.get_tool_config("noop");

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
        assert_eq!(NoopResult::NAME, "noop");
    }

    #[test]
    fn test_result_serializes_correctly() {
        let result: NoopResult = NoopParams {
            context: Some("test context".to_string()),
        }
        .try_into()
        .unwrap();
        let json = serde_json::to_string(&result).unwrap();

        assert!(json.contains(r#""name":"noop""#));
        assert!(json.contains(r#""context":"test context""#));
    }

    #[test]
    fn test_result_serializes_to_valid_json() {
        let result: NoopResult = NoopParams {
            context: Some("test".to_string()),
        }
        .try_into()
        .unwrap();
        let json_str = serde_json::to_string(&result).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();

        assert_eq!(parsed["name"], "noop");
        assert_eq!(parsed["context"], "test");
    }

    #[test]
    fn test_params_deserializes() {
        let json = r#"{"context": "test context"}"#;
        let params: NoopParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.context, Some("test context".to_string()));
    }

    #[test]
    fn test_params_deserializes_without_context() {
        let json = r#"{}"#;
        let params: NoopParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.context, None);
    }

    #[test]
    fn test_params_converts_to_result() {
        let params = NoopParams {
            context: Some("test context".to_string()),
        };
        let result: NoopResult = params.try_into().unwrap();
        assert_eq!(result.name, "noop");
        assert_eq!(result.context, Some("test context".to_string()));
    }

    #[test]
    fn test_validate_default_succeeds() {
        let params = NoopParams { context: None };
        assert!(params.validate().is_ok());
    }

    #[test]
    fn test_config_defaults_to_no_work_item() {
        let config = NoopConfig::default();
        assert!(config.work_item.is_none());
    }

    #[test]
    fn test_config_deserializes_with_work_item() {
        let yaml = r#"
work-item:
  title: "Agent reported no operation"
  work-item-type: Task
  area-path: "MyProject\\MyTeam"
  tags:
    - agent-noop
"#;
        let config: NoopConfig = serde_yaml::from_str(yaml).unwrap();
        let wi = config.work_item.unwrap();
        assert_eq!(wi.title, "Agent reported no operation");
        assert_eq!(wi.work_item_type, "Task");
        assert_eq!(wi.area_path.as_deref(), Some("MyProject\\MyTeam"));
        assert_eq!(wi.tags, vec!["agent-noop"]);
    }

    #[test]
    fn test_config_deserializes_without_work_item() {
        let yaml = r#"{}"#;
        let config: NoopConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(config.work_item.is_none());
    }

    #[test]
    fn test_work_item_config_default_type() {
        let yaml = r#"
work-item:
  title: "No op"
"#;
        let config: NoopConfig = serde_yaml::from_str(yaml).unwrap();
        let wi = config.work_item.unwrap();
        assert_eq!(wi.work_item_type, "Task");
        assert!(wi.area_path.is_none());
        assert!(wi.iteration_path.is_none());
        assert!(wi.tags.is_empty());
        assert!(wi.include_stats);
    }

    #[tokio::test]
    async fn test_execute_impl_without_work_item_config() {
        let result: NoopResult = NoopParams {
            context: Some("nothing to do".to_string()),
        }
        .try_into()
        .unwrap();

        let exec = result
            .execute_impl(&crate::safeoutputs::ExecutionContext::default())
            .await
            .unwrap();
        assert!(exec.success);
        assert!(exec.message.contains("No operation needed"));
        assert!(exec.message.contains("nothing to do"));
    }
}
