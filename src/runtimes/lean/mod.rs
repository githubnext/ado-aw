//! Lean 4 runtime support for the ado-aw compiler.
//!
//! When enabled via `runtimes: lean:`, the compiler auto-installs the Lean 4
//! toolchain (elan/lean/lake), adds Lean-specific domains to the AWF network
//! allowlist, extends the bash command allow-list, and appends a prompt
//! supplement informing the agent that Lean is available.
//!
//! Lean is installed via elan (the Lean toolchain manager) into `$HOME/.elan/bin`,
//! which is mounted read-only into the AWF chroot via the `required_awf_mounts()` mechanism.

pub mod extension;

pub use extension::LeanExtension;

use ado_aw_derive::SanitizeConfig;
use serde::Deserialize;

use crate::sanitize::SanitizeConfig as SanitizeConfigTrait;

/// Lean 4 runtime configuration — accepts both `true` and object formats
///
/// Examples:
/// ```yaml
/// # Simple enablement (installs latest stable toolchain)
/// runtimes:
///   lean: true
///
/// # With options (pin specific toolchain version)
/// runtimes:
///   lean:
///     toolchain: "leanprover/lean4:v4.29.1"
/// ```
#[derive(Debug, Deserialize, Clone)]
#[serde(untagged)]
pub enum LeanRuntimeConfig {
    /// Simple boolean enablement
    Enabled(bool),
    /// Full configuration with options
    WithOptions(LeanOptions),
}

impl LeanRuntimeConfig {
    /// Whether Lean is enabled
    pub fn is_enabled(&self) -> bool {
        match self {
            LeanRuntimeConfig::Enabled(enabled) => *enabled,
            LeanRuntimeConfig::WithOptions(_) => true,
        }
    }

    /// Get the toolchain override (None = use "stable" default)
    pub fn toolchain(&self) -> Option<&str> {
        match self {
            LeanRuntimeConfig::Enabled(_) => None,
            LeanRuntimeConfig::WithOptions(opts) => opts.toolchain.as_deref(),
        }
    }
}

impl SanitizeConfigTrait for LeanRuntimeConfig {
    fn sanitize_config_fields(&mut self) {
        match self {
            LeanRuntimeConfig::Enabled(_) => {}
            LeanRuntimeConfig::WithOptions(opts) => opts.sanitize_config_fields(),
        }
    }
}

/// Lean 4 options
#[derive(Debug, Deserialize, Clone, Default, SanitizeConfig)]
pub struct LeanOptions {
    /// Lean toolchain to install (e.g., "stable", "leanprover/lean4:v4.29.1").
    /// Defaults to "stable" if not specified. If a `lean-toolchain` file exists
    /// in the repository, elan will override to that version automatically.
    #[serde(default)]
    pub toolchain: Option<String>,
}

/// Bash commands that the Lean runtime adds to the allow-list.
pub const LEAN_BASH_COMMANDS: &[&str] = &["lean", "lake", "elan"];
