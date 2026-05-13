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

fn noop_default_work_item_title() -> String {
    "[ado-aw] Agent reported no operation".to_string()
}

fn noop_default_work_item() -> WorkItemReportConfig {
    WorkItemReportConfig {
        enabled: true,
        title: Some(noop_default_work_item_title()),
        work_item_type: "Task".to_string(),
        area_path: None,
        iteration_path: None,
        tags: Vec::new(),
        include_stats: true,
    }
}

/// Configuration for the noop tool (specified in front matter).
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
///   noop:
///     work-item:
///       title: "[ado-aw] Agent reported no operation"
///       work-item-type: Task
///       area-path: "MyProject\\MyTeam"
///       iteration-path: "MyProject\\Sprint 1"
///       tags:
///         - agent-noop
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NoopConfig {
    /// Work item to file (or append to) when a noop is reached.
    /// Defaults to a Task titled "[ado-aw] Agent reported no operation".
    #[serde(default = "noop_default_work_item", rename = "work-item")]
    pub work_item: WorkItemReportConfig,
}

impl Default for NoopConfig {
    fn default() -> Self {
        Self {
            work_item: noop_default_work_item(),
        }
    }
}

impl SanitizeConfig for NoopConfig {
    fn sanitize_config_fields(&mut self) {
        self.work_item.sanitize_config_fields();
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
        file_or_append_work_item(&config.work_item, &noop_default_work_item_title(), &message, ctx).await
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
    fn test_config_default_has_sensible_work_item() {
        let config = NoopConfig::default();
        assert!(config.work_item.enabled);
        assert_eq!(config.work_item.title.as_deref(), Some("[ado-aw] Agent reported no operation"));
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
  title: "My custom noop title"
  work-item-type: Bug
  area-path: "MyProject\\MyTeam"
  tags:
    - agent-noop
"#;
        let config: NoopConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.work_item.title.as_deref(), Some("My custom noop title"));
        assert_eq!(config.work_item.work_item_type, "Bug");
        assert_eq!(config.work_item.area_path.as_deref(), Some("MyProject\\MyTeam"));
        assert_eq!(config.work_item.tags, vec!["agent-noop"]);
    }

    #[test]
    fn test_config_deserializes_empty_uses_defaults() {
        let yaml = r#"{}"#;
        let config: NoopConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.work_item.title.as_deref(), Some("[ado-aw] Agent reported no operation"));
        assert_eq!(config.work_item.work_item_type, "Task");
    }

    #[test]
    fn test_config_partial_work_item_preserves_overrides() {
        // When a partial work-item: block is provided in front matter (e.g.
        // only `work-item-type:` with no `title:`), serde deserializes
        // `title` as `None` via `#[serde(default)]` — NOT via the per-tool
        // default function `noop_default_work_item()`.  The caller's
        // `unwrap_or(default_title)` in `file_or_append_work_item` recovers
        // the intended title at execution time.
        let yaml = r#"
work-item:
  work-item-type: Bug
  area-path: "MyProject\\MyTeam"
"#;
        let config: NoopConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(config.work_item.title.is_none(), "title should be None when omitted");
        assert_eq!(config.work_item.work_item_type, "Bug");
        assert_eq!(config.work_item.area_path.as_deref(), Some("MyProject\\MyTeam"));
    }

    #[tokio::test]
    async fn test_execute_impl_disabled_skips_work_item() {
        let result: NoopResult = NoopParams {
            context: Some("nothing to do".to_string()),
        }
        .try_into()
        .unwrap();

        let mut ctx = crate::safeoutputs::ExecutionContext::default();
        // Configure noop with enabled: false
        ctx.tool_configs.insert(
            "noop".to_string(),
            serde_json::to_value(NoopConfig {
                work_item: WorkItemReportConfig {
                    enabled: false,
                    ..noop_default_work_item()
                },
            })
            .unwrap(),
        );

        let exec = result.execute_impl(&ctx).await.unwrap();
        assert!(exec.success);
        assert!(!exec.is_warning());
        assert!(
            exec.message.contains("disabled"),
            "expected disabled message, got: {}",
            exec.message
        );
    }

    #[tokio::test]
    async fn test_execute_impl_without_ado_credentials_returns_warning() {
        let result: NoopResult = NoopParams {
            context: Some("nothing to do".to_string()),
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
