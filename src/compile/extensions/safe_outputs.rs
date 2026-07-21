use super::{CompileContext, CompilerExtension, Declarations, ExtensionPhase};
use anyhow::Result;

// ─── SafeOutputs (always-on, internal) ───────────────────────────────

/// SafeOutputs MCP extension.
///
/// Always-on internal extension that appends SafeOutputs prompt guidance and
/// grants the compiler-owned MCPG server to the agent. The working-directory
/// aware stdio container entry is assembled centrally by `generate_mcpg_config`.
pub struct SafeOutputsExtension;

impl CompilerExtension for SafeOutputsExtension {
    fn name(&self) -> &str {
        "SafeOutputs"
    }

    fn phase(&self) -> ExtensionPhase {
        ExtensionPhase::Tool
    }

    /// Typed-IR view. SafeOutputs contributes static prompt guidance and a
    /// single `--allow-tool safeoutputs` flag.
    fn declarations(&self, _ctx: &CompileContext) -> Result<Declarations> {
        Ok(Declarations {
            copilot_allow_tools: vec!["safeoutputs".to_string()],
            prompt_supplement: Some(
                r#"
---

## Important: Safe Outputs

You have access to the `safeoutputs` MCP server which provides tools for creating work items and reporting issues. **Always prefer using safeoutputs tools over other methods**.

These tools generate safe outputs that will be reviewed and executed in a separate pipeline stage, ensuring proper validation and security controls.
"#
                .to_string(),
            ),
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
    fn declarations_carries_prompt_and_allowtool() {
        let fm = parse_fm("name: t\ndescription: x\n");
        let ctx = CompileContext::for_test(&fm);
        let decl = SafeOutputsExtension.declarations(&ctx).unwrap();
        assert_eq!(decl.copilot_allow_tools, vec!["safeoutputs".to_string()]);
        assert!(decl.mcpg_servers.is_empty());
        assert!(decl.prompt_supplement.is_some());
        assert!(decl.agent_prepare_steps.is_empty());
    }
}
