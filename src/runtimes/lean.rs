//! Lean 4 runtime support for the ado-aw compiler.
//!
//! When enabled via `runtimes: lean:`, the compiler auto-installs the Lean 4
//! toolchain (elan/lean/lake), adds Lean-specific domains to the AWF network
//! allowlist, extends the bash command allow-list, and appends a prompt
//! supplement informing the agent that Lean is available.
//!
//! Lean is installed via elan (the Lean toolchain manager) into `$HOME/.elan/bin`,
//! then symlinked into `/tmp/awf-tools/` for AWF chroot compatibility.

use serde::Deserialize;

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

/// Lean 4 options
#[derive(Debug, Deserialize, Clone, Default)]
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
/// Installs elan (Lean toolchain manager) and the specified toolchain.
/// Defaults to "stable" if no toolchain is specified in the front matter.
/// Symlinks lean tools into `/tmp/awf-tools/` for AWF chroot compatibility.
pub fn generate_lean_install(config: &LeanRuntimeConfig) -> String {
    let toolchain = config.toolchain().unwrap_or("stable");
    let script = format!(
        "\
curl https://elan.lean-lang.org/elan-init.sh -sSf | sh -s -- -y --default-toolchain {toolchain}
echo \"##vso[task.prependpath]$HOME/.elan/bin\"
export PATH=\"$HOME/.elan/bin:$PATH\"
lean --version || echo \"Lean installed via elan\"
lake --version || echo \"Lake installed via elan\"
# Symlink lean tools into /tmp/awf-tools/ so they are accessible
# inside the AWF chroot (AWF mounts /tmp but reconstructs PATH
# from standard system locations, excluding $HOME/.elan/bin).
for cmd in lean lake elan; do
  if command -v \"$cmd\" >/dev/null 2>&1; then
    ln -sf \"$(command -v \"$cmd\")\" \"/tmp/awf-tools/$cmd\"
  fi
done
echo \"Lean tools symlinked to /tmp/awf-tools/\""
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
