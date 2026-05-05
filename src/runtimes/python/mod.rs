//! Python runtime support for the ado-aw compiler.
//!
//! When enabled via `runtimes: python:`, the compiler auto-installs a specific
//! Python version via `UsePythonVersion@0`, emits `PipAuthenticate@1` for
//! internal feed access, adds Python ecosystem domains to the AWF network
//! allowlist, extends the bash command allow-list, and optionally injects
//! feed URL env vars for `pip` and `uv`.
//!
//! No AWF mounts or PATH prepends are needed because `UsePythonVersion@0`
//! installs to `/opt/hostedtoolcache` (already mounted read-only by AWF)
//! and publishes `##vso[task.prependpath]` entries that AWF merges via
//! `$GITHUB_PATH`.

pub mod extension;

pub use extension::PythonExtension;

use ado_aw_derive::SanitizeConfig;
use serde::Deserialize;

use crate::sanitize::SanitizeConfig as SanitizeConfigTrait;

/// Python runtime configuration — accepts both `true` and object formats.
///
/// Examples:
/// ```yaml
/// # Simple enablement (installs default Python 3.x)
/// runtimes:
///   python: true
///
/// # With options (pin version, configure feed)
/// runtimes:
///   python:
///     version: "3.12"
///     feed-url: "https://pkgs.dev.azure.com/myorg/_packaging/myfeed/pypi/simple/"
/// ```
#[derive(Debug, Deserialize, Clone)]
#[serde(untagged)]
pub enum PythonRuntimeConfig {
    /// Simple boolean enablement
    Enabled(bool),
    /// Full configuration with options
    WithOptions(PythonOptions),
}

impl PythonRuntimeConfig {
    /// Whether Python is enabled.
    pub fn is_enabled(&self) -> bool {
        match self {
            PythonRuntimeConfig::Enabled(enabled) => *enabled,
            PythonRuntimeConfig::WithOptions(_) => true,
        }
    }

    /// Get the Python version (None = use ADO default, typically latest 3.x).
    pub fn version(&self) -> Option<&str> {
        match self {
            PythonRuntimeConfig::Enabled(_) => None,
            PythonRuntimeConfig::WithOptions(opts) => opts.version.as_deref(),
        }
    }

    /// Get the feed URL for pip/uv (None = use public PyPI).
    pub fn feed_url(&self) -> Option<&str> {
        match self {
            PythonRuntimeConfig::Enabled(_) => None,
            PythonRuntimeConfig::WithOptions(opts) => opts.feed_url.as_deref(),
        }
    }

    /// Get the config file path (None = not set).
    pub fn config(&self) -> Option<&str> {
        match self {
            PythonRuntimeConfig::Enabled(_) => None,
            PythonRuntimeConfig::WithOptions(opts) => opts.config.as_deref(),
        }
    }
}

impl SanitizeConfigTrait for PythonRuntimeConfig {
    fn sanitize_config_fields(&mut self) {
        match self {
            PythonRuntimeConfig::Enabled(_) => {}
            PythonRuntimeConfig::WithOptions(opts) => opts.sanitize_config_fields(),
        }
    }
}

/// Python runtime options.
#[derive(Debug, Deserialize, Clone, Default, SanitizeConfig)]
pub struct PythonOptions {
    /// Python version to install (e.g., "3.12", "3.11").
    /// Passed to `UsePythonVersion@0` `versionSpec`.
    /// Defaults to latest 3.x if not specified.
    #[serde(default)]
    pub version: Option<String>,

    /// Internal package feed URL. When set, the compiler injects
    /// `PIP_INDEX_URL` and `UV_DEFAULT_INDEX` env vars into the agent
    /// environment so pip/uv use this feed without config file changes.
    #[serde(default, rename = "feed-url")]
    pub feed_url: Option<String>,

    /// Path to a pip/uv config file. Currently recognized but not yet
    /// supported — specifying this field produces a compile error.
    /// Reserved for future proxy-auth integration (gh-aw-firewall#2547).
    #[serde(default)]
    pub config: Option<String>,
}

/// Bash commands that the Python runtime adds to the allow-list.
pub const PYTHON_BASH_COMMANDS: &[&str] = &["python", "python3", "pip", "pip3", "uv"];

/// Generate the `UsePythonVersion@0` pipeline step.
pub fn generate_python_install(config: &PythonRuntimeConfig) -> String {
    let version = config.version().unwrap_or("3.x");
    format!(
        "\
- task: UsePythonVersion@0
  inputs:
    versionSpec: '{version}'
  displayName: 'Install Python {version}'"
    )
}

/// Generate the `PipAuthenticate@1` pipeline step.
///
/// Emitted unconditionally when the Python runtime is enabled — the ADO
/// build service identity handles authentication. This runs before AWF,
/// setting up credentials via `##vso[task.setvariable]`.
pub fn generate_pip_authenticate() -> String {
    "\
- task: PipAuthenticate@1
  inputs:
    artifactFeeds: ''
  displayName: 'Authenticate pip (build service identity)'"
        .to_string()
}
