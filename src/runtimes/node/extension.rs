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
        "Node"
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
        let mut steps = vec![generate_node_install(&self.config)];
        // Emit ensure-npmrc + npmAuthenticate only when an internal feed is configured
        if self.config.feed_url().is_some() || self.config.config().is_some() {
            steps.push(generate_ensure_npmrc(&self.config));
            steps.push(generate_npm_authenticate());
        }
        steps
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

        // Mutual exclusivity: config + feed-url (check before individual field warnings)
        if self.config.config().is_some() && self.config.feed_url().is_some() {
            anyhow::bail!(
                "runtimes.node: 'config' and 'feed-url' are mutually exclusive. \
                 Use one or the other."
            );
        }

        // Warn if config: is set — accepted but not yet functional inside AWF
        if self.config.config().is_some() {
            warnings.push(
                "runtimes.node.config is accepted but the .npmrc file will not be \
                 available inside the AWF agent environment yet. Config file passthrough \
                 requires AWF proxy-auth support (gh-aw-firewall#2547)."
                    .to_string(),
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
