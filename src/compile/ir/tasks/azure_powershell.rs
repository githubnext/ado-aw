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

use super::common::{push_bool, push_opt};
use crate::compile::ir::step::TaskStep;

/// Non-terminating error behaviour (`errorActionPreference`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorActionPreference {
    Stop,
    Continue,
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

/// Optionals shared by both `AzurePowerShell@5` modes.
#[derive(Debug, Clone, Default)]
struct Shared {
    error_action_preference: Option<ErrorActionPreference>,
    fail_on_standard_error: Option<bool>,
    pwsh: Option<bool>,
    working_directory: Option<String>,
    azure_powershell_version: Option<AzurePowerShellVersion>,
}

impl Shared {
    fn apply(self, t: &mut TaskStep) {
        if let Some(v) = self.error_action_preference {
            t.inputs
                .insert("errorActionPreference".to_string(), v.as_ado_str().to_string());
        }
        push_bool(t, "FailOnStandardError", self.fail_on_standard_error);
        push_bool(t, "pwsh", self.pwsh);
        push_opt(t, "workingDirectory", self.working_directory);
        match self.azure_powershell_version {
            None => {}
            Some(AzurePowerShellVersion::Latest) => {
                t.inputs
                    .insert("azurePowerShellVersion".to_string(), "LatestVersion".to_string());
            }
            Some(AzurePowerShellVersion::Preferred(ver)) => {
                t.inputs
                    .insert("azurePowerShellVersion".to_string(), "OtherVersion".to_string());
                t.inputs
                    .insert("preferredAzurePowerShellVersion".to_string(), ver);
            }
        }
    }
}

/// Emit the optional setters that are shared by both `AzurePowerShell@5` builders.
macro_rules! shared_setters {
    () => {
        /// `errorActionPreference` — non-terminating error behaviour (default `stop`).
        pub fn error_action_preference(mut self, value: ErrorActionPreference) -> Self {
            self.shared.error_action_preference = Some(value);
            self
        }

        /// `FailOnStandardError` — fail the step when anything is written to stderr.
        pub fn fail_on_standard_error(mut self, value: bool) -> Self {
            self.shared.fail_on_standard_error = Some(value);
            self
        }

        /// `pwsh` — use PowerShell Core (`pwsh`) instead of Windows PowerShell.
        pub fn pwsh(mut self, value: bool) -> Self {
            self.shared.pwsh = Some(value);
            self
        }

        /// `workingDirectory` — working directory for the script.
        pub fn working_directory(mut self, value: impl Into<String>) -> Self {
            self.shared.working_directory = Some(value.into());
            self
        }

        /// Select the Azure PowerShell module version.
        ///
        /// [`AzurePowerShellVersion::Latest`] → `LatestVersion`.
        /// [`AzurePowerShellVersion::Preferred`] → `OtherVersion` +
        /// `preferredAzurePowerShellVersion`.
        pub fn azure_powershell_version(mut self, value: AzurePowerShellVersion) -> Self {
            self.shared.azure_powershell_version = Some(value);
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
#[derive(Debug, Clone)]
pub struct AzurePowerShellFile {
    azure_subscription: String,
    script_path: String,
    script_arguments: Option<String>,
    shared: Shared,
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
        self.shared.apply(&mut t);
        t
    }
}

/// Builder for `AzurePowerShell@5` in inline mode (`ScriptType: InlineScript`).
#[derive(Debug, Clone)]
pub struct AzurePowerShellInline {
    azure_subscription: String,
    script: String,
    shared: Shared,
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
        self.shared.apply(&mut t);
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
            shared: Shared::default(),
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
            shared: Shared::default(),
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
        assert_eq!(t.inputs.get("ScriptType").map(String::as_str), Some("FilePath"));
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
        assert_eq!(t.inputs.get("ScriptType").map(String::as_str), Some("InlineScript"));
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
            t.inputs.get("preferredAzurePowerShellVersion").map(String::as_str),
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
