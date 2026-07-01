//! Typed builder for `AzureWebApp@1`.
//!
//! ADO task reference:
//! <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/azure-web-app-v1>

use super::common::{de_opt_bool_flex, push_bool, push_opt};
use crate::compile::ir::step::TaskStep;
use serde::Deserialize;

/// The type of Azure App Service to deploy to.
///
/// Controls which optional inputs are applicable:
/// - [`AppType::WebApp`] enables `deployment_method` and `custom_web_config`.
/// - [`AppType::WebAppLinux`] enables `runtime_stack` and `startup_command`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum AppType {
    /// Web App on Windows (`webApp`).
    #[serde(rename = "webApp")]
    WebApp,
    /// Web App on Linux (`webAppLinux`).
    #[serde(rename = "webAppLinux")]
    WebAppLinux,
}

impl AppType {
    /// Return the exact ADO token for this app type.
    pub fn as_ado_str(self) -> &'static str {
        match self {
            AppType::WebApp => "webApp",
            AppType::WebAppLinux => "webAppLinux",
        }
    }
}

/// Deployment method for Windows web apps (`AzureWebApp@1`).
///
/// Only applicable when `app_type` is [`AppType::WebApp`] and the package is
/// not a `.war` or `.jar` file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum DeploymentMethod {
    /// ADO auto-selects the best method (`auto`). This is the ADO default.
    #[serde(rename = "auto")]
    Auto,
    /// Deploy as a zip package (`zipDeploy`).
    #[serde(rename = "zipDeploy")]
    ZipDeploy,
    /// Deploy as a run-from-package mount (`runFromPackage`).
    #[serde(rename = "runFromPackage")]
    RunFromPackage,
}

impl DeploymentMethod {
    /// Return the exact ADO token for this deployment method.
    pub fn as_ado_str(self) -> &'static str {
        match self {
            DeploymentMethod::Auto => "auto",
            DeploymentMethod::ZipDeploy => "zipDeploy",
            DeploymentMethod::RunFromPackage => "runFromPackage",
        }
    }
}

/// Builder for a [`TaskStep`] invoking `AzureWebApp@1`.
///
/// Deploys a web application to Azure App Service (Windows or Linux). The
/// `azure_subscription` field must name an Azure Resource Manager service
/// connection configured in the ADO project.
///
/// # Examples
///
/// Deploy a Windows web app:
/// ```rust
/// use ado_aw::compile::ir::tasks::azure_web_app::{AzureWebApp, AppType, DeploymentMethod};
///
/// let step = AzureWebApp::new(
///     "my-azure-sub",
///     AppType::WebApp,
///     "my-app-name",
///     "$(System.DefaultWorkingDirectory)/**/*.zip",
/// )
/// .deployment_method(DeploymentMethod::ZipDeploy)
/// .into_step();
/// ```
///
/// Deploy a Linux web app with a runtime stack:
/// ```rust
/// use ado_aw::compile::ir::tasks::azure_web_app::{AzureWebApp, AppType};
///
/// let step = AzureWebApp::new(
///     "my-azure-sub",
///     AppType::WebAppLinux,
///     "my-linux-app",
///     "$(Build.ArtifactStagingDirectory)/app.zip",
/// )
/// .runtime_stack("NODE|20-lts")
/// .into_step();
/// ```
///
/// ADO task reference:
/// <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/azure-web-app-v1>
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AzureWebApp {
    #[serde(rename = "azureSubscription")]
    azure_subscription: String,
    #[serde(rename = "appType")]
    app_type: AppType,
    #[serde(rename = "appName")]
    app_name: String,
    package: String,
    #[serde(
        rename = "deployToSlotOrASE",
        default,
        deserialize_with = "de_opt_bool_flex"
    )]
    deploy_to_slot_or_ase: Option<bool>,
    #[serde(rename = "resourceGroupName", default)]
    resource_group_name: Option<String>,
    #[serde(rename = "slotName", default)]
    slot_name: Option<String>,
    #[serde(rename = "customDeployFolder", default)]
    custom_deploy_folder: Option<String>,
    #[serde(rename = "runtimeStack", default)]
    runtime_stack: Option<String>,
    #[serde(rename = "startUpCommand", default)]
    startup_command: Option<String>,
    #[serde(rename = "customWebConfig", default)]
    custom_web_config: Option<String>,
    #[serde(rename = "deploymentMethod", default)]
    deployment_method: Option<DeploymentMethod>,
    #[serde(skip)]
    display_name: Option<String>,
}

impl AzureWebApp {
    /// Create a new `AzureWebApp@1` builder.
    ///
    /// # Parameters
    /// - `azure_subscription` — the Azure Resource Manager service connection
    ///   name configured in the ADO project.
    /// - `app_type` — [`AppType::WebApp`] for Windows or [`AppType::WebAppLinux`]
    ///   for Linux App Service targets.
    /// - `app_name` — the name of the Azure App Service instance to deploy to.
    /// - `package` — path to the package or folder to deploy (supports ADO
    ///   variables and wildcards, e.g.
    ///   `$(System.DefaultWorkingDirectory)/**/*.zip`).
    pub fn new(
        azure_subscription: impl Into<String>,
        app_type: AppType,
        app_name: impl Into<String>,
        package: impl Into<String>,
    ) -> Self {
        Self {
            azure_subscription: azure_subscription.into(),
            app_type,
            app_name: app_name.into(),
            package: package.into(),
            deploy_to_slot_or_ase: None,
            resource_group_name: None,
            slot_name: None,
            custom_deploy_folder: None,
            runtime_stack: None,
            startup_command: None,
            custom_web_config: None,
            deployment_method: None,
            display_name: None,
        }
    }

    /// `deployToSlotOrASE` — when `true`, deploys to a specific deployment
    /// slot or App Service Environment rather than the production slot.
    /// Requires `resource_group_name` and `slot_name` to be set.
    pub fn deploy_to_slot_or_ase(mut self, value: bool) -> Self {
        self.deploy_to_slot_or_ase = Some(value);
        self
    }

    /// `resourceGroupName` — the Azure resource group containing the App
    /// Service. Required when `deploy_to_slot_or_ase` is `true`.
    pub fn resource_group_name(mut self, value: impl Into<String>) -> Self {
        self.resource_group_name = Some(value.into());
        self
    }

    /// `slotName` — the deployment slot to target (ADO default: `production`).
    /// Required when `deploy_to_slot_or_ase` is `true`.
    pub fn slot_name(mut self, value: impl Into<String>) -> Self {
        self.slot_name = Some(value.into());
        self
    }

    /// `customDeployFolder` — custom folder path used when the `package` ends
    /// with `.war`. Leave unset for the default WAR deployment path.
    pub fn custom_deploy_folder(mut self, value: impl Into<String>) -> Self {
        self.custom_deploy_folder = Some(value.into());
        self
    }

    /// `runtimeStack` — the runtime stack for Linux web apps (e.g.
    /// `"NODE|20-lts"`, `"PYTHON|3.12"`, `"DOTNETCORE|8.0"`).
    /// Only applicable when `app_type` is [`AppType::WebAppLinux`].
    pub fn runtime_stack(mut self, value: impl Into<String>) -> Self {
        self.runtime_stack = Some(value.into());
        self
    }

    /// `startUpCommand` — the startup command for the Linux container (e.g.
    /// `"gunicorn app:app"`). Only applicable when `app_type` is
    /// [`AppType::WebAppLinux`].
    pub fn startup_command(mut self, value: impl Into<String>) -> Self {
        self.startup_command = Some(value.into());
        self
    }

    /// `customWebConfig` — web.config parameters to generate for Python,
    /// Node.js, Go, and Java apps on Windows. Not applicable to Linux or WAR
    /// deployments.
    pub fn custom_web_config(mut self, value: impl Into<String>) -> Self {
        self.custom_web_config = Some(value.into());
        self
    }

    /// `deploymentMethod` — how ADO deploys the package to the Windows App
    /// Service. Defaults to [`DeploymentMethod::Auto`] when unset. Not
    /// applicable to Linux web apps or WAR/JAR packages.
    pub fn deployment_method(mut self, value: DeploymentMethod) -> Self {
        self.deployment_method = Some(value);
        self
    }

    /// Override the default `displayName` (`"Azure Web App Deploy"`).
    pub fn with_display_name(mut self, value: impl Into<String>) -> Self {
        self.display_name = Some(value.into());
        self
    }

    /// Lower into a [`TaskStep`].
    pub fn into_step(self) -> TaskStep {
        let mut t = TaskStep::new(
            "AzureWebApp@1",
            self.display_name
                .unwrap_or_else(|| "Azure Web App Deploy".into()),
        )
        .with_input("azureSubscription", self.azure_subscription)
        .with_input("appType", self.app_type.as_ado_str())
        .with_input("appName", self.app_name)
        .with_input("package", self.package);
        push_bool(&mut t, "deployToSlotOrASE", self.deploy_to_slot_or_ase);
        push_opt(&mut t, "resourceGroupName", self.resource_group_name);
        push_opt(&mut t, "slotName", self.slot_name);
        push_opt(&mut t, "customDeployFolder", self.custom_deploy_folder);
        push_opt(&mut t, "runtimeStack", self.runtime_stack);
        push_opt(&mut t, "startUpCommand", self.startup_command);
        push_opt(&mut t, "customWebConfig", self.custom_web_config);
        if let Some(v) = self.deployment_method {
            t = t.with_input("deploymentMethod", v.as_ado_str());
        }
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sets_task_and_required_inputs() {
        let t = AzureWebApp::new(
            "my-azure-sub",
            AppType::WebApp,
            "my-app",
            "$(System.DefaultWorkingDirectory)/**/*.zip",
        )
        .into_step();
        assert_eq!(t.task, "AzureWebApp@1");
        assert_eq!(t.display_name, "Azure Web App Deploy");
        assert_eq!(
            t.inputs.get("azureSubscription").map(String::as_str),
            Some("my-azure-sub")
        );
        assert_eq!(t.inputs.get("appType").map(String::as_str), Some("webApp"));
        assert_eq!(t.inputs.get("appName").map(String::as_str), Some("my-app"));
        assert_eq!(
            t.inputs.get("package").map(String::as_str),
            Some("$(System.DefaultWorkingDirectory)/**/*.zip")
        );
    }

    #[test]
    fn optional_inputs_absent_by_default() {
        let t = AzureWebApp::new("sub", AppType::WebApp, "app", "pkg.zip").into_step();
        assert!(t.inputs.get("deployToSlotOrASE").is_none());
        assert!(t.inputs.get("resourceGroupName").is_none());
        assert!(t.inputs.get("slotName").is_none());
        assert!(t.inputs.get("runtimeStack").is_none());
        assert!(t.inputs.get("startUpCommand").is_none());
        assert!(t.inputs.get("customWebConfig").is_none());
        assert!(t.inputs.get("deploymentMethod").is_none());
    }

    #[test]
    fn linux_web_app_with_runtime_stack_and_startup_command() {
        let t = AzureWebApp::new("my-sub", AppType::WebAppLinux, "linux-app", "app.zip")
            .runtime_stack("NODE|20-lts")
            .startup_command("node server.js")
            .into_step();
        assert_eq!(
            t.inputs.get("appType").map(String::as_str),
            Some("webAppLinux")
        );
        assert_eq!(
            t.inputs.get("runtimeStack").map(String::as_str),
            Some("NODE|20-lts")
        );
        assert_eq!(
            t.inputs.get("startUpCommand").map(String::as_str),
            Some("node server.js")
        );
    }

    #[test]
    fn slot_deployment() {
        let t = AzureWebApp::new("sub", AppType::WebApp, "app", "pkg.zip")
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
    fn deployment_method_zip_deploy() {
        let t = AzureWebApp::new("sub", AppType::WebApp, "app", "pkg.zip")
            .deployment_method(DeploymentMethod::ZipDeploy)
            .into_step();
        assert_eq!(
            t.inputs.get("deploymentMethod").map(String::as_str),
            Some("zipDeploy")
        );
    }

    #[test]
    fn deployment_method_run_from_package() {
        let t = AzureWebApp::new("sub", AppType::WebApp, "app", "pkg.zip")
            .deployment_method(DeploymentMethod::RunFromPackage)
            .into_step();
        assert_eq!(
            t.inputs.get("deploymentMethod").map(String::as_str),
            Some("runFromPackage")
        );
    }

    #[test]
    fn deployment_method_auto() {
        let t = AzureWebApp::new("sub", AppType::WebApp, "app", "pkg.zip")
            .deployment_method(DeploymentMethod::Auto)
            .into_step();
        assert_eq!(
            t.inputs.get("deploymentMethod").map(String::as_str),
            Some("auto")
        );
    }

    #[test]
    fn display_name_override() {
        let t = AzureWebApp::new("sub", AppType::WebAppLinux, "app", "pkg.zip")
            .with_display_name("Deploy to production slot")
            .into_step();
        assert_eq!(t.display_name, "Deploy to production slot");
    }

    #[test]
    fn app_type_as_ado_str() {
        assert_eq!(AppType::WebApp.as_ado_str(), "webApp");
        assert_eq!(AppType::WebAppLinux.as_ado_str(), "webAppLinux");
    }

    #[test]
    fn deployment_method_as_ado_str() {
        assert_eq!(DeploymentMethod::Auto.as_ado_str(), "auto");
        assert_eq!(DeploymentMethod::ZipDeploy.as_ado_str(), "zipDeploy");
        assert_eq!(
            DeploymentMethod::RunFromPackage.as_ado_str(),
            "runFromPackage"
        );
    }
}
