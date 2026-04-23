//! Missing data reporting schemas

use schemars::JsonSchema;
use serde::Deserialize;

use crate::sanitize::{SanitizeContent, sanitize as sanitize_text};
use crate::tool_result;
use crate::safeoutputs::{ExecutionContext, ExecutionResult, Executor, Validate};

/// Parameters for reporting missing data
#[derive(Deserialize, JsonSchema)]
pub struct MissingDataParams {
    /// Type of data needed (e.g., 'API documentation', 'database schema')
    pub data_type: String,

    /// Why this data is required
    pub reason: String,

    /// Additional optional context about the missing information
    #[serde(default)]
    pub context: Option<String>,
}

impl Validate for MissingDataParams {}

tool_result! {
    name = "missing-data",
    params = MissingDataParams,
    /// Result of reporting missing data
    pub struct MissingDataResult {
        data_type: String,
        reason: String,
        #[serde(default)]
        context: Option<String>,
    }
}

impl SanitizeContent for MissingDataResult {
    fn sanitize_content_fields(&mut self) {
        self.data_type = sanitize_text(&self.data_type);
        self.reason = sanitize_text(&self.reason);
        self.context = self.context.as_deref().map(sanitize_text);
    }
}

#[async_trait::async_trait]
impl Executor for MissingDataResult {
    fn dry_run_summary(&self) -> String {
        format!("report missing data '{}'", self.data_type)
    }

    async fn execute_impl(&self, _ctx: &ExecutionContext) -> anyhow::Result<ExecutionResult> {
        let mut message = format!(
            "Missing data reported: {} ({})",
            self.data_type, self.reason
        );
        if let Some(context) = &self.context {
            message.push_str(&format!(" [{context}]"));
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
        assert_eq!(MissingDataResult::NAME, "missing-data");
    }

    #[test]
    fn test_params_converts_to_result() {
        let params = MissingDataParams {
            data_type: "API docs".to_string(),
            reason: "needed for integration".to_string(),
            context: None,
        };
        let result: MissingDataResult = params.try_into().unwrap();
        assert_eq!(result.name, "missing-data");
        assert_eq!(result.data_type, "API docs");
        assert_eq!(result.reason, "needed for integration");
        assert_eq!(result.context, None);
    }
}
