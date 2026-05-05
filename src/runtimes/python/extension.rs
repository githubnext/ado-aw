// ─── Python runtime ──────────────────────────────────────────────────────────

use anyhow::Result;

use crate::compile::extensions::{CompileContext, CompilerExtension, ExtensionPhase};
use crate::validate;

use super::{PYTHON_BASH_COMMANDS, PythonRuntimeConfig, generate_python_install};

/// Python runtime extension.
///
/// When enabled, injects:
/// - Network hosts: the `python` ecosystem identifier (expands to PyPI domains)
/// - Bash commands: `python`, `python3`, `pip`, `pip3`
/// - Prepare step: `UsePythonVersion@0` task (only when `version:` is set)
/// - Agent env vars: `PIP_INDEX_URL`, `UV_DEFAULT_INDEX`, `PIP_EXTRA_INDEX_URL`
///   (only when `index-url:` / `extra-index-url:` are configured)
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
        // The "python" ecosystem identifier expands to the PyPI domain set via
        // generate_allowed_domains(). Users who only want internal feeds can
        // omit this by blocking pypi.org via `network.blocked`.
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
## Python Runtime\n\
\n\
Python is installed and available. Use `python3` to run scripts, `pip3 install` \
to install packages, and `python3 -m venv` to create virtual environments.\n"
                .to_string(),
        )
    }

    fn prepare_steps(&self) -> Vec<String> {
        generate_python_install(&self.config)
            .into_iter()
            .collect()
    }

    fn agent_env_vars(&self) -> Vec<(String, String)> {
        let mut vars = Vec::new();

        if let Some(index_url) = self.config.index_url() {
            // pip uses PIP_INDEX_URL as the primary index
            vars.push(("PIP_INDEX_URL".to_string(), index_url.to_string()));
            // uv uses UV_DEFAULT_INDEX as the primary index
            vars.push(("UV_DEFAULT_INDEX".to_string(), index_url.to_string()));
        }

        if let Some(extra_url) = self.config.extra_index_url() {
            // pip uses PIP_EXTRA_INDEX_URL as a secondary fallback index
            vars.push(("PIP_EXTRA_INDEX_URL".to_string(), extra_url.to_string()));
        }

        vars
    }

    fn validate(&self, ctx: &CompileContext) -> Result<Vec<String>> {
        let mut warnings = Vec::new();

        if let Some(version) = self.config.version() {
            if !validate::is_valid_version(version) {
                anyhow::bail!(
                    "Agent '{}': runtimes.python.version '{}' contains invalid characters. \
                     Use a version spec such as '3.12', '3.x', or '3.12.x'.",
                    ctx.agent_name,
                    version,
                );
            }
        }

        if let Some(url) = self.config.index_url() {
            validate_feed_url(url, "runtimes.python.index-url", ctx.agent_name)?;
        }

        if let Some(url) = self.config.extra_index_url() {
            validate_feed_url(url, "runtimes.python.extra-index-url", ctx.agent_name)?;
        }

        let is_bash_disabled = ctx
            .front_matter
            .tools
            .as_ref()
            .and_then(|t| t.bash.as_ref())
            .is_some_and(|cmds| cmds.is_empty());

        if is_bash_disabled {
            warnings.push(format!(
                "Agent '{}' has runtimes.python enabled but tools.bash is empty. \
                 Python requires bash access (python3, pip3 commands).",
                ctx.agent_name
            ));
        }

        Ok(warnings)
    }
}

/// Validate a package feed URL for safe embedding in the pipeline YAML.
///
/// Rejects values that could cause ADO expression injection, pipeline command
/// injection, template marker injection, or YAML-breaking newlines.
fn validate_feed_url(url: &str, field: &str, agent_name: &str) -> Result<()> {
    if url.is_empty() {
        anyhow::bail!(
            "Agent '{}': {} must not be empty.",
            agent_name,
            field,
        );
    }
    if validate::contains_ado_expression(url) {
        anyhow::bail!(
            "Agent '{}': {} '{}' contains an ADO expression ('${{{{', '$(', or '$['). \
             Use literal URL values only.",
            agent_name,
            field,
            url,
        );
    }
    if validate::contains_pipeline_command(url) {
        anyhow::bail!(
            "Agent '{}': {} '{}' contains an ADO pipeline command ('##vso[' or '##['). \
             Use literal URL values only.",
            agent_name,
            field,
            url,
        );
    }
    if validate::contains_template_marker(url) {
        anyhow::bail!(
            "Agent '{}': {} '{}' contains a template marker delimiter '{{{{'. \
             Use literal URL values only.",
            agent_name,
            field,
            url,
        );
    }
    if validate::contains_newline(url) {
        anyhow::bail!(
            "Agent '{}': {} contains a newline character, \
             which would break YAML formatting.",
            agent_name,
            field,
        );
    }
    Ok(())
}
