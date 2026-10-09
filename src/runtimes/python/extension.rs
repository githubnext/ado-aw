// ─── Python ────────────────────────────────────────────────────────

use super::{PYTHON_BASH_COMMANDS, PythonRuntimeConfig};
use crate::compile::extensions::{CompileContext, CompilerExtension, Declarations, ExtensionPhase};
use crate::compile::ir::step::{Step, TaskStep};
use crate::compile::ir::tasks::use_python_version::UsePythonVersion;
use crate::validate;
use anyhow::Result;

/// Python runtime extension.
///
/// Injects: ecosystem network hosts (python), bash commands (python, pip, uv),
/// install steps (UsePythonVersion@0), env vars (PIP_INDEX_URL,
/// UV_DEFAULT_INDEX when feed-url is set), and a prompt supplement.
///
/// No `PipAuthenticate@1`: it exports a token-bearing index URL as a
/// non-secret job variable, which AWF's `--env-all` would hand to the agent.
pub struct PythonExtension {
    config: PythonRuntimeConfig,
}

impl PythonExtension {
    pub fn new(config: PythonRuntimeConfig) -> Self {
        Self { config }
    }
}

impl CompilerExtension for PythonExtension {
    fn name(&self) -> &str {
        "Python"
    }

    fn phase(&self) -> ExtensionPhase {
        ExtensionPhase::Runtime
    }

    /// Typed-IR view. Returns a [`Step::Task`] for `UsePythonVersion@0`
    /// alongside the static signals (hosts, bash commands, prompt
    /// supplement, agent env vars).
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
                "Agent '{}' has runtimes.python enabled but tools.bash is empty. \
                 Python requires bash access (python, pip, uv commands).",
                ctx.agent_name
            ));
        }

        // Mutual exclusivity: config + feed-url (check before individual field warnings)
        if self.config.config().is_some() && self.config.feed_url().is_some() {
            anyhow::bail!(
                "runtimes.python: 'config' and 'feed-url' are mutually exclusive. \
                 Use one or the other."
            );
        }
        crate::runtimes::validate_feed_exclusivity(
            "python",
            self.config.feed(),
            self.config.feed_url(),
            self.config.config(),
        )?;
        warnings.extend(crate::runtimes::public_registry_warning(
            ctx.front_matter,
            "python",
            self.config.public_registry(),
            self.config.feed(),
            self.config.feed_url().is_some() || self.config.config().is_some(),
        ));

        // Warn if config: is set — accepted but not yet functional inside AWF
        if self.config.config().is_some() {
            warnings.push(
                "runtimes.python.config is accepted but the config file will not be \
                 available inside the AWF agent environment yet. Config file passthrough \
                 requires AWF proxy-auth support (gh-aw-firewall#2547)."
                    .to_string(),
            );
        }

        // Validate feed URL
        if let Some(feed_url) = self.config.feed_url() {
            validate::validate_feed_url(feed_url, "runtimes.python.feed-url")?;
        }

        // Validate version string
        if let Some(version) = self.config.version() {
            validate::reject_pipeline_injection(version, "runtimes.python.version")?;
        }
        if self.config.feed_url().is_some() {
            warnings.push(crate::runtimes::unauthenticated_feed_warning(
                "runtimes.python.feed-url",
            ));
        }

        let agent_prepare_steps = vec![Step::Task(python_install_task_step(&self.config))];
        // A granted feed is reached through the credential-isolated package
        // proxy, so its URL carries no credential either.
        let source_url = match self.config.feed() {
            Some(handle) => Some(crate::runtimes::selected_feed_url(
                ctx,
                "python",
                handle,
                crate::compile::types::PackageProtocol::Pypi,
            )?),
            None => self.config.feed_url().map(str::to_string),
        };
        let mut agent_env_vars = Vec::new();
        if let Some(source_url) = source_url {
            agent_env_vars.push(("PIP_INDEX_URL".to_string(), source_url.clone()));
            agent_env_vars.push(("UV_DEFAULT_INDEX".to_string(), source_url));
        }
        Ok(Declarations {
            agent_prepare_steps,
            network_hosts: vec!["python".to_string()],
            bash_commands: PYTHON_BASH_COMMANDS
                .iter()
                .map(|c| (*c).to_string())
                .collect(),
            prompt_supplement: Some(
                "\n\
---\n\
\n\
## Python\n\
\n\
Python is installed and available. Use `python3` or `python` to run scripts, \
`pip` or `pip3` to install packages. If you need `uv` for fast package \
management, install it first with `pip install uv`.\n"
                    .to_string(),
            ),
            agent_env_vars,
            warnings,
            ..Declarations::default()
        })
    }
}

/// Build the typed [`TaskStep`] for installing Python.
fn python_install_task_step(config: &PythonRuntimeConfig) -> TaskStep {
    let version = config.version().unwrap_or("3.x");
    UsePythonVersion::new(version).into_step()
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
        let ext = PythonExtension::new(PythonRuntimeConfig::Enabled(true));
        let warnings = ext.declarations(&ctx_from(&fm)).unwrap().warnings;
        assert!(!warnings.is_empty());
        assert!(warnings[0].contains("tools.bash is empty"));
    }

    #[test]
    fn test_validate_config_and_feed_url_are_mutually_exclusive() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nruntimes:\n  python:\n    config: 'pip.conf'\n    feed-url: 'https://packages.example.test/org/_packaging/feed/pypi/simple/'\n---\n",
        )
        .unwrap();
        let python = fm.runtimes.as_ref().unwrap().python.as_ref().unwrap();
        let ext = PythonExtension::new(python.clone());
        let err = ext.declarations(&ctx_from(&fm)).unwrap_err();
        assert!(err.to_string().contains("mutually exclusive"));
    }

    #[test]
    fn test_validate_config_only_emits_warning() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nruntimes:\n  python:\n    config: 'pip.conf'\n---\n",
        )
        .unwrap();
        let python = fm.runtimes.as_ref().unwrap().python.as_ref().unwrap();
        let ext = PythonExtension::new(python.clone());
        let warnings = ext.declarations(&ctx_from(&fm)).unwrap().warnings;
        assert!(warnings.iter().any(|w| w.contains("will not be available")));
    }

    #[test]
    fn test_validate_invalid_feed_url_rejected() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nruntimes:\n  python:\n    feed-url: 'pkgs.dev.azure.com/no-scheme'\n---\n",
        )
        .unwrap();
        let python = fm.runtimes.as_ref().unwrap().python.as_ref().unwrap();
        let ext = PythonExtension::new(python.clone());
        let err = ext.declarations(&ctx_from(&fm)).unwrap_err();
        assert!(
            err.to_string().contains("https://") || err.to_string().contains("http://"),
            "expected scheme error, got: {err}"
        );
        assert!(err.to_string().contains("runtimes.python.feed-url"), "got: {err}");
    }

    #[test]
    fn test_validate_version_injection_rejected() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nruntimes:\n  python:\n    version: '$(SECRET)'\n---\n",
        )
        .unwrap();
        let python = fm.runtimes.as_ref().unwrap().python.as_ref().unwrap();
        let ext = PythonExtension::new(python.clone());
        let err = ext.declarations(&ctx_from(&fm)).unwrap_err();
        assert!(
            err.to_string().contains("ADO expression"),
            "expected injection error, got: {err}"
        );
        assert!(err.to_string().contains("runtimes.python.version"), "got: {err}");
    }

    /// Locks the `declarations()` override: must return a single
    /// `Step::Task(UsePythonVersion@0)` install step (no
    /// `Step::RawYaml`) when no feed-url is configured, plus the
    /// static signals.
    #[test]
    fn declarations_returns_typed_task_for_default_python() {
        let (fm, _) = parse_markdown("---\nname: t\ndescription: x\n---\n").unwrap();
        let ext = PythonExtension::new(PythonRuntimeConfig::Enabled(true));
        let decl = ext.declarations(&ctx_from(&fm)).unwrap();
        assert_eq!(decl.agent_prepare_steps.len(), 1);
        match &decl.agent_prepare_steps[0] {
            Step::Task(t) => {
                assert_eq!(t.task, "UsePythonVersion@0");
                assert_eq!(t.display_name, "Install Python 3.x");
                assert_eq!(t.inputs.get("versionSpec").map(String::as_str), Some("3.x"));
            }
            other => panic!("expected Step::Task, got {other:?}"),
        }
        assert_eq!(decl.network_hosts, vec!["python".to_string()]);
        assert!(decl.bash_commands.contains(&"python".to_string()));
        assert!(decl.prompt_supplement.is_some());
        assert!(decl.agent_env_vars.is_empty());
        assert!(decl.mcpg_servers.is_empty());
    }

    /// When `feed-url:` is set, no authenticate task is emitted (its outputs
    /// would reach the agent), `PIP_INDEX_URL` / `UV_DEFAULT_INDEX` select the
    /// source, and the author is warned that the agent holds no credential.
    #[test]
    fn declarations_feed_url_selects_source_without_authenticate_task() {
        let (fm, _) = parse_markdown(
            "---\nname: t\ndescription: x\nruntimes:\n  python:\n    feed-url: 'https://packages.example.test/org/_packaging/feed/pypi/simple/'\n---\n",
        )
        .unwrap();
        let python = fm.runtimes.as_ref().unwrap().python.as_ref().unwrap();
        let ext = PythonExtension::new(python.clone());
        let decl = ext.declarations(&ctx_from(&fm)).unwrap();
        assert_eq!(decl.agent_prepare_steps.len(), 1);
        assert!(
            decl.agent_prepare_steps
                .iter()
                .all(|s| !matches!(s, Step::Task(t) if t.task.contains("Authenticate")))
        );
        assert!(
            decl.warnings
                .iter()
                .any(|w| w.contains("no feed credential"))
        );
        // env vars must include both pip and uv index URLs.
        let keys: Vec<&str> = decl
            .agent_env_vars
            .iter()
            .map(|(k, _)| k.as_str())
            .collect();
        assert!(keys.contains(&"PIP_INDEX_URL"));
        assert!(keys.contains(&"UV_DEFAULT_INDEX"));
    }
}
