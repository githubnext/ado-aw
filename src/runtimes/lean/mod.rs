//! Lean 4 runtime support for the ado-aw compiler.
//!
//! When enabled via `runtimes: lean:`, the compiler auto-installs the Lean 4
//! toolchain (elan/lean/lake), adds Lean-specific domains to the AWF network
//! allowlist, extends the bash command allow-list, and appends a prompt
//! supplement informing the agent that Lean is available.
//!
//! Lean is installed via elan (the Lean toolchain manager) into the default
//! `$HOME/.elan` location, and `$HOME/.elan/bin` is added to the pipeline `PATH`
//! via `##vso[task.prependpath]`. AWF captures the host runner's `PATH` (and
//! any entries written to `$GITHUB_PATH`) into `AWF_HOST_PATH`, which the AWF
//! agent entrypoint exports as `PATH` inside the chroot — so the lean/lake
//! wrappers and the toolchain they re-exec into are reachable from the agent
//! sandbox without any extra mounting or symlinking.

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

/// Generate the elan installation step for Lean 4.
///
/// Installs elan (Lean toolchain manager) and the specified toolchain into
/// the default `$HOME/.elan` location, then prepends `$HOME/.elan/bin` to the
/// pipeline `PATH` via `##vso[task.prependpath]`. AWF captures the host
/// runner's `PATH` (and `$GITHUB_PATH` entries) into `AWF_HOST_PATH` and
/// re-exports it as `PATH` inside the chroot, so `lean`/`lake`/`elan` are
/// reachable from inside the agent sandbox without further configuration.
///
/// Defaults to "stable" if no toolchain is specified in the front matter.
pub fn generate_lean_install(config: &LeanRuntimeConfig) -> String {
    let toolchain = config.toolchain().unwrap_or("stable");
    let script = format!(
        "\
# Install elan (Lean toolchain manager) into the default $HOME/.elan location.
# Prepending $HOME/.elan/bin to the pipeline PATH via ##vso[task.prependpath]
# is enough — AWF captures the host PATH into AWF_HOST_PATH and re-exports it
# as PATH inside the agent chroot, so lean/lake/elan are reachable in the
# sandbox without any extra mounting.
curl https://elan.lean-lang.org/elan-init.sh -sSf \\
  | sh -s -- -y --default-toolchain {toolchain}
echo \"##vso[task.prependpath]$HOME/.elan/bin\"
export PATH=\"$HOME/.elan/bin:$PATH\"
lean --version || echo \"Lean installed via elan\"
lake --version || echo \"Lake installed via elan\""
    );
    // Indent each line of the script body by 4 spaces for YAML block scalar
    let indented: String = script
        .lines()
        .map(|line| format!("    {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    format!("- bash: |\n{indented}\n  displayName: \"Install Lean 4 (elan)\"")
}

/// Generate the prompt append step to inform the agent that Lean 4 is available.
pub fn generate_lean_prompt() -> String {
    r#"- bash: |
    cat >> "/tmp/awf-tools/agent-prompt.md" << 'LEAN_PROMPT_EOF'

    ---

    ## Lean 4 Formal Verification

    Lean 4 is installed and available. Use `lean` to typecheck `.lean` files, `lake build` to build Lake projects, and `lake env printPaths` to inspect the toolchain. Lean files use the `.lean` extension.
    LEAN_PROMPT_EOF

    echo "Lean prompt appended"
  displayName: "Append Lean 4 prompt""#
        .to_string()
}
