use super::{CompileContext, CompilerExtension, Declarations, ExtensionPhase, McpgServerConfig};
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

    /// Typed-IR view. SafeOutputs contributes only static
    /// signals — an MCPG HTTP backend, a prompt supplement, and a
    /// single `--allow-tool safeoutputs` flag.
    fn declarations(&self, ctx: &CompileContext) -> Result<Declarations> {
        Ok(Declarations {
            mcpg_servers: self.mcpg_servers(ctx)?,
            copilot_allow_tools: self.allowed_copilot_tools(),
            prompt_supplement: self.prompt_supplement(),
            ..Declarations::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::types::FrontMatter;

    fn parse_fm(yaml: &str) -> FrontMatter {
        serde_yaml::from_str(yaml).expect("front matter parses")
    }

    #[test]
    fn declarations_carries_mcpg_prompt_and_allowtool() {
        let fm = parse_fm("name: t\ndescription: x\n");
        let ctx = CompileContext::for_test(&fm);
        let decl = SafeOutputsExtension.declarations(&ctx).unwrap();
        assert_eq!(decl.copilot_allow_tools, vec!["safeoutputs".to_string()]);
        assert_eq!(decl.mcpg_servers.len(), 1);
        assert_eq!(decl.mcpg_servers[0].0, "safeoutputs");
        assert!(decl.prompt_supplement.is_some());
        assert!(decl.agent_prepare_steps.is_empty());
    }
}
