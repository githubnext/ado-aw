// ─── Python ────────────────────────────────────────────────────────

use super::{PYTHON_BASH_COMMANDS, PythonRuntimeConfig};
use crate::compile::extensions::{CompileContext, CompilerExtension, Declarations, ExtensionPhase};
use crate::compile::ir::step::{Step, TaskStep};
use crate::validate;
use anyhow::Result;

/// Python runtime extension.
///
/// Injects: ecosystem network hosts (python), bash commands (python, pip, uv),
/// install steps (UsePythonVersion@0), authenticate steps (PipAuthenticate@1),
/// env vars (PIP_INDEX_URL, UV_DEFAULT_INDEX when feed-url is set), and a
/// prompt supplement.
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

    fn required_hosts(&self) -> Vec<String> {
        vec!["python".to_string()]
    }

    fn required_bash_commands(&self) -> Vec<String> {
        PYTHON_BASH_COMMANDS
            .iter()
            .map(|c| (*c).to_string())
            .collect()
    }

    fn prompt_supplement(&self) -> Option<String> {
        Some(
            "\n\
---\n\
\n\
## Python\n\
\n\
Python is installed and available. Use `python3` or `python` to run scripts, \
`pip` or `pip3` to install packages. If you need `uv` for fast package \
management, install it first with `pip install uv`.\n"
                .to_string(),
        )
    }

    fn agent_env_vars(&self) -> Vec<(String, String)> {
        let mut vars = Vec::new();
        if let Some(feed_url) = self.config.feed_url() {
            vars.push(("PIP_INDEX_URL".to_string(), feed_url.to_string()));
            vars.push(("UV_DEFAULT_INDEX".to_string(), feed_url.to_string()));
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

        Ok(warnings)
    }

    /// Typed-IR view. Returns:
    ///
    /// * a [`Step::Task`] for `UsePythonVersion@0`,
    /// * an optional [`Step::Task`] for `PipAuthenticate@1` (only
    ///   when `feed-url:` is set),
    ///
    /// alongside the static signals (hosts, bash commands, prompt
    /// supplement, agent env vars).
    fn declarations(&self, ctx: &CompileContext) -> Result<Declarations> {
        let mut agent_prepare_steps: Vec<Step> = Vec::with_capacity(2);
        agent_prepare_steps.push(Step::Task(python_install_task_step(&self.config)));
        if self.config.feed_url().is_some() {
            agent_prepare_steps.push(Step::Task(pip_authenticate_task_step()));
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

/// Typed [`TaskStep`] mirror of [`generate_python_install`].
fn python_install_task_step(config: &PythonRuntimeConfig) -> TaskStep {
    let version = config.version().unwrap_or("3.x");
    TaskStep::new("UsePythonVersion@0", format!("Install Python {version}"))
        .with_input("versionSpec", version)
}

/// Typed [`TaskStep`] mirror of [`generate_pip_authenticate`].
fn pip_authenticate_task_step() -> TaskStep {
    TaskStep::new(
        "PipAuthenticate@1",
        "Authenticate pip (build service identity)",
    )
    .with_input("artifactFeeds", "")
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
        let warnings = ext.validate(&ctx_from(&fm)).unwrap();
        assert!(!warnings.is_empty());
        assert!(warnings[0].contains("tools.bash is empty"));
    }

    #[test]
    fn test_validate_config_and_feed_url_are_mutually_exclusive() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nruntimes:\n  python:\n    config: 'pip.conf'\n    feed-url: 'https://pkgs.dev.azure.com/org/_packaging/feed/pypi/simple/'\n---\n",
        )
        .unwrap();
        let python = fm.runtimes.as_ref().unwrap().python.as_ref().unwrap();
        let ext = PythonExtension::new(python.clone());
        let err = ext.validate(&ctx_from(&fm)).unwrap_err();
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
        let warnings = ext.validate(&ctx_from(&fm)).unwrap();
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
        assert!(ext.validate(&ctx_from(&fm)).is_err());
    }

    #[test]
    fn test_validate_version_injection_rejected() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nruntimes:\n  python:\n    version: '$(SECRET)'\n---\n",
        )
        .unwrap();
        let python = fm.runtimes.as_ref().unwrap().python.as_ref().unwrap();
        let ext = PythonExtension::new(python.clone());
        assert!(ext.validate(&ctx_from(&fm)).is_err());
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

    /// When `feed-url:` is set, a second `Step::Task(PipAuthenticate@1)`
    /// is appended and `PIP_INDEX_URL` / `UV_DEFAULT_INDEX` env vars
    /// surface on the declarations.
    #[test]
    fn declarations_adds_pip_authenticate_and_env_when_feed_url_set() {
        let (fm, _) = parse_markdown(
            "---\nname: t\ndescription: x\nruntimes:\n  python:\n    feed-url: 'https://pkgs.dev.azure.com/org/_packaging/feed/pypi/simple/'\n---\n",
        )
        .unwrap();
        let python = fm.runtimes.as_ref().unwrap().python.as_ref().unwrap();
        let ext = PythonExtension::new(python.clone());
        let decl = ext.declarations(&ctx_from(&fm)).unwrap();
        assert_eq!(decl.agent_prepare_steps.len(), 2);
        match &decl.agent_prepare_steps[1] {
            Step::Task(t) => {
                assert_eq!(t.task, "PipAuthenticate@1");
                assert_eq!(t.display_name, "Authenticate pip (build service identity)");
                assert_eq!(t.inputs.get("artifactFeeds").map(String::as_str), Some(""));
            }
            other => panic!("expected Step::Task, got {other:?}"),
        }
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
