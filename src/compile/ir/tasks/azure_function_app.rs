//! Typed builder for `AzureFunctionApp@2`.
//!
//! `AzureFunctionApp@2` deploys a function app package to Azure Functions on
//! Windows or Linux, including Flex Consumption plan apps. It supports zip
//! deploy, run-from-package, and auto-detect deployment modes, deployment slot
//! targeting, and per-app settings overrides.
//!
//! ADO task reference:
//! <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/azure-function-app-v2>

use super::common::{push_bool, push_opt};
use crate::compile::ir::step::TaskStep;

/// Hosting OS for the Azure Functions App (`appType` input).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FunctionAppType {
    /// Function App on Windows (`"functionApp"`).
    Windows,
    /// Function App on Linux (`"functionAppLinux"`).
    Linux,
}

impl FunctionAppType {
    /// Return the exact ADO token for this app type.
    pub fn as_ado_str(self) -> &'static str {
        match self {
            FunctionAppType::Windows => "functionApp",
            FunctionAppType::Linux => "functionAppLinux",
        }
    }
}

/// Package deployment method for the function app (`deploymentMethod` input).
///
/// Not applicable to Flex Consumption plan apps or WAR/JAR packages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FunctionDeploymentMethod {
    /// ADO auto-selects the best method (`"auto"`). ADO default.
    Auto,
    /// Deploy as a zip package (`"zipDeploy"`).
    ZipDeploy,
    /// Deploy as a run-from-package mount (`"runFromPackage"`).
    RunFromPackage,
}

impl FunctionDeploymentMethod {
    /// Return the exact ADO token for this deployment method.
    pub fn as_ado_str(self) -> &'static str {
        match self {
            FunctionDeploymentMethod::Auto => "auto",
            FunctionDeploymentMethod::ZipDeploy => "zipDeploy",
            FunctionDeploymentMethod::RunFromPackage => "runFromPackage",
        }
    }
}

/// Builder for a [`TaskStep`] invoking `AzureFunctionApp@2`.
///
/// Deploys a function app package to Azure Functions (Windows or Linux,
/// including Flex Consumption plan). The `azure_subscription` parameter must
/// name an Azure Resource Manager service connection configured in the ADO
/// project. Equivalent to the `AzureFunctionApp@2` Azure DevOps task.
///
/// # Examples
///
/// Deploy a Windows function app:
/// ```rust
/// use ado_aw::compile::ir::tasks::azure_function_app::{AzureFunctionApp, FunctionAppType};
///
/// let step = AzureFunctionApp::new(
///     "my-azure-sub",
///     FunctionAppType::Windows,
///     "my-func-app",
///     "$(System.DefaultWorkingDirectory)/**/*.zip",
/// )
/// .into_step();
/// ```
///
/// Deploy to a staging slot on a Linux function app:
/// ```rust
/// use ado_aw::compile::ir::tasks::azure_function_app::{AzureFunctionApp, FunctionAppType};
///
/// let step = AzureFunctionApp::new(
///     "my-azure-sub",
///     FunctionAppType::Linux,
///     "my-func-app",
///     "$(Build.ArtifactStagingDirectory)/app.zip",
/// )
/// .deploy_to_slot_or_ase(true)
/// .resource_group_name("my-resource-group")
/// .slot_name("staging")
/// .runtime_stack("NODE|20")
/// .into_step();
/// ```
///
/// ADO task reference:
/// <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/azure-function-app-v2>
#[derive(Debug, Clone)]
pub struct AzureFunctionApp {
    azure_subscription: String,
    app_type: FunctionAppType,
    app_name: String,
    package: String,
    is_flex_consumption: Option<bool>,
    deploy_to_slot_or_ase: Option<bool>,
    resource_group_name: Option<String>,
    slot_name: Option<String>,
    runtime_stack: Option<String>,
    app_settings: Option<String>,
    deployment_method: Option<FunctionDeploymentMethod>,
    display_name: Option<String>,
}

impl AzureFunctionApp {
    /// Create a new `AzureFunctionApp@2` builder.
    ///
    /// # Parameters
    /// - `azure_subscription` — the Azure Resource Manager service connection
    ///   name configured in the ADO project (maps to `connectedServiceNameARM`
    ///   / `azureSubscription`).
    /// - `app_type` — [`FunctionAppType::Windows`] or
    ///   [`FunctionAppType::Linux`].
    /// - `app_name` — the name of the Azure Functions App to deploy to.
    /// - `package` — path to the package or folder to deploy; supports ADO
    ///   variables and wildcards (e.g.
    ///   `"$(System.DefaultWorkingDirectory)/**/*.zip"`).
    pub fn new(
        azure_subscription: impl Into<String>,
        app_type: FunctionAppType,
        app_name: impl Into<String>,
        package: impl Into<String>,
    ) -> Self {
        Self {
            azure_subscription: azure_subscription.into(),
            app_type,
            app_name: app_name.into(),
            package: package.into(),
            is_flex_consumption: None,
            deploy_to_slot_or_ase: None,
            resource_group_name: None,
            slot_name: None,
            runtime_stack: None,
            app_settings: None,
            deployment_method: None,
            display_name: None,
        }
    }

    /// `isFlexConsumption` — set to `true` when the function app is hosted on
    /// a [Flex Consumption plan](https://learn.microsoft.com/en-us/azure/azure-functions/flex-consumption-plan).
    /// ADO default: `false`.
    pub fn flex_consumption(mut self, value: bool) -> Self {
        self.is_flex_consumption = Some(value);
        self
    }

    /// `deployToSlotOrASE` — when `true`, deploys to a specific deployment
    /// slot or App Service Environment rather than the default production slot.
    /// Requires [`resource_group_name`][Self::resource_group_name] and
    /// [`slot_name`][Self::slot_name] to be set.  Not applicable to Flex
    /// Consumption plan apps. ADO default: `false`.
    pub fn deploy_to_slot_or_ase(mut self, value: bool) -> Self {
        self.deploy_to_slot_or_ase = Some(value);
        self
    }
    /// `resourceGroupName` — the Azure resource group containing the Function
    /// App. Required when [`deploy_to_slot_or_ase`][Self::deploy_to_slot_or_ase]
    /// is `true`.
    pub fn resource_group_name(mut self, value: impl Into<String>) -> Self {
        self.resource_group_name = Some(value.into());
        self
    }

    /// `slotName` — the deployment slot to target (ADO default: `"production"`).
    /// Required when [`deploy_to_slot_or_ase`][Self::deploy_to_slot_or_ase]
    /// is `true`.
    pub fn slot_name(mut self, value: impl Into<String>) -> Self {
        self.slot_name = Some(value.into());
        self
    }

    /// `runtimeStack` — the runtime stack for Linux function apps (e.g.
    /// `"NODE|20"`, `"PYTHON|3.12"`, `"DOTNET-ISOLATED|8.0"`). Only
    /// applicable when `app_type` is [`FunctionAppType::Linux`] and the app
    /// is not on a Flex Consumption plan.
    pub fn runtime_stack(mut self, value: impl Into<String>) -> Self {
        self.runtime_stack = Some(value.into());
        self
    }

    /// `appSettings` — application settings to configure on the Function App,
    /// using `-key value` syntax (e.g. `"-Port 5000 -WEBSITE_TIME_ZONE \"Eastern Standard Time\""`).
    pub fn app_settings(mut self, value: impl Into<String>) -> Self {
        self.app_settings = Some(value.into());
        self
    }

    /// `deploymentMethod` — how ADO deploys the package to the Function App.
    /// ADO default: [`FunctionDeploymentMethod::Auto`]. Not applicable to Flex
    /// Consumption plan apps or WAR/JAR packages.
    pub fn deployment_method(mut self, value: FunctionDeploymentMethod) -> Self {
        self.deployment_method = Some(value);
        self
    }

    /// Override the default `displayName` (`"Azure Functions Deploy"`).
    pub fn with_display_name(mut self, value: impl Into<String>) -> Self {
        self.display_name = Some(value.into());
        self
    }

    /// Lower into a [`TaskStep`].
    pub fn into_step(self) -> TaskStep {
        let mut t = TaskStep::new(
            "AzureFunctionApp@2",
            self.display_name
                .unwrap_or_else(|| "Azure Functions Deploy".into()),
        )
        .with_input("azureSubscription", self.azure_subscription)
        .with_input("appType", self.app_type.as_ado_str())
        .with_input("appName", self.app_name)
        .with_input("package", self.package);
        push_bool(&mut t, "isFlexConsumption", self.is_flex_consumption);
        push_bool(&mut t, "deployToSlotOrASE", self.deploy_to_slot_or_ase);
        push_opt(&mut t, "resourceGroupName", self.resource_group_name);
        push_opt(&mut t, "slotName", self.slot_name);
        push_opt(&mut t, "runtimeStack", self.runtime_stack);
        push_opt(&mut t, "appSettings", self.app_settings);
        if let Some(m) = self.deployment_method {
            t = t.with_input("deploymentMethod", m.as_ado_str());
        }
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimal_windows_deploy_emits_required_inputs() {
        let t = AzureFunctionApp::new(
            "my-sub",
            FunctionAppType::Windows,
            "my-func",
            "$(System.DefaultWorkingDirectory)/**/*.zip",
        )
        .into_step();
        assert_eq!(t.task, "AzureFunctionApp@2");
        assert_eq!(t.display_name, "Azure Functions Deploy");
        assert_eq!(
            t.inputs.get("azureSubscription").map(String::as_str),
            Some("my-sub")
        );
        assert_eq!(
            t.inputs.get("appType").map(String::as_str),
            Some("functionApp")
        );
        assert_eq!(
            t.inputs.get("appName").map(String::as_str),
            Some("my-func")
        );
        assert_eq!(
            t.inputs.get("package").map(String::as_str),
            Some("$(System.DefaultWorkingDirectory)/**/*.zip")
        );
        assert!(t.inputs.get("isFlexConsumption").is_none());
        assert!(t.inputs.get("deployToSlotOrASE").is_none());
        assert!(t.inputs.get("runtimeStack").is_none());
        assert!(t.inputs.get("deploymentMethod").is_none());
    }

    #[test]
    fn linux_app_type_emits_correct_token() {
        let t = AzureFunctionApp::new(
            "sub",
            FunctionAppType::Linux,
            "func-app",
            "app.zip",
        )
        .into_step();
        assert_eq!(
            t.inputs.get("appType").map(String::as_str),
            Some("functionAppLinux")
        );
    }

    #[test]
    fn slot_deployment_sets_all_relevant_inputs() {
        let t = AzureFunctionApp::new(
            "sub",
            FunctionAppType::Windows,
            "func-app",
            "$(Build.ArtifactStagingDirectory)/func.zip",
        )
        .deploy_to_slot_or_ase(true)
        .resource_group_name("my-rg")
        .slot_name("staging")
        .into_step();
        assert_eq!(
            t.inputs.get("deployToSlotOrASE").map(String::as_str),
            Some("true")
        );
        assert_eq!(
            t.inputs.get("resourceGroupName").map(String::as_str),
            Some("my-rg")
        );
        assert_eq!(
            t.inputs.get("slotName").map(String::as_str),
            Some("staging")
        );
    }

    #[test]
    fn linux_runtime_stack_is_emitted() {
        let t = AzureFunctionApp::new("sub", FunctionAppType::Linux, "app", "pkg.zip")
            .runtime_stack("NODE|20")
            .into_step();
        assert_eq!(
            t.inputs.get("runtimeStack").map(String::as_str),
            Some("NODE|20")
        );
    }

    #[test]
    fn deployment_method_enum_round_trips() {
        for (method, expected) in [
            (FunctionDeploymentMethod::Auto, "auto"),
            (FunctionDeploymentMethod::ZipDeploy, "zipDeploy"),
            (FunctionDeploymentMethod::RunFromPackage, "runFromPackage"),
        ] {
            let t = AzureFunctionApp::new("sub", FunctionAppType::Windows, "app", "pkg.zip")
                .deployment_method(method)
                .into_step();
            assert_eq!(
                t.inputs.get("deploymentMethod").map(String::as_str),
                Some(expected),
                "method {method:?}"
            );
        }
    }

    #[test]
    fn flex_consumption_flag_is_emitted() {
        let t = AzureFunctionApp::new("sub", FunctionAppType::Linux, "app", "pkg.zip")
            .flex_consumption(true)
            .into_step();
        assert_eq!(
            t.inputs.get("isFlexConsumption").map(String::as_str),
            Some("true")
        );
    }

    #[test]
    fn app_settings_are_forwarded() {
        let t = AzureFunctionApp::new("sub", FunctionAppType::Windows, "app", "pkg.zip")
            .app_settings("-MY_SETTING value")
            .into_step();
        assert_eq!(
            t.inputs.get("appSettings").map(String::as_str),
            Some("-MY_SETTING value")
        );
    }

    #[test]
    fn display_name_override_is_respected() {
        let t = AzureFunctionApp::new("sub", FunctionAppType::Windows, "app", "pkg.zip")
            .with_display_name("Deploy to production")
            .into_step();
        assert_eq!(t.display_name, "Deploy to production");
    }
}
