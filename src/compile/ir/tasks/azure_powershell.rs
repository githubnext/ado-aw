//! Typed builder for `AzurePowerShell@5`.
//!
//! [`AzurePowerShell::file`] and [`AzurePowerShell::inline`] return **distinct
//! typestate builders** ([`AzurePowerShellFile`] / [`AzurePowerShellInline`]).
//! `script_path` and `script_arguments` exist only on the file builder, and
//! `script` (the inline body) only on the inline builder, so mixing inputs with
//! the wrong mode is unrepresentable.
//!
//! ADO task reference:
//! <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/azure-powershell-v5>

use super::common::{de_opt_bool_flex, push_bool, push_opt};
use crate::compile::ir::step::TaskStep;
use serde::{Deserialize, Deserializer};
use serde_yaml::Value;

/// Validate an authored `AzurePowerShell@5` `inputs:` mapping (advisory
/// front-matter validation, see [`super::parse`]).
pub(crate) fn validate_inputs(inputs: Value) -> Result<(), String> {
    let mut map = match inputs {
        Value::Mapping(m) => m,
        Value::Null => Default::default(),
        other => return Err(format!("`inputs` must be a mapping, got {other:?}")),
    };
    let script_type = match map.remove("ScriptType") {
        Some(v) => Some(
            v.as_str()
                .ok_or_else(|| "AzurePowerShell@5: `ScriptType` must be a string".to_string())?
                .to_string(),
        ),
        None => None,
    };
    let mode = script_type.as_deref().unwrap_or("FilePath");
    let rest = Value::Mapping(map);

    let result = match mode {
        "FilePath" => serde_yaml::from_value::<AzurePowerShellFile>(rest).map(drop),
        "InlineScript" => serde_yaml::from_value::<AzurePowerShellInline>(rest).map(drop),
        other => return Err(format!("AzurePowerShell@5: unknown ScriptType `{other}`")),
    };
    result.map_err(|e| format!("ScriptType `{mode}`: {e}"))
}

/// Non-terminating error behaviour (`errorActionPreference`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum ErrorActionPreference {
    #[serde(rename = "stop")]
    Stop,
    #[serde(rename = "continue")]
    Continue,
    #[serde(rename = "silentlyContinue")]
    SilentlyContinue,
}

impl ErrorActionPreference {
    /// The exact token the ADO task expects.
    pub fn as_ado_str(self) -> &'static str {
        match self {
            ErrorActionPreference::Stop => "stop",
            ErrorActionPreference::Continue => "continue",
            ErrorActionPreference::SilentlyContinue => "silentlyContinue",
        }
    }
}

/// Azure PowerShell module version selection (`azurePowerShellVersion`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AzurePowerShellVersion {
    /// Use the latest Azure PowerShell version available on the agent
    /// (`azurePowerShellVersion: LatestVersion`).
    Latest,
    /// Pin to a specific semantic version string, e.g. `"8.0.0"`.
    /// Sets `azurePowerShellVersion: OtherVersion` and
    /// `preferredAzurePowerShellVersion: <ver>`.
    Preferred(String),
}

impl<'de> Deserialize<'de> for AzurePowerShellVersion {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let token = String::deserialize(deserializer)?;
        match token.as_str() {
            "LatestVersion" => Ok(Self::Latest),
            // Sentinel: the flat ADO shape carries the pinned version in a
            // *separate* `preferredAzurePowerShellVersion` input, not inside
            // this one, so on deserialization we only know it's `OtherVersion`
            // here. The empty string is a placeholder; the real value is
            // validated via the struct's own `preferred_azure_powershell_version`
            // field. `into_step()` is never called on a deserialized value (the
            // validation path only checks that it *parses*), and in the builder
            // path this variant is constructed with the real version, so the
            // sentinel never reaches emitted YAML.
            "OtherVersion" => Ok(Self::Preferred(String::new())),
            other => Err(serde::de::Error::unknown_variant(
                other,
                &["LatestVersion", "OtherVersion"],
            )),
        }
    }
}

/// Emit the optional setters that are shared by both `AzurePowerShell@5` builders.
macro_rules! shared_setters {
    () => {
        /// `errorActionPreference` — non-terminating error behaviour (default `stop`).
        pub fn error_action_preference(mut self, value: ErrorActionPreference) -> Self {
            self.error_action_preference = Some(value);
            self
        }

        /// `FailOnStandardError` — fail the step when anything is written to stderr.
        pub fn fail_on_standard_error(mut self, value: bool) -> Self {
            self.fail_on_standard_error = Some(value);
            self
        }

        /// `pwsh` — use PowerShell Core (`pwsh`) instead of Windows PowerShell.
        pub fn pwsh(mut self, value: bool) -> Self {
            self.pwsh = Some(value);
            self
        }

        /// `workingDirectory` — working directory for the script.
        pub fn working_directory(mut self, value: impl Into<String>) -> Self {
            self.working_directory = Some(value.into());
            self
        }

        /// Select the Azure PowerShell module version.
        ///
        /// [`AzurePowerShellVersion::Latest`] → `LatestVersion`.
        /// [`AzurePowerShellVersion::Preferred`] → `OtherVersion` +
        /// `preferredAzurePowerShellVersion`.
        pub fn azure_powershell_version(mut self, value: AzurePowerShellVersion) -> Self {
            self.azure_powershell_version = Some(value);
            self
        }

        /// Override the default `displayName` (`"Azure PowerShell Script"`).
        pub fn with_display_name(mut self, value: impl Into<String>) -> Self {
            self.display_name = Some(value.into());
            self
        }
    };
}

/// Builder for `AzurePowerShell@5` in file-path mode (`ScriptType: FilePath`).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AzurePowerShellFile {
    #[serde(rename = "azureSubscription")]
    azure_subscription: String,
    #[serde(rename = "ScriptPath")]
    script_path: String,
    #[serde(rename = "ScriptArguments", default)]
    script_arguments: Option<String>,
    #[serde(rename = "errorActionPreference", default)]
    error_action_preference: Option<ErrorActionPreference>,
    #[serde(
        rename = "FailOnStandardError",
        default,
        deserialize_with = "de_opt_bool_flex"
    )]
    fail_on_standard_error: Option<bool>,
    #[serde(rename = "pwsh", default, deserialize_with = "de_opt_bool_flex")]
    pwsh: Option<bool>,
    #[serde(rename = "workingDirectory", default)]
    working_directory: Option<String>,
    #[serde(rename = "azurePowerShellVersion", default)]
    azure_powershell_version: Option<AzurePowerShellVersion>,
    #[serde(rename = "preferredAzurePowerShellVersion", default)]
    preferred_azure_powershell_version: Option<String>,
    #[serde(skip)]
    display_name: Option<String>,
}

impl AzurePowerShellFile {
    /// `ScriptArguments` — arguments passed to the script file.
    pub fn script_arguments(mut self, value: impl Into<String>) -> Self {
        self.script_arguments = Some(value.into());
        self
    }

    shared_setters!();

    /// Lower into a [`TaskStep`].
    pub fn into_step(self) -> TaskStep {
        let mut t = TaskStep::new(
            "AzurePowerShell@5",
            self.display_name
                .unwrap_or_else(|| "Azure PowerShell Script".into()),
        )
        .with_input("azureSubscription", self.azure_subscription)
        .with_input("ScriptType", "FilePath")
        .with_input("ScriptPath", self.script_path);
        push_opt(&mut t, "ScriptArguments", self.script_arguments);
        if let Some(v) = self.error_action_preference {
            t.inputs.insert(
                "errorActionPreference".to_string(),
                v.as_ado_str().to_string(),
            );
        }
        push_bool(&mut t, "FailOnStandardError", self.fail_on_standard_error);
        push_bool(&mut t, "pwsh", self.pwsh);
        push_opt(&mut t, "workingDirectory", self.working_directory);
        match self.azure_powershell_version {
            None => {}
            Some(AzurePowerShellVersion::Latest) => {
                t.inputs.insert(
                    "azurePowerShellVersion".to_string(),
                    "LatestVersion".to_string(),
                );
            }
            Some(AzurePowerShellVersion::Preferred(ver)) => {
                t.inputs.insert(
                    "azurePowerShellVersion".to_string(),
                    "OtherVersion".to_string(),
                );
                t.inputs
                    .insert("preferredAzurePowerShellVersion".to_string(), ver);
            }
        }
        push_opt(
            &mut t,
            "preferredAzurePowerShellVersion",
            self.preferred_azure_powershell_version,
        );
        t
    }
}

/// Builder for `AzurePowerShell@5` in inline mode (`ScriptType: InlineScript`).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AzurePowerShellInline {
    #[serde(rename = "azureSubscription")]
    azure_subscription: String,
    #[serde(rename = "Inline")]
    script: String,
    #[serde(rename = "errorActionPreference", default)]
    error_action_preference: Option<ErrorActionPreference>,
    #[serde(
        rename = "FailOnStandardError",
        default,
        deserialize_with = "de_opt_bool_flex"
    )]
    fail_on_standard_error: Option<bool>,
    #[serde(rename = "pwsh", default, deserialize_with = "de_opt_bool_flex")]
    pwsh: Option<bool>,
    #[serde(rename = "workingDirectory", default)]
    working_directory: Option<String>,
    #[serde(rename = "azurePowerShellVersion", default)]
    azure_powershell_version: Option<AzurePowerShellVersion>,
    #[serde(rename = "preferredAzurePowerShellVersion", default)]
    preferred_azure_powershell_version: Option<String>,
    #[serde(skip)]
    display_name: Option<String>,
}

impl AzurePowerShellInline {
    shared_setters!();

    /// Lower into a [`TaskStep`].
    pub fn into_step(self) -> TaskStep {
        let mut t = TaskStep::new(
            "AzurePowerShell@5",
            self.display_name
                .unwrap_or_else(|| "Azure PowerShell Script".into()),
        )
        .with_input("azureSubscription", self.azure_subscription)
        .with_input("ScriptType", "InlineScript")
        .with_input("Inline", self.script);
        if let Some(v) = self.error_action_preference {
            t.inputs.insert(
                "errorActionPreference".to_string(),
                v.as_ado_str().to_string(),
            );
        }
        push_bool(&mut t, "FailOnStandardError", self.fail_on_standard_error);
        push_bool(&mut t, "pwsh", self.pwsh);
        push_opt(&mut t, "workingDirectory", self.working_directory);
        match self.azure_powershell_version {
            None => {}
            Some(AzurePowerShellVersion::Latest) => {
                t.inputs.insert(
                    "azurePowerShellVersion".to_string(),
                    "LatestVersion".to_string(),
                );
            }
            Some(AzurePowerShellVersion::Preferred(ver)) => {
                t.inputs.insert(
                    "azurePowerShellVersion".to_string(),
                    "OtherVersion".to_string(),
                );
                t.inputs
                    .insert("preferredAzurePowerShellVersion".to_string(), ver);
            }
        }
        push_opt(
            &mut t,
            "preferredAzurePowerShellVersion",
            self.preferred_azure_powershell_version,
        );
        t
    }
}

/// Entry point for the `AzurePowerShell@5` builders.
///
/// `file` and `inline` return distinct typestate builders so each mode only
/// exposes its valid inputs (`ScriptArguments` is only available on the file
/// builder; the inline body `script` is only accepted by the inline builder).
pub struct AzurePowerShell;

impl AzurePowerShell {
    /// File-path mode: run the PowerShell script at `script_path` using the
    /// Azure service connection `azure_subscription`.
    pub fn file(
        azure_subscription: impl Into<String>,
        script_path: impl Into<String>,
    ) -> AzurePowerShellFile {
        AzurePowerShellFile {
            azure_subscription: azure_subscription.into(),
            script_path: script_path.into(),
            script_arguments: None,
            error_action_preference: None,
            fail_on_standard_error: None,
            pwsh: None,
            working_directory: None,
            azure_powershell_version: None,
            preferred_azure_powershell_version: None,
            display_name: None,
        }
    }

    /// Inline mode: run `script` as an inline PowerShell block using the Azure
    /// service connection `azure_subscription`.
    pub fn inline(
        azure_subscription: impl Into<String>,
        script: impl Into<String>,
    ) -> AzurePowerShellInline {
        AzurePowerShellInline {
            azure_subscription: azure_subscription.into(),
            script: script.into(),
            error_action_preference: None,
            fail_on_standard_error: None,
            pwsh: None,
            working_directory: None,
            azure_powershell_version: None,
            preferred_azure_powershell_version: None,
            display_name: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_mode_required_inputs() {
        let t = AzurePowerShell::file("my-azure-sub", "scripts/deploy.ps1").into_step();
        assert_eq!(t.task, "AzurePowerShell@5");
        assert_eq!(
            t.inputs.get("azureSubscription").map(String::as_str),
            Some("my-azure-sub")
        );
        assert_eq!(
            t.inputs.get("ScriptType").map(String::as_str),
            Some("FilePath")
        );
        assert_eq!(
            t.inputs.get("ScriptPath").map(String::as_str),
            Some("scripts/deploy.ps1")
        );
        assert!(!t.inputs.contains_key("ScriptArguments"));
        assert!(!t.inputs.contains_key("Inline"));
    }

    #[test]
    fn file_mode_with_arguments_and_options() {
        let t = AzurePowerShell::file("sub-conn", "deploy.ps1")
            .script_arguments("-Env Prod")
            .pwsh(true)
            .error_action_preference(ErrorActionPreference::Continue)
            .fail_on_standard_error(true)
            .with_display_name("Deploy to Azure")
            .into_step();
        assert_eq!(t.display_name, "Deploy to Azure");
        assert_eq!(
            t.inputs.get("ScriptArguments").map(String::as_str),
            Some("-Env Prod")
        );
        assert_eq!(t.inputs.get("pwsh").map(String::as_str), Some("true"));
        assert_eq!(
            t.inputs.get("errorActionPreference").map(String::as_str),
            Some("continue")
        );
        assert_eq!(
            t.inputs.get("FailOnStandardError").map(String::as_str),
            Some("true")
        );
    }

    #[test]
    fn inline_mode_sets_script() {
        let t = AzurePowerShell::inline("my-sub", "Write-Host 'hello'").into_step();
        assert_eq!(
            t.inputs.get("ScriptType").map(String::as_str),
            Some("InlineScript")
        );
        assert_eq!(
            t.inputs.get("Inline").map(String::as_str),
            Some("Write-Host 'hello'")
        );
        assert!(!t.inputs.contains_key("ScriptPath"));
        assert!(!t.inputs.contains_key("ScriptArguments"));
    }

    #[test]
    fn preferred_powershell_version() {
        let t = AzurePowerShell::inline("sub", "Get-AzResource")
            .azure_powershell_version(AzurePowerShellVersion::Preferred("8.0.0".into()))
            .into_step();
        assert_eq!(
            t.inputs.get("azurePowerShellVersion").map(String::as_str),
            Some("OtherVersion")
        );
        assert_eq!(
            t.inputs
                .get("preferredAzurePowerShellVersion")
                .map(String::as_str),
            Some("8.0.0")
        );
    }

    #[test]
    fn latest_version_shorthand() {
        let t = AzurePowerShell::file("sub", "script.ps1")
            .azure_powershell_version(AzurePowerShellVersion::Latest)
            .into_step();
        assert_eq!(
            t.inputs.get("azurePowerShellVersion").map(String::as_str),
            Some("LatestVersion")
        );
        assert!(!t.inputs.contains_key("preferredAzurePowerShellVersion"));
    }

    // `script_arguments` is not available on `AzurePowerShellInline` — the
    // following would not compile:
    //   AzurePowerShell::inline("sub", "body").script_arguments("-Arg")
    // That mismatch is unrepresentable rather than silently dropped.
}
