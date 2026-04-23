//! Report incomplete task safe output tool

use schemars::JsonSchema;
use serde::Deserialize;

use crate::sanitize::{SanitizeContent, sanitize as sanitize_text};
use crate::tool_result;
use crate::safeoutputs::{ExecutionContext, ExecutionResult, Executor, Validate};
use anyhow::ensure;

/// Parameters for reporting that a task could not be completed
#[derive(Deserialize, JsonSchema)]
pub struct ReportIncompleteParams {
    /// Why the task could not be completed
    pub reason: String,

    /// Additional context about what was attempted
    #[serde(default)]
    pub context: Option<String>,
}

impl Validate for ReportIncompleteParams {
    fn validate(&self) -> anyhow::Result<()> {
        ensure!(
            self.reason.len() >= 10,
            "reason must be at least 10 characters"
        );
        Ok(())
    }
}

tool_result! {
    name = "report-incomplete",
    params = ReportIncompleteParams,
    /// Result of reporting an incomplete task
    pub struct ReportIncompleteResult {
        reason: String,
        #[serde(default)]
        context: Option<String>,
    }
}

impl SanitizeContent for ReportIncompleteResult {
    fn sanitize_content_fields(&mut self) {
        self.reason = sanitize_text(&self.reason);
        if let Some(ref ctx) = self.context {
            self.context = Some(sanitize_text(ctx));
        }
    }
}

#[async_trait::async_trait]
impl Executor for ReportIncompleteResult {
    fn dry_run_summary(&self) -> String {
        "report task incomplete".to_string()
    }

    // Intentionally bypass default dry-run behavior: this tool signals that the
    // task itself failed, so Stage 3 should preserve failure semantics in dry-run.
    async fn execute_sanitized(
        &mut self,
        ctx: &ExecutionContext,
    ) -> anyhow::Result<ExecutionResult> {
        self.sanitize_content_fields();
        self.execute_impl(ctx).await
    }

    async fn execute_impl(&self, _: &ExecutionContext) -> anyhow::Result<ExecutionResult> {
        Ok(ExecutionResult::failure(format!(
            "Agent reported task incomplete: {}",
            self.reason
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::safeoutputs::ToolResult;

    #[test]
    fn test_result_has_correct_name() {
        assert_eq!(ReportIncompleteResult::NAME, "report-incomplete");
    }

    #[test]
    fn test_params_deserializes() {
        let json = r#"{"reason": "API timed out after 30s", "context": "tried 3 retries"}"#;
        let params: ReportIncompleteParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.reason, "API timed out after 30s");
        assert_eq!(params.context, Some("tried 3 retries".to_string()));
    }

    #[test]
    fn test_params_converts_to_result() {
        let params = ReportIncompleteParams {
            reason: "Build failed with exit code 1".to_string(),
            context: Some("ran cargo build".to_string()),
        };
        let result: ReportIncompleteResult = params.try_into().unwrap();
        assert_eq!(result.name, "report-incomplete");
        assert_eq!(result.reason, "Build failed with exit code 1");
        assert_eq!(result.context, Some("ran cargo build".to_string()));
    }

    #[test]
    fn test_validation_rejects_short_reason() {
        let params = ReportIncompleteParams {
            reason: "short".to_string(),
            context: None,
        };
        let result: Result<ReportIncompleteResult, _> = params.try_into();
        assert!(result.is_err());
    }

    #[test]
    fn test_result_serializes_correctly() {
        let result: ReportIncompleteResult = ReportIncompleteParams {
            reason: "API timed out after 30s".to_string(),
            context: None,
        }
        .try_into()
        .unwrap();
        let json = serde_json::to_string(&result).unwrap();

        assert!(json.contains(r#""name":"report-incomplete""#));
        assert!(json.contains(r#""reason":"API timed out after 30s""#));
    }
}
