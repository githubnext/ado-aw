//! Typed builder for `AzureCLI@2`.

use super::common::{push_bool, push_opt};
use crate::compile::ir::step::TaskStep;

/// Script type for `AzureCLI@2`.
///
/// Selects the shell that executes the script body.
///
/// ADO input: `scriptType`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptType {
    /// Bash shell (`bash`).
    Bash,
    /// Windows PowerShell (`ps`).
    Ps,
    /// PowerShell Core (`pscore`).
    PsCore,
    /// Windows batch script (`batch`).
    Batch,
}

impl ScriptType {
    /// The exact ADO token for this variant.
    pub fn as_ado_str(&self) -> &'static str {
        match self {
            ScriptType::Bash => "bash",
            ScriptType::Ps => "ps",
            ScriptType::PsCore => "pscore",
            ScriptType::Batch => "batch",
        }
    }
}

/// Script content location for `AzureCLI@2`.
///
/// The variant determines which of `scriptLocation`/`inlineScript`/`scriptPath`
/// inputs are emitted. Because each variant carries its required content, an
/// invalid combination (e.g. `scriptLocation: scriptPath` with no path) is
/// unrepresentable.
///
/// ADO input: `scriptLocation`
#[derive(Debug, Clone)]
pub enum ScriptLocation {
    /// Embed the script body inline (`scriptLocation: inlineScript`).
    Inline(String),
    /// Execute a script file by path (`scriptLocation: scriptPath`).
    ScriptPath(String),
}

/// `ErrorActionPreference` for PowerShell scripts (`scriptType: ps` or `pscore`).
///
/// ADO input: `powerShellErrorActionPreference`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerShellErrorActionPreference {
    /// Stop on first error (`stop`). This is the ADO default.
    Stop,
    /// Continue on errors (`continue`).
    Continue,
    /// Silently continue (`silentlyContinue`).
    SilentlyContinue,
}

impl PowerShellErrorActionPreference {
    /// The exact ADO token for this variant.
    pub fn as_ado_str(&self) -> &'static str {
        match self {
            PowerShellErrorActionPreference::Stop => "stop",
            PowerShellErrorActionPreference::Continue => "continue",
            PowerShellErrorActionPreference::SilentlyContinue => "silentlyContinue",
        }
    }
}

/// Builder for a [`TaskStep`] invoking `AzureCLI@2`.
///
/// Runs a bash, PowerShell, or batch script inside an authenticated Azure CLI
/// session. Required inputs — the ARM service connection, the script type, and
/// the script location/content — are positional parameters of [`AzureCli::new`].
/// Optional inputs are applied through typed chained setters; only those
/// explicitly set are emitted.
///
/// # Example
///
/// ```rust,ignore
/// use crate::compile::ir::tasks::azure_cli::{AzureCli, ScriptLocation, ScriptType};
///
/// let step = AzureCli::new(
///     "my-arm-connection",
///     ScriptType::Bash,
///     ScriptLocation::Inline("az acr login --name myregistry\n".into()),
/// )
/// .with_display_name("Log in to container registry")
/// .into_step();
/// ```
///
/// ADO task reference:
/// <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/azure-cli-v2>
#[derive(Debug, Clone)]
pub struct AzureCli {
    azure_subscription: String,
    script_type: ScriptType,
    location: ScriptLocation,
    arguments: Option<String>,
    ps_error_action_preference: Option<PowerShellErrorActionPreference>,
    add_spn_to_environment: Option<bool>,
    use_global_config: Option<bool>,
    working_directory: Option<String>,
    fail_on_standard_error: Option<bool>,
    ps_ignore_last_exit_code: Option<bool>,
    visible_az_login: Option<bool>,
    display_name: Option<String>,
}

impl AzureCli {
    /// Create a new builder.
    ///
    /// - `azure_subscription` — ARM service connection name (`azureSubscription`).
    /// - `script_type` — shell to run (`scriptType`).
    /// - `location` — inline script body or path to script file (`scriptLocation`).
    pub fn new(
        azure_subscription: impl Into<String>,
        script_type: ScriptType,
        location: ScriptLocation,
    ) -> Self {
        Self {
            azure_subscription: azure_subscription.into(),
            script_type,
            location,
            arguments: None,
            ps_error_action_preference: None,
            add_spn_to_environment: None,
            use_global_config: None,
            working_directory: None,
            fail_on_standard_error: None,
            ps_ignore_last_exit_code: None,
            visible_az_login: None,
            display_name: None,
        }
    }

    /// `arguments` — additional arguments passed to the script.
    pub fn arguments(mut self, value: impl Into<String>) -> Self {
        self.arguments = Some(value.into());
        self
    }

    /// `powerShellErrorActionPreference` — how PowerShell handles non-terminating errors.
    /// Relevant only when `script_type` is [`ScriptType::Ps`] or [`ScriptType::PsCore`].
    pub fn ps_error_action_preference(
        mut self,
        value: PowerShellErrorActionPreference,
    ) -> Self {
        self.ps_error_action_preference = Some(value);
        self
    }

    /// `addSpnToEnvironment` — expose service principal details (`$env:servicePrincipalId`,
    /// `$env:servicePrincipalKey`, `$env:tenantId`) to the script.
    pub fn add_spn_to_environment(mut self, value: bool) -> Self {
        self.add_spn_to_environment = Some(value);
        self
    }

    /// `useGlobalConfig` — use the global Azure CLI configuration instead of
    /// an isolated per-task configuration.
    pub fn use_global_config(mut self, value: bool) -> Self {
        self.use_global_config = Some(value);
        self
    }

    /// `workingDirectory` — working directory for the script.
    pub fn working_directory(mut self, value: impl Into<String>) -> Self {
        self.working_directory = Some(value.into());
        self
    }

    /// `failOnStandardError` — fail the task when the script writes to stderr.
    pub fn fail_on_standard_error(mut self, value: bool) -> Self {
        self.fail_on_standard_error = Some(value);
        self
    }

    /// `powerShellIgnoreLASTEXITCODE` — do not append `if ((Test-Path …))` to the
    /// script body. Relevant only when `script_type` is [`ScriptType::Ps`] or
    /// [`ScriptType::PsCore`].
    pub fn ps_ignore_last_exit_code(mut self, value: bool) -> Self {
        self.ps_ignore_last_exit_code = Some(value);
        self
    }

    /// `visibleAzLogin` — make `az login` output visible in the build log.
    pub fn visible_az_login(mut self, value: bool) -> Self {
        self.visible_az_login = Some(value);
        self
    }

    /// Override the default `displayName` (`"Azure CLI"`).
    pub fn with_display_name(mut self, value: impl Into<String>) -> Self {
        self.display_name = Some(value.into());
        self
    }

    /// Lower into a [`TaskStep`].
    pub fn into_step(self) -> TaskStep {
        let mut t = TaskStep::new(
            "AzureCLI@2",
            self.display_name.unwrap_or_else(|| "Azure CLI".into()),
        )
        .with_input("azureSubscription", self.azure_subscription)
        .with_input("scriptType", self.script_type.as_ado_str());

        match self.location {
            ScriptLocation::Inline(script) => {
                t = t
                    .with_input("scriptLocation", "inlineScript")
                    .with_input("inlineScript", script);
            }
            ScriptLocation::ScriptPath(path) => {
                t = t
                    .with_input("scriptLocation", "scriptPath")
                    .with_input("scriptPath", path);
            }
        }

        push_opt(&mut t, "arguments", self.arguments);
        if let Some(pref) = self.ps_error_action_preference {
            t.inputs.insert(
                "powerShellErrorActionPreference".to_string(),
                pref.as_ado_str().to_string(),
            );
        }
        push_bool(&mut t, "addSpnToEnvironment", self.add_spn_to_environment);
        push_bool(&mut t, "useGlobalConfig", self.use_global_config);
        push_opt(&mut t, "workingDirectory", self.working_directory);
        push_bool(&mut t, "failOnStandardError", self.fail_on_standard_error);
        push_bool(
            &mut t,
            "powerShellIgnoreLASTEXITCODE",
            self.ps_ignore_last_exit_code,
        );
        push_bool(&mut t, "visibleAzLogin", self.visible_az_login);

        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bash_inline_required_inputs() {
        let t = AzureCli::new(
            "my-arm-connection",
            ScriptType::Bash,
            ScriptLocation::Inline("echo hello\n".into()),
        )
        .into_step();

        assert_eq!(t.task, "AzureCLI@2");
        assert_eq!(t.display_name, "Azure CLI");
        assert_eq!(
            t.inputs.get("azureSubscription").map(String::as_str),
            Some("my-arm-connection")
        );
        assert_eq!(t.inputs.get("scriptType").map(String::as_str), Some("bash"));
        assert_eq!(
            t.inputs.get("scriptLocation").map(String::as_str),
            Some("inlineScript")
        );
        assert_eq!(
            t.inputs.get("inlineScript").map(String::as_str),
            Some("echo hello\n")
        );
        assert!(t.inputs.get("scriptPath").is_none());
    }

    #[test]
    fn script_path_location() {
        let t = AzureCli::new(
            "my-arm-connection",
            ScriptType::PsCore,
            ScriptLocation::ScriptPath("scripts/deploy.ps1".into()),
        )
        .into_step();

        assert_eq!(t.inputs.get("scriptType").map(String::as_str), Some("pscore"));
        assert_eq!(
            t.inputs.get("scriptLocation").map(String::as_str),
            Some("scriptPath")
        );
        assert_eq!(
            t.inputs.get("scriptPath").map(String::as_str),
            Some("scripts/deploy.ps1")
        );
        assert!(t.inputs.get("inlineScript").is_none());
    }

    #[test]
    fn optional_inputs_emit_only_when_set() {
        let t = AzureCli::new(
            "conn",
            ScriptType::Bash,
            ScriptLocation::Inline("az account show\n".into()),
        )
        .add_spn_to_environment(true)
        .use_global_config(false)
        .working_directory("$(Build.SourcesDirectory)")
        .fail_on_standard_error(true)
        .visible_az_login(false)
        .into_step();

        assert_eq!(
            t.inputs.get("addSpnToEnvironment").map(String::as_str),
            Some("true")
        );
        assert_eq!(
            t.inputs.get("useGlobalConfig").map(String::as_str),
            Some("false")
        );
        assert_eq!(
            t.inputs.get("workingDirectory").map(String::as_str),
            Some("$(Build.SourcesDirectory)")
        );
        assert_eq!(
            t.inputs.get("failOnStandardError").map(String::as_str),
            Some("true")
        );
        assert_eq!(
            t.inputs.get("visibleAzLogin").map(String::as_str),
            Some("false")
        );
        // Untouched optionals are absent.
        assert!(t.inputs.get("arguments").is_none());
        assert!(t.inputs.get("powerShellErrorActionPreference").is_none());
    }

    #[test]
    fn ps_optionals() {
        let t = AzureCli::new(
            "conn",
            ScriptType::Ps,
            ScriptLocation::Inline("Write-Host 'hello'\n".into()),
        )
        .ps_error_action_preference(PowerShellErrorActionPreference::Continue)
        .ps_ignore_last_exit_code(true)
        .into_step();

        assert_eq!(
            t.inputs
                .get("powerShellErrorActionPreference")
                .map(String::as_str),
            Some("continue")
        );
        assert_eq!(
            t.inputs
                .get("powerShellIgnoreLASTEXITCODE")
                .map(String::as_str),
            Some("true")
        );
    }

    #[test]
    fn display_name_override() {
        let t = AzureCli::new(
            "conn",
            ScriptType::Bash,
            ScriptLocation::Inline("az acr login --name myacr\n".into()),
        )
        .with_display_name("Log in to container registry")
        .into_step();

        assert_eq!(t.display_name, "Log in to container registry");
    }

    #[test]
    fn script_type_ado_strings() {
        assert_eq!(ScriptType::Bash.as_ado_str(), "bash");
        assert_eq!(ScriptType::Ps.as_ado_str(), "ps");
        assert_eq!(ScriptType::PsCore.as_ado_str(), "pscore");
        assert_eq!(ScriptType::Batch.as_ado_str(), "batch");
    }

    #[test]
    fn ps_error_preference_ado_strings() {
        assert_eq!(
            PowerShellErrorActionPreference::Stop.as_ado_str(),
            "stop"
        );
        assert_eq!(
            PowerShellErrorActionPreference::Continue.as_ado_str(),
            "continue"
        );
        assert_eq!(
            PowerShellErrorActionPreference::SilentlyContinue.as_ado_str(),
            "silentlyContinue"
        );
    }
}
