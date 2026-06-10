use super::{CompileContext, CompilerExtension, Declarations, ExtensionPhase};

// ─── GitHub (always-on, internal) ────────────────────────────────────

/// GitHub MCP extension.
///
/// Always-on internal extension that grants the agent access to the
/// Copilot CLI built-in GitHub MCP server via `--allow-tool github`.
/// The GitHub MCP uses `GITHUB_TOKEN` from the pipeline environment.
pub struct GitHubExtension;

impl CompilerExtension for GitHubExtension {
    fn name(&self) -> &str {
        "GitHub"
    }

    fn phase(&self) -> ExtensionPhase {
        ExtensionPhase::Tool
    }

    fn allowed_copilot_tools(&self) -> Vec<String> {
        vec!["github".to_string()]
    }

    /// Typed-IR view. The GitHub extension only contributes a single
    /// `--allow-tool github` flag — no steps, hosts, or env vars —
    /// so the override is essentially the same shape as the
    /// `allowed_copilot_tools` legacy method but routed through the
    /// `Declarations` bundle. Keeps the IR migration self-contained
    /// once the legacy method is removed.
    fn declarations(&self, _ctx: &CompileContext) -> anyhow::Result<Declarations> {
        Ok(Declarations {
            copilot_allow_tools: self.allowed_copilot_tools(),
            ..Declarations::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::extensions::CompileContext;
    use crate::compile::types::FrontMatter;

    fn parse_fm(yaml: &str) -> FrontMatter {
        serde_yaml::from_str(yaml).expect("front matter parses")
    }

    #[test]
    fn declarations_carries_only_copilot_allow_tools() {
        let fm = parse_fm("name: t\ndescription: x\n");
        let ctx = CompileContext::for_test(&fm);
        let decl = GitHubExtension.declarations(&ctx).unwrap();
        assert_eq!(decl.copilot_allow_tools, vec!["github".to_string()]);
        assert!(decl.agent_prepare_steps.is_empty());
        assert!(decl.network_hosts.is_empty());
        assert!(decl.mcpg_servers.is_empty());
    }
}
