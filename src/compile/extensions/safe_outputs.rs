use super::{CompileContext, CompilerExtension, ExtensionPhase, McpgConfigReplacement, McpgServerConfig, PipelineEnvMapping};
use anyhow::Result;
use std::collections::BTreeMap;

// ─── SafeOutputs (always-on, internal) ───────────────────────────────

/// SafeOutputs MCP extension.
///
/// Always-on internal extension that configures the SafeOutputs HTTP
/// backend in MCPG and appends prompt guidance for the agent.
pub struct SafeOutputsExtension;

impl CompilerExtension for SafeOutputsExtension {
    fn name(&self) -> &str {
        "SafeOutputs"
    }

    fn phase(&self) -> ExtensionPhase {
        ExtensionPhase::Tool
    }

    fn allowed_copilot_tools(&self) -> Vec<String> {
        vec!["safeoutputs".to_string()]
    }

    fn mcpg_servers(&self, _ctx: &CompileContext) -> Result<Vec<(String, McpgServerConfig)>> {
        Ok(vec![(
            "safeoutputs".to_string(),
            McpgServerConfig {
                server_type: "http".to_string(),
                container: None,
                entrypoint: None,
                entrypoint_args: None,
                mounts: None,
                args: None,
                url: Some("http://localhost:${SAFE_OUTPUTS_PORT}/mcp".to_string()),
                headers: Some(BTreeMap::from([(
                    "Authorization".to_string(),
                    "Bearer ${SAFE_OUTPUTS_API_KEY}".to_string(),
                )])),
                env: None,
                tools: None,
            },
        )])
    }

    fn prompt_supplement(&self) -> Option<String> {
        Some(
            r#"
---

## Important: Safe Outputs

You have access to the `safeoutputs` MCP server which provides tools for creating work items and reporting issues. **Always prefer using safeoutputs tools over other methods**.

These tools generate safe outputs that will be reviewed and executed in a separate pipeline stage, ensuring proper validation and security controls.
"#
            .to_string(),
        )
    }

    fn required_pipeline_vars(&self) -> Vec<PipelineEnvMapping> {
        vec![
            PipelineEnvMapping {
                container_var: "SAFE_OUTPUTS_PORT".to_string(),
                pipeline_var: "SAFE_OUTPUTS_PORT".to_string(),
            },
            PipelineEnvMapping {
                container_var: "SAFE_OUTPUTS_API_KEY".to_string(),
                pipeline_var: "SAFE_OUTPUTS_API_KEY".to_string(),
            },
        ]
    }

    fn mcpg_config_replacements(&self) -> Vec<McpgConfigReplacement> {
        vec![
            McpgConfigReplacement {
                placeholder: "SAFE_OUTPUTS_PORT".to_string(),
                pipeline_var: "SAFE_OUTPUTS_PORT".to_string(),
            },
            McpgConfigReplacement {
                placeholder: "SAFE_OUTPUTS_API_KEY".to_string(),
                pipeline_var: "SAFE_OUTPUTS_API_KEY".to_string(),
            },
        ]
    }
}
