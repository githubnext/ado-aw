// ─── Node.js ───────────────────────────────────────────────────────

use crate::compile::extensions::{CompileContext, CompilerExtension, ExtensionPhase};
use crate::validate;
use super::{NODE_BASH_COMMANDS, NodeRuntimeConfig, generate_ensure_npmrc, generate_node_install, generate_npm_authenticate};
use anyhow::Result;

/// Node.js runtime extension.
///
/// Injects: ecosystem network hosts (node), bash commands (node, npm, npx),
/// install steps (NodeTool@0), authenticate steps (npmAuthenticate@0),
/// env vars (NPM_CONFIG_REGISTRY when feed-url is set), and a prompt
/// supplement.
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
## Node.js\n\
\n\
Node.js is installed and available. Use `node` to run scripts, \
`npm` to manage packages, and `npx` to run package binaries.\n"
                .to_string(),
        )
    }

    fn prepare_steps(&self) -> Vec<String> {
        vec![
            generate_node_install(&self.config),
            generate_ensure_npmrc(&self.config),
            generate_npm_authenticate(),
        ]
    }

    fn agent_env_vars(&self) -> Vec<(String, String)> {
        let mut vars = Vec::new();
        if let Some(feed_url) = self.config.feed_url() {
            vars.push(("NPM_CONFIG_REGISTRY".to_string(), feed_url.to_string()));
        }
        vars
    }

    fn validate(&self, ctx: &CompileContext) -> Result<Vec<String>> {
        let mut warnings = Vec::new();

        // Warn if bash is disabled
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

        // Error if config: is set (not yet supported)
        if self.config.config().is_some() {
            anyhow::bail!(
                "runtimes.node.config is not yet supported. \
                 Use feed-url instead to configure an internal npm registry. \
                 Config file support will be added when AWF proxy-auth lands \
                 (gh-aw-firewall#2547)."
            );
        }

        // Mutual exclusivity: config + feed-url
        if self.config.config().is_some() && self.config.feed_url().is_some() {
            anyhow::bail!(
                "runtimes.node: 'config' and 'feed-url' are mutually exclusive. \
                 Use one or the other."
            );
        }

        // Validate feed URL
        if let Some(feed_url) = self.config.feed_url() {
            validate::validate_feed_url(feed_url, "runtimes.node.feed-url")?;
        }

        // Validate version string
        if let Some(version) = self.config.version() {
            validate::reject_pipeline_injection(version, "runtimes.node.version")?;
        }

        Ok(warnings)
    }
}
