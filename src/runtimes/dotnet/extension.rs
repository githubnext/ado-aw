// ─── .NET ──────────────────────────────────────────────────────────

use crate::compile::extensions::{CompileContext, CompilerExtension, ExtensionPhase};
use crate::validate;
use super::{
    DOTNET_BASH_COMMANDS, DotnetRuntimeConfig, GLOBAL_JSON_SENTINEL, generate_dotnet_install,
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

    fn prepare_steps(&self, _ctx: &CompileContext) -> Vec<String> {
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

        // Validate version string. Skip the injection check for the
        // `global.json` sentinel — it's a literal keyword, not a version.
        if let Some(version) = self.config.version()
            && !version.eq_ignore_ascii_case(GLOBAL_JSON_SENTINEL)
        {
            validate::reject_pipeline_injection(version, "runtimes.dotnet.version")?;
        }

        // global.json conflict detection: if the agent's compile directory
        // contains a `global.json`, the SDK version is already pinned by
        // that file and the front matter must not also specify an explicit
        // version. Either drop the version or set `version: "global.json"`.
        if let Some(compile_dir) = ctx.compile_dir
            && compile_dir.join("global.json").exists()
            && self.config.version().is_some()
            && !self.config.use_global_json()
        {
            anyhow::bail!(
                "runtimes.dotnet.version: a 'global.json' file exists at '{}', \
                 which already pins the .NET SDK version. Either remove \
                 'runtimes.dotnet.version' or set it to the literal string \
                 'global.json' to use UseDotNet@2's useGlobalJson mode.",
                compile_dir.display()
            );
        }

        // Validate config path (defend against pipeline injection). The value
        // is not currently embedded in any generated YAML — `NuGetAuthenticate@1`
        // auto-discovers `nuget.config` — but we still validate it as a
        // defence-in-depth measure in case it is surfaced in displayName or
        // logs in the future.
        if let Some(config) = self.config.config() {
            validate::reject_pipeline_injection(config, "runtimes.dotnet.config")?;
        }

        Ok(warnings)
    }
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
        let ext = DotnetExtension::new(DotnetRuntimeConfig::Enabled(true));
        let warnings = ext.validate(&ctx_from(&fm)).unwrap();
        assert!(!warnings.is_empty());
        assert!(warnings[0].contains("tools.bash is empty"));
    }

    #[test]
    fn test_validate_config_and_feed_url_are_mutually_exclusive() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nruntimes:\n  dotnet:\n    config: 'nuget.config'\n    feed-url: 'https://pkgs.dev.azure.com/myorg/_packaging/myfeed/nuget/v3/index.json'\n---\n",
        )
        .unwrap();
        let dotnet = fm.runtimes.as_ref().unwrap().dotnet.as_ref().unwrap();
        let ext = DotnetExtension::new(dotnet.clone());
        let err = ext.validate(&ctx_from(&fm)).unwrap_err();
        assert!(err.to_string().contains("mutually exclusive"));
    }

    #[test]
    fn test_validate_invalid_feed_url_rejected() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nruntimes:\n  dotnet:\n    feed-url: 'https://example.com/$(SECRET)/nuget'\n---\n",
        )
        .unwrap();
        let dotnet = fm.runtimes.as_ref().unwrap().dotnet.as_ref().unwrap();
        let ext = DotnetExtension::new(dotnet.clone());
        assert!(ext.validate(&ctx_from(&fm)).is_err());
    }

    #[test]
    fn test_validate_version_injection_rejected() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nruntimes:\n  dotnet:\n    version: '$(SECRET)'\n---\n",
        )
        .unwrap();
        let dotnet = fm.runtimes.as_ref().unwrap().dotnet.as_ref().unwrap();
        let ext = DotnetExtension::new(dotnet.clone());
        assert!(ext.validate(&ctx_from(&fm)).is_err());
    }

    #[test]
    fn test_validate_global_json_conflict_bails() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("global.json"), r#"{"sdk":{"version":"8.0.100"}}"#).unwrap();

        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nruntimes:\n  dotnet:\n    version: '9.0.x'\n---\n",
        )
        .unwrap();
        let dotnet = fm.runtimes.as_ref().unwrap().dotnet.as_ref().unwrap();
        let ext = DotnetExtension::new(dotnet.clone());
        let ctx = CompileContext::for_test_with_compile_dir(&fm, tmp.path());
        let err = ext.validate(&ctx).unwrap_err();
        assert!(err.to_string().contains("global.json"));
    }

    #[test]
    fn test_validate_global_json_sentinel_accepted_with_file_present() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("global.json"), r#"{"sdk":{"version":"8.0.100"}}"#).unwrap();

        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nruntimes:\n  dotnet:\n    version: 'global.json'\n---\n",
        )
        .unwrap();
        let dotnet = fm.runtimes.as_ref().unwrap().dotnet.as_ref().unwrap();
        let ext = DotnetExtension::new(dotnet.clone());
        let ctx = CompileContext::for_test_with_compile_dir(&fm, tmp.path());
        assert!(ext.validate(&ctx).is_ok());
    }

    #[test]
    fn test_validate_config_injection_rejected() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\nruntimes:\n  dotnet:\n    config: '$(SECRET)/nuget.config'\n---\n",
        )
        .unwrap();
        let dotnet = fm.runtimes.as_ref().unwrap().dotnet.as_ref().unwrap();
        let ext = DotnetExtension::new(dotnet.clone());
        assert!(ext.validate(&ctx_from(&fm)).is_err());
    }
}
