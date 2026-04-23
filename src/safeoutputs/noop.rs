use schemars::JsonSchema;
use serde::Deserialize;

use crate::tool_result;
use crate::safeoutputs::{ExecutionContext, ExecutionResult, Executor, Validate};

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

#[async_trait::async_trait]
impl Executor for NoopResult {
    fn dry_run_summary(&self) -> String {
        "noop".to_string()
    }

    async fn execute_impl(&self, _ctx: &ExecutionContext) -> anyhow::Result<ExecutionResult> {
        let message = match &self.context {
            Some(context) => format!("No operation needed: {context}"),
            None => "No operation needed".to_string(),
        };
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
}
