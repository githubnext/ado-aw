//! Missing tool reporting schemas

use schemars::JsonSchema;
use serde::Deserialize;

use crate::safe_outputs::{ExecutionContext, ExecutionResult, Executor, Validate};
use crate::sanitize::{SanitizeContent, sanitize as sanitize_text};
use crate::tool_result;

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

#[async_trait::async_trait]
impl Executor for MissingToolResult {
    fn dry_run_summary(&self) -> String {
        format!("report missing tool '{}'", self.tool_name)
    }

    async fn execute_impl(&self, _ctx: &ExecutionContext) -> anyhow::Result<ExecutionResult> {
        let mut message = format!("Missing tool reported: {}", self.tool_name);
        if let Some(context) = &self.context {
            message.push_str(&format!(" [{context}]"));
        }
        Ok(ExecutionResult::success(message))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let result = serde_json::from_str::<MissingToolParams>(json);
        let err = result.map(|_| ()).unwrap_err().to_string();
        assert!(
            err.contains("tool_name"),
            "expected error about missing tool_name field, got: {err}"
        );
    }

    #[tokio::test]
    async fn test_execute_impl_returns_success_message() {
        let result: MissingToolResult = MissingToolParams {
            tool_name: "bash".to_string(),
            context: Some("needed for script execution".to_string()),
        }
        .try_into()
        .unwrap();

        let exec = result
            .execute_impl(&crate::safe_outputs::ExecutionContext::default())
            .await
            .unwrap();
        assert!(exec.success);
        assert!(!exec.is_warning());
        assert_eq!(
            exec.message,
            "Missing tool reported: bash [needed for script execution]"
        );
    }
}
