// ─── Node.js runtime ─────────────────────────────────────────────────

use crate::compile::extensions::{CompileContext, CompilerExtension, ExtensionPhase};
use super::{NODE_BASH_COMMANDS, NodeRuntimeConfig, generate_node_feed_config, generate_node_install};
use anyhow::Result;

/// Node.js runtime extension.
///
/// Injects: network hosts (npm registry domains), bash commands (`node`,
/// `npm`, `npx`), install steps (`NodeTool@0` + optional internal-feed
/// configuration), and a prompt supplement.
pub struct NodeExtension {
    config: NodeRuntimeConfig,
}

impl NodeExtension {
    pub fn new(config: NodeRuntimeConfig) -> Self {
        Self { config }
    }
}

impl CompilerExtension for NodeExtension {
    fn name(&self) -> &str {
        "Node.js"
    }

    fn phase(&self) -> ExtensionPhase {
        ExtensionPhase::Runtime
    }

    fn required_hosts(&self) -> Vec<String> {
        vec!["node".to_string()]
    }

    fn required_bash_commands(&self) -> Vec<String> {
        NODE_BASH_COMMANDS
            .iter()
            .map(|c| (*c).to_string())
            .collect()
    }

    fn prompt_supplement(&self) -> Option<String> {
        Some(
            "\n\
---\n\
\n\
## Node.js Runtime\n\
\n\
Node.js is installed and available. Use `node` to run JavaScript files, \
`npm` for package management, and `npx` to execute package binaries. \
The `node_modules` directory is available after `npm install`.\n"
                .to_string(),
        )
    }

    fn prepare_steps(&self) -> Vec<String> {
        let mut steps = vec![generate_node_install(&self.config)];

        if let Some(feed) = self.config.internal_feed() {
            steps.push(generate_node_feed_config(feed));
        }

        steps
    }

    fn validate(&self, ctx: &CompileContext) -> Result<Vec<String>> {
        let mut warnings = Vec::new();

        let is_bash_disabled = ctx
            .front_matter
            .tools
            .as_ref()
            .and_then(|t| t.bash.as_ref())
            .is_some_and(|cmds| cmds.is_empty());

        if is_bash_disabled {
            warnings.push(format!(
                "Agent '{}' has runtimes.node enabled but tools.bash is empty. \
                 Node.js requires bash access (node, npm, npx commands).",
                ctx.agent_name
            ));
        }

        Ok(warnings)
    }
}
