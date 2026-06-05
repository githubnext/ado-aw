//! Node.js runtime support for the ado-aw compiler.
//!
//! When enabled via `runtimes: node:`, the compiler auto-installs a specific
//! Node.js version via `UseNode@1`, emits `npmAuthenticate@0` for internal
//! feed access, adds Node ecosystem domains to the AWF network allowlist,
//! extends the bash command allow-list, and optionally injects feed URL env
//! vars for npm.
//!
//! No AWF mounts or PATH prepends are needed because `UseNode@1` installs
//! to `/opt/hostedtoolcache` (already mounted read-only by AWF) and publishes
//! `##vso[task.prependpath]` entries that AWF merges via `$GITHUB_PATH`.
//!
//! This module generates `UseNode@1` YAML inline rather than importing
//! the `node_tool_step()` helper from `compile/extensions/mod.rs`, keeping
//! the runtime decoupled from the ado-script infrastructure.

pub mod extension;

pub use extension::NodeExtension;

use ado_aw_derive::SanitizeConfig;
use serde::Deserialize;

use crate::sanitize::SanitizeConfig as SanitizeConfigTrait;

/// Node.js runtime configuration — accepts both `true` and object formats.
///
/// Examples:
/// ```yaml
/// # Simple enablement (installs default Node LTS)
/// runtimes:
///   node: true
///
/// # With options (pin version, configure feed)
/// runtimes:
///   node:
///     version: "22.x"
///     feed-url: "https://pkgs.dev.azure.com/ORG/PROJECT/_packaging/FEED/npm/registry/"
/// ```
#[derive(Debug, Deserialize, Clone)]
#[serde(untagged)]
pub enum NodeRuntimeConfig {
    /// Simple boolean enablement
    Enabled(bool),
    /// Full configuration with options
    WithOptions(NodeOptions),
}

impl NodeRuntimeConfig {
    /// Whether Node.js is enabled.
    pub fn is_enabled(&self) -> bool {
        match self {
            NodeRuntimeConfig::Enabled(enabled) => *enabled,
            NodeRuntimeConfig::WithOptions(_) => true,
        }
    }

    /// Get the Node.js version (None = use ADO default).
    pub fn version(&self) -> Option<&str> {
        match self {
            NodeRuntimeConfig::Enabled(_) => None,
            NodeRuntimeConfig::WithOptions(opts) => opts.version.as_deref(),
        }
    }

    /// Get the npm registry feed URL (None = use public npmjs).
    pub fn feed_url(&self) -> Option<&str> {
        match self {
            NodeRuntimeConfig::Enabled(_) => None,
            NodeRuntimeConfig::WithOptions(opts) => opts.feed_url.as_deref(),
        }
    }

    /// Get the config file path (None = not set).
    pub fn config(&self) -> Option<&str> {
        match self {
            NodeRuntimeConfig::Enabled(_) => None,
            NodeRuntimeConfig::WithOptions(opts) => opts.config.as_deref(),
        }
    }
}

impl SanitizeConfigTrait for NodeRuntimeConfig {
    fn sanitize_config_fields(&mut self) {
        match self {
            NodeRuntimeConfig::Enabled(_) => {}
            NodeRuntimeConfig::WithOptions(opts) => opts.sanitize_config_fields(),
        }
    }
}

/// Node.js runtime options.
#[derive(Debug, Deserialize, Clone, Default, SanitizeConfig)]
pub struct NodeOptions {
    /// Node.js version to install (e.g., "22.x", "20.x").
    /// Passed to `UseNode@1` `version`.
    #[serde(default)]
    pub version: Option<String>,

    /// Internal npm registry URL. When set, the compiler injects
    /// `NPM_CONFIG_REGISTRY` env var into the agent environment so npm
    /// uses this feed without .npmrc changes (which would conflict with
    /// AWF's credential overlay of `~/.npmrc`).
    #[serde(default, rename = "feed-url")]
    pub feed_url: Option<String>,

    /// Path to an .npmrc config file. Currently recognized but not yet
    /// supported — specifying this field produces a compile error.
    /// Reserved for future proxy-auth integration (gh-aw-firewall#2547).
    #[serde(default)]
    pub config: Option<String>,
}

/// Bash commands that the Node.js runtime adds to the allow-list.
pub const NODE_BASH_COMMANDS: &[&str] = &["node", "npm", "npx"];

/// Generate the `UseNode@1` pipeline step (inline, decoupled from ado-script).
pub fn generate_node_install(config: &NodeRuntimeConfig) -> String {
    let version = config.version().unwrap_or("22.x");
    format!(
        "\
- task: UseNode@1
  inputs:
    version: '{version}'
  displayName: 'Install Node.js {version}'"
    )
}

/// Generate the `npmAuthenticate@0` pipeline step.
///
/// Emitted when `feed-url:` or `config:` is set, authenticating the ADO
/// build service identity for internal npm feeds. This runs before AWF.
///
/// Requires a `.npmrc` file to exist; call [`generate_ensure_npmrc`] first
/// to create one if the repo doesn't already have one.
pub fn generate_npm_authenticate() -> String {
    "\
- task: npmAuthenticate@0
  inputs:
    workingFile: .npmrc
  displayName: 'Authenticate npm (build service identity)'"
        .to_string()
}

/// Generate a step that ensures `.npmrc` exists before `npmAuthenticate@0`.
///
/// `npmAuthenticate@0` requires `workingFile:` to point at an existing file —
/// unlike `PipAuthenticate@1` it fails if the file is missing. This step
/// creates a minimal `.npmrc` (with the configured registry or the default
/// npmjs registry) only when one doesn't already exist, preserving any
/// repo-checked-in `.npmrc`.
pub fn generate_ensure_npmrc(config: &NodeRuntimeConfig) -> String {
    let registry = config
        .feed_url()
        .unwrap_or("https://registry.npmjs.org/");

    format!(
        r#"- bash: |
    set -eo pipefail
    if [ ! -f .npmrc ]; then
      echo 'registry={registry}' > .npmrc
      echo 'Created .npmrc with registry={registry}'
    else
      echo '.npmrc already exists, skipping creation'
    fi
  displayName: 'Ensure .npmrc exists'"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_node_install_default_version() {
        let config = NodeRuntimeConfig::Enabled(true);
        let step = generate_node_install(&config);
        assert!(
            step.contains("version: '22.x'"),
            "should default to 22.x, got: {step}"
        );
        assert!(step.contains("UseNode@1"));
        assert!(step.contains("Install Node.js 22.x"));
    }

    #[test]
    fn test_generate_node_install_pinned_version() {
        let config = NodeRuntimeConfig::WithOptions(NodeOptions {
            version: Some("20.x".into()),
            ..Default::default()
        });
        let step = generate_node_install(&config);
        assert!(
            step.contains("version: '20.x'"),
            "should use pinned version, got: {step}"
        );
        assert!(step.contains("Install Node.js 20.x"));
    }

    #[test]
    fn test_generate_npm_authenticate_emits_task() {
        let step = generate_npm_authenticate();
        assert!(step.contains("npmAuthenticate@0"));
        assert!(step.contains("workingFile: .npmrc"));
    }

    #[test]
    fn test_generate_ensure_npmrc_default_registry() {
        let config = NodeRuntimeConfig::Enabled(true);
        let step = generate_ensure_npmrc(&config);
        assert!(
            step.contains("https://registry.npmjs.org/"),
            "should fallback to npm registry, got: {step}"
        );
        assert!(step.contains("Ensure .npmrc exists"));
    }

    #[test]
    fn test_generate_ensure_npmrc_custom_feed_url() {
        let custom = "https://pkgs.dev.azure.com/myorg/_packaging/myfeed/npm/registry/";
        let config = NodeRuntimeConfig::WithOptions(NodeOptions {
            feed_url: Some(custom.into()),
            ..Default::default()
        });
        let step = generate_ensure_npmrc(&config);
        assert!(
            step.contains("pkgs.dev.azure.com"),
            "should use custom feed URL, got: {step}"
        );
        assert!(
            !step.contains("https://registry.npmjs.org/"),
            "should not fall back to default when custom feed is set, got: {step}"
        );
    }
}
