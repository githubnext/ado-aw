//! Node.js runtime support for the ado-aw compiler.
//!
//! When enabled via `runtimes: node:`, the compiler auto-installs the Node.js
//! toolchain via `NodeTool@0` (the same ADO task used internally by the
//! `gate.js` and `prompt.js` ado-script bundles), adds Node-specific domains
//! to the AWF network allowlist, extends the bash command allow-list, and
//! appends a prompt supplement informing the agent that Node.js is available.
//!
//! Optional `internal-feed` configuration replaces the public npm registry with
//! a private feed (e.g., an Azure Artifacts feed), with optional bearer-token
//! authentication injected from a pipeline variable.

pub mod extension;

pub use extension::NodeExtension;

use ado_aw_derive::SanitizeConfig;
use serde::Deserialize;

use crate::sanitize::SanitizeConfig as SanitizeConfigTrait;

/// Node.js runtime configuration — accepts both `true` and object formats
///
/// Examples:
/// ```yaml
/// # Simple enablement (installs Node.js 20.x LTS)
/// runtimes:
///   node: true
///
/// # Pin to a specific LTS version
/// runtimes:
///   node:
///     version: "22.x"
///
/// # With an internal npm feed
/// runtimes:
///   node:
///     version: "20.x"
///     internal-feed:
///       registry: "https://pkgs.dev.azure.com/ORG/PROJECT/_packaging/FEED/npm/registry/"
///       auth-token-var: "SC_READ_TOKEN"
/// ```
#[derive(Debug, Deserialize, Clone)]
#[serde(untagged)]
pub enum NodeRuntimeConfig {
    /// Simple boolean enablement
    Enabled(bool),
    /// Full configuration with options
    WithOptions(NodeOptions),
}

/// Default Node.js version spec installed when no `version` is specified.
const DEFAULT_NODE_VERSION: &str = "20.x";

impl NodeRuntimeConfig {
    /// Whether the Node.js runtime is enabled.
    pub fn is_enabled(&self) -> bool {
        match self {
            NodeRuntimeConfig::Enabled(enabled) => *enabled,
            NodeRuntimeConfig::WithOptions(_) => true,
        }
    }

    /// The Node.js version spec to install (e.g., `"20.x"`, `"22.x"`).
    /// Defaults to [`DEFAULT_NODE_VERSION`] if not specified.
    pub fn version(&self) -> &str {
        match self {
            NodeRuntimeConfig::Enabled(_) => DEFAULT_NODE_VERSION,
            NodeRuntimeConfig::WithOptions(opts) => {
                opts.version.as_deref().unwrap_or(DEFAULT_NODE_VERSION)
            }
        }
    }

    /// Optional internal npm feed configuration.
    pub fn internal_feed(&self) -> Option<&NodeInternalFeedConfig> {
        match self {
            NodeRuntimeConfig::Enabled(_) => None,
            NodeRuntimeConfig::WithOptions(opts) => opts.internal_feed.as_ref(),
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
    /// Node.js version spec to install (e.g., `"20.x"`, `"22.x"`).
    /// Defaults to `"20.x"` (current LTS) if not specified.
    #[serde(default)]
    pub version: Option<String>,
    /// Optional internal npm feed configuration.
    /// When set, the agent's `npm` commands will use this registry instead
    /// of the default public registry.
    #[serde(default, rename = "internal-feed")]
    pub internal_feed: Option<NodeInternalFeedConfig>,
}

/// Internal npm feed configuration for using private/enterprise registries.
///
/// Example (Azure Artifacts feed):
/// ```yaml
/// runtimes:
///   node:
///     internal-feed:
///       registry: "https://pkgs.dev.azure.com/ORG/PROJECT/_packaging/FEED/npm/registry/"
///       auth-token-var: "SC_READ_TOKEN"
/// ```
#[derive(Debug, Deserialize, Clone, SanitizeConfig)]
pub struct NodeInternalFeedConfig {
    /// The npm registry URL (e.g., `"https://pkgs.dev.azure.com/ORG/PROJECT/_packaging/FEED/npm/registry/"`).
    /// Replaces the public `https://registry.npmjs.org/` for all `npm install` and `npm publish` commands.
    pub registry: String,
    /// Pipeline variable name holding the npm authentication token.
    ///
    /// When provided, the generated step passes the token to `npm config set`
    /// so the registry URL is authenticated. The variable is read at pipeline
    /// runtime via the ADO `$(VAR)` syntax and is never embedded in the
    /// compiled YAML.
    ///
    /// Example: `"SC_READ_TOKEN"` (a pipeline variable configured in the
    /// ADO pipeline settings, typically backed by a service connection PAT).
    #[serde(default, rename = "auth-token-var")]
    pub auth_token_var: Option<String>,
}

/// Bash commands that the Node.js runtime adds to the allow-list.
pub const NODE_BASH_COMMANDS: &[&str] = &["node", "npm", "npx"];

/// Generate the `NodeTool@0` installation step for the Node.js runtime.
///
/// Uses the shared [`crate::compile::extensions::node_tool_step`] helper
/// introduced in ado-aw PR #395, ensuring the version/display-name stay in
/// lockstep with the internal ado-script bundles.
pub fn generate_node_install(config: &NodeRuntimeConfig) -> String {
    crate::compile::extensions::node_tool_step("Install Node.js for agent runtime", config.version())
}

/// Generate an npm registry configuration step for an internal feed.
///
/// Emits a `bash` step that runs `npm config set registry` to redirect all
/// npm commands to the specified private registry. If `auth-token-var` is
/// also configured, the step additionally configures the per-registry
/// `_authToken` so authenticated feeds work without a pre-existing `.npmrc`.
///
/// The auth token is read from the pipeline variable at runtime via the
/// `$(VAR)` ADO syntax — it is never embedded in the compiled YAML.
pub fn generate_node_feed_config(feed: &NodeInternalFeedConfig) -> String {
    let registry = &feed.registry;

    if let Some(token_var) = &feed.auth_token_var {
        // Derive the npm per-registry auth key by stripping the URL scheme.
        // npm expects: //host/path/:_authToken  (no https: or http: prefix)
        let without_scheme = registry
            .strip_prefix("https:")
            .or_else(|| registry.strip_prefix("http:"))
            .unwrap_or(registry.as_str());
        // Strip trailing slashes, then append /:_authToken so the key is
        // always in the canonical form npm expects (one slash before the colon).
        let auth_key = without_scheme.trim_end_matches('/');
        let auth_setting = format!("{auth_key}/:_authToken");

        format!(
            r#"- bash: |
    npm config set registry "{registry}"
    npm config set "{auth_setting}" "$NPM_FEED_TOKEN"
  displayName: "Configure internal npm feed"
  env:
    NPM_FEED_TOKEN: $({token_var})"#
        )
    } else {
        format!(
            r#"- bash: |
    npm config set registry "{registry}"
  displayName: "Configure internal npm feed""#
        )
    }
}
