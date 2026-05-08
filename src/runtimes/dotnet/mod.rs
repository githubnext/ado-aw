//! .NET runtime support for the ado-aw compiler.
//!
//! When enabled via `runtimes: dotnet:`, the compiler auto-installs a specific
//! .NET SDK version via `UseDotNet@2`, emits `NuGetAuthenticate@1` for internal
//! feed access, adds .NET ecosystem domains to the AWF network allowlist,
//! and extends the bash command allow-list with `dotnet`.
//!
//! No AWF mounts or PATH prepends are needed because `UseDotNet@2` installs
//! to `/opt/hostedtoolcache` (already mounted read-only by AWF) and publishes
//! `##vso[task.prependpath]` entries that AWF merges via `$GITHUB_PATH`.
//!
//! ## Difference from Python / Node runtimes
//!
//! Unlike `pip`/`npm`, NuGet has no first-class environment-variable
//! equivalent for selecting a package source — the convention is a
//! `nuget.config` file in the workspace. This runtime therefore configures
//! feeds via `nuget.config` (either generated or checked in) rather than
//! through `agent_env_vars()`. AWF preserves workspace files (it only
//! overlays things in `$HOME` such as `~/.npmrc`), so a checked-in or
//! generated `nuget.config` is fully usable inside the agent sandbox.

pub mod extension;

pub use extension::DotnetExtension;

use ado_aw_derive::SanitizeConfig;
use serde::Deserialize;

use crate::sanitize::SanitizeConfig as SanitizeConfigTrait;

/// .NET runtime configuration — accepts both `true` and object formats.
///
/// Examples:
/// ```yaml
/// # Simple enablement (installs default .NET SDK)
/// runtimes:
///   dotnet: true
///
/// # With options (pin version, configure feed)
/// runtimes:
///   dotnet:
///     version: "8.0.x"
///     feed-url: "https://pkgs.dev.azure.com/myorg/_packaging/myfeed/nuget/v3/index.json"
/// ```
#[derive(Debug, Deserialize, Clone)]
#[serde(untagged)]
pub enum DotnetRuntimeConfig {
    /// Simple boolean enablement
    Enabled(bool),
    /// Full configuration with options
    WithOptions(DotnetOptions),
}

/// The sentinel value users can set in `runtimes.dotnet.version` to opt
/// into `UseDotNet@2`'s `useGlobalJson: true` mode, which installs every
/// SDK referenced by `global.json` files in the workspace.
pub const GLOBAL_JSON_SENTINEL: &str = "global.json";

impl DotnetRuntimeConfig {
    /// Whether .NET is enabled.
    pub fn is_enabled(&self) -> bool {
        match self {
            DotnetRuntimeConfig::Enabled(enabled) => *enabled,
            DotnetRuntimeConfig::WithOptions(_) => true,
        }
    }

    /// Get the .NET SDK version (None = use ADO default).
    pub fn version(&self) -> Option<&str> {
        match self {
            DotnetRuntimeConfig::Enabled(_) => None,
            DotnetRuntimeConfig::WithOptions(opts) => opts.version.as_deref(),
        }
    }

    /// Whether the user opted into `useGlobalJson: true` by setting
    /// `version: "global.json"` (case-insensitive).
    pub fn use_global_json(&self) -> bool {
        self.version()
            .is_some_and(|v| v.eq_ignore_ascii_case(GLOBAL_JSON_SENTINEL))
    }

    /// Get the NuGet source URL (None = use public nuget.org / repo defaults).
    pub fn feed_url(&self) -> Option<&str> {
        match self {
            DotnetRuntimeConfig::Enabled(_) => None,
            DotnetRuntimeConfig::WithOptions(opts) => opts.feed_url.as_deref(),
        }
    }

    /// Get the path to a checked-in `nuget.config` (None = not set).
    pub fn config(&self) -> Option<&str> {
        match self {
            DotnetRuntimeConfig::Enabled(_) => None,
            DotnetRuntimeConfig::WithOptions(opts) => opts.config.as_deref(),
        }
    }
}

impl SanitizeConfigTrait for DotnetRuntimeConfig {
    fn sanitize_config_fields(&mut self) {
        match self {
            DotnetRuntimeConfig::Enabled(_) => {}
            DotnetRuntimeConfig::WithOptions(opts) => opts.sanitize_config_fields(),
        }
    }
}

/// .NET runtime options.
#[derive(Debug, Deserialize, Clone, Default, SanitizeConfig)]
pub struct DotnetOptions {
    /// .NET SDK version to install (e.g., `"8.0.x"`, `"9.0.x"`).
    /// Passed to `UseDotNet@2` `version` with `packageType: 'sdk'`.
    ///
    /// The special value `"global.json"` (case-insensitive) opts into
    /// `UseDotNet@2`'s `useGlobalJson: true` mode, which discovers and
    /// installs every SDK version referenced by `global.json` files in
    /// the workspace. When this sentinel is used the explicit `version`
    /// input is omitted from the generated step.
    ///
    /// If a `global.json` exists at the agent's compile directory and a
    /// concrete version is specified here, the compiler errors out — pick
    /// one source of truth.
    #[serde(default)]
    pub version: Option<String>,

    /// Internal NuGet feed URL (typically the v3 `index.json` of an Azure
    /// Artifacts feed). When set, the compiler emits a step that creates a
    /// minimal `nuget.config` referencing this source (only if the repo
    /// doesn't already have one) and then runs `NuGetAuthenticate@1` so the
    /// ADO build service identity can authenticate to the feed.
    ///
    /// Unlike Python (`PIP_INDEX_URL`) and Node (`NPM_CONFIG_REGISTRY`),
    /// no env var is injected — NuGet does not have a first-class env-var
    /// equivalent for selecting a package source.
    #[serde(default, rename = "feed-url")]
    pub feed_url: Option<String>,

    /// Path to a checked-in `nuget.config` file in the repo. When set, the
    /// compiler runs `NuGetAuthenticate@1` against the workspace (which
    /// auto-discovers `nuget.config` files); the file is fully functional
    /// inside the AWF agent environment because AWF preserves workspace
    /// files. Mutually exclusive with `feed-url`.
    #[serde(default)]
    pub config: Option<String>,
}

/// Bash commands that the .NET runtime adds to the allow-list.
pub const DOTNET_BASH_COMMANDS: &[&str] = &["dotnet"];

/// Generate the `UseDotNet@2` pipeline step.
///
/// Emits one of three shapes:
/// - `version: "global.json"` → `useGlobalJson: true` (discovers SDK
///   versions from `global.json` files in the workspace).
/// - explicit `version: "8.0.x"` → `version: '8.0.x'`.
/// - no version → `version: '8.0.x'` (compiler default).
pub fn generate_dotnet_install(config: &DotnetRuntimeConfig) -> String {
    if config.use_global_json() {
        return "\
- task: UseDotNet@2
  inputs:
    packageType: 'sdk'
    useGlobalJson: true
  displayName: 'Install .NET SDK (from global.json)'"
            .to_string();
    }

    let version = config.version().unwrap_or("8.0.x");
    format!(
        "\
- task: UseDotNet@2
  inputs:
    packageType: 'sdk'
    version: '{version}'
  displayName: 'Install .NET SDK {version}'"
    )
}

/// Generate the `NuGetAuthenticate@1` pipeline step.
///
/// Emitted when `feed-url:` or `config:` is set, authenticating the ADO
/// build service identity against any Azure Artifacts feeds referenced by
/// `nuget.config` files in the workspace. `NuGetAuthenticate@1` auto-
/// discovers `nuget.config` files — no `workingFile:` input is required,
/// unlike `npmAuthenticate@0`.
pub fn generate_nuget_authenticate() -> String {
    "\
- task: NuGetAuthenticate@1
  displayName: 'Authenticate NuGet (build service identity)'"
        .to_string()
}

/// Generate a step that ensures a `nuget.config` exists before
/// `NuGetAuthenticate@1`.
///
/// `NuGetAuthenticate@1` is a no-op without a `nuget.config` to authenticate
/// against. This step writes a minimal `nuget.config` (with the configured
/// feed URL added as a package source) only when one doesn't already exist
/// at the repo root, preserving any repo-checked-in `nuget.config`.
///
/// The existence check covers the three case variations NuGet itself
/// recognises on case-sensitive filesystems (`nuget.config`, `NuGet.config`,
/// `NuGet.Config`); the file is always created with the lowercase form,
/// matching the cross-platform convention.
pub fn generate_ensure_nuget_config(config: &DotnetRuntimeConfig) -> String {
    let feed_url = config.feed_url().unwrap_or("https://api.nuget.org/v3/index.json");

    format!(
        "\
- bash: |\n\
    if [ ! -f nuget.config ] && [ ! -f NuGet.config ] && [ ! -f NuGet.Config ]; then\n\
      cat > nuget.config <<'EOF'\n\
    <?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
    <configuration>\n\
      <packageSources>\n\
        <clear />\n\
        <add key=\"internal\" value=\"{feed_url}\" />\n\
      </packageSources>\n\
    </configuration>\n\
    EOF\n\
      echo 'Created nuget.config with source={feed_url}'\n\
    else\n\
      echo 'nuget.config already exists, skipping creation'\n\
    fi\n\
  displayName: 'Ensure nuget.config exists'"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── generate_dotnet_install ────────────────────────────────────

    #[test]
    fn test_generate_dotnet_install_default() {
        let config = DotnetRuntimeConfig::Enabled(true);
        let step = generate_dotnet_install(&config);
        assert!(step.contains("UseDotNet@2"), "should use UseDotNet@2 task");
        assert!(
            step.contains("packageType: 'sdk'"),
            "should pin packageType to 'sdk'"
        );
        assert!(
            step.contains("version: '8.0.x'"),
            "default version should be 8.0.x: {step}"
        );
        assert!(
            !step.contains("useGlobalJson"),
            "should not emit useGlobalJson for default"
        );
    }

    #[test]
    fn test_generate_dotnet_install_explicit_version() {
        let config = DotnetRuntimeConfig::WithOptions(DotnetOptions {
            version: Some("9.0.x".to_string()),
            ..Default::default()
        });
        let step = generate_dotnet_install(&config);
        assert!(
            step.contains("version: '9.0.x'"),
            "should use specified version: {step}"
        );
        assert!(
            !step.contains("useGlobalJson"),
            "should not emit useGlobalJson with explicit version"
        );
    }

    #[test]
    fn test_generate_dotnet_install_global_json() {
        let config = DotnetRuntimeConfig::WithOptions(DotnetOptions {
            version: Some("global.json".to_string()),
            ..Default::default()
        });
        let step = generate_dotnet_install(&config);
        assert!(
            step.contains("useGlobalJson: true"),
            "should emit useGlobalJson: true: {step}"
        );
        assert!(
            !step.contains("version: '"),
            "should not emit explicit version with useGlobalJson: {step}"
        );
    }

    #[test]
    fn test_generate_dotnet_install_global_json_case_insensitive() {
        let config = DotnetRuntimeConfig::WithOptions(DotnetOptions {
            version: Some("Global.JSON".to_string()),
            ..Default::default()
        });
        let step = generate_dotnet_install(&config);
        assert!(
            step.contains("useGlobalJson: true"),
            "sentinel should be case-insensitive: {step}"
        );
    }

    // ── generate_ensure_nuget_config ──────────────────────────────

    #[test]
    fn test_generate_ensure_nuget_config_contains_feed_url() {
        let feed = "https://pkgs.dev.azure.com/myorg/_packaging/myfeed/nuget/v3/index.json";
        let config = DotnetRuntimeConfig::WithOptions(DotnetOptions {
            feed_url: Some(feed.to_string()),
            ..Default::default()
        });
        let step = generate_ensure_nuget_config(&config);
        assert!(
            step.contains(feed),
            "step should interpolate the configured feed URL: {step}"
        );
        assert!(
            step.contains("<packageSources>"),
            "should emit valid nuget.config XML"
        );
        assert!(
            step.contains("nuget.config"),
            "step should reference nuget.config"
        );
        assert!(
            step.contains("displayName: 'Ensure nuget.config exists'"),
            "step should carry the expected displayName"
        );
    }

    #[test]
    fn test_generate_ensure_nuget_config_default_feed() {
        let config = DotnetRuntimeConfig::Enabled(true);
        let step = generate_ensure_nuget_config(&config);
        assert!(
            step.contains("https://api.nuget.org/v3/index.json"),
            "default feed should be the public nuget.org v3 index: {step}"
        );
    }
}
