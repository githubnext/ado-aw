// ─── Node.js ───────────────────────────────────────────────────────

use super::{NODE_BASH_COMMANDS, NodeRuntimeConfig};
use crate::compile::extensions::{CompileContext, CompilerExtension, Declarations, ExtensionPhase};
use crate::compile::ir::step::{BashStep, Step, TaskStep};
use crate::validate;
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

    /// Typed-IR view. Returns:
    ///
    /// * a [`Step::Task`] for `NodeTool@0`,
    /// * (optionally, when `feed-url:` or `config:` is set):
    ///   a [`Step::Bash`] that creates a minimal `.npmrc` if missing,
    ///   then a [`Step::Task`] for `npmAuthenticate@0`.
    ///
    /// All other declarations (hosts, bash commands, env vars, prompt
    /// supplement) flow through the typed bundle as well.
    fn declarations(&self, ctx: &CompileContext) -> Result<Declarations> {
        let mut agent_prepare_steps: Vec<Step> = Vec::with_capacity(3);
        agent_prepare_steps.push(Step::Task(node_install_task_step(&self.config)));
        if self.config.feed_url().is_some() || self.config.config().is_some() {
            agent_prepare_steps.push(Step::Bash(ensure_npmrc_bash_step(&self.config)));
            agent_prepare_steps.push(Step::Task(npm_authenticate_task_step()));
        }
        Ok(Declarations {
            agent_prepare_steps,
            network_hosts: self.required_hosts(),
            bash_commands: self.required_bash_commands(),
            prompt_supplement: self.prompt_supplement(),
            agent_env_vars: self.agent_env_vars(),
            warnings: self.validate(ctx)?,
            ..Declarations::default()
        })
    }
}

/// Typed [`TaskStep`] mirror of [`generate_node_install`]. The version
/// default ("22.x") matches the legacy emitter.
fn node_install_task_step(config: &NodeRuntimeConfig) -> TaskStep {
    let version = config.version().unwrap_or("22.x");
    TaskStep::new("NodeTool@0", format!("Install Node.js {version}"))
        .with_input("versionSpec", version)
}

/// Typed [`TaskStep`] mirror of [`generate_npm_authenticate`].
fn npm_authenticate_task_step() -> TaskStep {
    TaskStep::new(
        "npmAuthenticate@0",
        "Authenticate npm (build service identity)",
    )
    .with_input("workingFile", ".npmrc")
}

/// Typed [`BashStep`] mirror of [`generate_ensure_npmrc`]. The script
/// preserves the legacy semantics: leave any repo-checked-in `.npmrc`
/// untouched; otherwise create a minimal one pointing at the
/// configured feed (or the default npmjs registry).
fn ensure_npmrc_bash_step(config: &NodeRuntimeConfig) -> BashStep {
    let registry = config.feed_url().unwrap_or("https://registry.npmjs.org/");
    let script = format!(
        "set -eo pipefail\n\
         if [ ! -f .npmrc ]; then\n  \
           echo 'registry={registry}' > .npmrc\n  \
           echo 'Created .npmrc with registry={registry}'\n\
         else\n  \
           echo '.npmrc already exists, skipping creation'\n\
         fi\n"
    );
    BashStep::new("Ensure .npmrc exists", script)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::parse_markdown;

    fn ctx_from(front_matter: &crate::compile::types::FrontMatter) -> CompileContext<'_> {
        CompileContext::for_test(front_matter)
    }

    #[test]
    fn test_validate_bash_disabled_warning() {
        let (fm, _) =
            parse_markdown("---\nname: test\ndescription: test\ntools:\n  bash: []\n---\n")
                .unwrap();
        let ext = NodeExtension::new(NodeRuntimeConfig::Enabled(true));
        let warnings = ext.validate(&ctx_from(&fm)).unwrap();
        assert!(!warnings.is_empty());
        assert!(warnings[0].contains("tools.bash is empty"));
    }

    #[test]
    fn test_validate_config_and_feed_url_are_mutually_exclusive() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nruntimes:\n  node:\n    config: '.npmrc'\n    feed-url: 'https://pkgs.dev.azure.com/org/project/_packaging/feed/npm/registry/'\n---\n",
        )
        .unwrap();
        let node = fm.runtimes.as_ref().unwrap().node.as_ref().unwrap();
        let ext = NodeExtension::new(node.clone());
        let err = ext.validate(&ctx_from(&fm)).unwrap_err();
        assert!(err.to_string().contains("mutually exclusive"));
    }

    #[test]
    fn test_validate_config_only_emits_warning() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nruntimes:\n  node:\n    config: '.npmrc'\n---\n",
        )
        .unwrap();
        let node = fm.runtimes.as_ref().unwrap().node.as_ref().unwrap();
        let ext = NodeExtension::new(node.clone());
        let warnings = ext.validate(&ctx_from(&fm)).unwrap();
        assert!(warnings.iter().any(|w| w.contains("will not be available")));
    }

    #[test]
    fn test_validate_invalid_feed_url_rejected() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nruntimes:\n  node:\n    feed-url: 'pkgs.dev.azure.com/no-scheme'\n---\n",
        )
        .unwrap();
        let node = fm.runtimes.as_ref().unwrap().node.as_ref().unwrap();
        let ext = NodeExtension::new(node.clone());
        assert!(ext.validate(&ctx_from(&fm)).is_err());
    }

    #[test]
    fn test_validate_version_injection_rejected() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nruntimes:\n  node:\n    version: '$(SECRET)'\n---\n",
        )
        .unwrap();
        let node = fm.runtimes.as_ref().unwrap().node.as_ref().unwrap();
        let ext = NodeExtension::new(node.clone());
        assert!(ext.validate(&ctx_from(&fm)).is_err());
    }

    /// Default Node install: only a single `Step::Task(NodeTool@0)`
    /// surfaces; no npmrc / npmAuthenticate steps are emitted.
    #[test]
    fn declarations_returns_typed_task_for_default_node() {
        let (fm, _) = parse_markdown("---\nname: t\ndescription: x\n---\n").unwrap();
        let ext = NodeExtension::new(NodeRuntimeConfig::Enabled(true));
        let decl = ext.declarations(&ctx_from(&fm)).unwrap();
        assert_eq!(decl.agent_prepare_steps.len(), 1);
        match &decl.agent_prepare_steps[0] {
            Step::Task(t) => {
                assert_eq!(t.task, "NodeTool@0");
                assert_eq!(t.display_name, "Install Node.js 22.x");
                assert_eq!(
                    t.inputs.get("versionSpec").map(String::as_str),
                    Some("22.x")
                );
            }
            other => panic!("expected Step::Task, got {other:?}"),
        }
        assert!(decl.agent_env_vars.is_empty());
    }

    /// With `feed-url:` set, three steps surface in order:
    /// `NodeTool@0` → `Ensure .npmrc exists` → `npmAuthenticate@0`,
    /// and `NPM_CONFIG_REGISTRY` flows into agent env vars.
    #[test]
    fn declarations_with_feed_url_appends_npmrc_and_auth() {
        let (fm, _) = parse_markdown(
            "---\nname: t\ndescription: x\nruntimes:\n  node:\n    feed-url: 'https://pkgs.dev.azure.com/org/project/_packaging/feed/npm/registry/'\n---\n",
        )
        .unwrap();
        let node = fm.runtimes.as_ref().unwrap().node.as_ref().unwrap();
        let ext = NodeExtension::new(node.clone());
        let decl = ext.declarations(&ctx_from(&fm)).unwrap();
        assert_eq!(decl.agent_prepare_steps.len(), 3);
        match &decl.agent_prepare_steps[1] {
            Step::Bash(b) => {
                assert_eq!(b.display_name, "Ensure .npmrc exists");
                assert!(
                    b.script.contains("pkgs.dev.azure.com"),
                    "expected configured feed URL in script: {}",
                    b.script
                );
            }
            other => panic!("expected Step::Bash for ensure-npmrc, got {other:?}"),
        }
        match &decl.agent_prepare_steps[2] {
            Step::Task(t) => {
                assert_eq!(t.task, "npmAuthenticate@0");
                assert_eq!(
                    t.inputs.get("workingFile").map(String::as_str),
                    Some(".npmrc")
                );
            }
            other => panic!("expected Step::Task for npmAuthenticate@0, got {other:?}"),
        }
        let keys: Vec<&str> = decl
            .agent_env_vars
            .iter()
            .map(|(k, _)| k.as_str())
            .collect();
        assert!(keys.contains(&"NPM_CONFIG_REGISTRY"));
    }
}
