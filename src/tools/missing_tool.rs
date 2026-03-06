//! Missing tool reporting schemas

use schemars::JsonSchema;
use serde::Deserialize;

use crate::tool_result;
use crate::tools::Validate;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::ToolResult;

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
}
