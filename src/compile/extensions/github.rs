use super::{CompilerExtension, ExtensionPhase};

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
}
