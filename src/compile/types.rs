//! Common types for the agentic pipeline compiler.
//!
//! This module defines the front matter grammar that is shared across all compile targets.

use crate::sanitize::SanitizeConfig as SanitizeConfigTrait;
use ado_aw_derive::SanitizeConfig;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::OnceLock;

/// Shared empty overrides map returned by `PoolConfig::overrides()` and
/// `FrontMatter::pool_overrides()` when no per-job overrides are configured.
static EMPTY_OVERRIDES: OnceLock<HashMap<String, PoolConfig>> = OnceLock::new();

/// Target platform for compiled pipeline
#[derive(Debug, Deserialize, Clone, Default, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum CompileTarget {
    /// Standalone pipeline with full feature set (default)
    #[default]
    Standalone,
    /// 1ES Pipeline Template integration using agencyJob
    #[serde(rename = "1es")]
    OneES,
    /// Job-level ADO template: produces `jobs:` at root for inclusion in existing pipelines
    Job,
    /// Stage-level ADO template: produces `stages:` wrapping jobs for multi-stage pipelines
    Stage,
}

impl CompileTarget {
    /// Canonical lowercase string for this target (matches the serde rename).
    /// Used by the always-on ado-aw-marker extension when emitting the
    /// machine-readable metadata blob.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Standalone => "standalone",
            Self::OneES => "1es",
            Self::Job => "job",
            Self::Stage => "stage",
        }
    }
}

/// Pool configuration — object form only.
///
/// The typed layer accepts only the object form. The legacy scalar shorthand
/// (`pool: MySelfHostedPool`) is still valid *source* input: codemod
/// `0002_pool_object_form` rewrites it to `pool: { name: MySelfHostedPool }`
/// before typed deserialization is reached, so a bare string never lands here.
///
/// Examples:
/// ```yaml
/// # Microsoft-hosted
/// pool:
///   vmImage: ubuntu-22.04
///
/// # Self-hosted
/// pool:
///   name: MySelfHostedPool
///   demands:
///     - CustomCapability -equals required-value
///
/// # 1ES object format
/// pool:
///   name: AZS-1ES-L-MMS-ubuntu-22.04
///   os: linux
/// ```
#[derive(Debug, Deserialize, Clone, Default)]
pub struct PoolConfig {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default, rename = "vmImage")]
    pub vm_image: Option<String>,
    #[serde(default)]
    pub os: Option<String>,
    #[serde(default)]
    pub demands: Vec<String>,
    /// Per-job pool overrides — optional map of canonical job name to
    /// [`PoolConfig`]. Any entry replaces the resolved default `pool:` for
    /// exactly that job; unspecified jobs inherit `pool:`.
    ///
    /// Only the object form is accepted here (a bare pool-name string is not
    /// supported; use `name:` instead). The top-level `pool:` legacy string is
    /// handled by a codemod before compilation reaches this point.
    ///
    /// Valid keys: `setup`, `agent`, `detection`, `safe-outputs`,
    /// `safe-outputs-reviewed`, `teardown`, `conclusion`.
    /// `manual-review` is always rejected (agentless job, fixed to `pool: server`).
    /// Unknown keys emit a compiler warning and are ignored for forward-compat.
    ///
    /// Not supported for `target: 1es`; specifying it there is a compile-time
    /// error.
    ///
    /// See `docs/front-matter.md` for the full reference.
    #[serde(default)]
    pub overrides: HashMap<String, PoolConfig>,
}

impl PoolConfig {
    /// Get the self-hosted pool name, if configured.
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// Get the Microsoft-hosted VM image, if configured.
    pub fn vm_image(&self) -> Option<&str> {
        self.vm_image.as_deref()
    }

    /// Get the ordered Azure Pipelines demands list.
    pub fn demands(&self) -> &[String] {
        &self.demands
    }

    /// Get the OS (defaults to "linux" if not specified).
    ///
    /// Primarily applicable to 1ES pool configuration.
    pub fn os(&self) -> &str {
        self.os.as_deref().unwrap_or("linux")
    }

    /// Get the per-job pool overrides map.
    pub fn overrides(&self) -> &HashMap<String, PoolConfig> {
        &self.overrides
    }
}

impl SanitizeConfigTrait for PoolConfig {
    fn sanitize_config_fields(&mut self) {
        self.name = self.name.as_deref().map(crate::sanitize::sanitize_config);
        self.vm_image = self.vm_image.as_deref().map(crate::sanitize::sanitize_config);
        self.os = self.os.as_deref().map(crate::sanitize::sanitize_config);
        self.demands = self
            .demands
            .iter()
            .map(|s| crate::sanitize::sanitize_config(s))
            .collect();
        for pool in self.overrides.values_mut() {
            pool.sanitize_config_fields();
        }
    }
}

/// Schedule configuration - accepts both string and object formats
///
/// Examples:
/// ```yaml
/// # Simple string format (defaults to main branch only)
/// schedule: daily around 14:00
///
/// # Object format (with custom branch filtering)
/// schedule:
///   run: daily around 14:00
///   branches:
///     - main
///     - release/*
/// ```
#[derive(Debug, Deserialize, Clone)]
#[serde(untagged)]
pub enum ScheduleConfig {
    /// Simple schedule expression string
    Simple(String),
    /// Schedule with options (branch filtering)
    WithOptions(ScheduleOptions),
}

impl ScheduleConfig {
    /// Get the schedule expression string
    pub fn expression(&self) -> &str {
        match self {
            ScheduleConfig::Simple(s) => s,
            ScheduleConfig::WithOptions(opts) => &opts.run,
        }
    }

    /// Get the branches filter (empty means default to "main" branch)
    pub fn branches(&self) -> &[String] {
        match self {
            ScheduleConfig::Simple(_) => &[],
            ScheduleConfig::WithOptions(opts) => &opts.branches,
        }
    }
}

impl SanitizeConfigTrait for ScheduleConfig {
    fn sanitize_config_fields(&mut self) {
        match self {
            ScheduleConfig::Simple(s) => *s = crate::sanitize::sanitize_config(s),
            ScheduleConfig::WithOptions(opts) => opts.sanitize_config_fields(),
        }
    }
}

#[derive(Debug, Deserialize, Clone, SanitizeConfig)]
pub struct ScheduleOptions {
    /// Fuzzy schedule expression (e.g., "daily around 14:00")
    pub run: String,
    /// Branches to restrict the schedule to (empty = defaults to "main")
    #[serde(default)]
    pub branches: Vec<String>,
}

/// Engine configuration — aligned with gh-aw's engine front matter.
///
/// The string form is an engine identifier (e.g., `copilot`). The object form
/// uses `id` for the engine identifier plus additional options.
///
/// Currently only `copilot` (GitHub Copilot CLI) is supported. Other engine
/// identifiers produce a compile error.
///
/// Examples:
/// ```yaml
/// # Simple string format (engine identifier, defaults to copilot)
/// engine: copilot
///
/// # Object format (with additional options)
/// engine:
///   id: copilot
///   model: claude-opus-4.7
///   timeout-minutes: 30
///   version: latest
///   agent: my-custom-agent
///   api-target: api.acme.ghe.com
///   args: ["--verbose"]
///   env:
///     DEBUG_MODE: "true"
///   command: /usr/local/bin/copilot
/// ```
#[derive(Debug, Deserialize, Clone)]
#[serde(untagged)]
pub enum EngineConfig {
    /// Engine identifier string (e.g., "copilot")
    Simple(String),
    /// Full engine configuration object
    Full(Box<EngineOptions>),
}

impl Default for EngineConfig {
    fn default() -> Self {
        EngineConfig::Simple("copilot".to_string())
    }
}

impl EngineConfig {
    /// Get the engine identifier (e.g., "copilot").
    pub fn engine_id(&self) -> &str {
        match self {
            EngineConfig::Simple(s) => s,
            EngineConfig::Full(opts) => opts.id.as_deref().unwrap_or("copilot"),
        }
    }

    /// Get the model name override, if specified.
    /// Returns `None` when the engine should use its default model.
    pub fn model(&self) -> Option<&str> {
        match self {
            EngineConfig::Simple(_) => None,
            EngineConfig::Full(opts) => opts.model.as_deref(),
        }
    }

    /// Get the timeout in minutes
    pub fn timeout_minutes(&self) -> Option<u32> {
        match self {
            EngineConfig::Simple(_) => None,
            EngineConfig::Full(opts) => opts.timeout_minutes,
        }
    }

    /// Get the engine version override (e.g., "0.0.422", "latest")
    pub fn version(&self) -> Option<&str> {
        match self {
            EngineConfig::Simple(_) => None,
            EngineConfig::Full(opts) => opts.version.as_deref(),
        }
    }

    /// Get the custom agent file identifier (Copilot only, e.g., "my-agent")
    pub fn agent(&self) -> Option<&str> {
        match self {
            EngineConfig::Simple(_) => None,
            EngineConfig::Full(opts) => opts.agent.as_deref(),
        }
    }

    /// Get the custom API endpoint hostname (GHEC/GHES)
    pub fn api_target(&self) -> Option<&str> {
        match self {
            EngineConfig::Simple(_) => None,
            EngineConfig::Full(opts) => opts.api_target.as_deref(),
        }
    }

    /// Get custom CLI arguments
    pub fn args(&self) -> &[String] {
        match self {
            EngineConfig::Simple(_) => &[],
            EngineConfig::Full(opts) => &opts.args,
        }
    }

    /// Get custom environment variables
    pub fn env(&self) -> Option<&HashMap<String, String>> {
        match self {
            EngineConfig::Simple(_) => None,
            EngineConfig::Full(opts) => opts.env.as_ref(),
        }
    }

    /// Get custom engine command path
    pub fn command(&self) -> Option<&str> {
        match self {
            EngineConfig::Simple(_) => None,
            EngineConfig::Full(opts) => opts.command.as_deref(),
        }
    }

    /// Get the GitHub App token configuration, if specified.
    /// Returns `None` when the engine uses the default `$(GITHUB_TOKEN)`
    /// pipeline-variable source.
    pub fn github_app_token(&self) -> Option<&GithubAppTokenConfig> {
        match self {
            EngineConfig::Simple(_) => None,
            EngineConfig::Full(opts) => opts.github_app_token.as_ref(),
        }
    }

    /// Get the external model-provider (BYOK) configuration, if specified.
    pub fn provider(&self) -> Option<&ProviderConfig> {
        match self {
            EngineConfig::Simple(_) => None,
            EngineConfig::Full(opts) => opts.provider.as_ref(),
        }
    }
}

impl SanitizeConfigTrait for EngineConfig {
    fn sanitize_config_fields(&mut self) {
        match self {
            EngineConfig::Simple(s) => *s = crate::sanitize::sanitize_config(s),
            EngineConfig::Full(opts) => opts.sanitize_config_fields(),
        }
    }
}

#[derive(Debug, Deserialize, Clone, SanitizeConfig)]
pub struct EngineOptions {
    /// Engine identifier (e.g., "copilot"). Defaults to "copilot" when omitted.
    #[serde(default)]
    pub id: Option<String>,
    /// AI model to use (engine-specific default when omitted)
    #[serde(default)]
    pub model: Option<String>,
    /// Engine CLI version to install (e.g., "0.0.422", "latest")
    #[serde(default)]
    pub version: Option<String>,
    /// Custom agent file identifier (Copilot only — references .github/agents/)
    #[serde(default)]
    pub agent: Option<String>,
    /// Custom API endpoint hostname (GHEC/GHES, e.g., "api.acme.ghe.com")
    #[serde(default, rename = "api-target")]
    pub api_target: Option<String>,
    /// Custom CLI arguments injected before the prompt
    #[serde(default)]
    pub args: Vec<String>,
    /// Engine-specific environment variables
    #[serde(default)]
    pub env: Option<HashMap<String, String>>,
    /// Custom engine executable path (skips default installation)
    #[serde(default)]
    pub command: Option<String>,
    /// Workflow timeout in minutes
    #[serde(default, rename = "timeout-minutes")]
    pub timeout_minutes: Option<u32>,
    /// GitHub App-backed Copilot engine authentication (Copilot only).
    ///
    /// When set, the compiler emits a token-mint step (the
    /// `github-app-token` ado-script bundle) immediately before the Copilot
    /// invocation in the Agent and Detection jobs. The minted GitHub App
    /// installation token is wired into `GITHUB_TOKEN` for the Copilot engine
    /// env only — never SafeOutputs, user steps, ManualReview, Teardown, or
    /// Conclusion. Absent ⇒ `GITHUB_TOKEN` is sourced from the
    /// `$(GITHUB_TOKEN)` pipeline variable as before.
    #[serde(default, rename = "github-app-token")]
    #[sanitize_config(skip)]
    pub github_app_token: Option<GithubAppTokenConfig>,
    /// External model-provider (BYOK) configuration (Copilot only).
    ///
    /// A dedicated, typed block that owns the Copilot **model provider**
    /// settings and maps them to the correct `COPILOT_PROVIDER_*` environment
    /// variables. When `provider.token` is set, the compiler mints the provider
    /// bearer token **in the same job** as each engine run (Agent + Detection)
    /// via `AzureCLI@2` + a service connection — so the token resolves at
    /// runtime without any cross-job output plumbing. See docs/engine.md.
    #[serde(default)]
    #[sanitize_config(skip)]
    pub provider: Option<ProviderConfig>,
}

/// GitHub App-backed Copilot engine authentication configuration.
///
/// Mirrors gh-aw's `create-github-app-token` model, adapted to Azure DevOps.
/// The **App ID** (`app-id`) is a literal, non-secret value (a numeric App ID
/// or an alphanumeric client ID) — like `owner`, it is written verbatim. Only
/// the **private key** is secret: `private-key` names an ADO **secret** pipeline
/// variable (set via `ado-aw secrets set`), defaulting to
/// `GITHUB_APP_PRIVATE_KEY`, so the key material never appears in the source or
/// the generated lock.
///
/// ```yaml
/// engine:
///   id: copilot
///   github-app-token:
///     app-id: 1234567            # literal App ID or client ID (required)
///     owner: octo-org            # installation owner (org or user)
///     repositories: [octo-repo]  # optional; scopes the installation token
///     # private-key: MY_SECRET   # optional; defaults to GITHUB_APP_PRIVATE_KEY
/// ```
#[derive(Debug, Deserialize, Clone, SanitizeConfig)]
pub struct GithubAppTokenConfig {
    /// The GitHub App ID — a **literal** value, either a numeric App ID
    /// (e.g. `1234567`, quoted or unquoted) or an alphanumeric client ID
    /// (e.g. `Iv23liABC…`). The App ID is not secret (it is visible in the
    /// App's settings and is the JWT `iss`), so it is plain per-app config like
    /// `owner` — it is emitted verbatim, never indirected through a variable.
    #[serde(rename = "app-id", deserialize_with = "de_string_or_number")]
    pub app_id: String,
    /// Optional name of the ADO **secret** pipeline variable holding the GitHub
    /// App private key (PEM). Defaults to
    /// [`DEFAULT_GITHUB_APP_PRIVATE_KEY_VAR`] when omitted — the compiler owns
    /// the variable name, exactly like `GITHUB_TOKEN`, so the common case just
    /// runs `ado-aw secrets set GITHUB_APP_PRIVATE_KEY …`. Set this only to
    /// point at a differently-named secret, including hyphenated Key Vault
    /// secret names surfaced through ADO variable groups. The key material is
    /// never inlined.
    #[serde(default, rename = "private-key")]
    pub private_key: Option<String>,
    /// GitHub installation owner (organization or user login) the App is
    /// installed on.
    pub owner: String,
    /// Optional list of repository names (owner-relative) the installation
    /// token should be scoped to. Empty ⇒ token spans all repositories the
    /// installation grants.
    #[serde(default)]
    pub repositories: Vec<String>,
    /// Optional GitHub API base URL. Defaults to `https://api.github.com`
    /// (GHEC). For GitHub Enterprise Server, set the `/api/v3` base URL
    /// (e.g. `https://ghe.example.com/api/v3`). Must be an `https://` URL.
    #[serde(default, rename = "api-url")]
    pub api_url: Option<String>,
    /// When true, skip revoking the installation token after the Copilot run.
    /// By default (false) the compiler emits a best-effort post-run step that
    /// deletes the token (`DELETE /installation/token`) so it does not remain
    /// valid for its full ~1h lifetime.
    #[serde(default, rename = "skip-token-revocation")]
    pub skip_token_revocation: bool,
}

/// Default name of the ADO secret pipeline variable holding the GitHub App
/// private key when `engine.github-app-token.private-key` is omitted. The
/// compiler owns this name (mirroring the fixed `GITHUB_TOKEN` contract) so the
/// common case needs no `private-key` line — set the value with
/// `ado-aw secrets set GITHUB_APP_PRIVATE_KEY …`.
pub const DEFAULT_GITHUB_APP_PRIVATE_KEY_VAR: &str = "GITHUB_APP_PRIVATE_KEY";

/// Deserialize a scalar that may be a YAML string **or** an integer into a
/// `String`. Used for `github-app-token.app-id` so an unquoted numeric App ID
/// (`app-id: 1234567`) is accepted alongside a quoted string or client ID.
fn de_string_or_number<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct StringOrNumber;
    impl serde::de::Visitor<'_> for StringOrNumber {
        type Value = String;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("a GitHub App ID (numeric) or client ID (string)")
        }
        fn visit_str<E>(self, v: &str) -> Result<String, E> {
            Ok(v.to_string())
        }
        fn visit_string<E>(self, v: String) -> Result<String, E> {
            Ok(v)
        }
        fn visit_u64<E>(self, v: u64) -> Result<String, E> {
            Ok(v.to_string())
        }
        fn visit_i64<E>(self, v: i64) -> Result<String, E> {
            Ok(v.to_string())
        }
    }
    deserializer.deserialize_any(StringOrNumber)
}

impl GithubAppTokenConfig {
    /// The ADO secret pipeline-variable name that holds the private key — the
    /// `private-key` override if set, else [`DEFAULT_GITHUB_APP_PRIVATE_KEY_VAR`].
    pub fn private_key_var(&self) -> &str {
        self.private_key
            .as_deref()
            .unwrap_or(DEFAULT_GITHUB_APP_PRIVATE_KEY_VAR)
    }

    /// Validate the literal App ID, the optional `private-key` override variable
    /// name, the GitHub owner/repository name segments, and the optional API
    /// URL. `app-id` must be a non-empty `[A-Za-z0-9._-]` literal (covers
    /// numeric App IDs and alphanumeric/`Iv1.`-style client IDs);
    /// `private-key` (when set) must be a valid ADO variable name; `owner` and
    /// each `repositories` entry must be a single safe path segment; `api-url`
    /// (when set) must be an `https://` URL with a host.
    pub fn validate(&self) -> anyhow::Result<()> {
        use crate::validate::{is_safe_path_segment, is_valid_ado_variable_name};
        if self.app_id.is_empty()
            || !self.app_id.starts_with(|c: char| c.is_ascii_alphanumeric())
            || !self
                .app_id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        {
            anyhow::bail!(
                "engine.github-app-token.app-id '{}' must be a non-empty GitHub App ID \
                 (numeric, e.g. 1234567) or client ID (e.g. Iv23liABC): it must start with \
                 an alphanumeric character and contain only [A-Za-z0-9._-]. It is a literal \
                 value, not a variable name (a leading '-', e.g. a negative number, is invalid).",
                self.app_id
            );
        }
        if let Some(private_key) = &self.private_key
            && !is_valid_ado_variable_name(private_key)
        {
            anyhow::bail!(
                "engine.github-app-token.private-key '{}' must be an ADO variable name \
                 starting with a letter, digit, or '_' and containing only letters, digits, \
                 '.', '_', or '-' (it names the secret variable holding the PEM; omit it to \
                 use the default '{}').",
                private_key,
                DEFAULT_GITHUB_APP_PRIVATE_KEY_VAR
            );
        }
        if !is_safe_path_segment(&self.owner) {
            anyhow::bail!(
                "engine.github-app-token.owner '{}' is not a valid GitHub owner name \
                 (allowed: [A-Za-z0-9._-], no '/', no leading '.').",
                self.owner
            );
        }
        for repo in &self.repositories {
            if !is_safe_path_segment(repo) {
                anyhow::bail!(
                    "engine.github-app-token.repositories entry '{}' is not a valid \
                     GitHub repository name (allowed: [A-Za-z0-9._-], no '/', no \
                     leading '.').",
                    repo
                );
            }
        }
        if let Some(api_url) = &self.api_url {
            let parsed = url::Url::parse(api_url).map_err(|e| {
                anyhow::anyhow!(
                    "engine.github-app-token.api-url '{}' is not a valid URL: {}",
                    api_url,
                    e
                )
            })?;
            if parsed.scheme() != "https" || parsed.host_str().is_none() {
                anyhow::bail!(
                    "engine.github-app-token.api-url '{}' must be an https:// URL with a host \
                     (e.g. https://ghe.example.com/api/v3).",
                    api_url
                );
            }
        }
        Ok(())
    }
}

/// Internal same-job secret pipeline-variable name that the provider
/// token-mint step sets and the engine env references. Compiler-owned (like
/// `GITHUB_TOKEN`) so it never collides with user config; masked via
/// `issecret=true`.
pub const PROVIDER_BEARER_TOKEN_VAR: &str = "AW_PROVIDER_BEARER_TOKEN";

/// Default Azure resource (audience) for `az account get-access-token` when
/// `provider.token.resource` is omitted — the Azure AI Foundry / Cognitive
/// Services audience.
pub const DEFAULT_PROVIDER_TOKEN_RESOURCE: &str = "https://cognitiveservices.azure.com";

/// Copilot provider wire format — maps to `COPILOT_PROVIDER_TYPE`.
#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ProviderType {
    /// OpenAI-compatible (`openai`) — the Copilot CLI default.
    Openai,
    /// Azure OpenAI / Azure AI Foundry (`azure`).
    Azure,
    /// Anthropic (`anthropic`).
    Anthropic,
}

impl ProviderType {
    /// The exact `COPILOT_PROVIDER_TYPE` value.
    pub fn as_ado_str(&self) -> &'static str {
        match self {
            ProviderType::Openai => "openai",
            ProviderType::Azure => "azure",
            ProviderType::Anthropic => "anthropic",
        }
    }
}

/// Copilot provider wire-API variant — maps to `COPILOT_PROVIDER_WIRE_API`.
#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum WireApi {
    /// Chat-completions wire API (`completions`) — the default.
    Completions,
    /// Responses wire API (`responses`).
    Responses,
}

impl WireApi {
    /// The exact `COPILOT_PROVIDER_WIRE_API` value.
    pub fn as_ado_str(&self) -> &'static str {
        match self {
            WireApi::Completions => "completions",
            WireApi::Responses => "responses",
        }
    }
}

/// Compiler-owned provider credential acquisition via Azure CLI.
///
/// When set, the compiler emits an in-job `AzureCLI@2` step (authenticated by
/// the ARM `service-connection`) that runs `az account get-access-token` and
/// sets the same-job secret [`PROVIDER_BEARER_TOKEN_VAR`], which is wired into
/// **`COPILOT_PROVIDER_API_KEY`** (the credential env var the AWF api-proxy
/// sidecar reads and forwards as `Authorization: Bearer <value>` — there is no
/// `COPILOT_PROVIDER_BEARER_TOKEN` in the sidecar). Because the token is minted
/// in the same job as the engine run, it resolves via a plain `$(...)` macro —
/// no cross-job output plumbing (the failure mode in #1372).
#[derive(Debug, Deserialize, Clone, SanitizeConfig)]
#[serde(deny_unknown_fields)]
pub struct ProviderToken {
    /// ARM service connection used to authenticate `az` before minting the
    /// token. Validated at deserialization.
    #[serde(rename = "service-connection")]
    #[sanitize_config(skip)]
    pub service_connection: crate::secure::ServiceConnection,
    /// Azure resource (audience) passed to `az account get-access-token
    /// --resource`. Defaults to [`DEFAULT_PROVIDER_TOKEN_RESOURCE`]. Validated
    /// at deserialization to be shell-safe (it is interpolated into the
    /// generated mint script).
    #[serde(default)]
    #[sanitize_config(skip)]
    pub resource: Option<crate::secure::ProviderResourceUrl>,
}

impl ProviderToken {
    /// The effective resource/audience — the `resource` override if set, else
    /// [`DEFAULT_PROVIDER_TOKEN_RESOURCE`].
    pub fn resource(&self) -> &str {
        self.resource
            .as_deref()
            .unwrap_or(DEFAULT_PROVIDER_TOKEN_RESOURCE)
    }
}

/// External model-provider (BYOK) configuration under `engine.provider`.
///
/// Maps to the `COPILOT_PROVIDER_*` environment variables the Copilot CLI reads
/// to route model requests to an external provider (e.g. Azure AI Foundry).
/// This is the sanctioned surface; raw `engine.env COPILOT_PROVIDER_*` keys stay
/// supported for back-compat but must not be combined with this block.
///
/// **Field validation strategy (parse-don't-validate, AGENTS.md §5):**
/// `base_url` and `resource` are validated newtypes checked at *deserialization*
/// time, so a malformed value is rejected structurally before any codegen. The
/// `resource` newtype's checks are strictest (it is shell-interpolated into the
/// mint script); `base_url` accepts a literal https URL or an `$(VAR)` macro.
/// `api_key` stays a raw `String` validated in [`ProviderConfig::validate`]:
/// its rules are secret-macro-specific (non-empty + no injection), it is neither
/// shell-interpolated nor host-extracted, and every compile path runs `validate`
/// via `validate_engine_feature_support` before the value is used.
#[derive(Debug, Deserialize, Clone, SanitizeConfig)]
#[serde(deny_unknown_fields)]
pub struct ProviderConfig {
    /// Provider base URL → `COPILOT_PROVIDER_BASE_URL`. Required; activates BYOK
    /// routing. A literal host is auto-added to the AWF network allowlist.
    /// Validated at deserialization (literal https URL or `$(VAR)` macro).
    #[serde(rename = "base-url")]
    #[sanitize_config(skip)]
    pub base_url: crate::secure::ProviderBaseUrl,
    /// Provider wire format → `COPILOT_PROVIDER_TYPE` (optional).
    #[serde(default, rename = "type")]
    pub provider_type: Option<ProviderType>,
    /// Wire-API variant → `COPILOT_PROVIDER_WIRE_API` (optional).
    #[serde(default, rename = "wire-api")]
    pub wire_api: Option<WireApi>,
    /// Compiler-owned credential acquisition (Azure CLI + service connection);
    /// the minted AAD token is wired into `COPILOT_PROVIDER_API_KEY`.
    /// Mutually exclusive with [`ProviderConfig::api_key`].
    #[serde(default)]
    #[sanitize_config(skip)]
    pub token: Option<ProviderToken>,
    /// Static API key → `COPILOT_PROVIDER_API_KEY` (optional). Expected to be a
    /// single-name secret macro `$(VAR)`. Mutually exclusive with `token`.
    #[serde(default, rename = "api-key")]
    pub api_key: Option<String>,
}

impl ProviderConfig {
    /// Cross-field / value validation. `base_url` is already structurally valid
    /// (its newtype validates at deserialization); this enforces the
    /// `token` XOR `api-key` rule and the `api-key` value checks. Engine-wide
    /// checks (Copilot-only gating, `engine.env` conflict) live in
    /// `crate::engine::validate_engine_feature_support`.
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.token.is_some() && self.api_key.is_some() {
            anyhow::bail!(
                "engine.provider sets both `token` and `api-key`; they are mutually \
                 exclusive. Use `token` (compiler-minted bearer via a service connection) \
                 OR `api-key` (a static `$(VAR)` secret)."
            );
        }
        // `api-key` is rendered verbatim into COPILOT_PROVIDER_API_KEY. Like the
        // raw engine.env provider-key path it may carry an ADO macro `$(VAR)`, but
        // NOT a template/runtime expression, pipeline-command injection, or a
        // newline — and must be non-empty (an empty credential would make the AWF
        // sidecar fall back to DefaultAzureCredential → the #1372 403 class).
        if let Some(api_key) = &self.api_key {
            if api_key.trim().is_empty() {
                anyhow::bail!(
                    "engine.provider.api-key must not be empty. Provide a secret \
                     macro reference (e.g. `$(OPENAI_API_KEY)`) or omit the field."
                );
            }
            if crate::validate::contains_ado_template_expression(api_key) || api_key.contains("$[")
            {
                anyhow::bail!(
                    "engine.provider.api-key '{api_key}' contains an ADO template ('${{{{ }}}}') \
                     or runtime ('$[...]') expression. Use a literal value or a macro '$(VAR)' \
                     reference."
                );
            }
            if crate::validate::contains_pipeline_command(api_key) {
                anyhow::bail!(
                    "engine.provider.api-key contains pipeline command injection \
                     ('##vso[' or '##['). This is not allowed."
                );
            }
            if crate::validate::contains_newline(api_key) {
                anyhow::bail!("engine.provider.api-key must not contain newline characters.");
            }
        }
        Ok(())
    }
}

/// Tools configuration for the agent
///
/// Controls which tools are available and their settings.
/// If not specified, defaults are used.
///
/// Examples:
/// ```yaml
/// tools:
///   bash: ["cat", "ls", "grep"]
///   edit: true
///   cache-memory:
///     allowed-extensions: [.md, .json]
///   azure-devops:
///     toolsets: [repos, wit]
///     allowed: [wit_get_work_item]
/// ```
#[derive(Debug, Deserialize, Clone, Default)]
pub struct ToolsConfig {
    /// Bash command allow-list. If empty/not set, defaults to safe commands.
    /// Use [":*"] for unrestricted access.
    #[serde(default)]
    pub bash: Option<Vec<String>>,
    /// Enable the file editing tool (default: true)
    #[serde(default)]
    pub edit: Option<bool>,
    /// Persistent cache memory across agent runs.
    /// Enables the agent to read/write files to a memory directory
    /// that persists between pipeline executions.
    #[serde(default, rename = "cache-memory")]
    pub cache_memory: Option<CacheMemoryToolConfig>,
    /// First-class Azure DevOps MCP integration.
    /// Auto-configures the ADO MCP container, token mapping, MCPG entry,
    /// and network allowlist domains.
    #[serde(default, rename = "azure-devops")]
    pub azure_devops: Option<AzureDevOpsToolConfig>,
}

impl SanitizeConfigTrait for ToolsConfig {
    fn sanitize_config_fields(&mut self) {
        self.bash = self.bash.as_ref().map(|v| {
            v.iter()
                .map(|s| crate::sanitize::sanitize_config(s))
                .collect()
        });
        if let Some(ref mut cm) = self.cache_memory {
            cm.sanitize_config_fields();
        }
        if let Some(ref mut ado) = self.azure_devops {
            ado.sanitize_config_fields();
        }
    }
}

/// Cache memory tool configuration — accepts both `true` and object formats
///
/// Examples:
/// ```yaml
/// # Simple enablement
/// cache-memory: true
///
/// # With options
/// cache-memory:
///   allowed-extensions: [.md, .json, .txt]
/// ```
#[derive(Debug, Deserialize, Clone)]
#[serde(untagged)]
pub enum CacheMemoryToolConfig {
    /// Simple boolean enablement
    Enabled(bool),
    /// Full configuration with options
    WithOptions(CacheMemoryOptions),
}

impl CacheMemoryToolConfig {
    /// Whether cache memory is enabled
    pub fn is_enabled(&self) -> bool {
        match self {
            CacheMemoryToolConfig::Enabled(enabled) => *enabled,
            CacheMemoryToolConfig::WithOptions(_) => true,
        }
    }

    /// Get the allowed file extensions (empty = all allowed)
    pub fn allowed_extensions(&self) -> &[String] {
        match self {
            CacheMemoryToolConfig::Enabled(_) => &[],
            CacheMemoryToolConfig::WithOptions(opts) => &opts.allowed_extensions,
        }
    }
}

impl SanitizeConfigTrait for CacheMemoryToolConfig {
    fn sanitize_config_fields(&mut self) {
        match self {
            CacheMemoryToolConfig::Enabled(_) => {}
            CacheMemoryToolConfig::WithOptions(opts) => opts.sanitize_config_fields(),
        }
    }
}

/// Cache memory options
#[derive(Debug, Deserialize, Clone, Default, SanitizeConfig)]
pub struct CacheMemoryOptions {
    /// Allowed file extensions (e.g., [".md", ".json", ".txt"]).
    /// Defaults to all extensions if empty or not specified.
    #[serde(default, rename = "allowed-extensions")]
    pub allowed_extensions: Vec<String>,
}

/// Azure DevOps MCP tool configuration — accepts both `true` and object formats
///
/// Examples:
/// ```yaml
/// # Simple enablement (auto-infers org from git remote)
/// azure-devops: true
///
/// # With scoping options
/// azure-devops:
///   toolsets: [repos, wit, core]
///   allowed: [wit_get_work_item, wit_my_work_items]
///   org: myorg
/// ```
#[derive(Debug, Deserialize, Clone)]
#[serde(untagged)]
pub enum AzureDevOpsToolConfig {
    /// Simple boolean enablement
    Enabled(bool),
    /// Full configuration with options
    WithOptions(AzureDevOpsOptions),
}

impl AzureDevOpsToolConfig {
    /// Whether the ADO MCP is enabled
    pub fn is_enabled(&self) -> bool {
        match self {
            AzureDevOpsToolConfig::Enabled(enabled) => *enabled,
            AzureDevOpsToolConfig::WithOptions(_) => true,
        }
    }

    /// Get the ADO API toolset groups to enable (e.g., repos, wit, core)
    pub fn toolsets(&self) -> &[String] {
        match self {
            AzureDevOpsToolConfig::Enabled(_) => &[],
            AzureDevOpsToolConfig::WithOptions(opts) => &opts.toolsets,
        }
    }

    /// Get the explicit tool allow-list
    pub fn allowed(&self) -> &[String] {
        match self {
            AzureDevOpsToolConfig::Enabled(_) => &[],
            AzureDevOpsToolConfig::WithOptions(opts) => &opts.allowed,
        }
    }

    /// Get the org override (None = auto-infer from git remote)
    pub fn org(&self) -> Option<&str> {
        match self {
            AzureDevOpsToolConfig::Enabled(_) => None,
            AzureDevOpsToolConfig::WithOptions(opts) => opts.org.as_deref(),
        }
    }
}

impl SanitizeConfigTrait for AzureDevOpsToolConfig {
    fn sanitize_config_fields(&mut self) {
        match self {
            AzureDevOpsToolConfig::Enabled(_) => {}
            AzureDevOpsToolConfig::WithOptions(opts) => opts.sanitize_config_fields(),
        }
    }
}

/// Azure DevOps MCP options
#[derive(Debug, Deserialize, Clone, Default, SanitizeConfig)]
pub struct AzureDevOpsOptions {
    /// ADO API toolset groups to enable (e.g., repos, wit, core, work-items)
    /// Passed as `-d` flags to the ADO MCP entrypoint.
    #[serde(default)]
    pub toolsets: Vec<String>,
    /// Explicit tool allow-list (e.g., wit_get_work_item, core_list_projects)
    /// Passed to MCPG for tool-level filtering.
    #[serde(default)]
    pub allowed: Vec<String>,
    /// Azure DevOps organization name override.
    /// Auto-inferred from the git remote URL at compile time if not specified.
    #[serde(default)]
    pub org: Option<String>,
}

/// Runtime configuration for language environments.
///
/// Runtimes are language toolchains installed before the agent runs.
/// Unlike tools (which are agent capabilities like edit, bash, memory),
/// runtimes are execution environments (Lean, Python, Node, etc.).
///
/// Aligned with gh-aw's `runtimes:` front matter field.
#[derive(Debug, Deserialize, Clone, Default)]
pub struct RuntimesConfig {
    /// Lean 4 theorem prover runtime.
    /// Auto-installs elan/lean/lake, adds Lean domains to the network allowlist,
    /// extends the bash command allow-list, and appends a prompt supplement.
    #[serde(default)]
    pub lean: Option<crate::runtimes::lean::LeanRuntimeConfig>,

    /// Python runtime.
    /// Auto-installs Python via UsePythonVersion@0, emits PipAuthenticate@1,
    /// adds Python ecosystem domains to the AWF network allowlist, extends
    /// the bash command allow-list, and optionally injects feed URL env vars.
    #[serde(default)]
    pub python: Option<crate::runtimes::python::PythonRuntimeConfig>,

    /// Node.js runtime.
    /// Auto-installs Node.js via UseNode@1, emits npmAuthenticate@0,
    /// adds Node ecosystem domains to the AWF network allowlist, extends
    /// the bash command allow-list, and optionally injects feed URL env vars.
    #[serde(default)]
    pub node: Option<crate::runtimes::node::NodeRuntimeConfig>,

    /// .NET runtime.
    /// Auto-installs the .NET SDK via UseDotNet@2, emits NuGetAuthenticate@1,
    /// adds .NET ecosystem domains to the AWF network allowlist, and extends
    /// the bash command allow-list. Feed configuration uses `nuget.config`
    /// (generated or checked in) rather than env vars — NuGet has no env-var
    /// equivalent for selecting a package source.
    #[serde(default)]
    pub dotnet: Option<crate::runtimes::dotnet::DotnetRuntimeConfig>,
}

impl SanitizeConfigTrait for RuntimesConfig {
    fn sanitize_config_fields(&mut self) {
        if let Some(ref mut lean) = self.lean {
            lean.sanitize_config_fields();
        }
        if let Some(ref mut python) = self.python {
            python.sanitize_config_fields();
        }
        if let Some(ref mut node) = self.node {
            node.sanitize_config_fields();
        }
        if let Some(ref mut dotnet) = self.dotnet {
            dotnet.sanitize_config_fields();
        }
    }
}

/// Azure DevOps runtime parameter definition.
///
/// These are emitted as top-level `parameters:` in the generated pipeline YAML,
/// surfaced in the ADO UI when manually queuing a run.
///
/// Example front matter:
/// ```yaml
/// parameters:
///   - name: debugLevel
///     displayName: "Debug verbosity"
///     type: string
///     default: "info"
///     values:
///       - info
///       - debug
///       - trace
/// ```
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, SanitizeConfig)]
pub struct PipelineParameter {
    /// Parameter name (must be a valid ADO identifier)
    pub name: String,
    /// Human-readable label shown in the ADO UI
    #[serde(rename = "displayName", skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// ADO parameter type: boolean, string, number, object, etc.
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub param_type: Option<String>,
    /// Default value for the parameter
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<serde_yaml::Value>,
    /// Allowed values (for string/number parameters)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub values: Option<Vec<serde_yaml::Value>>,
}

/// Front matter configuration from the input markdown file
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrontMatter {
    /// Agent name (required)
    pub name: String,
    /// One-line description (required)
    pub description: String,
    /// Target platform: "standalone" (default) or "1es"
    #[serde(default)]
    pub target: CompileTarget,
    /// Workspace setting: "root" or "repo" (auto-computed if not set)
    #[serde(default)]
    pub workspace: Option<String>,
    /// Agent pool configuration
    #[serde(default)]
    pub pool: Option<PoolConfig>,
    /// AI engine configuration (defaults to copilot)
    #[serde(default)]
    pub engine: EngineConfig,
    /// Tools configuration
    #[serde(default)]
    pub tools: Option<ToolsConfig>,
    /// Runtime configuration for language environments (e.g., Lean 4)
    #[serde(default)]
    pub runtimes: Option<RuntimesConfig>,
    /// Compact repository declarations.
    /// Each entry declares a repository resource and optionally whether to check it out.
    #[serde(default)]
    pub repos: Vec<ReposItem>,
    /// Lowered `Vec<Repository>` form, populated by `lower_repos()` in
    /// `compile/common.rs` after the codemod registry has converted any
    /// legacy `repositories:` + `checkout:` shape into the unified
    /// `repos:` shape. Not deserialized from YAML directly.
    #[serde(skip)]
    pub repositories: Vec<Repository>,
    /// Lowered checkout-alias list, populated by `lower_repos()` from
    /// `repos:` entries with `checkout: true`. Not deserialized from
    /// YAML directly.
    #[serde(skip)]
    pub checkout: Vec<String>,
    /// Lowered per-checkout fetch tuning, populated by `lower_repos()`. Keyed by
    /// alias, plus [`SELF_CHECKOUT_ALIAS`] for the trigger repo. Not
    /// deserialized from YAML directly.
    #[serde(skip)]
    pub checkout_fetch: HashMap<String, CheckoutFetchOpts>,
    /// MCP server configurations
    #[serde(default, rename = "mcp-servers")]
    pub mcp_servers: HashMap<String, McpConfig>,
    /// Per-tool configuration for safe outputs
    #[serde(default, rename = "safe-outputs")]
    pub safe_outputs: HashMap<String, serde_json::Value>,
    /// Debug-only configuration. Top-level section that gates features only
    /// intended for ado-aw dogfood/debug pipelines (e.g., `create-issue` for
    /// filing failure reports back to GitHub during local testing). NOT a
    /// regular safe-output section — anything declared here is omitted from
    /// the regular safe-outputs documentation surface.
    #[serde(default, rename = "ado-aw-debug")]
    pub ado_aw_debug: Option<AdoAwDebugConfig>,
    /// Unified trigger configuration: schedule, pipeline, PR triggers and filters
    #[serde(default, rename = "on")]
    pub on_config: Option<OnConfig>,
    /// Network policy for standalone target (ignored in 1ES)
    #[serde(default)]
    pub network: Option<NetworkConfig>,
    /// Azure DevOps variable group imports (issue #1385).
    ///
    /// Each entry is the **name** of a project-level ADO Library variable
    /// group whose variables should be pulled into the run. The compiler emits
    /// a top-level `variables:\n  - group: <name>` import for each entry, in
    /// declaration order (ADO evaluates later groups after earlier ones, so a
    /// later group wins on key collisions).
    ///
    /// Only group **names** belong here — never secret values. ado-aw never
    /// resolves, prints, logs, or serialises a group's variable values; steps
    /// still reference secrets by macro (`$(VAR_NAME)`) exactly as before.
    ///
    /// In Azure DevOps a group must be **both** authorized for the pipeline
    /// definition **and** imported in YAML; this field provides the YAML
    /// import. Not representable in `target: job` / `target: stage` templates
    /// (the parent pipeline owns pipeline-level `variables:`) — declaring it
    /// there is a compile-time error.
    #[serde(default, rename = "variable-groups")]
    pub variable_groups: Vec<String>,
    /// Custom steps before agent runs (same job)
    #[serde(default)]
    pub steps: Vec<serde_yaml::Value>,
    /// Custom steps after agent runs (same job)
    #[serde(default, rename = "post-steps")]
    pub post_steps: Vec<serde_yaml::Value>,
    /// Separate setup job before agentic task
    #[serde(default)]
    pub setup: Vec<serde_yaml::Value>,
    /// Separate teardown job after safe outputs
    #[serde(default)]
    pub teardown: Vec<serde_yaml::Value>,
    /// Permissions configuration for ADO access tokens.
    ///
    /// ADO supports two access levels: blanket read and blanket write.
    /// Tokens are minted from ARM service connections — System.AccessToken is never used.
    ///
    /// - `read`: MI for Stage 1 (agent) — read-only ADO access
    /// - `write`: MI for Stage 3 (executor) — write access for safe-outputs, never given to agent
    #[serde(default)]
    pub permissions: Option<PermissionsConfig>,
    /// When `true`, the compiler inlines all `{{#runtime-import …}}` markers
    /// (including the implicit top-level body marker) at compile time,
    /// embedding referenced content directly into the emitted YAML. When
    /// `false` (default), markers are preserved and resolved at pipeline
    /// runtime, so prompt-body edits do not require recompilation.
    #[serde(rename = "inlined-imports", default)]
    pub inlined_imports: bool,
    /// Workflow-level environment variables
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// Runtime parameters for the pipeline (surfaced in ADO UI when queuing a run)
    #[serde(default)]
    pub parameters: Vec<PipelineParameter>,
    /// Execution-context configuration — controls the always-on
    /// `ExecContextExtension` that stages per-trigger context (PR diff,
    /// changed files, snapshots, etc.) under `aw-context/` before the
    /// agent runs. See `docs/execution-context.md`.
    ///
    /// When omitted, defaults activate per trigger configured in `on:`:
    /// PR context is on when `on.pr` is set.
    #[serde(default, rename = "execution-context")]
    pub execution_context: Option<ExecutionContextConfig>,
    /// Internal supply-chain configuration — when present, mirrors the
    /// GitHub/GHCR artifacts (compiler, AWF binary, ado-script bundle, AWF/MCPG
    /// images) from an internal Azure DevOps Artifacts feed and/or container
    /// registry. See `docs/supply-chain.md`.
    #[serde(default, rename = "supply-chain")]
    pub supply_chain: Option<SupplyChainConfig>,
}

impl FrontMatter {
    /// Returns the per-job pool overrides map from the pool configuration.
    ///
    /// Delegates to `pool.overrides` when a pool object is present;
    /// returns an empty map when `pool:` is absent.
    pub fn pool_overrides(&self) -> &HashMap<String, PoolConfig> {
        self.pool
            .as_ref()
            .map(|p| p.overrides())
            .unwrap_or_else(|| EMPTY_OVERRIDES.get_or_init(HashMap::new))
    }
}

/// Reserved keys inside the `safe-outputs:` map that configure the section
/// itself rather than naming a safe-output tool. These must never be treated
/// as tool names (e.g. in `--enabled-tools`, Stage-3 budgets, or unknown-key
/// validation).
pub const SAFE_OUTPUT_RESERVED_KEYS: &[&str] = &["require-approval"];

/// Automatic action a manual-validation gate takes when its pending period
/// elapses with no human response. Mirrors `ManualValidation@1`'s `onTimeout`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ApprovalOnTimeout {
    /// Reject the run on timeout (fail-closed — the default).
    Reject,
    /// Resume (approve) the run on timeout.
    Resume,
}

/// Detailed manual-review settings for a safe-output approval gate. Lowered
/// into a `ManualValidation@1` agentless job. See `docs/safe-outputs.md`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct ApprovalConfig {
    /// Users/groups permitted to act on the validation. Empty → anyone with
    /// run permission can approve or reject.
    #[serde(default)]
    pub approvers: Vec<String>,
    /// Users/groups to email when the validation is pending. Empty → no email.
    #[serde(default)]
    pub notify_users: Vec<String>,
    /// Pending-period timeout in minutes. None → ADO job/stage timeout applies.
    #[serde(default)]
    pub timeout_minutes: Option<u32>,
    /// Automatic action on timeout. None → fail-closed (`reject`).
    #[serde(default)]
    pub on_timeout: Option<ApprovalOnTimeout>,
    /// Free-text message shown to the reviewer (run "Review" panel + email).
    /// None → an auto-generated summary of the proposed outputs is used.
    #[serde(default)]
    pub instructions: Option<String>,
}

/// The `require-approval` value, accepted either as a bare boolean toggle or a
/// detailed [`ApprovalConfig`] object. Usable at the `safe-outputs:` section
/// level (global default) or inside an individual tool's config (override).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum RequireApproval {
    /// `require-approval: true|false`.
    Bool(bool),
    /// `require-approval: { approvers: …, on-timeout: …, … }`.
    Detailed(ApprovalConfig),
}

impl RequireApproval {
    /// Whether manual review is required by this setting.
    pub fn is_required(&self) -> bool {
        match self {
            RequireApproval::Bool(b) => *b,
            RequireApproval::Detailed(_) => true,
        }
    }

    /// The reviewer settings (defaults for the bare-boolean form).
    pub fn config(&self) -> ApprovalConfig {
        match self {
            RequireApproval::Bool(_) => ApprovalConfig::default(),
            RequireApproval::Detailed(c) => c.clone(),
        }
    }
}

impl FrontMatter {
    /// Iterator over enabled safe-output **tool** names, skipping reserved
    /// section-level config keys (e.g. `require-approval`). Every consumer that
    /// treats `safe-outputs:` keys as tool names MUST go through this so a
    /// reserved key is never mistaken for a tool.
    pub fn safe_output_tool_names(&self) -> impl Iterator<Item = &String> {
        self.safe_outputs
            .keys()
            .filter(|k| !SAFE_OUTPUT_RESERVED_KEYS.contains(&k.as_str()))
    }

    /// Whether the workflow enables **any** safe-output tool.
    ///
    /// Single source of truth for the safe-outputs-summary feature gate: it
    /// drives BOTH the ado-script bundle download
    /// (`AdoScriptExtension::safe_outputs_summary_active`, set in
    /// `collect_extensions`) and the end-of-Agent-job render step emission
    /// (`build_agent_job`). Both call sites MUST go through this so the bundle
    /// is downloaded iff the step that runs it is emitted — a drift between two
    /// independent copies of this predicate would make the step invoke a bundle
    /// that was never downloaded.
    pub fn has_any_safe_output_tool(&self) -> bool {
        self.safe_output_tool_names().next().is_some()
    }

    /// The parsed, sanitized `create-pull-request` config, or `None` when the
    /// tool is not configured. Mirrors Stage 3's `ExecutionContext::get_tool_config`
    /// (deserialize + `sanitize_config_fields`) so the compiler resolves per-repo
    /// target branches (for prepare-pr-base deepening) from the SAME values the
    /// Stage 3 executor uses to open the PR — the deepened branch and the PR base
    /// cannot drift.
    ///
    /// On a malformed config (e.g. `target-branches: "not-a-map"`) the compiler
    /// warns and falls back to defaults — deliberately matching Stage 3's
    /// `get_tool_config`, which also swallows a deserialization error and uses
    /// `Default` (`.ok().unwrap_or_default()`). Bailing here would make the two
    /// paths diverge (compile fails / Stage 3 silently defaults); the warning
    /// surfaces the mistake without breaking that symmetry.
    ///
    /// A bare `create-pull-request:` (YAML null) is the standard "enable with
    /// defaults" idiom, so it maps to `CreatePrConfig::default()` silently — it
    /// is not a malformed config and must not warn.
    pub fn create_pr_config(&self) -> Option<crate::safe_outputs::CreatePrConfig> {
        self.safe_outputs.get("create-pull-request").map(|v| {
            if v.is_null() {
                return crate::safe_outputs::CreatePrConfig::default();
            }
            let mut cfg: crate::safe_outputs::CreatePrConfig = serde_json::from_value(v.clone())
                .unwrap_or_else(|e| {
                    eprintln!(
                        "Warning: could not parse create-pull-request config ({e}); \
                        using defaults. Stage 3 will do the same — check the \
                        `create-pull-request:` block (e.g. `target-branches` must be a map)."
                    );
                    crate::safe_outputs::CreatePrConfig::default()
                });
            cfg.sanitize_config_fields();
            cfg
        })
    }

    /// Map each checked-out repo alias to its `repos: ref`, for resolving a
    /// per-repo create-pull-request target branch. `self` is intentionally
    /// absent (its ref is the runtime trigger branch, not a static `repos:` ref).
    pub fn checkout_repo_refs(&self) -> std::collections::HashMap<String, String> {
        self.repositories
            .iter()
            .filter(|r| self.checkout.iter().any(|a| a == &r.repository))
            .map(|r| (r.repository.clone(), r.repo_ref.clone()))
            .collect()
    }

    /// Section-level (global) `require-approval` default, if configured.
    pub fn global_require_approval(&self) -> Option<RequireApproval> {
        self.safe_outputs
            .get("require-approval")
            .and_then(|v| serde_json::from_value::<RequireApproval>(v.clone()).ok())
    }

    /// Per-tool `require-approval` override for `tool`, if present.
    fn tool_require_approval(&self, tool: &str) -> Option<RequireApproval> {
        self.safe_outputs
            .get(tool)
            .and_then(|v| v.get("require-approval"))
            .and_then(|v| serde_json::from_value::<RequireApproval>(v.clone()).ok())
    }

    /// Effective approval setting for `tool`: the per-tool override if present,
    /// otherwise the section-level default. Returns `Some(config)` only when
    /// the tool's outputs require manual review.
    pub fn tool_requires_approval(&self, tool: &str) -> Option<ApprovalConfig> {
        let setting = self
            .tool_require_approval(tool)
            .or_else(|| self.global_require_approval())?;
        setting.is_required().then(|| setting.config())
    }

    /// Partition enabled safe-output tool names into `(auto, reviewed)` where
    /// `reviewed` tools require manual approval and `auto` tools do not. Both
    /// lists are sorted for deterministic emission.
    pub fn partition_safe_outputs_by_approval(&self) -> (Vec<String>, Vec<String>) {
        let mut auto = Vec::new();
        let mut reviewed = Vec::new();
        for tool in self.safe_output_tool_names() {
            if self.tool_requires_approval(tool).is_some() {
                reviewed.push(tool.clone());
            } else {
                auto.push(tool.clone());
            }
        }
        auto.sort();
        reviewed.sort();
        (auto, reviewed)
    }

    /// Eagerly validate every `require-approval` value (section-level and
    /// per-tool) so a malformed config is surfaced as a compilation error
    /// instead of being silently discarded by the `.ok()` paths in
    /// [`global_require_approval`](Self::global_require_approval) /
    /// [`tool_require_approval`](Self::tool_require_approval). Without this,
    /// a typo like `on-timeout: rejec` (or any unknown field — `ApprovalConfig`
    /// uses `deny_unknown_fields`) would make the affected tool silently fall
    /// out of the reviewed list, emitting no `ManualReview` gate and letting a
    /// high-impact output bypass the intended approval step.
    pub fn validate_require_approval(&self) -> anyhow::Result<()> {
        fn check(label: &str, v: &serde_json::Value) -> anyhow::Result<()> {
            let parsed = serde_json::from_value::<RequireApproval>(v.clone()).map_err(|e| {
                anyhow::anyhow!(
                    "{label} has an invalid `require-approval` value: {e}\n\n\
                     `require-approval` must be a boolean or an object with the keys: \
                     approvers, notify-users, timeout-minutes, on-timeout \
                     (resume | reject), instructions. See docs/safe-outputs.md."
                )
            })?;
            // Reject ADO **template** expressions (`${{ ... }}`) in the
            // author-supplied string fields: these are expanded by ADO's YAML
            // template engine at queue time before `ManualValidation@1` sees
            // them, so e.g. `approvers: "${{ variables['secret-token'] }}"`
            // would leak a pipeline value into the gate. Runtime macros
            // (`$(...)`) are intentionally NOT rejected — `instructions`
            // documents support for `$(...)` interpolation.
            if let RequireApproval::Detailed(cfg) = &parsed {
                let mut fields: Vec<(&str, &str)> = Vec::new();
                for a in &cfg.approvers {
                    fields.push(("approvers", a));
                }
                for n in &cfg.notify_users {
                    fields.push(("notify-users", n));
                }
                if let Some(instr) = &cfg.instructions {
                    fields.push(("instructions", instr));
                }
                for (field, value) in fields {
                    if crate::validate::contains_ado_template_expression(value) {
                        anyhow::bail!(
                            "{label} field `{field}` contains an ADO template expression \
                             (`${{{{ ... }}}}`), which is expanded at queue time and is not \
                             allowed here. Use a literal value. See docs/safe-outputs.md."
                        );
                    }
                }
            }
            Ok(())
        }

        if let Some(v) = self.safe_outputs.get("require-approval") {
            check("safe-outputs.require-approval", v)?;
        }
        for tool in self.safe_output_tool_names() {
            if let Some(v) = self.safe_outputs.get(tool).and_then(|c| c.get("require-approval")) {
                check(&format!("safe-outputs.{tool}.require-approval"), v)?;
            }
        }
        Ok(())
    }

    /// Get the schedule configuration (if any).
    pub fn schedule(&self) -> Option<&ScheduleConfig> {
        self.on_config.as_ref().and_then(|o| o.schedule.as_ref())
    }

    /// Get the pipeline trigger configuration (if any).
    pub fn pipeline_trigger(&self) -> Option<&PipelineTrigger> {
        self.on_config.as_ref().and_then(|o| o.pipeline.as_ref())
    }

    /// Get the PR trigger configuration (if any).
    pub fn pr_trigger(&self) -> Option<&PrTriggerConfig> {
        self.on_config.as_ref().and_then(|o| o.pr.as_ref())
    }

    /// Whether the synthetic-from-ci path is active for this agent —
    /// i.e. `on.pr` is configured AND `on.pr.mode == PrMode::Synthetic`
    /// (the default). Centralised here so the three compile-time call
    /// sites (`collect_extensions`, `ExecContextExtension::new`,
    /// `compile_shared`) cannot drift on the predicate if a future
    /// `PrMode` variant is added.
    pub fn is_synthetic_pr(&self) -> bool {
        self.pr_trigger()
            .is_some_and(|p| matches!(p.mode, PrMode::Synthetic))
    }

    /// Get the PR runtime filters (if any).
    pub fn pr_filters(&self) -> Option<&PrFilters> {
        self.pr_trigger().and_then(|pr| pr.filters.as_ref())
    }

    /// Get the pipeline runtime filters (if any).
    pub fn pipeline_filters(&self) -> Option<&PipelineFilters> {
        self.pipeline_trigger().and_then(|pt| pt.filters.as_ref())
    }

    /// Get the internal supply-chain configuration (if any).
    pub fn supply_chain(&self) -> Option<&SupplyChainConfig> {
        self.supply_chain.as_ref()
    }
}

impl SanitizeConfigTrait for FrontMatter {
    fn sanitize_config_fields(&mut self) {
        self.name = crate::sanitize::sanitize_config(&self.name);
        self.description = crate::sanitize::sanitize_config(&self.description);
        self.workspace = self
            .workspace
            .as_deref()
            .map(crate::sanitize::sanitize_config);
        if let Some(ref mut p) = self.pool {
            p.sanitize_config_fields();
        }
        self.engine.sanitize_config_fields();
        if let Some(ref mut t) = self.tools {
            t.sanitize_config_fields();
        }
        if let Some(ref mut r) = self.runtimes {
            r.sanitize_config_fields();
        }
        for item in &mut self.repos {
            item.sanitize();
        }
        for repo in &mut self.repositories {
            repo.sanitize_config_fields();
        }
        self.checkout = self
            .checkout
            .iter()
            .map(|s| crate::sanitize::sanitize_config(s))
            .collect();
        for mcp in self.mcp_servers.values_mut() {
            mcp.sanitize_config_fields();
        }
        // safe_outputs: HashMap<String, serde_json::Value> — opaque JSON, sanitized at
        // Stage 3 execution via get_tool_config() when deserialized into typed configs.
        if let Some(ref mut o) = self.on_config {
            o.sanitize_config_fields();
        }
        if let Some(ref mut n) = self.network {
            n.sanitize_config_fields();
        }
        // steps, post_steps, setup, teardown: Vec<serde_yaml::Value> — opaque YAML
        // passed through to the pipeline, validated by ADO at parse time.
        if let Some(ref mut p) = self.permissions {
            p.sanitize_config_fields();
        }
        for v in self.env.values_mut() {
            *v = crate::sanitize::sanitize_config(v);
        }
        for p in &mut self.parameters {
            p.sanitize_config_fields();
        }
        if let Some(ref mut d) = self.ado_aw_debug {
            d.sanitize_config_fields();
        }
        if let Some(ref mut ec) = self.execution_context {
            ec.sanitize_config_fields();
        }
        if let Some(ref mut sc) = self.supply_chain {
            sc.sanitize_config_fields();
        }
        // pool overrides are sanitized as part of pool.sanitize_config_fields()
        // (PoolConfig::sanitize_config_fields iterates self.overrides)
    }
}

/// Network policy configuration (standalone target only)
///
/// Network isolation uses AWF (Agentic Workflow Firewall) for L7 domain whitelisting.
/// The domain allowlist is dynamically generated based on:
/// - Core Azure DevOps/GitHub endpoints (always included)
/// - MCP-specific endpoints for each enabled MCP
/// - User-specified additional hosts from `allowed` field
#[derive(Debug, Deserialize, Clone, Default, SanitizeConfig)]
#[serde(deny_unknown_fields)]
pub struct NetworkConfig {
    /// Additional allowed host patterns (supports wildcards like *.example.com)
    /// Core Azure DevOps and GitHub hosts are always allowed.
    #[serde(default)]
    pub allowed: Vec<String>,
    /// Blocked host patterns (takes precedence over allowed)
    #[serde(default)]
    pub blocked: Vec<String>,
}

/// Permissions configuration for ADO access tokens.
///
/// ADO does not support fine-grained permissions. There are two access levels:
/// blanket read and blanket write, each backed by an ARM service connection
/// that mints an ADO-scoped token.
///
/// Examples:
/// ```yaml
/// # Both read and write
/// permissions:
///   read: my-read-arm-connection
///   write: my-write-arm-connection
///
/// # Read-only (agent can query ADO APIs, no write safe-outputs)
/// permissions:
///   read: my-read-arm-connection
///
/// # Write-only (safe-outputs can write, agent gets no ADO token)
/// permissions:
///   write: my-write-arm-connection
/// ```
#[derive(Debug, Deserialize, Clone, Default, SanitizeConfig)]
pub struct PermissionsConfig {
    /// ARM service connection for read-only ADO access.
    /// Token is minted and given to the agent in Stage 1 (inside AWF sandbox).
    #[serde(default)]
    pub read: Option<String>,
    /// ARM service connection for write ADO access.
    /// Token is minted and used only by the executor in Stage 3 (Execution).
    /// This token is never exposed to the agent.
    #[serde(default)]
    pub write: Option<String>,
}

/// Debug-only configuration block.
///
/// Lives under the `ado-aw-debug:` top-level front-matter key. Holds knobs
/// that only make sense for pipelines we're actively dogfooding from
/// `githubnext/ado-aw` and that we explicitly do **not** want to advertise
/// as part of the regular agent surface.
///
/// Adding a new field: pair the front-matter knob with a corresponding
/// compile-side hook (e.g., a debug-only safe output should also be added
/// to `crate::safe_outputs::DEBUG_ONLY_TOOLS` so the MCP layer enforces a
/// matching default-deny gate).
#[derive(Debug, Deserialize, Clone, Default)]
#[serde(deny_unknown_fields)]
pub struct AdoAwDebugConfig {
    /// When true, the "Verify pipeline integrity" step is omitted from the
    /// generated pipeline. Mirrors and OR-s with the `--skip-integrity`
    /// CLI flag.
    #[serde(default, rename = "skip-integrity")]
    pub skip_integrity: bool,

    /// Configuration for the debug-only `create-issue` safe output.
    /// Presence of this field is what enables the tool — when omitted
    /// the SafeOutputs MCP layer hides it via `DEBUG_ONLY_TOOLS`.
    #[serde(default, rename = "create-issue")]
    pub create_issue: Option<crate::safe_outputs::CreateIssueConfig>,
}

impl SanitizeConfigTrait for AdoAwDebugConfig {
    fn sanitize_config_fields(&mut self) {
        // skip_integrity: bool — nothing to sanitize
        if let Some(ref mut ci) = self.create_issue {
            ci.sanitize_config_fields();
        }
    }
}

/// Internal supply-chain configuration.
///
/// Lives under the optional `supply-chain:` top-level front-matter key. When
/// present, the compiler mirrors the artifacts it normally fetches from
/// GitHub Releases / GHCR from an internal Azure DevOps Artifacts feed and/or
/// an internal container registry instead. `feed` and `registry` are
/// independent — a user may set either, both, or neither.
///
/// See `docs/supply-chain.md`.
#[derive(Debug, Deserialize, Clone, Default)]
#[serde(deny_unknown_fields)]
pub struct SupplyChainConfig {
    /// Internal Azure DevOps Artifacts feed for the binary artifacts
    /// (`ado-aw`, `awf`, `ado-script`). When omitted those binaries are
    /// fetched from GitHub Releases as today.
    #[serde(default)]
    pub feed: Option<FeedConfig>,
    /// Internal container registry (ACR login server) for the AWF and MCPG
    /// images. When omitted images are pulled from GHCR as today.
    #[serde(default)]
    pub registry: Option<RegistryConfig>,
    /// Shared fallback service connection used by whichever target does not
    /// declare its own `service-connection`.
    #[serde(default, rename = "service-connection")]
    pub service_connection: Option<crate::secure::ServiceConnection>,
}

/// A feed target. Accepts either a bare scalar (the feed reference) or an
/// object `{ name, service-connection }`. The scalar form is sugar for an
/// object with no per-target connection.
#[derive(Debug, Clone)]
pub struct FeedConfig {
    /// Feed reference: a bare feed name or `project/feed`.
    pub name: crate::secure::FeedRef,
    /// Optional per-target service connection (overrides the top-level one).
    pub service_connection: Option<crate::secure::ServiceConnection>,
}

/// A registry target. Accepts either a bare scalar (the registry host or base
/// path) or an object `{ name, service-connection }`.
#[derive(Debug, Clone)]
pub struct RegistryConfig {
    /// Internal container-registry host or base path, e.g.
    /// `myacr.azurecr.io` or `myacr.azurecr.io/mirror`. The mirrored images
    /// keep their original artifact names (`squid`, `agent`, `gh-aw-mcpg`)
    /// directly under this path.
    pub name: crate::secure::RegistryRef,
    /// Optional per-target service connection (overrides the top-level one).
    pub service_connection: Option<crate::secure::ServiceConnection>,
}

impl<'de> Deserialize<'de> for FeedConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Obj {
            name: crate::secure::FeedRef,
            #[serde(default, rename = "service-connection")]
            service_connection: Option<crate::secure::ServiceConnection>,
        }
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            Scalar(crate::secure::FeedRef),
            Obj(Obj),
        }
        Ok(match Repr::deserialize(deserializer)? {
            Repr::Scalar(name) => FeedConfig {
                name,
                service_connection: None,
            },
            Repr::Obj(o) => FeedConfig {
                name: o.name,
                service_connection: o.service_connection,
            },
        })
    }
}

impl<'de> Deserialize<'de> for RegistryConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Obj {
            name: crate::secure::RegistryRef,
            #[serde(default, rename = "service-connection")]
            service_connection: Option<crate::secure::ServiceConnection>,
        }
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            Scalar(crate::secure::RegistryRef),
            Obj(Obj),
        }
        Ok(match Repr::deserialize(deserializer)? {
            Repr::Scalar(name) => RegistryConfig {
                name,
                service_connection: None,
            },
            Repr::Obj(o) => RegistryConfig {
                name: o.name,
                service_connection: o.service_connection,
            },
        })
    }
}

impl SupplyChainConfig {
    /// Effective service connection for the feed (binary) mirror.
    ///
    /// Resolution: the feed's own `service-connection` → the top-level
    /// `service-connection`. `None` means "authenticate with
    /// `$(System.AccessToken)`" (valid for same-org feeds).
    pub fn feed_connection(&self) -> Option<&str> {
        self.feed
            .as_ref()
            .and_then(|f| f.service_connection.as_deref())
            .or(self.service_connection.as_deref())
    }

    /// Effective service connection for the registry (image) mirror.
    ///
    /// Resolution: the registry's own `service-connection` → the top-level
    /// `service-connection`. `None` is invalid when `registry` is set (ACR has
    /// no `System.AccessToken` path) — see [`SupplyChainConfig::validate`].
    pub fn registry_connection(&self) -> Option<&str> {
        self.registry
            .as_ref()
            .and_then(|r| r.service_connection.as_deref())
            .or(self.service_connection.as_deref())
    }

    /// Validate cross-field rules. Errors when `registry` is configured but no
    /// service connection resolves for it.
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.registry.is_some() && self.registry_connection().is_none() {
            anyhow::bail!(
                "supply-chain.registry requires a service connection: set \
                 `registry.service-connection` or a top-level \
                 `supply-chain.service-connection`. A container registry (ACR) \
                 cannot be accessed with $(System.AccessToken)."
            );
        }
        Ok(())
    }
}

impl SanitizeConfigTrait for SupplyChainConfig {
    fn sanitize_config_fields(&mut self) {
        // All fields are validated newtypes (FeedRef / HostName /
        // ServiceConnection) constrained at deserialization time; there is
        // nothing further to sanitize.
    }
}

/// Repository resource definition
#[derive(Debug, Deserialize, Clone, SanitizeConfig)]
pub struct Repository {
    pub repository: String,
    #[serde(rename = "type")]
    pub repo_type: String,
    pub name: String,
    #[serde(default = "default_ref")]
    #[serde(rename = "ref")]
    pub repo_ref: String,
}

fn default_ref() -> String {
    "refs/heads/main".to_string()
}

// ──────────────────────────────────────────────────────────────────────────────
// Compact `repos:` syntax — a single block that replaces both `repositories:`
// and `checkout:` with sensible defaults.
// ──────────────────────────────────────────────────────────────────────────────

/// Object form for a `repos:` entry.
#[derive(Debug, Deserialize, Clone)]
pub struct RepoEntry {
    /// Full repo name in the form `org/repo` (maps to ADO `name:`).
    pub name: String,
    /// Optional alias (maps to ADO `repository:`). Defaults to the last segment of `name`.
    #[serde(default)]
    pub alias: Option<String>,
    /// ADO repository resource type. Defaults to `"git"`.
    #[serde(default = "default_repo_type", rename = "type")]
    pub repo_type: String,
    /// Branch/tag ref. Defaults to `"refs/heads/main"`.
    #[serde(default = "default_ref", rename = "ref")]
    pub repo_ref: String,
    /// Whether the agent job checks out this repository. Defaults to `true`.
    #[serde(default = "default_checkout")]
    pub checkout: bool,
    /// Shallow-clone depth for this repository's checkout step. Maps to ADO
    /// `fetchDepth`. `0` means full history (no `fetchDepth` emitted). When
    /// omitted, the ADO default applies. Also settable on a reserved `self`
    /// entry to tune the auto-generated `checkout: self`.
    #[serde(default, rename = "fetch-depth")]
    pub fetch_depth: Option<u32>,
    /// Whether to fetch git tags during this repository's checkout step. Maps
    /// to ADO `fetchTags`. When omitted, the ADO default applies. Also settable
    /// on a reserved `self` entry.
    #[serde(default, rename = "fetch-tags")]
    pub fetch_tags: Option<bool>,
}

fn default_repo_type() -> String {
    "git".to_string()
}

fn default_checkout() -> bool {
    true
}

/// Reserved `repos:` alias that tunes the auto-generated `checkout: self`
/// step rather than declaring an additional repository resource.
pub const SELF_CHECKOUT_ALIAS: &str = "self";

/// Resolved fetch tuning for a single checkout step (`self` or a named
/// repository). Populated by `lower_repos()` from `repos:` entries and keyed by
/// alias (with [`SELF_CHECKOUT_ALIAS`] for the trigger repo).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CheckoutFetchOpts {
    /// ADO `fetchDepth`. `Some(0)` means full history.
    pub fetch_depth: Option<u32>,
    /// ADO `fetchTags`.
    pub fetch_tags: Option<bool>,
}

impl CheckoutFetchOpts {
    /// `true` when no tuning is set (a no-op, e.g. a bare `self` entry).
    pub fn is_empty(&self) -> bool {
        self.fetch_depth.is_none() && self.fetch_tags.is_none()
    }

    /// The `fetchDepth` value to emit. `Some(0)` must remain explicit because
    /// Azure DevOps interprets omission as "use the pipeline setting", which
    /// may still be a shallow checkout.
    pub fn depth_for_emit(&self) -> Option<u32> {
        self.fetch_depth
    }
}

/// A single item in the `repos:` list — either a string shorthand or an object.
#[derive(Debug, Clone)]
pub enum ReposItem {
    /// String shorthand: `"org/repo"` or `"alias=org/repo"`.
    Shorthand(String),
    /// Full object form with explicit fields.
    Full(RepoEntry),
}

impl<'de> Deserialize<'de> for ReposItem {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de;

        struct ReposItemVisitor;

        impl<'de> de::Visitor<'de> for ReposItemVisitor {
            type Value = ReposItem;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a string shorthand (\"org/repo\" or \"alias=org/repo\") or an object with at least a `name` field")
            }

            fn visit_str<E: de::Error>(self, value: &str) -> std::result::Result<ReposItem, E> {
                Ok(ReposItem::Shorthand(value.to_string()))
            }

            fn visit_map<M>(self, map: M) -> std::result::Result<ReposItem, M::Error>
            where
                M: de::MapAccess<'de>,
            {
                let entry = RepoEntry::deserialize(de::value::MapAccessDeserializer::new(map))?;
                Ok(ReposItem::Full(entry))
            }
        }

        deserializer.deserialize_any(ReposItemVisitor)
    }
}

impl ReposItem {
    /// Sanitize all user-provided fields through `sanitize_config`.
    pub fn sanitize(&mut self) {
        match self {
            ReposItem::Shorthand(s) => {
                *s = crate::sanitize::sanitize_config(s);
            }
            ReposItem::Full(entry) => {
                entry.name = crate::sanitize::sanitize_config(&entry.name);
                if let Some(ref mut a) = entry.alias {
                    *a = crate::sanitize::sanitize_config(a);
                }
                entry.repo_type = crate::sanitize::sanitize_config(&entry.repo_type);
                entry.repo_ref = crate::sanitize::sanitize_config(&entry.repo_ref);
            }
        }
    }
}

/// MCP configuration - can be `true` for simple enablement or an object with options
#[derive(Debug, Deserialize, Clone)]
#[serde(untagged)]
pub enum McpConfig {
    Enabled(bool),
    WithOptions(Box<McpOptions>),
}

impl SanitizeConfigTrait for McpConfig {
    fn sanitize_config_fields(&mut self) {
        match self {
            McpConfig::Enabled(_) => {}
            McpConfig::WithOptions(opts) => opts.sanitize_config_fields(),
        }
    }
}

/// Detailed MCP options
#[derive(Debug, Deserialize, Clone, Default, SanitizeConfig)]
pub struct McpOptions {
    /// Whether this MCP is enabled (default: true)
    #[serde(default)]
    pub enabled: Option<bool>,
    /// Docker container image for containerized stdio MCPs (MCPG-native)
    #[serde(default)]
    pub container: Option<String>,
    /// Container entrypoint override (equivalent to `docker run --entrypoint`)
    #[serde(default)]
    pub entrypoint: Option<String>,
    /// Arguments passed to the container entrypoint
    #[serde(default, rename = "entrypoint-args")]
    pub entrypoint_args: Vec<String>,
    /// Additional Docker runtime arguments (inserted before the image in `docker run`)
    #[serde(default)]
    pub args: Vec<String>,
    /// HTTP endpoint URL for remote MCPs
    #[serde(default)]
    pub url: Option<String>,
    /// HTTP headers for remote MCPs (e.g., Authorization, X-MCP-Toolsets)
    #[serde(default)]
    pub headers: HashMap<String, String>,
    /// Volume mounts for containerized MCPs (format: "source:dest:mode")
    #[serde(default)]
    pub mounts: Vec<String>,
    /// Allowed tool names (for MCPG tool filtering)
    #[serde(default)]
    pub allowed: Vec<String>,
    /// Environment variables for the MCP server process
    #[serde(default)]
    pub env: HashMap<String, String>,
}

/// Unified trigger configuration — `on:` front matter key.
///
/// Consolidates all trigger types: schedule, pipeline completion, and PR triggers.
/// Aligns with gh-aw's `on:` key.
#[derive(Debug, Deserialize, Clone, Default)]
pub struct OnConfig {
    /// Fuzzy schedule configuration
    #[serde(default)]
    pub schedule: Option<ScheduleConfig>,
    /// Pipeline completion trigger
    #[serde(default)]
    pub pipeline: Option<PipelineTrigger>,
    /// PR trigger configuration (native ADO branch/path filters + runtime filters)
    #[serde(default)]
    pub pr: Option<PrTriggerConfig>,
}

impl SanitizeConfigTrait for OnConfig {
    fn sanitize_config_fields(&mut self) {
        if let Some(ref mut s) = self.schedule {
            s.sanitize_config_fields();
        }
        if let Some(ref mut p) = self.pipeline {
            p.sanitize_config_fields();
        }
        if let Some(ref mut pr) = self.pr {
            pr.sanitize_config_fields();
        }
    }
}

/// Pipeline completion trigger configuration
#[derive(Debug, Deserialize, Clone)]
pub struct PipelineTrigger {
    /// The name of the source pipeline that triggers this one
    pub name: String,
    /// Optional project name if the pipeline is in a different project
    #[serde(default)]
    pub project: Option<String>,
    /// Branches to trigger on (empty = any branch)
    #[serde(default)]
    pub branches: Vec<String>,
    /// Pipeline-specific runtime filters
    #[serde(default)]
    pub filters: Option<PipelineFilters>,
}

impl SanitizeConfigTrait for PipelineTrigger {
    fn sanitize_config_fields(&mut self) {
        self.name = crate::sanitize::sanitize_config(&self.name);
        if let Some(ref mut p) = self.project {
            *p = crate::sanitize::sanitize_config(p);
        }
        self.branches = self
            .branches
            .iter()
            .map(|s| crate::sanitize::sanitize_config(s))
            .collect();
        if let Some(ref mut f) = self.filters {
            f.sanitize_config_fields();
        }
    }
}

/// Pipeline completion trigger filters.
/// Only exposes filters applicable to pipeline triggers.
#[derive(Debug, Deserialize, Clone, Default)]
pub struct PipelineFilters {
    /// Only run during a specific time window (UTC)
    #[serde(default, rename = "time-window")]
    pub time_window: Option<TimeWindowFilter>,
    /// Glob match on upstream pipeline name (Build.TriggeredBy.DefinitionName)
    #[serde(default, rename = "source-pipeline")]
    pub source_pipeline: Option<PatternFilter>,
    /// Glob match on triggering branch (Build.SourceBranch)
    #[serde(default)]
    pub branch: Option<PatternFilter>,
    /// Include/exclude by build reason
    #[serde(default, rename = "build-reason")]
    pub build_reason: Option<IncludeExcludeFilter>,
    /// Raw ADO condition expression escape hatch
    #[serde(default)]
    pub expression: Option<String>,
}

impl SanitizeConfigTrait for PipelineFilters {
    fn sanitize_config_fields(&mut self) {
        if let Some(ref mut tw) = self.time_window {
            tw.sanitize_config_fields();
        }
        if let Some(ref mut sp) = self.source_pipeline {
            sp.sanitize_config_fields();
        }
        if let Some(ref mut b) = self.branch {
            b.sanitize_config_fields();
        }
        if let Some(ref mut br) = self.build_reason {
            br.sanitize_config_fields();
        }
        if let Some(ref mut e) = self.expression {
            *e = crate::sanitize::sanitize_config(e);
        }
    }
}

// ─── Execution Context Types ────────────────────────────────────────────────

/// Top-level configuration for the always-on execution-context plugin.
///
/// The plugin owns a small framework of per-trigger "context contributors"
/// that materialise execution context on disk under `aw-context/` and as
/// `AW_*` environment variables before the agent runs. v1 ships one
/// contributor (`pr`); future contributors can plug in via the same
/// internal trait without breaking changes.
///
/// All fields are optional. Defaults activate per trigger configured in
/// `on:` — e.g. the PR contributor is on by default when `on.pr` is set.
///
/// See `docs/execution-context.md`.
#[derive(Debug, Deserialize, Clone, Default)]
pub struct ExecutionContextConfig {
    /// Master switch. When `false`, no contributor runs and no
    /// `aw-context/` is staged. Defaults to `true`.
    #[serde(default)]
    pub enabled: Option<bool>,
    /// PR-context contributor configuration.
    #[serde(default)]
    pub pr: Option<PrContextConfig>,
    /// Manual-context contributor configuration. Activates whenever the
    /// agent declares any `parameters:` block (Stage 1 of the
    /// execution-context contributor build-out — see
    /// `docs/execution-context.md`).
    #[serde(default)]
    pub manual: Option<ManualContextConfig>,
    /// Pipeline-context contributor configuration. Activates whenever
    /// the agent declares an `on.pipeline` trigger (Stage 2 of the
    /// execution-context contributor build-out — see
    /// `docs/execution-context.md`).
    #[serde(default)]
    pub pipeline: Option<PipelineContextConfig>,
    /// CI-push contributor configuration. Stages "since last green
    /// build" diff context on non-PR push builds (Stage 3 of the
    /// execution-context contributor build-out — see
    /// `docs/execution-context.md`). Defaults to OFF — opt in via
    /// `ci-push.enabled: true`.
    #[serde(rename = "ci-push", default)]
    pub ci_push: Option<CiPushContextConfig>,
    /// Workitem-context contributor configuration. PR-linked mode only
    /// in this iteration — activates on PR builds and fetches the
    /// linked WI(s) so a reviewer agent can verify acceptance
    /// criteria. Stage 4 of the build-out — see
    /// `docs/execution-context.md`. **Crosses an untrusted-prose
    /// boundary** (WI bodies are user-authored).
    #[serde(default)]
    pub workitem: Option<WorkitemContextConfig>,
    /// Schedule-context contributor configuration. Stages "since last
    /// run of this pipeline" diff context for scheduled builds.
    /// Stage 5 of the build-out — see `docs/execution-context.md`.
    /// Defaults to OFF (opt-in) — many scheduled agents are
    /// operational (not repo-aware) and don't need diff context.
    #[serde(default)]
    pub schedule: Option<ScheduleContextConfig>,
    /// Repo-context contributor configuration. Always-on capability
    /// (Stage 7 of the build-out — see `docs/execution-context.md`).
    /// Stages repository identity info (branch, SHA, last release
    /// tag, commits-since-tag). Defaults to OFF to avoid
    /// prompt-clutter regression.
    #[serde(default)]
    pub repo: Option<RepoContextConfig>,
}

impl ExecutionContextConfig {
    /// Whether the master switch is on. Defaults to `true` when unset.
    pub fn is_enabled(&self) -> bool {
        self.enabled.unwrap_or(true)
    }
}

impl SanitizeConfigTrait for ExecutionContextConfig {
    fn sanitize_config_fields(&mut self) {
        if let Some(ref mut p) = self.pr {
            p.sanitize_config_fields();
        }
        if let Some(ref mut m) = self.manual {
            m.sanitize_config_fields();
        }
        if let Some(ref mut p) = self.pipeline {
            p.sanitize_config_fields();
        }
        if let Some(ref mut c) = self.ci_push {
            c.sanitize_config_fields();
        }
        if let Some(ref mut w) = self.workitem {
            w.sanitize_config_fields();
        }
        if let Some(ref mut s) = self.schedule {
            s.sanitize_config_fields();
        }
        if let Some(ref mut r) = self.repo {
            r.sanitize_config_fields();
        }
    }
}

/// Configuration for the PR-context contributor.
///
/// Controls whether the precompute step materialises `aw-context/pr/*` for
/// PR-triggered builds. v6.2 onward exposes only an opt-out switch — the
/// agent decides at runtime what to diff (via its own `git diff $BASE..$HEAD`
/// calls); the compiler no longer scopes or caps the diff.
#[derive(Debug, Deserialize, Clone, Default)]
pub struct PrContextConfig {
    /// Whether the PR contributor is active. Defaults to `true` when
    /// `on.pr` is configured. Set `false` to opt out.
    #[serde(default)]
    pub enabled: Option<bool>,
    /// PR-checks (build validation) extension (Stage 6 of the
    /// build-out — see plan.md). Stages a list of failing /
    /// succeeded build-validation runs on the PR so a remediation
    /// agent can read the failing logs and propose a fix.
    /// Default OFF — opt in via `pr.checks.enabled: true`.
    #[serde(default)]
    pub checks: Option<PrChecksContextConfig>,
}

impl PrContextConfig {
    /// Resolved-enabled value; `None` means "depends on whether `on.pr` is set".
    pub fn explicit_enabled(&self) -> Option<bool> {
        self.enabled
    }
}

impl SanitizeConfigTrait for PrContextConfig {
    fn sanitize_config_fields(&mut self) {
        if let Some(ref mut c) = self.checks {
            c.sanitize_config_fields();
        }
    }
}

/// Configuration for the `pr.checks` extension of the PR contributor.
/// Default OFF. When enabled, stages
/// `aw-context/pr/checks/{failing,succeeded}.json` listing Build
/// Validation runs whose source matches the PR.
#[derive(Debug, Deserialize, Clone, Default)]
pub struct PrChecksContextConfig {
    /// Default OFF.
    #[serde(default)]
    pub enabled: Option<bool>,
}

impl PrChecksContextConfig {
    pub fn is_enabled(&self) -> bool {
        self.enabled.unwrap_or(false)
    }
}

impl SanitizeConfigTrait for PrChecksContextConfig {
    fn sanitize_config_fields(&mut self) {
        // No free-form string fields — booleans only.
    }
}

/// Configuration for the `manual` execution-context contributor.
///
/// Activates whenever the agent declares any `parameters:` block (and
/// the execution-context master switch is on). Runtime gate:
/// `eq(variables['Build.Reason'], 'Manual')`. Stages requestor
/// identity and a snapshot of parameter values under
/// `aw-context/manual/` so manually-queued agents can surface intent
/// (selected options, free-text reasons) without the markdown body
/// having to restate them.
///
/// No bearer, no network — pure ADO predefined-variable + template
/// expansion. See `docs/execution-context.md` for the staged layout.
#[derive(Debug, Deserialize, Clone, Default)]
pub struct ManualContextConfig {
    /// Whether the manual contributor is active. Defaults to `true`
    /// when any `parameters:` block is declared. Set `false` to opt
    /// out.
    #[serde(default)]
    pub enabled: Option<bool>,
    /// Whether to surface `Build.RequestedForEmail` in the staged
    /// metadata + prompt fragment. Defaults to `false` (hygiene
    /// posture; ADO already exposes the address to the build but
    /// we don't want it appearing in agent prompts by default).
    #[serde(rename = "include-email", default)]
    pub include_email: Option<bool>,
}

impl ManualContextConfig {
    /// Resolved-enabled value; `None` means "depends on whether any
    /// `parameters:` are declared".
    pub fn explicit_enabled(&self) -> Option<bool> {
        self.enabled
    }

    /// Whether the staged `requested-for-email` file and prompt-fragment
    /// line should be populated. Defaults to `false`.
    pub fn include_email_resolved(&self) -> bool {
        self.include_email.unwrap_or(false)
    }
}

impl SanitizeConfigTrait for ManualContextConfig {
    fn sanitize_config_fields(&mut self) {
        // No free-form string fields — booleans only.
    }
}

/// Configuration for the `pipeline` execution-context contributor.
///
/// Activates whenever the agent declares an `on.pipeline` trigger
/// (and the execution-context master switch is on). Runtime gate:
/// `eq(variables['Build.Reason'], 'ResourceTrigger')`. Stages
/// upstream-build metadata (id, status, source SHA/branch, artifact
/// list) under `aw-context/pipeline/` so the agent can decide what
/// to do based on the run that triggered it.
///
/// Bearer required — fetches via the ADO Build REST API. See
/// `docs/execution-context.md` for the staged layout.
#[derive(Debug, Deserialize, Clone, Default)]
pub struct PipelineContextConfig {
    /// Whether the pipeline contributor is active. Defaults to `true`
    /// when `on.pipeline` is configured. Set `false` to opt out.
    #[serde(default)]
    pub enabled: Option<bool>,
}

impl PipelineContextConfig {
    /// Resolved-enabled value; `None` means "depends on whether
    /// `on.pipeline` is configured".
    pub fn explicit_enabled(&self) -> Option<bool> {
        self.enabled
    }
}

impl SanitizeConfigTrait for PipelineContextConfig {
    fn sanitize_config_fields(&mut self) {
        // No free-form string fields — booleans only.
    }
}

/// Configuration for the `ci-push` execution-context contributor.
///
/// Stages "since last green build" diff context for non-PR push
/// builds. Defaults to OFF (opt-in) — most agents don't need this,
/// and the helper does ADO REST + git fetch work that adds startup
/// latency. Activates only when `enabled: true` is set explicitly.
/// Runtime gate: `in(variables['Build.Reason'], 'IndividualCI',
/// 'BatchedCI')`. Bearer required for both REST lookup and git fetch
/// deepening.
///
/// See `docs/execution-context.md` for the staged layout.
#[derive(Debug, Deserialize, Clone, Default)]
pub struct CiPushContextConfig {
    /// Whether the ci-push contributor is active. **Defaults to
    /// `false`** (opposite of PR / manual / pipeline contributors).
    /// Set `true` to opt in.
    #[serde(default)]
    pub enabled: Option<bool>,
}

impl CiPushContextConfig {
    /// Resolved-enabled value; default is `false`.
    pub fn is_enabled(&self) -> bool {
        self.enabled.unwrap_or(false)
    }
}

impl SanitizeConfigTrait for CiPushContextConfig {
    fn sanitize_config_fields(&mut self) {
        // No free-form string fields — booleans only.
    }
}

/// Configuration for the `workitem` execution-context contributor.
///
/// PR-linked mode only in this iteration (Stage 4 of the build-out —
/// see plan.md). Activates whenever the PR contributor activates and
/// the workitem contributor isn't explicitly disabled. Fetches the
/// linked WI(s) via the ADO REST API and stages per-WI directories
/// with description / acceptance criteria / repro / comments /
/// links / attachment-metadata.
///
/// **Crosses an untrusted-prose boundary** — WI body fields are
/// user-authored and may contain arbitrary content. All staged
/// prose is wrapped via `shared/untrusted.ts::wrapAgentReadableUntrusted`
/// before being written, and the agent prompt fragment explicitly
/// flags this. See `docs/execution-context.md` for the full guidance.
#[derive(Debug, Deserialize, Clone, Default)]
pub struct WorkitemContextConfig {
    /// Whether the workitem contributor is active. Defaults to
    /// `true` when the PR contributor activates.
    #[serde(default)]
    pub enabled: Option<bool>,
    /// Cap on the number of linked WIs staged per build. Defaults to
    /// 5 — additional WIs are listed in
    /// `aw-context/workitem/truncated.txt` for visibility but their
    /// bodies are NOT fetched.
    #[serde(rename = "max-items", default)]
    pub max_items: Option<usize>,
    /// Cap on the size of each WI body field (description / acceptance /
    /// repro), in kilobytes. Defaults to 32 KB. Bodies larger than the
    /// cap are truncated with a trailing marker.
    #[serde(rename = "max-body-kb", default)]
    pub max_body_kb: Option<usize>,
}

impl WorkitemContextConfig {
    pub fn explicit_enabled(&self) -> Option<bool> {
        self.enabled
    }
    pub fn max_items_resolved(&self) -> usize {
        self.max_items.unwrap_or(5)
    }
    pub fn max_body_kb_resolved(&self) -> usize {
        self.max_body_kb.unwrap_or(32)
    }
}

impl SanitizeConfigTrait for WorkitemContextConfig {
    fn sanitize_config_fields(&mut self) {
        // No free-form string fields — booleans + numbers only.
    }
}

/// Configuration for the `schedule` execution-context contributor.
///
/// Stages "since last run of this pipeline on this branch" diff
/// context for scheduled builds (Stage 5 of the build-out — see
/// plan.md). Defaults to OFF (opt-in) — many scheduled agents are
/// operational (e.g. "every morning, summarize open work items")
/// and don't need diff context. Runtime gate:
/// `eq(variables['Build.Reason'], 'Schedule')`.
#[derive(Debug, Deserialize, Clone, Default)]
pub struct ScheduleContextConfig {
    /// Whether the schedule contributor is active. **Default OFF**.
    #[serde(default)]
    pub enabled: Option<bool>,
}

impl ScheduleContextConfig {
    pub fn is_enabled(&self) -> bool {
        self.enabled.unwrap_or(false)
    }
}

impl SanitizeConfigTrait for ScheduleContextConfig {
    fn sanitize_config_fields(&mut self) {
        // No free-form string fields — booleans only.
    }
}

/// Configuration for the `repo` execution-context contributor.
///
/// Always-on capability (Stage 7 of the build-out — see plan.md):
/// stages repository identity info (branch, SHA, last release tag,
/// commits-since-tag). Pure git — no REST, no bearer. Defaults to
/// OFF to avoid prompt-clutter regression for the agents that
/// already get sufficient repo identity from PR / ci-push / pipeline
/// contributors.
#[derive(Debug, Deserialize, Clone, Default)]
pub struct RepoContextConfig {
    /// Whether the repo contributor is active. **Default OFF**.
    #[serde(default)]
    pub enabled: Option<bool>,
    /// Whether to additionally stage `conventions.json` — a probe of
    /// CODEOWNERS / CONTRIBUTING.md / .editorconfig / AGENTS.md
    /// presence + first 50 lines of each found. Defaults to `false`.
    #[serde(default)]
    pub conventions: Option<bool>,
}

impl RepoContextConfig {
    pub fn is_enabled(&self) -> bool {
        self.enabled.unwrap_or(false)
    }
    pub fn conventions_enabled(&self) -> bool {
        self.conventions.unwrap_or(false)
    }
}

impl SanitizeConfigTrait for RepoContextConfig {
    fn sanitize_config_fields(&mut self) {
        // No free-form string fields — booleans only.
    }
}

// ─── PR Trigger Types ───────────────────────────────────────────────────────

/// PR trigger configuration with native ADO filters and runtime gate filters.
#[derive(Debug, Deserialize, Clone, Default)]
pub struct PrTriggerConfig {
    /// Native ADO branch filter for PR triggers
    #[serde(default)]
    pub branches: Option<BranchFilter>,
    /// Native ADO path filter for PR triggers
    #[serde(default)]
    pub paths: Option<PathFilter>,
    /// Runtime filters evaluated via gate steps in the Setup job
    #[serde(default)]
    pub filters: Option<PrFilters>,
    /// Determines how `on.pr` builds reach the pipeline; see
    /// [`PrMode`] for the two supported strategies (`synthetic`,
    /// `policy`). Defaults to [`PrMode::Synthetic`].
    #[serde(default, rename = "mode")]
    pub mode: PrMode,
}

/// How `on.pr` builds reach the pipeline.
///
/// Azure DevOps Services ignores the YAML `pr:` block unless a
/// per-branch Build Validation policy is registered server-side. This
/// enum lets the agent author pick one of two coherent strategies:
///
/// * [`PrMode::Synthetic`] (default) — the compiler emits a Setup-job
///   script (`exec-context-pr-synth.js`) that calls the ADO REST API
///   on CI-triggered builds, finds the open PR for `Build.SourceBranch`,
///   and exposes PR identifiers as `dependencies.Setup.outputs['synthPr.*']`
///   so the gate and `exec-context-pr.js` behave as if
///   `Build.Reason == PullRequest`. The top-level `trigger:` stays at
///   ADO's "trigger on every branch" default. **No branch policy
///   required.** This is the right choice for the vast majority of
///   agents.
///
/// * [`PrMode::Policy`] — the compiler omits all synth wiring AND
///   emits `trigger: none` at the top level, so the pipeline only
///   queues when ADO's Build Validation branch policy fires a real
///   `Build.Reason == PullRequest` build. Choose this when the
///   operator has explicitly installed a branch policy and wants to
///   avoid duplicate CI builds firing on every push.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PrMode {
    /// Synthesise PR context from the ADO REST API on CI-triggered
    /// builds. Top-level `trigger:` left at ADO default.
    #[default]
    Synthetic,
    /// Operator-installed Build Validation branch policy drives PR
    /// builds; CI trigger is suppressed via `trigger: none`.
    Policy,
}

impl SanitizeConfigTrait for PrTriggerConfig {
    fn sanitize_config_fields(&mut self) {
        // `mode` (PrMode enum, `Copy`) has no string content to
        // sanitize — it's a closed kebab-case-deserialised enum, so
        // any malformed input is already rejected at deserialisation
        // time. Intentionally absent here.
        if let Some(ref mut b) = self.branches {
            b.sanitize_config_fields();
        }
        if let Some(ref mut p) = self.paths {
            p.sanitize_config_fields();
        }
        if let Some(ref mut f) = self.filters {
            f.sanitize_config_fields();
        }
    }
}

/// Branch include/exclude filter for PR triggers.
#[derive(Debug, Deserialize, Clone, Default, SanitizeConfig)]
pub struct BranchFilter {
    #[serde(default)]
    pub include: Vec<String>,
    #[serde(default)]
    pub exclude: Vec<String>,
}

/// Path include/exclude filter for PR triggers.
#[derive(Debug, Deserialize, Clone, Default, SanitizeConfig)]
pub struct PathFilter {
    #[serde(default)]
    pub include: Vec<String>,
    #[serde(default)]
    pub exclude: Vec<String>,
}

/// Runtime PR filters evaluated via gate steps in the Setup job.
/// Multiple filters use AND semantics — all must pass for the agent to run.
#[derive(Debug, Deserialize, Clone, Default)]
pub struct PrFilters {
    /// Glob match on PR title (System.PullRequest.Title)
    #[serde(default)]
    pub title: Option<PatternFilter>,
    /// Include/exclude by author email (Build.RequestedForEmail)
    #[serde(default)]
    pub author: Option<IncludeExcludeFilter>,
    /// Glob match on source branch (System.PullRequest.SourceBranch)
    #[serde(default, rename = "source-branch")]
    pub source_branch: Option<PatternFilter>,
    /// Glob match on target branch (System.PullRequest.TargetBranch)
    #[serde(default, rename = "target-branch")]
    pub target_branch: Option<PatternFilter>,
    /// Glob match on last commit message (Build.SourceVersionMessage)
    #[serde(default, rename = "commit-message")]
    pub commit_message: Option<PatternFilter>,
    /// PR label matching (any-of, all-of, none-of)
    #[serde(default)]
    pub labels: Option<LabelFilter>,
    /// Filter by PR draft status
    #[serde(default)]
    pub draft: Option<bool>,
    /// Glob patterns for changed file paths
    #[serde(default, rename = "changed-files")]
    pub changed_files: Option<IncludeExcludeFilter>,
    /// Only run during a specific time window (UTC)
    #[serde(default, rename = "time-window")]
    pub time_window: Option<TimeWindowFilter>,
    /// Minimum number of changed files required
    #[serde(default, rename = "min-changes")]
    pub min_changes: Option<u32>,
    /// Maximum number of changed files allowed
    #[serde(default, rename = "max-changes")]
    pub max_changes: Option<u32>,
    /// Include/exclude by build reason (e.g., PullRequest, Manual, IndividualCI)
    #[serde(default, rename = "build-reason")]
    pub build_reason: Option<IncludeExcludeFilter>,
    /// Raw ADO condition expression appended to the Agent job condition (escape hatch)
    #[serde(default)]
    pub expression: Option<String>,
}

impl SanitizeConfigTrait for PrFilters {
    fn sanitize_config_fields(&mut self) {
        if let Some(ref mut t) = self.title {
            t.sanitize_config_fields();
        }
        if let Some(ref mut a) = self.author {
            a.sanitize_config_fields();
        }
        if let Some(ref mut s) = self.source_branch {
            s.sanitize_config_fields();
        }
        if let Some(ref mut t) = self.target_branch {
            t.sanitize_config_fields();
        }
        if let Some(ref mut cm) = self.commit_message {
            cm.sanitize_config_fields();
        }
        if let Some(ref mut l) = self.labels {
            l.sanitize_config_fields();
        }
        if let Some(ref mut c) = self.changed_files {
            c.sanitize_config_fields();
        }
        if let Some(ref mut tw) = self.time_window {
            tw.sanitize_config_fields();
        }
        if let Some(ref mut br) = self.build_reason {
            br.sanitize_config_fields();
        }
        if let Some(ref mut e) = self.expression {
            *e = crate::sanitize::sanitize_config(e);
        }
    }
}

/// Time window filter — only run during a specific UTC time range.
///
/// Example: `{ start: "09:00", end: "17:00" }` means business hours UTC.
/// Handles overnight windows (e.g., `{ start: "22:00", end: "06:00" }`).
#[derive(Debug, Deserialize, Clone, SanitizeConfig)]
pub struct TimeWindowFilter {
    /// Start time in HH:MM format (UTC)
    pub start: String,
    /// End time in HH:MM format (UTC)
    pub end: String,
}

/// A glob pattern filter. Supports `*` (any chars) and `?` (single char).
///
/// ```yaml
/// title: "*[review]*"
/// source-branch: "feature/*"
/// target-branch: "main"
/// ```
#[derive(Debug, Deserialize, Clone)]
#[serde(transparent)]
pub struct PatternFilter {
    /// Glob pattern to match against
    pub pattern: String,
}

impl SanitizeConfigTrait for PatternFilter {
    fn sanitize_config_fields(&mut self) {
        self.pattern = crate::sanitize::sanitize_config(&self.pattern);
    }
}

/// Include/exclude list filter.
#[derive(Debug, Deserialize, Clone, Default, SanitizeConfig)]
pub struct IncludeExcludeFilter {
    #[serde(default)]
    pub include: Vec<String>,
    #[serde(default)]
    pub exclude: Vec<String>,
}

/// Label matching filter for PR labels.
#[derive(Debug, Deserialize, Clone, Default)]
pub struct LabelFilter {
    /// PR must have at least one of these labels
    #[serde(default, rename = "any-of")]
    pub any_of: Vec<String>,
    /// PR must have all of these labels
    #[serde(default, rename = "all-of")]
    pub all_of: Vec<String>,
    /// PR must not have any of these labels
    #[serde(default, rename = "none-of")]
    pub none_of: Vec<String>,
}

impl SanitizeConfigTrait for LabelFilter {
    fn sanitize_config_fields(&mut self) {
        self.any_of = self
            .any_of
            .iter()
            .map(|s| crate::sanitize::sanitize_config(s))
            .collect();
        self.all_of = self
            .all_of
            .iter()
            .map(|s| crate::sanitize::sanitize_config(s))
            .collect();
        self.none_of = self
            .none_of
            .iter()
            .map(|s| crate::sanitize::sanitize_config(s))
            .collect();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── SupplyChainConfig deserialization + resolution ──────────────────────

    fn parse_supply_chain(yaml: &str) -> SupplyChainConfig {
        let v: serde_yaml::Value = serde_yaml::from_str(yaml).unwrap();
        serde_yaml::from_value(v["supply-chain"].clone()).unwrap()
    }

    #[test]
    fn test_supply_chain_scalar_feed_shorthand() {
        let sc = parse_supply_chain("supply-chain:\n  feed: my-feed");
        let feed = sc.feed.as_ref().unwrap();
        assert_eq!(feed.name.as_str(), "my-feed");
        assert!(feed.service_connection.is_none());
        // No connection resolves → System.AccessToken (None).
        assert_eq!(sc.feed_connection(), None);
    }

    #[test]
    fn test_supply_chain_object_feed_with_connection() {
        let sc = parse_supply_chain(
            "supply-chain:\n  feed:\n    name: proj/my-feed\n    service-connection: feed-conn",
        );
        let feed = sc.feed.as_ref().unwrap();
        assert_eq!(feed.name.as_str(), "proj/my-feed");
        assert_eq!(sc.feed_connection(), Some("feed-conn"));
    }

    #[test]
    fn test_supply_chain_top_level_connection_is_fallback() {
        let sc = parse_supply_chain(
            "supply-chain:\n  feed: my-feed\n  registry: myacr.azurecr.io\n  service-connection: shared",
        );
        assert_eq!(sc.feed_connection(), Some("shared"));
        assert_eq!(sc.registry_connection(), Some("shared"));
    }

    #[test]
    fn test_supply_chain_per_target_overrides_top_level() {
        let sc = parse_supply_chain(
            "supply-chain:\n  \
             feed:\n    name: my-feed\n    service-connection: feed-conn\n  \
             registry:\n    name: myacr.azurecr.io\n    service-connection: acr-conn\n  \
             service-connection: shared",
        );
        assert_eq!(sc.feed_connection(), Some("feed-conn"));
        assert_eq!(sc.registry_connection(), Some("acr-conn"));
    }

    #[test]
    fn test_supply_chain_validate_registry_requires_connection() {
        let sc = parse_supply_chain("supply-chain:\n  registry: myacr.azurecr.io");
        assert!(sc.validate().is_err());

        let ok = parse_supply_chain(
            "supply-chain:\n  registry:\n    name: myacr.azurecr.io\n    service-connection: acr-conn",
        );
        assert!(ok.validate().is_ok());
    }

    #[test]
    fn test_supply_chain_registry_accepts_base_path() {
        // A registry may be a host with an arbitrary namespace path; the
        // mirrored images keep their artifact names directly under it.
        let sc = parse_supply_chain(
            "supply-chain:\n  registry:\n    name: myacr.azurecr.io/oss-mirror\n    service-connection: acr-conn",
        );
        let registry = sc.registry.as_ref().unwrap();
        assert_eq!(registry.name.as_str(), "myacr.azurecr.io/oss-mirror");
        assert!(sc.validate().is_ok());
    }

    #[test]
    fn test_supply_chain_feed_only_validates() {
        // feed-only: registry is None, so validate() never errors regardless of feed.
        let sc = parse_supply_chain("supply-chain:\n  feed: my-feed");
        assert!(sc.validate().is_ok());
        // combined feed + registry-with-connection must also validate OK.
        let sc2 = parse_supply_chain(
            "supply-chain:\n  feed: my-feed\n  registry:\n    name: myacr.azurecr.io\n    service-connection: acr-conn",
        );
        assert!(sc2.validate().is_ok());
    }

    #[test]
    fn test_supply_chain_rejects_unknown_fields() {
        let v: serde_yaml::Value =
            serde_yaml::from_str("supply-chain:\n  feed: my-feed\n  bogus: x").unwrap();
        let res: Result<SupplyChainConfig, _> = serde_yaml::from_value(v["supply-chain"].clone());
        assert!(res.is_err(), "deny_unknown_fields must reject unknown keys");
    }

    // ─── PoolConfig deserialization ──────────────────────────────────────────

    #[test]
    fn test_pool_config_object_form_with_os() {
        let yaml = "pool:\n  name: WinPool\n  os: windows";
        let fm: serde_yaml::Value = serde_yaml::from_str(yaml).unwrap();
        let pool: PoolConfig = serde_yaml::from_value(fm["pool"].clone()).unwrap();
        assert_eq!(pool.name(), Some("WinPool"));
        assert_eq!(pool.vm_image(), None);
        assert_eq!(pool.os(), "windows");
        assert!(pool.demands().is_empty());
    }

    #[test]
    fn test_pool_config_object_form_default_os() {
        let yaml = "pool:\n  name: LinuxPool";
        let fm: serde_yaml::Value = serde_yaml::from_str(yaml).unwrap();
        let pool: PoolConfig = serde_yaml::from_value(fm["pool"].clone()).unwrap();
        assert_eq!(pool.name(), Some("LinuxPool"));
        assert_eq!(pool.vm_image(), None);
        assert_eq!(pool.os(), "linux");
        assert!(pool.demands().is_empty());
    }

    #[test]
    fn test_pool_config_object_vm_image_form() {
        let yaml = "pool:\n  vmImage: ubuntu-22.04";
        let fm: serde_yaml::Value = serde_yaml::from_str(yaml).unwrap();
        let pool: PoolConfig = serde_yaml::from_value(fm["pool"].clone()).unwrap();
        assert_eq!(pool.name(), None);
        assert_eq!(pool.vm_image(), Some("ubuntu-22.04"));
        assert_eq!(pool.os(), "linux");
        assert!(pool.demands().is_empty());
    }

    #[test]
    fn test_pool_config_object_form_with_demands() {
        let yaml = "pool:\n  name: CustomPool\n  demands:\n    - CustomCapability -equals required-value\n    - Agent.OS -equals Linux";
        let fm: serde_yaml::Value = serde_yaml::from_str(yaml).unwrap();
        let pool: PoolConfig = serde_yaml::from_value(fm["pool"].clone()).unwrap();
        assert_eq!(pool.name(), Some("CustomPool"));
        assert_eq!(pool.vm_image(), None);
        assert_eq!(pool.os(), "linux");
        assert_eq!(
            pool.demands(),
            &[
                "CustomCapability -equals required-value".to_string(),
                "Agent.OS -equals Linux".to_string()
            ]
        );
    }

    #[test]
    fn test_pool_config_default_is_empty() {
        // The struct default is all-empty; the "no pool -> ubuntu-22.04"
        // behaviour lives in resolve_pool_typed(None), not the type default.
        let pool = PoolConfig::default();
        assert_eq!(pool.name(), None);
        assert_eq!(pool.vm_image(), None);
        assert_eq!(pool.os(), "linux");
        assert!(pool.demands().is_empty());
        assert!(pool.overrides().is_empty());
    }

    #[test]
    fn test_legacy_scalar_pool_migrated_by_codemod() {
        // A legacy scalar `pool: MyPool` source is no longer a typed form, but
        // it still compiles: codemod 0002_pool_object_form rewrites it to the
        // object form before typed deserialization. This locks in that legacy
        // sources are not broken by the object-only typed layer.
        let content = "---\nname: x\ndescription: y\npool: MyPool\n---\nbody\n";
        let (fm, _) = super::super::common::parse_markdown(content).unwrap();
        let pool = fm.pool.expect("pool present");
        assert_eq!(pool.name(), Some("MyPool"));
        assert_eq!(pool.vm_image(), None);
    }

    // ─── ScheduleConfig deserialization ─────────────────────────────────────

    #[test]
    fn test_schedule_config_with_options_empty_branches() {
        let yaml = "run: hourly";
        let opts: ScheduleOptions = serde_yaml::from_str(yaml).unwrap();
        let sc = ScheduleConfig::WithOptions(opts);
        assert_eq!(sc.expression(), "hourly");
        assert!(sc.branches().is_empty());
    }

    #[test]
    fn test_schedule_config_deserialized_as_simple_string() {
        let yaml = "schedule: daily around 14:00";
        let fm: serde_yaml::Value = serde_yaml::from_str(yaml).unwrap();
        let sc: ScheduleConfig = serde_yaml::from_value(fm["schedule"].clone()).unwrap();
        assert_eq!(sc.expression(), "daily around 14:00");
        assert!(sc.branches().is_empty());
    }

    #[test]
    fn test_schedule_config_deserialized_as_object() {
        let yaml = "schedule:\n  run: weekly on friday\n  branches:\n    - main\n    - develop";
        let fm: serde_yaml::Value = serde_yaml::from_str(yaml).unwrap();
        let sc: ScheduleConfig = serde_yaml::from_value(fm["schedule"].clone()).unwrap();
        assert_eq!(sc.expression(), "weekly on friday");
        assert_eq!(sc.branches(), &["main", "develop"]);
    }

    // ─── EngineConfig deserialization ────────────────────────────────────────

    #[test]
    fn test_engine_config_full_object_partial_fields() {
        let yaml = "timeout-minutes: 10";
        let opts: EngineOptions = serde_yaml::from_str(yaml).unwrap();
        let ec = EngineConfig::Full(Box::new(opts));
        // id defaults to "copilot" when not specified
        assert_eq!(ec.engine_id(), "copilot");
        // model is None when not specified (engine impl decides default)
        assert_eq!(ec.model(), None);
        assert_eq!(ec.timeout_minutes(), Some(10));
    }

    #[test]
    fn test_engine_config_default() {
        let ec = EngineConfig::default();
        assert_eq!(ec.engine_id(), "copilot");
        assert_eq!(ec.model(), None);
        assert_eq!(ec.timeout_minutes(), None);
    }

    #[test]
    fn test_engine_config_deserialized_as_string() {
        let yaml = "engine: copilot";
        let fm: serde_yaml::Value = serde_yaml::from_str(yaml).unwrap();
        let ec: EngineConfig = serde_yaml::from_value(fm["engine"].clone()).unwrap();
        assert_eq!(ec.engine_id(), "copilot");
        assert_eq!(ec.model(), None);
        assert_eq!(ec.timeout_minutes(), None);
    }

    #[test]
    fn test_engine_config_deserialized_as_object() {
        let yaml = "engine:\n  id: copilot\n  model: claude-opus-4.5\n  timeout-minutes: 30";
        let fm: serde_yaml::Value = serde_yaml::from_str(yaml).unwrap();
        let ec: EngineConfig = serde_yaml::from_value(fm["engine"].clone()).unwrap();
        assert_eq!(ec.engine_id(), "copilot");
        assert_eq!(ec.model(), Some("claude-opus-4.5"));
        assert_eq!(ec.timeout_minutes(), Some(30));
    }

    #[test]
    fn test_engine_config_full_with_all_gh_aw_fields() {
        let yaml = r#"
id: copilot
model: gpt-5
version: "0.0.422"
agent: my-custom-agent
api-target: api.acme.ghe.com
args: ["--verbose", "--add-dir", "/workspace"]
env:
  DEBUG_MODE: "true"
  AWS_REGION: us-west-2
command: /usr/local/bin/copilot
timeout-minutes: 60
"#;
        let opts: EngineOptions = serde_yaml::from_str(yaml).unwrap();
        let ec = EngineConfig::Full(Box::new(opts));
        assert_eq!(ec.engine_id(), "copilot");
        assert_eq!(ec.model(), Some("gpt-5"));
        assert_eq!(ec.version(), Some("0.0.422"));
        assert_eq!(ec.agent(), Some("my-custom-agent"));
        assert_eq!(ec.api_target(), Some("api.acme.ghe.com"));
        assert_eq!(ec.args(), &["--verbose", "--add-dir", "/workspace"]);
        assert_eq!(ec.command(), Some("/usr/local/bin/copilot"));
        assert_eq!(ec.timeout_minutes(), Some(60));
        let env = ec.env().unwrap();
        assert_eq!(env.get("DEBUG_MODE").unwrap(), "true");
        assert_eq!(env.get("AWS_REGION").unwrap(), "us-west-2");
    }

    // ─── GithubAppTokenConfig ────────────────────────────────────────────

    #[test]
    fn test_engine_github_app_token_deserialized() {
        let yaml = r#"
id: copilot
github-app-token:
  app-id: "1234567"
  private-key: GH_APP_KEY
  owner: octo-org
  repositories: [octo-repo, other-repo]
"#;
        let opts: EngineOptions = serde_yaml::from_str(yaml).unwrap();
        let ec = EngineConfig::Full(Box::new(opts));
        let gat = ec.github_app_token().expect("github-app-token present");
        assert_eq!(gat.app_id, "1234567");
        // Explicit private-key override.
        assert_eq!(gat.private_key.as_deref(), Some("GH_APP_KEY"));
        assert_eq!(gat.private_key_var(), "GH_APP_KEY");
        assert_eq!(gat.owner, "octo-org");
        assert_eq!(gat.repositories, vec!["octo-repo", "other-repo"]);
        assert!(gat.api_url.is_none());
        assert!(!gat.skip_token_revocation);
        gat.validate().expect("valid config passes validation");
    }

    #[test]
    fn test_engine_github_app_token_defaults_private_key_var() {
        // private-key omitted ⇒ default compiler-owned secret variable name.
        let yaml = r#"
id: copilot
github-app-token:
  app-id: 1234567
  owner: octo-org
"#;
        let opts: EngineOptions = serde_yaml::from_str(yaml).unwrap();
        let gat = EngineConfig::Full(Box::new(opts))
            .github_app_token()
            .unwrap()
            .clone();
        assert!(gat.private_key.is_none());
        assert_eq!(gat.private_key_var(), "GITHUB_APP_PRIVATE_KEY");
        gat.validate().expect("default private-key is valid");
    }

    #[test]
    fn test_engine_github_app_token_unquoted_numeric_and_api_url() {
        // Unquoted numeric app-id + api-url + skip-token-revocation.
        let yaml = r#"
id: copilot
github-app-token:
  app-id: 1234567
  owner: octo-org
  api-url: https://ghe.example.com/api/v3
  skip-token-revocation: true
"#;
        let opts: EngineOptions = serde_yaml::from_str(yaml).unwrap();
        let gat = EngineConfig::Full(Box::new(opts))
            .github_app_token()
            .unwrap()
            .clone();
        assert_eq!(gat.app_id, "1234567");
        assert_eq!(gat.api_url.as_deref(), Some("https://ghe.example.com/api/v3"));
        assert!(gat.skip_token_revocation);
        gat.validate().expect("numeric app-id + https api-url is valid");
    }

    #[test]
    fn test_engine_github_app_token_accepts_client_id() {
        // Alphanumeric client ID is a valid literal app-id (regression guard
        // against the removed digits-only heuristic).
        let yaml = r#"
id: copilot
github-app-token:
  app-id: Iv23liABCdef
  owner: octo-org
"#;
        let opts: EngineOptions = serde_yaml::from_str(yaml).unwrap();
        let gat = EngineConfig::Full(Box::new(opts))
            .github_app_token()
            .unwrap()
            .clone();
        assert_eq!(gat.app_id, "Iv23liABCdef");
        gat.validate().expect("client-id app-id is valid");
    }

    #[test]
    fn test_github_app_token_validate_rejects_non_https_api_url() {
        let yaml = r#"
id: copilot
github-app-token:
  app-id: 1234567
  owner: octo-org
  api-url: http://insecure.example.com/api/v3
"#;
        let opts: EngineOptions = serde_yaml::from_str(yaml).unwrap();
        let gat = EngineConfig::Full(Box::new(opts))
            .github_app_token()
            .unwrap()
            .clone();
        let err = gat.validate().unwrap_err().to_string();
        assert!(err.contains("api-url"), "unexpected error: {err}");
    }

    #[test]
    fn test_engine_github_app_token_absent_by_default() {
        let ec = EngineConfig::default();
        assert!(ec.github_app_token().is_none());
        let opts: EngineOptions = serde_yaml::from_str("id: copilot").unwrap();
        assert!(EngineConfig::Full(Box::new(opts)).github_app_token().is_none());
    }

    #[test]
    fn test_engine_github_app_token_repositories_optional() {
        let yaml = r#"
id: copilot
github-app-token:
  app-id: 1234567
  owner: octo-org
"#;
        let opts: EngineOptions = serde_yaml::from_str(yaml).unwrap();
        let gat = EngineConfig::Full(Box::new(opts)).github_app_token().unwrap().clone();
        assert!(gat.repositories.is_empty());
        gat.validate().unwrap();
    }

    #[test]
    fn test_github_app_token_validate_rejects_bad_app_id() {
        let gat = GithubAppTokenConfig {
            app_id: "not a valid id".to_string(),
            private_key: None,
            owner: "octo-org".to_string(),
            repositories: vec![],
            api_url: None,
            skip_token_revocation: false,
        };
        let err = gat.validate().unwrap_err().to_string();
        assert!(err.contains("app-id"), "unexpected error: {err}");
    }

    #[test]
    fn test_github_app_token_validate_rejects_negative_app_id() {
        // A negative unquoted integer (app-id: -7654321) stringifies to
        // "-7654321"; the leading '-' must be rejected (it is in the charset
        // but is not a valid App ID and would produce a bad JWT `iss`).
        let yaml = r#"
id: copilot
github-app-token:
  app-id: -7654321
  owner: octo-org
"#;
        let opts: EngineOptions = serde_yaml::from_str(yaml).unwrap();
        let gat = EngineConfig::Full(Box::new(opts))
            .github_app_token()
            .unwrap()
            .clone();
        assert_eq!(gat.app_id, "-7654321");
        let err = gat.validate().unwrap_err().to_string();
        assert!(err.contains("app-id"), "unexpected error: {err}");
    }

    #[test]
    fn test_github_app_token_validate_rejects_bad_private_key_override() {
        let gat = GithubAppTokenConfig {
            app_id: "1234567".to_string(),
            private_key: Some("not a var".to_string()),
            owner: "octo-org".to_string(),
            repositories: vec![],
            api_url: None,
            skip_token_revocation: false,
        };
        let err = gat.validate().unwrap_err().to_string();
        assert!(err.contains("private-key"), "unexpected error: {err}");
    }

    #[test]
    fn test_github_app_token_validate_accepts_hyphenated_private_key_override() {
        let gat = GithubAppTokenConfig {
            app_id: "1234567".to_string(),
            private_key: Some("AGENTIC-WORKFLOWS-GITHUB-APP-PRIVATE-KEY".to_string()),
            owner: "octo-org".to_string(),
            repositories: vec![],
            api_url: None,
            skip_token_revocation: false,
        };
        gat.validate()
            .expect("hyphenated ADO variable names are valid macro targets");
        assert_eq!(
            gat.private_key_var(),
            "AGENTIC-WORKFLOWS-GITHUB-APP-PRIVATE-KEY"
        );
    }

    #[test]
    fn test_github_app_token_validate_rejects_private_key_injection_attempts() {
        for private_key in [
            "$(SECRET)",
            "$[variables.SECRET]",
            "${{ variables.SECRET }}",
            "##vso[task.setvariable variable=x]y",
            "##[debug]x",
            "variables['SECRET']",
            "SECRET\"",
        ] {
            let gat = GithubAppTokenConfig {
                app_id: "1234567".to_string(),
                private_key: Some(private_key.to_string()),
                owner: "octo-org".to_string(),
                repositories: vec![],
                api_url: None,
                skip_token_revocation: false,
            };
            let err = gat.validate().unwrap_err().to_string();
            assert!(
                err.contains("private-key"),
                "unexpected error for {private_key:?}: {err}"
            );
        }
    }

    #[test]
    fn test_github_app_token_validate_rejects_bad_owner() {
        let gat = GithubAppTokenConfig {
            app_id: "1234567".to_string(),
            private_key: None,
            owner: "octo/org".to_string(),
            repositories: vec![],
            api_url: None,
            skip_token_revocation: false,
        };
        let err = gat.validate().unwrap_err().to_string();
        assert!(err.contains("owner"), "unexpected error: {err}");
    }

    #[test]
    fn test_github_app_token_validate_rejects_bad_repository() {
        let gat = GithubAppTokenConfig {
            app_id: "1234567".to_string(),
            private_key: None,
            owner: "octo-org".to_string(),
            repositories: vec!["ok-repo".to_string(), "bad;repo".to_string()],
            api_url: None,
            skip_token_revocation: false,
        };
        let err = gat.validate().unwrap_err().to_string();
        assert!(err.contains("repositories"), "unexpected error: {err}");
    }

    // ─── PermissionsConfig deserialization ───────────────────────────────

    #[test]
    fn test_permissions_both_fields() {
        let yaml = "read: my-read-sc\nwrite: my-write-sc";
        let pc: PermissionsConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(pc.read.as_deref(), Some("my-read-sc"));
        assert_eq!(pc.write.as_deref(), Some("my-write-sc"));
    }

    #[test]
    fn test_permissions_read_only() {
        let yaml = "read: my-read-sc";
        let pc: PermissionsConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(pc.read.as_deref(), Some("my-read-sc"));
        assert!(pc.write.is_none());
    }

    #[test]
    fn test_permissions_write_only() {
        let yaml = "write: my-write-sc";
        let pc: PermissionsConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(pc.read.is_none());
        assert_eq!(pc.write.as_deref(), Some("my-write-sc"));
    }

    #[test]
    fn test_permissions_default() {
        // Deserialising `permissions: {}` must produce None/None — guards against
        // accidentally introducing a required field or a non-None serde default.
        let yaml = "permissions: {}";
        let fm: serde_yaml::Value = serde_yaml::from_str(yaml).unwrap();
        let pc: PermissionsConfig =
            serde_yaml::from_value(fm["permissions"].clone()).unwrap();
        assert!(pc.read.is_none());
        assert!(pc.write.is_none());
    }

    #[test]
    fn test_permissions_in_front_matter() {
        let content = r#"---
name: "Test Agent"
description: "Test"
permissions:
  read: my-read-sc
  write: my-write-sc
---

Body
"#;
        let (fm, _) = super::super::common::parse_markdown(content).unwrap();
        let perms = fm.permissions.unwrap();
        assert_eq!(perms.read.as_deref(), Some("my-read-sc"));
        assert_eq!(perms.write.as_deref(), Some("my-write-sc"));
    }

    #[test]
    fn test_permissions_omitted_in_front_matter() {
        let content = r#"---
name: "Test Agent"
description: "Test"
---

Body
"#;
        let (fm, _) = super::super::common::parse_markdown(content).unwrap();
        assert!(fm.permissions.is_none());
    }

    // ─── FrontMatter inlined-imports deserialization ───────────────────────

    #[test]
    fn test_frontmatter_inlined_imports_defaults_to_false() {
        let content = r#"---
name: "Test Agent"
description: "Test"
---

Body
"#;
        let (fm, _) = super::super::common::parse_markdown(content).unwrap();
        assert!(!fm.inlined_imports);
    }

    #[test]
    fn test_frontmatter_inlined_imports_true_explicit() {
        let content = r#"---
name: "Test Agent"
description: "Test"
inlined-imports: true
---

Body
"#;
        let (fm, _) = super::super::common::parse_markdown(content).unwrap();
        assert!(fm.inlined_imports);
    }

    #[test]
    fn test_frontmatter_inlined_imports_false_explicit() {
        let content = r#"---
name: "Test Agent"
description: "Test"
inlined-imports: false
---

Body
"#;
        let (fm, _) = super::super::common::parse_markdown(content).unwrap();
        assert!(!fm.inlined_imports);
    }

    // ─── CacheMemoryToolConfig deserialization ──────────────────────────────

    #[test]
    fn test_cache_memory_bool_true() {
        let content = r#"---
name: "Test"
description: "Test"
tools:
  cache-memory: true
---

Body
"#;
        let (fm, _) = super::super::common::parse_markdown(content).unwrap();
        let cm = fm.tools.as_ref().unwrap().cache_memory.as_ref().unwrap();
        assert!(cm.is_enabled());
        assert!(cm.allowed_extensions().is_empty());
    }

    #[test]
    fn test_cache_memory_bool_false() {
        let content = r#"---
name: "Test"
description: "Test"
tools:
  cache-memory: false
---

Body
"#;
        let (fm, _) = super::super::common::parse_markdown(content).unwrap();
        let cm = fm.tools.as_ref().unwrap().cache_memory.as_ref().unwrap();
        assert!(!cm.is_enabled());
        assert!(cm.allowed_extensions().is_empty());
    }

    #[test]
    fn test_cache_memory_with_options() {
        let content = r#"---
name: "Test"
description: "Test"
tools:
  cache-memory:
    allowed-extensions:
      - .md
      - .json
---

Body
"#;
        let (fm, _) = super::super::common::parse_markdown(content).unwrap();
        let cm = fm.tools.as_ref().unwrap().cache_memory.as_ref().unwrap();
        assert!(cm.is_enabled());
        assert_eq!(cm.allowed_extensions(), &[".md", ".json"]);
    }

    // ─── AzureDevOpsToolConfig deserialization ──────────────────────────────

    #[test]
    fn test_azure_devops_bool_true() {
        let content = r#"---
name: "Test"
description: "Test"
tools:
  azure-devops: true
---

Body
"#;
        let (fm, _) = super::super::common::parse_markdown(content).unwrap();
        let ado = fm.tools.as_ref().unwrap().azure_devops.as_ref().unwrap();
        assert!(ado.is_enabled());
        assert!(ado.toolsets().is_empty());
        assert!(ado.allowed().is_empty());
        assert!(ado.org().is_none());
    }

    #[test]
    fn test_azure_devops_with_options() {
        let content = r#"---
name: "Test"
description: "Test"
tools:
  azure-devops:
    toolsets: [repos, wit, core]
    allowed: [wit_get_work_item, core_list_projects]
    org: myorg
---

Body
"#;
        let (fm, _) = super::super::common::parse_markdown(content).unwrap();
        let ado = fm.tools.as_ref().unwrap().azure_devops.as_ref().unwrap();
        assert!(ado.is_enabled());
        assert_eq!(ado.toolsets(), &["repos", "wit", "core"]);
        assert_eq!(ado.allowed(), &["wit_get_work_item", "core_list_projects"]);
        assert_eq!(ado.org(), Some("myorg"));
    }

    #[test]
    fn test_azure_devops_partial_config() {
        let content = r#"---
name: "Test"
description: "Test"
tools:
  azure-devops:
    toolsets: [wit]
---

Body
"#;
        let (fm, _) = super::super::common::parse_markdown(content).unwrap();
        let ado = fm.tools.as_ref().unwrap().azure_devops.as_ref().unwrap();
        assert!(ado.is_enabled());
        assert_eq!(ado.toolsets(), &["wit"]);
        assert!(ado.allowed().is_empty());
        assert!(ado.org().is_none());
    }

    // ─── LeanRuntimeConfig deserialization ──────────────────────────────

    #[test]
    fn test_lean_bool_true() {
        let content = r#"---
name: "Test"
description: "Test"
runtimes:
  lean: true
---

Body
"#;
        let (fm, _) = super::super::common::parse_markdown(content).unwrap();
        let lean = fm.runtimes.as_ref().unwrap().lean.as_ref().unwrap();
        assert!(lean.is_enabled());
        assert!(lean.toolchain().is_none());
    }

    #[test]
    fn test_lean_bool_false() {
        let content = r#"---
name: "Test"
description: "Test"
runtimes:
  lean: false
---

Body
"#;
        let (fm, _) = super::super::common::parse_markdown(content).unwrap();
        let lean = fm.runtimes.as_ref().unwrap().lean.as_ref().unwrap();
        assert!(!lean.is_enabled());
        assert!(lean.toolchain().is_none());
    }

    #[test]
    fn test_lean_with_toolchain() {
        let content = r#"---
name: "Test"
description: "Test"
runtimes:
  lean:
    toolchain: "leanprover/lean4:v4.29.1"
---

Body
"#;
        let (fm, _) = super::super::common::parse_markdown(content).unwrap();
        let lean = fm.runtimes.as_ref().unwrap().lean.as_ref().unwrap();
        assert!(lean.is_enabled());
        assert_eq!(lean.toolchain(), Some("leanprover/lean4:v4.29.1"));
    }

    #[test]
    fn test_all_tools_and_runtimes_together() {
        let content = r#"---
name: "Test"
description: "Test"
tools:
  bash: ["cat", "ls"]
  edit: true
  cache-memory: true
  azure-devops:
    toolsets: [wit]
runtimes:
  lean: true
---

Body
"#;
        let (fm, _) = super::super::common::parse_markdown(content).unwrap();
        let tools = fm.tools.as_ref().unwrap();
        assert!(tools.cache_memory.as_ref().unwrap().is_enabled());
        assert!(tools.azure_devops.as_ref().unwrap().is_enabled());
        assert_eq!(tools.bash.as_ref().unwrap(), &["cat", "ls"]);
        assert_eq!(tools.edit, Some(true));
        let runtimes = fm.runtimes.as_ref().unwrap();
        assert!(runtimes.lean.as_ref().unwrap().is_enabled());
    }

    // ─── NetworkConfig deny_unknown_fields ──────────────────────────────────

    #[test]
    fn test_network_config_rejects_old_allow_field() {
        let content = r#"---
name: "Test"
description: "Test"
network:
  allow:
    - "*.mycompany.com"
---

Body
"#;
        let result = super::super::common::parse_markdown(content);
        assert!(
            result.is_err(),
            "network.allow (old field name) should be rejected"
        );
        let err = format!("{:#}", result.unwrap_err());
        assert!(
            err.contains("unknown field `allow`"),
            "error should mention unknown field `allow`, got: {}",
            err
        );
    }

    #[test]
    fn test_network_config_accepts_allowed_field() {
        let content = r#"---
name: "Test"
description: "Test"
network:
  allowed:
    - "*.mycompany.com"
---

Body
"#;
        let (fm, _) = super::super::common::parse_markdown(content).unwrap();
        let net = fm.network.unwrap();
        assert_eq!(net.allowed, vec!["*.mycompany.com"]);
        assert!(net.blocked.is_empty());
    }

    #[test]
    fn test_network_config_rejects_arbitrary_unknown_field() {
        let content = r#"---
name: "Test"
description: "Test"
network:
  typo-field: true
---

Body
"#;
        let result = super::super::common::parse_markdown(content);
        assert!(
            result.is_err(),
            "unknown fields in network should be rejected"
        );
        let err = format!("{:#}", result.unwrap_err());
        assert!(
            err.contains("typo-field") || err.contains("unknown field"),
            "error should mention the unknown field, got: {}",
            err
        );
    }

    // ─── FrontMatter deny_unknown_fields ─────────────────────────────────────

    #[test]
    fn test_front_matter_rejects_unknown_top_level_field() {
        let content = r#"---
name: "Test"
description: "Test"
safeoutputs:
  upload-pipeline-artifact: {}
---

Body
"#;
        let result = super::super::common::parse_markdown(content);
        assert!(
            result.is_err(),
            "unknown top-level field 'safeoutputs' should be rejected"
        );
        let err = format!("{:#}", result.unwrap_err());
        assert!(
            err.contains("unknown field `safeoutputs`"),
            "error should mention unknown field `safeoutputs`, got: {}",
            err
        );
    }

    #[test]
    fn test_front_matter_rejects_top_level_schedule() {
        let content = r#"---
name: "Test"
description: "Test"
schedule: daily around 14:00
---

Body
"#;
        let result = super::super::common::parse_markdown(content);
        assert!(
            result.is_err(),
            "top-level 'schedule' should be rejected (use on.schedule)"
        );
        let err = format!("{:#}", result.unwrap_err());
        assert!(
            err.contains("unknown field `schedule`"),
            "error should mention unknown field `schedule`, got: {}",
            err
        );
    }

    #[test]
    fn test_front_matter_accepts_safe_outputs_with_hyphen() {
        let content = r#"---
name: "Test"
description: "Test"
safe-outputs:
  upload-pipeline-artifact: {}
---

Body
"#;
        let (fm, _) = super::super::common::parse_markdown(content).unwrap();
        assert!(fm.safe_outputs.contains_key("upload-pipeline-artifact"));
    }

    #[test]
    fn test_require_approval_global_bool() {
        let content = r#"---
name: "Test"
description: "Test"
safe-outputs:
  require-approval: true
  create-pull-request: {}
  add-pr-comment: {}
---

Body
"#;
        let (fm, _) = super::super::common::parse_markdown(content).unwrap();
        // Reserved key is not surfaced as a tool name.
        let tools: Vec<&String> = fm.safe_output_tool_names().collect();
        assert!(!tools.iter().any(|t| t.as_str() == "require-approval"));
        assert_eq!(tools.len(), 2);
        // Global default makes every tool require approval.
        assert!(fm.tool_requires_approval("create-pull-request").is_some());
        assert!(fm.tool_requires_approval("add-pr-comment").is_some());
        let (auto, reviewed) = fm.partition_safe_outputs_by_approval();
        assert!(auto.is_empty());
        assert_eq!(reviewed, vec!["add-pr-comment", "create-pull-request"]);
    }

    #[test]
    fn test_require_approval_per_tool_override() {
        let content = r#"---
name: "Test"
description: "Test"
safe-outputs:
  require-approval: true
  create-pull-request:
    require-approval: false
  add-pr-comment: {}
---

Body
"#;
        let (fm, _) = super::super::common::parse_markdown(content).unwrap();
        // Per-tool false overrides the global true.
        assert!(fm.tool_requires_approval("create-pull-request").is_none());
        assert!(fm.tool_requires_approval("add-pr-comment").is_some());
        let (auto, reviewed) = fm.partition_safe_outputs_by_approval();
        assert_eq!(auto, vec!["create-pull-request"]);
        assert_eq!(reviewed, vec!["add-pr-comment"]);
    }

    #[test]
    fn test_require_approval_detailed_object() {
        let content = r#"---
name: "Test"
description: "Test"
safe-outputs:
  create-pull-request:
    require-approval:
      approvers: ["[Org]\\release"]
      notify-users: ["ops@example.com"]
      timeout-minutes: 120
      on-timeout: resume
      instructions: "Review the proposed PR."
---

Body
"#;
        let (fm, _) = super::super::common::parse_markdown(content).unwrap();
        let cfg = fm
            .tool_requires_approval("create-pull-request")
            .expect("approval required");
        assert_eq!(cfg.approvers, vec!["[Org]\\release"]);
        assert_eq!(cfg.notify_users, vec!["ops@example.com"]);
        assert_eq!(cfg.timeout_minutes, Some(120));
        assert_eq!(cfg.on_timeout, Some(ApprovalOnTimeout::Resume));
        assert_eq!(cfg.instructions.as_deref(), Some("Review the proposed PR."));
    }

    #[test]
    fn test_require_approval_absent_means_no_review() {
        let content = r#"---
name: "Test"
description: "Test"
safe-outputs:
  create-pull-request: {}
---

Body
"#;
        let (fm, _) = super::super::common::parse_markdown(content).unwrap();
        assert!(fm.tool_requires_approval("create-pull-request").is_none());
        let (auto, reviewed) = fm.partition_safe_outputs_by_approval();
        assert_eq!(auto, vec!["create-pull-request"]);
        assert!(reviewed.is_empty());
    }

    #[test]
    fn test_validate_require_approval_accepts_valid() {
        let content = r#"---
name: "Test"
description: "Test"
safe-outputs:
  require-approval: true
  create-pull-request:
    require-approval:
      on-timeout: reject
---

Body
"#;
        let (fm, _) = super::super::common::parse_markdown(content).unwrap();
        assert!(fm.validate_require_approval().is_ok());
    }

    #[test]
    fn test_validate_require_approval_rejects_bad_on_timeout() {
        // A typo in `on-timeout` must NOT silently disable the gate — it has
        // to surface as a compilation error (regression test for the
        // `.ok()`-swallowed-error bug).
        let content = r#"---
name: "Test"
description: "Test"
safe-outputs:
  create-pull-request:
    require-approval:
      on-timeout: rejec
---

Body
"#;
        let (fm, _) = super::super::common::parse_markdown(content).unwrap();
        let err = fm
            .validate_require_approval()
            .expect_err("malformed on-timeout must be rejected");
        let msg = err.to_string();
        assert!(
            msg.contains("create-pull-request") && msg.contains("require-approval"),
            "error should name the offending tool and field: {msg}"
        );
    }

    #[test]
    fn test_validate_require_approval_rejects_unknown_field() {
        // `ApprovalConfig` uses deny_unknown_fields — a misspelled key must
        // error rather than silently drop the tool from the reviewed list.
        let content = r#"---
name: "Test"
description: "Test"
safe-outputs:
  require-approval:
    approver: ["[Org]\\release"]
---

Body
"#;
        let (fm, _) = super::super::common::parse_markdown(content).unwrap();
        assert!(
            fm.validate_require_approval().is_err(),
            "unknown require-approval field must be rejected"
        );
    }

    #[test]
    fn test_validate_require_approval_rejects_ado_template_expression() {
        // A `${{ ... }}` template expression in an approval identity field is
        // expanded at queue time and could leak a pipeline value — it must be
        // rejected at compile time.
        let content = r#"---
name: "Test"
description: "Test"
safe-outputs:
  create-pull-request:
    require-approval:
      approvers: ["${{ variables['secret-token'] }}"]
---

Body
"#;
        let (fm, _) = super::super::common::parse_markdown(content).unwrap();
        let err = fm
            .validate_require_approval()
            .expect_err("ADO template expression in approvers must be rejected");
        assert!(
            err.to_string().contains("template expression"),
            "error should explain the template-expression rejection: {err}"
        );
    }

    #[test]
    fn test_validate_require_approval_allows_runtime_macro_in_instructions() {
        // `instructions` documents support for `$(...)` runtime interpolation,
        // so a macro (not a `${{ }}` template) must be allowed.
        let content = r#"---
name: "Test"
description: "Test"
safe-outputs:
  create-pull-request:
    require-approval:
      instructions: "Review build $(Build.BuildId) before approving."
---

Body
"#;
        let (fm, _) = super::super::common::parse_markdown(content).unwrap();
        assert!(
            fm.validate_require_approval().is_ok(),
            "runtime macro $(...) in instructions must be allowed"
        );
    }

    #[test]
    fn test_front_matter_parses_ado_aw_debug() {
        let content = r#"---
name: "Test"
description: "Test"
ado-aw-debug:
  skip-integrity: true
  create-issue:
    target-repo: githubnext/ado-aw
    title-prefix: "[bug] "
    labels: [pipeline-failure]
    allowed-labels: ["agent-*"]
    assignees: [jamesdevine]
---

Body
"#;
        let (fm, _) = super::super::common::parse_markdown(content).unwrap();
        let debug = fm.ado_aw_debug.expect("ado-aw-debug should parse");
        assert!(debug.skip_integrity);
        let ci = debug.create_issue.expect("create-issue should parse");
        assert_eq!(ci.target_repo, "githubnext/ado-aw");
        assert_eq!(ci.title_prefix.as_deref(), Some("[bug] "));
        assert_eq!(ci.labels, vec!["pipeline-failure".to_string()]);
        assert_eq!(ci.allowed_labels, vec!["agent-*".to_string()]);
        assert_eq!(ci.assignees, vec!["jamesdevine".to_string()]);
    }

    #[test]
    fn test_front_matter_ado_aw_debug_defaults() {
        let content = r#"---
name: "Test"
description: "Test"
ado-aw-debug: {}
---

Body
"#;
        let (fm, _) = super::super::common::parse_markdown(content).unwrap();
        let debug = fm.ado_aw_debug.unwrap();
        assert!(!debug.skip_integrity);
        assert!(debug.create_issue.is_none());
    }

    #[test]
    fn test_front_matter_ado_aw_debug_rejects_unknown_field() {
        let content = r#"---
name: "Test"
description: "Test"
ado-aw-debug:
  bogus-knob: true
---

Body
"#;
        let result = super::super::common::parse_markdown(content);
        assert!(
            result.is_err(),
            "unknown ado-aw-debug field should be rejected"
        );
        let err = format!("{:#}", result.unwrap_err());
        assert!(
            err.contains("bogus-knob") || err.contains("unknown field"),
            "expected error to mention unknown field, got: {}",
            err
        );
    }

    // ─── PrTriggerConfig deserialization ─────────────────────────────────────
    // NOTE: These tests use `triggers:` as a wrapper key and deserialize
    // OnConfig directly (not through FrontMatter). They test struct
    // deserialization in isolation. The `on:` rename is tested via
    // `test_pr_trigger_in_full_front_matter` at the bottom of this section.

    #[test]
    fn test_pr_trigger_config_title_filter() {
        let yaml = r#"
triggers:
  pr:
    filters:
      title: "*[agent]*"
"#;
        let val: serde_yaml::Value = serde_yaml::from_str(yaml).unwrap();
        let tc: OnConfig = serde_yaml::from_value(val["triggers"].clone()).unwrap();
        let pr = tc.pr.unwrap();
        let filters = pr.filters.unwrap();
        assert_eq!(filters.title.unwrap().pattern, "*[agent]*");
    }

    #[test]
    fn test_pr_trigger_config_author_filter() {
        let yaml = r#"
triggers:
  pr:
    filters:
      author:
        include: ["alice@corp.com", "bob@corp.com"]
        exclude: ["bot@noreply.com"]
"#;
        let val: serde_yaml::Value = serde_yaml::from_str(yaml).unwrap();
        let tc: OnConfig = serde_yaml::from_value(val["triggers"].clone()).unwrap();
        let pr = tc.pr.unwrap();
        let author = pr.filters.unwrap().author.unwrap();
        assert_eq!(author.include, vec!["alice@corp.com", "bob@corp.com"]);
        assert_eq!(author.exclude, vec!["bot@noreply.com"]);
    }

    #[test]
    fn test_pr_trigger_config_branch_filters() {
        let yaml = r#"
triggers:
  pr:
    branches:
      include: [main, "release/*"]
      exclude: ["test/*"]
    filters:
      source-branch: "feature/*"
      target-branch: "main"
"#;
        let val: serde_yaml::Value = serde_yaml::from_str(yaml).unwrap();
        let tc: OnConfig = serde_yaml::from_value(val["triggers"].clone()).unwrap();
        let pr = tc.pr.unwrap();
        let branches = pr.branches.unwrap();
        assert_eq!(branches.include, vec!["main", "release/*"]);
        assert_eq!(branches.exclude, vec!["test/*"]);
        let filters = pr.filters.unwrap();
        assert_eq!(filters.source_branch.unwrap().pattern, "feature/*");
        assert_eq!(filters.target_branch.unwrap().pattern, "main");
    }

    #[test]
    fn test_pr_trigger_config_label_filter() {
        let yaml = r#"
triggers:
  pr:
    filters:
      labels:
        any-of: ["run-agent", "automated"]
        none-of: ["do-not-run"]
"#;
        let val: serde_yaml::Value = serde_yaml::from_str(yaml).unwrap();
        let tc: OnConfig = serde_yaml::from_value(val["triggers"].clone()).unwrap();
        let labels = tc.pr.unwrap().filters.unwrap().labels.unwrap();
        assert_eq!(labels.any_of, vec!["run-agent", "automated"]);
        assert!(labels.all_of.is_empty());
        assert_eq!(labels.none_of, vec!["do-not-run"]);
    }

    #[test]
    fn test_pr_trigger_config_draft_filter() {
        let yaml = r#"
triggers:
  pr:
    filters:
      draft: false
"#;
        let val: serde_yaml::Value = serde_yaml::from_str(yaml).unwrap();
        let tc: OnConfig = serde_yaml::from_value(val["triggers"].clone()).unwrap();
        assert_eq!(tc.pr.unwrap().filters.unwrap().draft, Some(false));
    }

    #[test]
    fn test_pr_trigger_config_changed_files_filter() {
        let yaml = r#"
triggers:
  pr:
    filters:
      changed-files:
        include: ["src/**/*.rs"]
        exclude: ["docs/**"]
"#;
        let val: serde_yaml::Value = serde_yaml::from_str(yaml).unwrap();
        let tc: OnConfig = serde_yaml::from_value(val["triggers"].clone()).unwrap();
        let changed = tc.pr.unwrap().filters.unwrap().changed_files.unwrap();
        assert_eq!(changed.include, vec!["src/**/*.rs"]);
        assert_eq!(changed.exclude, vec!["docs/**"]);
    }

    #[test]
    fn test_pr_trigger_config_mode_default_synthetic() {
        let yaml = r#"
triggers:
  pr:
    branches:
      include: [main]
"#;
        let val: serde_yaml::Value = serde_yaml::from_str(yaml).unwrap();
        let tc: OnConfig = serde_yaml::from_value(val["triggers"].clone()).unwrap();
        assert_eq!(
            tc.pr.unwrap().mode,
            PrMode::Synthetic,
            "mode must default to synthetic when omitted"
        );
    }

    #[test]
    fn test_pr_trigger_config_mode_explicit_policy() {
        let yaml = r#"
triggers:
  pr:
    branches:
      include: [main]
    mode: policy
"#;
        let val: serde_yaml::Value = serde_yaml::from_str(yaml).unwrap();
        let tc: OnConfig = serde_yaml::from_value(val["triggers"].clone()).unwrap();
        assert_eq!(
            tc.pr.unwrap().mode,
            PrMode::Policy,
            "mode: policy must deserialise correctly"
        );
    }

    #[test]
    fn test_pr_trigger_config_mode_invalid_value_errors() {
        let yaml = r#"
triggers:
  pr:
    branches:
      include: [main]
    mode: bananas
"#;
        let val: serde_yaml::Value = serde_yaml::from_str(yaml).unwrap();
        let err = serde_yaml::from_value::<OnConfig>(val["triggers"].clone())
            .expect_err("mode: bananas must be rejected at deserialisation");
        let msg = err.to_string();
        assert!(
            msg.contains("bananas") || msg.contains("variant") || msg.contains("unknown"),
            "error must mention the bad variant; got: {msg}"
        );
    }

    #[test]
    fn test_pr_trigger_config_paths_only() {
        let yaml = r#"
triggers:
  pr:
    paths:
      include: ["src/*"]
"#;
        let val: serde_yaml::Value = serde_yaml::from_str(yaml).unwrap();
        let tc: OnConfig = serde_yaml::from_value(val["triggers"].clone()).unwrap();
        let pr = tc.pr.unwrap();
        assert!(pr.filters.is_none());
        assert_eq!(pr.paths.unwrap().include, vec!["src/*"]);
    }

    #[test]
    fn test_pr_trigger_config_combined_with_pipeline_trigger() {
        let yaml = r#"
triggers:
  pipeline:
    name: "Build Pipeline"
  pr:
    filters:
      title: "*[review]*"
"#;
        let val: serde_yaml::Value = serde_yaml::from_str(yaml).unwrap();
        let tc: OnConfig = serde_yaml::from_value(val["triggers"].clone()).unwrap();
        assert_eq!(tc.pipeline.as_ref().unwrap().name, "Build Pipeline");
        assert_eq!(
            tc.pr.unwrap().filters.unwrap().title.unwrap().pattern,
            "*[review]*"
        );
    }

    #[test]
    fn test_pr_trigger_config_empty_filters() {
        let yaml = r#"
triggers:
  pr:
    filters: {}
"#;
        let val: serde_yaml::Value = serde_yaml::from_str(yaml).unwrap();
        let tc: OnConfig = serde_yaml::from_value(val["triggers"].clone()).unwrap();
        // `filters: {}` must produce a Some with every optional field defaulting to None
        let filters = tc.pr.unwrap().filters.unwrap();
        assert!(filters.title.is_none());
        assert!(filters.author.is_none());
        assert!(filters.source_branch.is_none());
        assert!(filters.target_branch.is_none());
        assert!(filters.commit_message.is_none());
        assert!(filters.labels.is_none());
        assert!(filters.draft.is_none());
        assert!(filters.changed_files.is_none());
        assert!(filters.time_window.is_none());
        assert!(filters.min_changes.is_none());
        assert!(filters.max_changes.is_none());
        assert!(filters.build_reason.is_none());
        assert!(filters.expression.is_none());
    }

    #[test]
    fn test_pr_trigger_in_full_front_matter() {
        let content = r#"---
name: "Test Agent"
description: "Test"
on:
  pr:
    branches:
      include: [main]
    filters:
      title: "*[agent]*"
      draft: false
---

Body
"#;
        let (fm, _) = super::super::common::parse_markdown(content).unwrap();
        let pr = fm.on_config.unwrap().pr.unwrap();
        assert_eq!(pr.branches.unwrap().include, vec!["main"]);
        let filters = pr.filters.unwrap();
        assert_eq!(filters.title.unwrap().pattern, "*[agent]*");
        assert_eq!(filters.draft, Some(false));
    }

    #[test]
    fn test_front_matter_safe_outputs_report_failure_config() {
        let content = r#"---
name: "Test Agent"
description: "Test"
safe-outputs:
  report-failure-as-work-item: false
  noop:
    title-prefix: "[ado-aw] Agent noop"
    work-item-type: Task
    area-path: "MyProject\\MyTeam"
    tags:
      - agent-noop
---

Body
"#;
        let (fm, _) = super::super::common::parse_markdown(content).unwrap();
        // report-failure-as-work-item is stored as opaque JSON in safe_outputs HashMap
        let report_flag = fm
            .safe_outputs
            .get("report-failure-as-work-item")
            .and_then(|v| v.as_bool());
        assert_eq!(report_flag, Some(false));
        // noop config with flat fields
        let noop = fm.safe_outputs.get("noop").unwrap();
        assert_eq!(noop.get("title-prefix").and_then(|v| v.as_str()), Some("[ado-aw] Agent noop"));
        assert_eq!(noop.get("area-path").and_then(|v| v.as_str()), Some("MyProject\\MyTeam"));
    }

    #[test]
    fn test_front_matter_safe_outputs_noop_disabled() {
        let content = r#"---
name: "Test Agent"
description: "Test"
safe-outputs:
  noop: false
---

Body
"#;
        let (fm, _) = super::super::common::parse_markdown(content).unwrap();
        let noop = fm.safe_outputs.get("noop").unwrap();
        assert_eq!(noop.as_bool(), Some(false));
    }

    #[test]
    fn test_create_pr_config_resolves_explicit_and_default_target() {
        use std::collections::HashMap;
        let no_refs: HashMap<String, String> = HashMap::new();

        // Explicit target-branch is returned verbatim for self.
        let explicit = r#"---
name: "PR Agent"
description: "opens a PR"
safe-outputs:
  create-pull-request:
    target-branch: release/2.x
---

Body
"#;
        let (fm, _) = super::super::common::parse_markdown(explicit).unwrap();
        let cfg = fm.create_pr_config().unwrap();
        assert_eq!(cfg.resolve_target_branch("self", &no_refs), "release/2.x");

        // Bare `create-pull-request: {}` (target-branch key absent) falls back
        // to the shared CreatePrConfig default.
        let implicit = r#"---
name: "PR Agent"
description: "opens a PR"
safe-outputs:
  create-pull-request: {}
---

Body
"#;
        let (fm, _) = super::super::common::parse_markdown(implicit).unwrap();
        let default_branch = crate::safe_outputs::CreatePrConfig::default().target_branch;
        assert_eq!(
            fm.create_pr_config().unwrap().resolve_target_branch("self", &no_refs),
            default_branch
        );
        // Guard the shared default so a future rename can't silently make the
        // prepare step fetch a branch the executor doesn't target.
        assert_eq!(
            default_branch, "main",
            "CreatePrConfig default target branch must remain 'main'"
        );

        // Absent create-pull-request ⇒ None (no prepare step emitted).
        let absent = r#"---
name: "WI Agent"
description: "files a work item"
safe-outputs:
  create-work-item:
    work-item-type: Task
---

Body
"#;
        let (fm, _) = super::super::common::parse_markdown(absent).unwrap();
        assert!(fm.create_pr_config().is_none());
    }

    #[test]
    fn test_create_pr_config_null_maps_to_defaults() {
        // A bare `create-pull-request:` (YAML null) is the "enable with
        // defaults" idiom — create_pr_config() returns the default config
        // (Some, so the prepare step is still emitted) without treating it as a
        // malformed config.
        let content = r#"---
name: "Null PR Agent"
description: "bare create-pull-request"
safe-outputs:
  create-pull-request:
---

Body
"#;
        let (fm, _) = super::super::common::parse_markdown(content).unwrap();
        let cfg = fm
            .create_pr_config()
            .expect("bare create-pull-request must still enable the tool");
        assert_eq!(cfg.target_branch, "main");
        assert!(cfg.target_branches.is_empty());
        assert!(!cfg.infer_target_from_checkout_ref);
        // Absent create-pull-request ⇒ None.
        let absent = r#"---
name: "No PR"
description: "x"
safe-outputs:
  create-work-item:
    work-item-type: Task
---

Body
"#;
        let (fm, _) = super::super::common::parse_markdown(absent).unwrap();
        assert!(fm.create_pr_config().is_none());
    }

    #[test]
    fn test_front_matter_safe_outputs_noop_object_form() {
        // `noop: {}` must parse as an empty mapping — distinct from
        // `noop: false` (disable) and bare `noop:` (null/default).
        let content = r#"---
name: "Test Agent"
description: "Test"
safe-outputs:
  noop: {}
---

Body
"#;
        let (fm, _) = super::super::common::parse_markdown(content).unwrap();
        let noop = fm.safe_outputs.get("noop").expect("noop key must be present");
        assert!(
            noop.is_object(),
            "noop: {{}} must parse as an object, not bool or null; got: {:?}",
            noop
        );
    }
}
