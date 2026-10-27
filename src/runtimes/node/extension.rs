// ─── Node.js ───────────────────────────────────────────────────────

use super::{NODE_BASH_COMMANDS, NodeRuntimeConfig};
use crate::compile::extensions::{CompileContext, CompilerExtension, Declarations, ExtensionPhase};
use crate::compile::ir::step::{Step, TaskStep};
use crate::compile::ir::tasks::use_node::UseNode;
use crate::validate;
use anyhow::Result;

/// Node.js runtime extension.
///
/// Injects: ecosystem network hosts (node), bash commands (node, npm, npx),
/// install steps (UseNode@1), env vars (NPM_CONFIG_REGISTRY when feed-url is
/// set), and a prompt supplement.
///
/// No `npmAuthenticate@0`: it appends the job's token to the workspace
/// `.npmrc`, which the agent can read.
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

    /// Typed-IR view. Returns a [`Step::Task`] for `UseNode@1`; all other
    /// declarations (hosts, bash commands, env vars, prompt supplement) flow
    /// through the typed bundle as well.
    fn declarations(&self, ctx: &CompileContext) -> Result<Declarations> {
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
        crate::runtimes::validate_feed_exclusivity(
            "node",
            self.config.feed(),
            self.config.feed_url(),
            self.config.config(),
        )?;
        warnings.extend(crate::runtimes::public_registry_warning(
            ctx.front_matter,
            "node",
            self.config.public_registry(),
            self.config.feed(),
            self.config.feed_url().is_some() || self.config.config().is_some(),
        ));

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
        if self.config.feed_url().is_some() {
            warnings.push(crate::runtimes::unauthenticated_feed_warning(
                "runtimes.node.feed-url",
            ));
        }

        let agent_prepare_steps = vec![Step::Task(node_install_task_step(&self.config))];
        // A granted feed is reached through the credential-isolated package
        // proxy, so its URL carries no credential either.
        let source_url = match self.config.feed() {
            Some(handle) => Some(crate::runtimes::selected_feed_url(
                ctx,
                "node",
                handle,
                crate::compile::types::PackageProtocol::Npm,
            )?),
            None => self.config.feed_url().map(str::to_string),
        };
        let mut agent_env_vars = Vec::new();
        if let Some(source_url) = source_url {
            agent_env_vars.push(("NPM_CONFIG_REGISTRY".to_string(), source_url));
        }
        Ok(Declarations {
            agent_prepare_steps,
            network_hosts: vec!["node".to_string()],
            bash_commands: NODE_BASH_COMMANDS
                .iter()
                .map(|c| (*c).to_string())
                .collect(),
            prompt_supplement: Some(
                "\n\
---\n\
\n\
## Node.js\n\
\n\
Node.js is installed and available. Use `node` to run scripts, \
`npm` to manage packages, and `npx` to run package binaries.\n"
                    .to_string(),
            ),
            agent_env_vars,
            warnings,
            ..Declarations::default()
        })
    }
}

/// Build the typed [`TaskStep`] for installing Node.js. The version
/// default ("22.x") matches the legacy emitter.
fn node_install_task_step(config: &NodeRuntimeConfig) -> TaskStep {
    let version = config.version().unwrap_or("22.x");
    UseNode::new(version).into_step()
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
        let warnings = ext.declarations(&ctx_from(&fm)).unwrap().warnings;
        assert!(!warnings.is_empty());
        assert!(warnings[0].contains("tools.bash is empty"));
    }

    #[test]
    fn test_validate_config_and_feed_url_are_mutually_exclusive() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nruntimes:\n  node:\n    config: '.npmrc'\n    feed-url: 'https://packages.example.test/org/project/_packaging/feed/npm/registry/'\n---\n",
        )
        .unwrap();
        let node = fm.runtimes.as_ref().unwrap().node.as_ref().unwrap();
        let ext = NodeExtension::new(node.clone());
        let err = ext.declarations(&ctx_from(&fm)).unwrap_err();
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
        let warnings = ext.declarations(&ctx_from(&fm)).unwrap().warnings;
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
        let err = ext.declarations(&ctx_from(&fm)).unwrap_err();
        assert!(
            err.to_string().contains("https://") || err.to_string().contains("http://"),
            "expected scheme error, got: {err}"
        );
        assert!(err.to_string().contains("runtimes.node.feed-url"), "got: {err}");
    }

    #[test]
    fn test_validate_version_injection_rejected() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nruntimes:\n  node:\n    version: '$(SECRET)'\n---\n",
        )
        .unwrap();
        let node = fm.runtimes.as_ref().unwrap().node.as_ref().unwrap();
        let ext = NodeExtension::new(node.clone());
        let err = ext.declarations(&ctx_from(&fm)).unwrap_err();
        assert!(
            err.to_string().contains("ADO expression"),
            "expected injection error, got: {err}"
        );
        assert!(err.to_string().contains("runtimes.node.version"), "got: {err}");
    }

    /// Default Node install: only a single `Step::Task(UseNode@1)`
    /// surfaces; no npmrc / npmAuthenticate steps are emitted.
    #[test]
    fn declarations_returns_typed_task_for_default_node() {
        let (fm, _) = parse_markdown("---\nname: t\ndescription: x\n---\n").unwrap();
        let ext = NodeExtension::new(NodeRuntimeConfig::Enabled(true));
        let decl = ext.declarations(&ctx_from(&fm)).unwrap();
        assert_eq!(decl.agent_prepare_steps.len(), 1);
        match &decl.agent_prepare_steps[0] {
            Step::Task(t) => {
                assert_eq!(t.task, "UseNode@1");
                assert_eq!(t.display_name, "Install Node.js 22.x");
                assert_eq!(t.inputs.get("version").map(String::as_str), Some("22.x"));
            }
            other => panic!("expected Step::Task, got {other:?}"),
        }
        assert!(decl.agent_env_vars.is_empty());
    }

    /// With `feed-url:` set, only `UseNode@1` is emitted — no workspace
    /// `.npmrc` and no `npmAuthenticate@0`, whose token-bearing `.npmrc`
    /// would be readable by the agent. `NPM_CONFIG_REGISTRY` still selects
    /// the source, and the author is warned that the agent holds no credential.
    #[test]
    fn declarations_feed_url_selects_registry_without_authenticate_task() {
        let (fm, _) = parse_markdown(
            "---\nname: t\ndescription: x\nruntimes:\n  node:\n    feed-url: 'https://packages.example.test/org/project/_packaging/feed/npm/registry/'\n---\n",
        )
        .unwrap();
        let node = fm.runtimes.as_ref().unwrap().node.as_ref().unwrap();
        let ext = NodeExtension::new(node.clone());
        let decl = ext.declarations(&ctx_from(&fm)).unwrap();
        assert_eq!(decl.agent_prepare_steps.len(), 1);
        assert!(matches!(&decl.agent_prepare_steps[0], Step::Task(t) if t.task == "UseNode@1"));
        assert!(
            decl.warnings
                .iter()
                .any(|w| w.contains("no feed credential"))
        );
        let keys: Vec<&str> = decl
            .agent_env_vars
            .iter()
            .map(|(k, _)| k.as_str())
            .collect();
        assert!(keys.contains(&"NPM_CONFIG_REGISTRY"));
    }
}
