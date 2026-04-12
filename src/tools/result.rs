//! Tool result infrastructure: traits, macros, and error conversion

use rmcp::ErrorData as McpError;
use rmcp::model::ErrorCode;
use serde::Serialize;
use std::collections::HashMap;

use crate::sanitize::Sanitize;

/// Trait for tool results that include a name field
pub trait ToolResult: Serialize {
    /// The constant name identifier for this tool
    const NAME: &'static str;

    /// Default maximum number of outputs allowed per pipeline run.
    /// Each tool can override this; the operator can further override via `max` in front matter.
    const DEFAULT_MAX: u32 = 1;

    /// Whether this tool requires write access to ADO.
    /// Write-requiring tools need a `permissions.write` service connection.
    /// Diagnostic/read-only tools default to `false`.
    const REQUIRES_WRITE: bool = false;
}

/// Trait for validating tool parameters before conversion to results.
/// Implement this on your Params struct to add custom validation logic.
/// Uses anyhow::Result so you can use anyhow!, bail!, ensure!, etc.
pub trait Validate {
    /// Validates the parameters, returning an error if invalid.
    fn validate(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

/// Context provided to executors during Stage 2 execution
#[derive(Debug, Clone)]
pub struct ExecutionContext {
    /// Azure DevOps organization URL (e.g., "https://dev.azure.com/myorg")
    pub ado_org_url: Option<String>,
    /// Azure DevOps organization name (extracted from ado_org_url, e.g., "myorg")
    pub ado_organization: Option<String>,
    /// Azure DevOps project name
    pub ado_project: Option<String>,
    /// Personal access token or system access token
    pub access_token: Option<String>,
    /// Working directory for file operations (safe outputs directory)
    pub working_directory: std::path::PathBuf,
    /// Source checkout directory (BUILD_SOURCESDIRECTORY) where git repos are checked out
    pub source_directory: std::path::PathBuf,
    /// Per-tool configuration, keyed by tool name
    pub tool_configs: HashMap<String, serde_json::Value>,
    /// Repository ID (from BUILD_REPOSITORY_ID)
    pub repository_id: Option<String>,
    /// Repository name (from BUILD_REPOSITORY_NAME)
    pub repository_name: Option<String>,
    /// Allowed repositories for PRs: "self" + checkout list aliases
    /// Maps alias to ADO repo name (e.g., "other-repo" -> "org/other-repo")
    pub allowed_repositories: HashMap<String, String>,
}

impl ExecutionContext {
    /// Get typed configuration for a specific tool
    pub fn get_tool_config<T: serde::de::DeserializeOwned + Default>(&self, tool_name: &str) -> T {
        self.tool_configs
            .get(tool_name)
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default()
    }
}

impl Default for ExecutionContext {
    fn default() -> Self {
        // Try AZURE_DEVOPS_ORG_URL first, then fall back to Azure DevOps built-in var
        let ado_org_url = std::env::var("AZURE_DEVOPS_ORG_URL")
            .ok()
            .or_else(|| std::env::var("SYSTEM_TEAMFOUNDATIONCOLLECTIONURI").ok());

        // Extract organization name from URL (e.g., "https://dev.azure.com/myorg/" -> "myorg")
        let ado_organization = ado_org_url.as_ref().and_then(|url| {
            url.trim_end_matches('/')
                .rsplit('/')
                .next()
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
        });

        // Source directory is where git repos are checked out (BUILD_SOURCESDIRECTORY)
        let source_directory = std::env::var("BUILD_SOURCESDIRECTORY")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());

        Self {
            ado_org_url,
            ado_organization,
            ado_project: std::env::var("SYSTEM_TEAMPROJECT").ok(),
            access_token: std::env::var("SYSTEM_ACCESSTOKEN")
                .ok()
                .or_else(|| std::env::var("AZURE_DEVOPS_EXT_PAT").ok()),
            working_directory: std::env::current_dir().unwrap_or_default(),
            source_directory,
            tool_configs: HashMap::new(),
            repository_id: std::env::var("BUILD_REPOSITORY_ID").ok(),
            repository_name: std::env::var("BUILD_REPOSITORY_NAME").ok(),
            allowed_repositories: HashMap::new(),
        }
    }
}

/// Result of executing a tool action in Stage 2
#[derive(Debug, Serialize)]
pub struct ExecutionResult {
    /// Whether the execution succeeded
    pub success: bool,
    /// Human-readable message describing the outcome
    pub message: String,
    /// Optional additional data (e.g., work item ID)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

impl ExecutionResult {
    /// Create a successful execution result
    pub fn success(message: impl Into<String>) -> Self {
        Self {
            success: true,
            message: message.into(),
            data: None,
        }
    }

    /// Create a successful execution result with additional data
    pub fn success_with_data(message: impl Into<String>, data: serde_json::Value) -> Self {
        Self {
            success: true,
            message: message.into(),
            data: Some(data),
        }
    }

    /// Create a failed execution result
    pub fn failure(message: impl Into<String>) -> Self {
        Self {
            success: false,
            message: message.into(),
            data: None,
        }
    }
}

/// Trait for executing tool results in Stage 2 of the pipeline.
///
/// After the agent generates safe outputs (serialized ToolResult structs),
/// Stage 2 parses these outputs and calls `execute` on each to perform
/// the actual action (e.g., create work items, update files, etc.)
#[async_trait::async_trait]
pub trait Executor: Sanitize + Send + Sync {
    /// Internal execution logic. Implementors define this; callers should
    /// use `execute_sanitized()` instead to ensure inputs are sanitized.
    async fn execute_impl(&self, ctx: &ExecutionContext) -> anyhow::Result<ExecutionResult>;

    /// Sanitize all untrusted fields then execute.
    ///
    /// This is the primary entry point for Stage 2 execution. It guarantees
    /// `sanitize_fields()` is called before `execute_impl()`, making it impossible
    /// to accidentally skip sanitization.
    async fn execute_sanitized(&mut self, ctx: &ExecutionContext) -> anyhow::Result<ExecutionResult> {
        self.sanitize_fields();
        self.execute_impl(ctx).await
    }
}

/// Convert an anyhow error to an MCP error
pub fn anyhow_to_mcp_error(err: anyhow::Error) -> McpError {
    McpError {
        code: ErrorCode::INVALID_PARAMS,
        message: err.to_string().into(),
        data: None,
    }
}

/// Macro to generate a tool result struct with automatic `name` field and TryFrom<Params> conversion
///
/// The generated struct derives `Serialize`, `Deserialize`, and `JsonSchema`, making it suitable
/// for both Stage 1 (serialization to safe outputs) and Stage 2 (deserialization for execution).
///
/// # Usage
///
/// Basic (uses trait default of `DEFAULT_MAX = 1`):
/// ```ignore
/// tool_result! {
///     name = "my_tool",
///     params = MyToolParams,
///     pub struct MyToolResult {
///         field1: String,
///         field2: i32,
///     }
/// }
/// ```
///
/// With custom default max (overrides `DEFAULT_MAX` for this tool):
/// ```ignore
/// tool_result! {
///     name = "my_tool",
///     params = MyToolParams,
///     default_max = 5,
///     pub struct MyToolResult {
///         field1: String,
///     }
/// }
/// ```
///
/// Write-requiring tool (sets `REQUIRES_WRITE = true`):
/// ```ignore
/// tool_result! {
///     name = "my_tool",
///     write = true,
///     params = MyToolParams,
///     pub struct MyToolResult {
///         field1: String,
///     }
/// }
/// ```
#[macro_export]
macro_rules! tool_result {
    // write = true, with default_max
    (
        name = $tool_name:literal,
        write = true,
        params = $params:ty,
        default_max = $default_max:literal,
        $(#[$meta:meta])*
        $vis:vis struct $name:ident {
            $(
                $(#[$field_meta:meta])*
                $field_vis:vis $field:ident : $ty:ty
            ),* $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
        $vis struct $name {
            /// Tool identifier
            pub name: String,
            $(
                $(#[$field_meta])*
                pub $field: $ty,
            )*
        }

        impl $crate::tools::ToolResult for $name {
            const NAME: &'static str = $tool_name;
            const DEFAULT_MAX: u32 = $default_max;
            const REQUIRES_WRITE: bool = true;
        }

        impl TryFrom<$params> for $name {
            type Error = rmcp::ErrorData;

            fn try_from(params: $params) -> Result<Self, Self::Error> {
                <$params as $crate::tools::Validate>::validate(&params)
                    .map_err($crate::tools::anyhow_to_mcp_error)?;
                Ok(Self {
                    name: <Self as $crate::tools::ToolResult>::NAME.to_string(),
                    $($field: params.$field,)*
                })
            }
        }
    };
    // write = true, without default_max
    (
        name = $tool_name:literal,
        write = true,
        params = $params:ty,
        $(#[$meta:meta])*
        $vis:vis struct $name:ident {
            $(
                $(#[$field_meta:meta])*
                $field_vis:vis $field:ident : $ty:ty
            ),* $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
        $vis struct $name {
            /// Tool identifier
            pub name: String,
            $(
                $(#[$field_meta])*
                pub $field: $ty,
            )*
        }

        impl $crate::tools::ToolResult for $name {
            const NAME: &'static str = $tool_name;
            const REQUIRES_WRITE: bool = true;
        }

        impl TryFrom<$params> for $name {
            type Error = rmcp::ErrorData;

            fn try_from(params: $params) -> Result<Self, Self::Error> {
                <$params as $crate::tools::Validate>::validate(&params)
                    .map_err($crate::tools::anyhow_to_mcp_error)?;
                Ok(Self {
                    name: <Self as $crate::tools::ToolResult>::NAME.to_string(),
                    $($field: params.$field,)*
                })
            }
        }
    };
    // default_max, without write
    (
        name = $tool_name:literal,
        params = $params:ty,
        default_max = $default_max:literal,
        $(#[$meta:meta])*
        $vis:vis struct $name:ident {
            $(
                $(#[$field_meta:meta])*
                $field_vis:vis $field:ident : $ty:ty
            ),* $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
        $vis struct $name {
            /// Tool identifier
            pub name: String,
            $(
                $(#[$field_meta])*
                pub $field: $ty,
            )*
        }

        impl $crate::tools::ToolResult for $name {
            const NAME: &'static str = $tool_name;
            const DEFAULT_MAX: u32 = $default_max;
        }

        impl TryFrom<$params> for $name {
            type Error = rmcp::ErrorData;

            fn try_from(params: $params) -> Result<Self, Self::Error> {
                <$params as $crate::tools::Validate>::validate(&params)
                    .map_err($crate::tools::anyhow_to_mcp_error)?;
                Ok(Self {
                    name: <Self as $crate::tools::ToolResult>::NAME.to_string(),
                    $($field: params.$field,)*
                })
            }
        }
    };
    // basic (no write, no default_max)
    (
        name = $tool_name:literal,
        params = $params:ty,
        $(#[$meta:meta])*
        $vis:vis struct $name:ident {
            $(
                $(#[$field_meta:meta])*
                $field_vis:vis $field:ident : $ty:ty
            ),* $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
        $vis struct $name {
            /// Tool identifier
            pub name: String,
            $(
                $(#[$field_meta])*
                pub $field: $ty,
            )*
        }

        impl $crate::tools::ToolResult for $name {
            const NAME: &'static str = $tool_name;
        }

        impl TryFrom<$params> for $name {
            type Error = rmcp::ErrorData;

            fn try_from(params: $params) -> Result<Self, Self::Error> {
                <$params as $crate::tools::Validate>::validate(&params)
                    .map_err($crate::tools::anyhow_to_mcp_error)?;
                Ok(Self {
                    name: <Self as $crate::tools::ToolResult>::NAME.to_string(),
                    $($field: params.$field,)*
                })
            }
        }
    };
}

/// Derive a `&[&str]` array of tool names from a list of types implementing `ToolResult`.
///
/// This macro is the foundation for compile-time safe output tool list generation.
/// Instead of maintaining string arrays by hand, list the concrete types and the
/// macro extracts each type's `NAME` constant automatically.
///
/// # Usage
/// ```ignore
/// const MY_TOOLS: &[&str] = tool_names![FooResult, BarResult];
/// // expands to: &[FooResult::NAME, BarResult::NAME]
/// ```
#[macro_export]
macro_rules! tool_names {
    ($($ty:ty),* $(,)?) => {
        &[$(<$ty as $crate::tools::ToolResult>::NAME),*]
    };
}

/// Derive `ALL_KNOWN_SAFE_OUTPUTS` from multiple type lists plus extra string literals.
///
/// Combines write-requiring tool types, diagnostic tool types, and non-MCP string keys
/// (like `"memory"`) into a single `&[&str]` array.
///
/// # Usage
/// ```ignore
/// const ALL: &[&str] = all_safe_output_names![
///     WriteToolA, WriteToolB;   // write-requiring types
///     DiagToolA, DiagToolB;     // diagnostic types
///     "memory"                  // non-MCP string keys
/// ];
/// ```
#[macro_export]
macro_rules! all_safe_output_names {
    ($($ty:ty),* $(,)?; $($extra:expr),* $(,)?) => {
        &[$(<$ty as $crate::tools::ToolResult>::NAME),*, $($extra),*]
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_execution_result_success() {
        let r = ExecutionResult::success("all good");
        assert!(r.success);
        assert_eq!(r.message, "all good");
        assert!(r.data.is_none());
    }

    #[test]
    fn test_execution_result_success_with_data() {
        let data = serde_json::json!({"id": 42});
        let r = ExecutionResult::success_with_data("created", data.clone());
        assert!(r.success);
        assert_eq!(r.message, "created");
        assert_eq!(r.data, Some(data));
    }

    #[test]
    fn test_execution_result_failure() {
        let r = ExecutionResult::failure("something broke");
        assert!(!r.success);
        assert_eq!(r.message, "something broke");
        assert!(r.data.is_none());
    }

    #[test]
    fn test_anyhow_to_mcp_error_preserves_message() {
        let err = anyhow::anyhow!("test error message");
        let mcp_err = anyhow_to_mcp_error(err);
        assert!(mcp_err.message.contains("test error message"));
    }

    #[test]
    fn test_anyhow_to_mcp_error_uses_invalid_params_code() {
        let err = anyhow::anyhow!("some error");
        let mcp_err = anyhow_to_mcp_error(err);
        assert_eq!(mcp_err.code, rmcp::model::ErrorCode::INVALID_PARAMS);
    }
}
