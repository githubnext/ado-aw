// ─── Python ────────────────────────────────────────────────────────

use crate::compile::extensions::{CompileContext, CompilerExtension, ExtensionPhase};
use crate::validate;
use super::{PYTHON_BASH_COMMANDS, PythonRuntimeConfig, generate_pip_authenticate, generate_python_install};
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

    fn prepare_steps(&self) -> Vec<String> {
        let mut steps = vec![generate_python_install(&self.config)];
        // Emit PipAuthenticate only when feed-url is set (config alone is not
        // sufficient — PipAuthenticate needs a feed to authenticate against)
        if self.config.feed_url().is_some() {
            steps.push(generate_pip_authenticate());
        }
        steps
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
}
