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
