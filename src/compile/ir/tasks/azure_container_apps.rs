//! Typed builder for `AzureContainerApps@1`.
//!
//! ADO task reference:
//! <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/azure-container-apps-v1>

use super::common::{de_opt_bool_flex, push_bool, push_opt};
use crate::compile::ir::step::TaskStep;
use serde::Deserialize;

/// Ingress setting for an Azure Container App (`AzureContainerApps@1`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum ContainerAppIngress {
    /// Accepts traffic from the internet (`external`).
    #[serde(rename = "external")]
    External,
    /// Accepts traffic only from within the Azure Container Apps environment
    /// (`internal`).
    #[serde(rename = "internal")]
    Internal,
}

impl ContainerAppIngress {
    /// Return the exact ADO token for this ingress setting.
    pub fn as_ado_str(self) -> &'static str {
        match self {
            ContainerAppIngress::External => "external",
            ContainerAppIngress::Internal => "internal",
        }
    }
}

/// Builder for a [`TaskStep`] invoking `AzureContainerApps@1`.
///
/// Builds and/or deploys an image to Azure Container Apps. There are two main
/// usage patterns:
///
/// 1. **Build from source** — set `app_source_path` and `acr_name`; the task
///    builds the Docker image and pushes it to ACR before deploying.
/// 2. **Deploy an existing image** — set `image_to_deploy`; the task updates
///    the Container App to use the specified image without building.
///
/// In both cases `azure_subscription` (the Azure RM service connection) is the
/// only required input.  All other inputs are optional and are only emitted when
/// explicitly set.
///
/// ADO task reference:
/// <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/azure-container-apps-v1>
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AzureContainerApps {
    /// `azureSubscription` / `connectedServiceNameARM` — Azure RM service connection (required).
    #[serde(rename = "azureSubscription")]
    azure_subscription: String,
    /// `workingDirectory` / `cwd` — working directory for the task.
    #[serde(rename = "workingDirectory", default)]
    working_directory: Option<String>,
    /// `appSourcePath` — local path to the application source to build.
    #[serde(rename = "appSourcePath", default)]
    app_source_path: Option<String>,
    /// `acrName` — Azure Container Registry name (without `.azurecr.io`).
    #[serde(rename = "acrName", default)]
    acr_name: Option<String>,
    /// `acrUsername` — ACR username (use a secret variable in practice).
    #[serde(rename = "acrUsername", default)]
    acr_username: Option<String>,
    /// `acrPassword` — ACR password (use a secret variable in practice).
    #[serde(rename = "acrPassword", default)]
    acr_password: Option<String>,
    /// `dockerfilePath` — path to a Dockerfile relative to `app_source_path`.
    #[serde(rename = "dockerfilePath", default)]
    dockerfile_path: Option<String>,
    /// `imageToBuild` — fully-qualified image name to build and push.
    #[serde(rename = "imageToBuild", default)]
    image_to_build: Option<String>,
    /// `imageToDeploy` — fully-qualified image name of an existing image to
    /// deploy (mutually exclusive with build-from-source inputs).
    #[serde(rename = "imageToDeploy", default)]
    image_to_deploy: Option<String>,
    /// `containerAppName` — name of the Container App to create or update.
    #[serde(rename = "containerAppName", default)]
    container_app_name: Option<String>,
    /// `resourceGroup` — Azure resource group that contains the Container App.
    #[serde(rename = "resourceGroup", default)]
    resource_group: Option<String>,
    /// `containerAppEnvironment` — name or resource ID of the Container Apps
    /// environment.
    #[serde(rename = "containerAppEnvironment", default)]
    container_app_environment: Option<String>,
    /// `runtimeStack` — runtime stack to use when Oryx detects the runtime
    /// automatically (e.g. `"node:18"`, `"python:3.11"`).
    #[serde(rename = "runtimeStack", default)]
    runtime_stack: Option<String>,
    /// `targetPort` — port that the container listens on.
    #[serde(rename = "targetPort", default)]
    target_port: Option<String>,
    /// `location` — Azure region for the Container App (e.g. `"eastus"`).
    #[serde(default)]
    location: Option<String>,
    /// `environmentVariables` — space-separated list of `KEY=VALUE` pairs (or
    /// `KEY=secretref:SECRET_NAME` for secret references).
    #[serde(rename = "environmentVariables", default)]
    environment_variables: Option<String>,
    /// `ingress` — ingress setting for the Container App.
    #[serde(default)]
    ingress: Option<ContainerAppIngress>,
    /// `yamlConfigPath` — path to a YAML configuration file that defines the
    /// Container App.  When set, most other inputs are ignored.
    #[serde(rename = "yamlConfigPath", default)]
    yaml_config_path: Option<String>,
    /// `disableTelemetry` — suppress telemetry sent to Microsoft.
    #[serde(
        rename = "disableTelemetry",
        default,
        deserialize_with = "de_opt_bool_flex"
    )]
    disable_telemetry: Option<bool>,
    /// Override the default `displayName`.
    #[serde(skip)]
    display_name: Option<String>,
}

impl AzureContainerApps {
    /// Create a new builder with the required `azure_subscription` service
    /// connection name.
    ///
    /// All other inputs are optional and are only emitted when explicitly set
    /// via the typed setters.
    pub fn new(azure_subscription: impl Into<String>) -> Self {
        Self {
            azure_subscription: azure_subscription.into(),
            working_directory: None,
            app_source_path: None,
            acr_name: None,
            acr_username: None,
            acr_password: None,
            dockerfile_path: None,
            image_to_build: None,
            image_to_deploy: None,
            container_app_name: None,
            resource_group: None,
            container_app_environment: None,
            runtime_stack: None,
            target_port: None,
            location: None,
            environment_variables: None,
            ingress: None,
            yaml_config_path: None,
            disable_telemetry: None,
            display_name: None,
        }
    }

    /// `workingDirectory` — working directory for the task.
    pub fn working_directory(mut self, value: impl Into<String>) -> Self {
        self.working_directory = Some(value.into());
        self
    }

    /// `appSourcePath` — local path to the application source to build from.
    pub fn app_source_path(mut self, value: impl Into<String>) -> Self {
        self.app_source_path = Some(value.into());
        self
    }

    /// `acrName` — Azure Container Registry name (without `.azurecr.io`).
    pub fn acr_name(mut self, value: impl Into<String>) -> Self {
        self.acr_name = Some(value.into());
        self
    }

    /// `acrUsername` — ACR username for authentication.
    pub fn acr_username(mut self, value: impl Into<String>) -> Self {
        self.acr_username = Some(value.into());
        self
    }

    /// `acrPassword` — ACR password for authentication.
    pub fn acr_password(mut self, value: impl Into<String>) -> Self {
        self.acr_password = Some(value.into());
        self
    }

    /// `dockerfilePath` — path to a Dockerfile relative to `app_source_path`.
    pub fn dockerfile_path(mut self, value: impl Into<String>) -> Self {
        self.dockerfile_path = Some(value.into());
        self
    }

    /// `imageToBuild` — fully-qualified image name to build and push.
    pub fn image_to_build(mut self, value: impl Into<String>) -> Self {
        self.image_to_build = Some(value.into());
        self
    }

    /// `imageToDeploy` — fully-qualified image name of an existing image to
    /// deploy.
    pub fn image_to_deploy(mut self, value: impl Into<String>) -> Self {
        self.image_to_deploy = Some(value.into());
        self
    }

    /// `containerAppName` — name of the Container App to create or update.
    pub fn container_app_name(mut self, value: impl Into<String>) -> Self {
        self.container_app_name = Some(value.into());
        self
    }

    /// `resourceGroup` — Azure resource group containing the Container App.
    pub fn resource_group(mut self, value: impl Into<String>) -> Self {
        self.resource_group = Some(value.into());
        self
    }

    /// `containerAppEnvironment` — name or resource ID of the Container Apps
    /// environment.
    pub fn container_app_environment(mut self, value: impl Into<String>) -> Self {
        self.container_app_environment = Some(value.into());
        self
    }

    /// `runtimeStack` — runtime stack for Oryx auto-detection
    /// (e.g. `"node:18"`, `"python:3.11"`).
    pub fn runtime_stack(mut self, value: impl Into<String>) -> Self {
        self.runtime_stack = Some(value.into());
        self
    }

    /// `targetPort` — port the container listens on.
    pub fn target_port(mut self, value: impl Into<String>) -> Self {
        self.target_port = Some(value.into());
        self
    }

    /// `location` — Azure region for the Container App (e.g. `"eastus"`).
    pub fn location(mut self, value: impl Into<String>) -> Self {
        self.location = Some(value.into());
        self
    }

    /// `environmentVariables` — space-separated `KEY=VALUE` pairs (or
    /// `KEY=secretref:SECRET_NAME` for secret references).
    pub fn environment_variables(mut self, value: impl Into<String>) -> Self {
        self.environment_variables = Some(value.into());
        self
    }

    /// `ingress` — ingress accessibility setting for the Container App.
    pub fn ingress(mut self, value: ContainerAppIngress) -> Self {
        self.ingress = Some(value);
        self
    }

    /// `yamlConfigPath` — path to a YAML file defining the Container App
    /// configuration.  When set, most other inputs are ignored.
    pub fn yaml_config_path(mut self, value: impl Into<String>) -> Self {
        self.yaml_config_path = Some(value.into());
        self
    }

    /// `disableTelemetry` — suppress telemetry sent to Microsoft.
    pub fn disable_telemetry(mut self, value: bool) -> Self {
        self.disable_telemetry = Some(value);
        self
    }

    /// Override the default `displayName` (`"Deploy Azure Container App"`).
    pub fn with_display_name(mut self, value: impl Into<String>) -> Self {
        self.display_name = Some(value.into());
        self
    }

    /// Lower into a [`TaskStep`].
    pub fn into_step(self) -> TaskStep {
        let mut t = TaskStep::new(
            "AzureContainerApps@1",
            self.display_name
                .unwrap_or_else(|| "Deploy Azure Container App".into()),
        )
        .with_input("azureSubscription", self.azure_subscription);

        push_opt(&mut t, "workingDirectory", self.working_directory);
        push_opt(&mut t, "appSourcePath", self.app_source_path);
        push_opt(&mut t, "acrName", self.acr_name);
        push_opt(&mut t, "acrUsername", self.acr_username);
        push_opt(&mut t, "acrPassword", self.acr_password);
        push_opt(&mut t, "dockerfilePath", self.dockerfile_path);
        push_opt(&mut t, "imageToBuild", self.image_to_build);
        push_opt(&mut t, "imageToDeploy", self.image_to_deploy);
        push_opt(&mut t, "containerAppName", self.container_app_name);
        push_opt(&mut t, "resourceGroup", self.resource_group);
        push_opt(
            &mut t,
            "containerAppEnvironment",
            self.container_app_environment,
        );
        push_opt(&mut t, "runtimeStack", self.runtime_stack);
        push_opt(&mut t, "targetPort", self.target_port);
        push_opt(&mut t, "location", self.location);
        push_opt(&mut t, "environmentVariables", self.environment_variables);
        if let Some(v) = self.ingress {
            t = t.with_input("ingress", v.as_ado_str());
        }
        push_opt(&mut t, "yamlConfigPath", self.yaml_config_path);
        push_bool(&mut t, "disableTelemetry", self.disable_telemetry);

        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn required_input_only() {
        let t = AzureContainerApps::new("my-azure-connection").into_step();
        assert_eq!(t.task, "AzureContainerApps@1");
        assert_eq!(t.display_name, "Deploy Azure Container App");
        assert_eq!(
            t.inputs.get("azureSubscription").map(String::as_str),
            Some("my-azure-connection")
        );
        // Optional inputs should not be emitted.
        assert!(t.inputs.get("imageToDeploy").is_none());
        assert!(t.inputs.get("containerAppName").is_none());
        assert!(t.inputs.get("resourceGroup").is_none());
    }

    #[test]
    fn deploy_existing_image() {
        let t = AzureContainerApps::new("my-azure-connection")
            .image_to_deploy("mcr.microsoft.com/myapp:latest")
            .container_app_name("my-app")
            .resource_group("my-rg")
            .into_step();
        assert_eq!(t.task, "AzureContainerApps@1");
        assert_eq!(
            t.inputs.get("imageToDeploy").map(String::as_str),
            Some("mcr.microsoft.com/myapp:latest")
        );
        assert_eq!(
            t.inputs.get("containerAppName").map(String::as_str),
            Some("my-app")
        );
        assert_eq!(
            t.inputs.get("resourceGroup").map(String::as_str),
            Some("my-rg")
        );
        // Build-from-source inputs should be absent.
        assert!(t.inputs.get("appSourcePath").is_none());
        assert!(t.inputs.get("acrName").is_none());
    }

    #[test]
    fn build_from_source() {
        let t = AzureContainerApps::new("my-azure-connection")
            .app_source_path("$(System.DefaultWorkingDirectory)")
            .acr_name("mytestacr")
            .container_app_name("my-app")
            .resource_group("my-rg")
            .into_step();
        assert_eq!(
            t.inputs.get("appSourcePath").map(String::as_str),
            Some("$(System.DefaultWorkingDirectory)")
        );
        assert_eq!(
            t.inputs.get("acrName").map(String::as_str),
            Some("mytestacr")
        );
        assert!(t.inputs.get("imageToDeploy").is_none());
    }

    #[test]
    fn ingress_enum_variants() {
        let ext = AzureContainerApps::new("sc")
            .ingress(ContainerAppIngress::External)
            .into_step();
        assert_eq!(
            ext.inputs.get("ingress").map(String::as_str),
            Some("external")
        );

        let int = AzureContainerApps::new("sc")
            .ingress(ContainerAppIngress::Internal)
            .into_step();
        assert_eq!(
            int.inputs.get("ingress").map(String::as_str),
            Some("internal")
        );
    }

    #[test]
    fn optional_fields() {
        let t = AzureContainerApps::new("sc")
            .working_directory("$(Build.SourcesDirectory)")
            .container_app_environment("my-env")
            .runtime_stack("node:18")
            .target_port("3000")
            .location("eastus")
            .environment_variables("KEY=VALUE OTHER=secretref:MY_SECRET")
            .disable_telemetry(true)
            .with_display_name("Deploy my app")
            .into_step();

        assert_eq!(t.display_name, "Deploy my app");
        assert_eq!(
            t.inputs.get("workingDirectory").map(String::as_str),
            Some("$(Build.SourcesDirectory)")
        );
        assert_eq!(
            t.inputs.get("containerAppEnvironment").map(String::as_str),
            Some("my-env")
        );
        assert_eq!(
            t.inputs.get("runtimeStack").map(String::as_str),
            Some("node:18")
        );
        assert_eq!(t.inputs.get("targetPort").map(String::as_str), Some("3000"));
        assert_eq!(t.inputs.get("location").map(String::as_str), Some("eastus"));
        assert_eq!(
            t.inputs.get("environmentVariables").map(String::as_str),
            Some("KEY=VALUE OTHER=secretref:MY_SECRET")
        );
        assert_eq!(
            t.inputs.get("disableTelemetry").map(String::as_str),
            Some("true")
        );
    }

    #[test]
    fn yaml_config_path_variant() {
        let t = AzureContainerApps::new("sc")
            .yaml_config_path("container-app.yaml")
            .into_step();
        assert_eq!(
            t.inputs.get("yamlConfigPath").map(String::as_str),
            Some("container-app.yaml")
        );
    }
}
