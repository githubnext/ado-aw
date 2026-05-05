//! Python runtime support for the ado-aw compiler.
//!
//! When enabled via `runtimes: python:`, the compiler installs the requested
//! Python version via the `UsePythonVersion@0` ADO task, adds Python-specific
//! domains to the AWF network allowlist, extends the bash command allow-list,
//! and optionally sets environment variables to redirect `pip` and `uv` to
//! an internal package feed.
//!
//! # Example front matter
//!
//! ```yaml
//! # Simple enablement (uses system Python, no feed override)
//! runtimes:
//!   python: true
//!
//! # Install a specific version and point pip/uv at an internal feed
//! runtimes:
//!   python:
//!     version: "3.12"
//!     index-url: "https://pkgs.dev.azure.com/myorg/_packaging/myfeed/pypi/simple/"
//! ```

pub mod extension;

pub use extension::PythonExtension;

use ado_aw_derive::SanitizeConfig;
use serde::Deserialize;

use crate::sanitize::SanitizeConfig as SanitizeConfigTrait;

/// Python runtime configuration — accepts both `true` and object formats.
///
/// Examples:
/// ```yaml
/// # Simple enablement (uses system Python, no feed override)
/// runtimes:
///   python: true
///
/// # Install a specific version
/// runtimes:
///   python:
///     version: "3.12"
///
/// # Install a version and point pip/uv at an internal feed
/// runtimes:
///   python:
///     version: "3.12"
///     index-url: "https://pkgs.dev.azure.com/myorg/_packaging/myfeed/pypi/simple/"
///
/// # Pin to a version with both an internal primary and extra index
/// runtimes:
///   python:
///     version: "3.x"
///     index-url: "https://pkgs.dev.azure.com/myorg/_packaging/myfeed/pypi/simple/"
///     extra-index-url: "https://pypi.org/simple/"
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
    /// Whether the Python runtime is enabled.
    pub fn is_enabled(&self) -> bool {
        match self {
            PythonRuntimeConfig::Enabled(enabled) => *enabled,
            PythonRuntimeConfig::WithOptions(_) => true,
        }
    }

    /// Python version spec for `UsePythonVersion@0` (e.g., `"3.12"`, `"3.x"`).
    /// Returns `None` when simple `true` enablement is used (no install step emitted).
    pub fn version(&self) -> Option<&str> {
        match self {
            PythonRuntimeConfig::Enabled(_) => None,
            PythonRuntimeConfig::WithOptions(opts) => opts.version.as_deref(),
        }
    }

    /// Primary pip/uv index URL (`PIP_INDEX_URL` / `UV_DEFAULT_INDEX`).
    /// When set, overrides the default PyPI index so `pip install` and `uv` use
    /// the configured internal feed instead.
    pub fn index_url(&self) -> Option<&str> {
        match self {
            PythonRuntimeConfig::Enabled(_) => None,
            PythonRuntimeConfig::WithOptions(opts) => opts.index_url.as_deref(),
        }
    }

    /// Extra pip index URL (`PIP_EXTRA_INDEX_URL`).
    /// Adds a secondary feed consulted when packages are not found in the primary index.
    pub fn extra_index_url(&self) -> Option<&str> {
        match self {
            PythonRuntimeConfig::Enabled(_) => None,
            PythonRuntimeConfig::WithOptions(opts) => opts.extra_index_url.as_deref(),
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
    /// Python version spec for the `UsePythonVersion@0` ADO task
    /// (e.g., `"3.12"`, `"3.x"`, `"3.12.x"`).
    /// When omitted, no install step is generated and the system Python is used.
    #[serde(default)]
    pub version: Option<String>,

    /// Primary package index URL.
    ///
    /// Injected as `PIP_INDEX_URL` (pip) and `UV_DEFAULT_INDEX` (uv) into the
    /// agent environment so package installs use the specified feed instead of
    /// the default PyPI registry.  Set to an internal ADO Artifacts feed URL to
    /// redirect all package lookups to the internal registry.
    #[serde(default, rename = "index-url")]
    pub index_url: Option<String>,

    /// Extra package index URL.
    ///
    /// Injected as `PIP_EXTRA_INDEX_URL` (pip) into the agent environment as a
    /// secondary fallback feed consulted when a package is not found in the
    /// primary index.
    #[serde(default, rename = "extra-index-url")]
    pub extra_index_url: Option<String>,
}

/// Bash commands that the Python runtime adds to the allow-list.
pub const PYTHON_BASH_COMMANDS: &[&str] = &["python", "python3", "pip", "pip3"];

/// Generate the `UsePythonVersion@0` install step for a specific Python version.
///
/// Returns `None` when no version is specified (simple `true` enablement).
pub fn generate_python_install(config: &PythonRuntimeConfig) -> Option<String> {
    let version = config.version()?;
    Some(format!(
        "- task: UsePythonVersion@0\n  inputs:\n    versionSpec: '{version}'\n    addToPath: true\n  displayName: \"Install Python {version}\""
    ))
}
