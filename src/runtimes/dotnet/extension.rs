// ─── .NET ──────────────────────────────────────────────────────────

use crate::compile::extensions::{CompileContext, CompilerExtension, ExtensionPhase};
use crate::validate;
use super::{
    DOTNET_BASH_COMMANDS, DotnetRuntimeConfig, generate_dotnet_install,
    generate_ensure_nuget_config, generate_nuget_authenticate,
};
use anyhow::Result;

/// .NET runtime extension.
///
/// Injects: ecosystem network hosts (dotnet), bash commands (dotnet),
/// install steps (UseDotNet@2), authenticate steps (NuGetAuthenticate@1),
/// optionally a `nuget.config` shim, and a prompt supplement.
///
/// Unlike the Python and Node extensions, no agent env vars are emitted —
/// NuGet's package-source convention is the `nuget.config` file, not env
/// vars. See `runtimes/dotnet/mod.rs` for the rationale.
pub struct DotnetExtension {
    config: DotnetRuntimeConfig,
}

impl DotnetExtension {
    pub fn new(config: DotnetRuntimeConfig) -> Self {
        Self { config }
    }
}

impl CompilerExtension for DotnetExtension {
    fn name(&self) -> &str {
        "dotnet"
    }

    fn phase(&self) -> ExtensionPhase {
        ExtensionPhase::Runtime
    }

    fn required_hosts(&self) -> Vec<String> {
        vec!["dotnet".to_string()]
    }

    fn required_bash_commands(&self) -> Vec<String> {
        DOTNET_BASH_COMMANDS
            .iter()
            .map(|c| (*c).to_string())
            .collect()
    }

    fn prompt_supplement(&self) -> Option<String> {
        Some(
            "\n\
---\n\
\n\
## .NET\n\
\n\
The .NET SDK is installed and available. Use `dotnet` to build, test, run, \
and manage projects (e.g., `dotnet build`, `dotnet test`, `dotnet restore`, \
`dotnet run`). NuGet package sources are configured via `nuget.config` files \
in the repository.\n"
                .to_string(),
        )
    }

    fn prepare_steps(&self) -> Vec<String> {
        let mut steps = vec![generate_dotnet_install(&self.config)];
        // Emit ensure-nuget.config + NuGetAuthenticate when an internal feed
        // is configured. When only `config:` is set, the user-checked-in
        // nuget.config is assumed to exist — emit only the auth step.
        if self.config.feed_url().is_some() {
            steps.push(generate_ensure_nuget_config(&self.config));
            steps.push(generate_nuget_authenticate());
        } else if self.config.config().is_some() {
            steps.push(generate_nuget_authenticate());
        }
        steps
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
                "Agent '{}' has runtimes.dotnet enabled but tools.bash is empty. \
                 .NET requires bash access (dotnet command).",
                ctx.agent_name
            ));
        }

        // Mutual exclusivity: config + feed-url
        if self.config.config().is_some() && self.config.feed_url().is_some() {
            anyhow::bail!(
                "runtimes.dotnet: 'config' and 'feed-url' are mutually exclusive. \
                 Use one or the other."
            );
        }

        // Validate feed URL
        if let Some(feed_url) = self.config.feed_url() {
            validate::validate_feed_url(feed_url, "runtimes.dotnet.feed-url")?;
        }

        // Validate version string
        if let Some(version) = self.config.version() {
            validate::reject_pipeline_injection(version, "runtimes.dotnet.version")?;
        }

        // Validate config path (just defend against pipeline injection — the
        // path itself is user-supplied and ends up in displayName/log output)
        if let Some(config) = self.config.config() {
            validate::reject_pipeline_injection(config, "runtimes.dotnet.config")?;
        }

        Ok(warnings)
    }
}
