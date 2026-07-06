//! Typed builder for `AzureResourceManagerTemplateDeployment@3`.
//!
//! Deploys Azure Resource Manager (ARM) or Bicep templates to any deployment
//! scope (Resource Group, Subscription, Management Group, Tenant). The
//! canonical command-dispatch pattern is used because each deployment scope
//! exposes a distinct required-input set; invalid scope/input combinations are
//! therefore unrepresentable.
//!
//! ADO task reference:
//! <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/azure-resource-manager-template-deployment-v3>

use super::common::{de_opt_bool_flex, push_bool, push_opt};
use crate::compile::ir::step::TaskStep;
use serde::Deserialize;
use serde_yaml::Value;

// ── Shared enums ─────────────────────────────────────────────────────────────

/// Deployment mode for ARM template deployments.
///
/// Only relevant for scopes that deploy a template (not `ResourceGroupDelete`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum ArmDeploymentMode {
    /// Apply only the changes in the template; leave unchanged resources
    /// untouched (ADO default).
    #[serde(rename = "Incremental")]
    Incremental,
    /// Delete all resources in the scope that are **not** in the template.
    #[serde(rename = "Complete")]
    Complete,
    /// Validate the template without creating any resources.
    #[serde(rename = "Validation")]
    Validation,
}

impl ArmDeploymentMode {
    pub fn as_ado_str(self) -> &'static str {
        match self {
            Self::Incremental => "Incremental",
            Self::Complete => "Complete",
            Self::Validation => "Validation",
        }
    }
}

// ── Template source ───────────────────────────────────────────────────────────

/// Specifies where the ARM/Bicep template (and optional parameters file) live.
#[derive(Debug, Clone)]
pub enum ArmTemplateSource {
    /// `templateLocation: Linked artifact` — files checked in alongside the
    /// pipeline (most common).
    LinkedArtifact {
        /// `csmFile` — path to the ARM or Bicep template file.
        csm_file: String,
        /// `csmParametersFile` — optional path to a parameters file.
        csm_parameters_file: Option<String>,
    },
    /// `templateLocation: URL of the file` — template referenced by URL.
    Url {
        /// `csmFileLink` — URL of the template file.
        csm_file_link: String,
        /// `csmParametersFileLink` — optional URL of the parameters file.
        csm_parameters_file_link: Option<String>,
    },
}

impl ArmTemplateSource {
    /// Convenience constructor: linked artifact with no parameters file.
    pub fn linked_artifact(csm_file: impl Into<String>) -> Self {
        Self::LinkedArtifact {
            csm_file: csm_file.into(),
            csm_parameters_file: None,
        }
    }

    /// Convenience constructor: URL-referenced template with no parameters file.
    pub fn url(csm_file_link: impl Into<String>) -> Self {
        Self::Url {
            csm_file_link: csm_file_link.into(),
            csm_parameters_file_link: None,
        }
    }

    /// Attach a parameters file path (for `LinkedArtifact`) or URL (for `Url`).
    pub fn with_parameters_file(mut self, value: impl Into<String>) -> Self {
        match &mut self {
            Self::LinkedArtifact {
                csm_parameters_file,
                ..
            } => {
                *csm_parameters_file = Some(value.into());
            }
            Self::Url {
                csm_parameters_file_link,
                ..
            } => {
                *csm_parameters_file_link = Some(value.into());
            }
        }
        self
    }
}

// ── Shared deploy options ─────────────────────────────────────────────────────

/// Optional inputs shared by all "deploy template" commands.
#[derive(Debug, Clone, Default)]
struct DeployOptions {
    deployment_mode: Option<ArmDeploymentMode>,
    override_parameters: Option<String>,
    deployment_name: Option<String>,
    deployment_outputs: Option<String>,
    add_spn_to_environment: Option<bool>,
    use_without_json: Option<bool>,
}

// ── Per-scope command structs ─────────────────────────────────────────────────

/// Required inputs for `deploymentScope: "Resource Group"` with
/// `action: "Create Or Update Resource Group"`.
#[derive(Debug, Clone)]
pub struct ResourceGroupDeploy {
    connection: String,
    subscription_id: String,
    resource_group: String,
    location: String,
    template: ArmTemplateSource,
    opts: DeployOptions,
}

impl ResourceGroupDeploy {
    /// Create a resource-group deployment spec.
    ///
    /// - `connection` — Azure RM service connection name.
    /// - `subscription_id` — subscription ID or name.
    /// - `resource_group` — target resource group.
    /// - `location` — Azure region (e.g. `"East US"`).
    /// - `template` — [`ArmTemplateSource`] for the ARM/Bicep template.
    pub fn new(
        connection: impl Into<String>,
        subscription_id: impl Into<String>,
        resource_group: impl Into<String>,
        location: impl Into<String>,
        template: ArmTemplateSource,
    ) -> Self {
        Self {
            connection: connection.into(),
            subscription_id: subscription_id.into(),
            resource_group: resource_group.into(),
            location: location.into(),
            template,
            opts: DeployOptions::default(),
        }
    }

    /// `deploymentMode` — how the template is applied (default: `Incremental`).
    pub fn deployment_mode(mut self, mode: ArmDeploymentMode) -> Self {
        self.opts.deployment_mode = Some(mode);
        self
    }

    /// `overrideParameters` — space-separated `-name "value"` overrides.
    pub fn override_parameters(mut self, value: impl Into<String>) -> Self {
        self.opts.override_parameters = Some(value.into());
        self
    }

    /// `deploymentName` — name for this deployment resource.
    pub fn deployment_name(mut self, value: impl Into<String>) -> Self {
        self.opts.deployment_name = Some(value.into());
        self
    }

    /// `deploymentOutputs` — variable name to receive ARM output JSON.
    pub fn deployment_outputs(mut self, value: impl Into<String>) -> Self {
        self.opts.deployment_outputs = Some(value.into());
        self
    }

    /// `addSpnToEnvironment` — expose service principal details in override params.
    pub fn add_spn_to_environment(mut self, value: bool) -> Self {
        self.opts.add_spn_to_environment = Some(value);
        self
    }

    /// `useWithoutJSON` — return individual output values without JSON.Stringify.
    pub fn use_without_json(mut self, value: bool) -> Self {
        self.opts.use_without_json = Some(value);
        self
    }
}

/// Required inputs for `deploymentScope: "Resource Group"` with
/// `action: "DeleteRG"`.
#[derive(Debug, Clone)]
pub struct ResourceGroupDelete {
    connection: String,
    subscription_id: String,
    resource_group: String,
}

impl ResourceGroupDelete {
    pub fn new(
        connection: impl Into<String>,
        subscription_id: impl Into<String>,
        resource_group: impl Into<String>,
    ) -> Self {
        Self {
            connection: connection.into(),
            subscription_id: subscription_id.into(),
            resource_group: resource_group.into(),
        }
    }
}

/// Required inputs for `deploymentScope: "Subscription"`.
#[derive(Debug, Clone)]
pub struct SubscriptionDeploy {
    connection: String,
    subscription_id: String,
    location: String,
    template: ArmTemplateSource,
    opts: DeployOptions,
}

impl SubscriptionDeploy {
    pub fn new(
        connection: impl Into<String>,
        subscription_id: impl Into<String>,
        location: impl Into<String>,
        template: ArmTemplateSource,
    ) -> Self {
        Self {
            connection: connection.into(),
            subscription_id: subscription_id.into(),
            location: location.into(),
            template,
            opts: DeployOptions::default(),
        }
    }

    /// `deploymentMode` — how the template is applied (default: `Incremental`).
    pub fn deployment_mode(mut self, mode: ArmDeploymentMode) -> Self {
        self.opts.deployment_mode = Some(mode);
        self
    }

    /// `overrideParameters` — space-separated `-name "value"` overrides.
    pub fn override_parameters(mut self, value: impl Into<String>) -> Self {
        self.opts.override_parameters = Some(value.into());
        self
    }

    /// `deploymentName` — name for this deployment resource.
    pub fn deployment_name(mut self, value: impl Into<String>) -> Self {
        self.opts.deployment_name = Some(value.into());
        self
    }

    /// `deploymentOutputs` — variable name to receive ARM output JSON.
    pub fn deployment_outputs(mut self, value: impl Into<String>) -> Self {
        self.opts.deployment_outputs = Some(value.into());
        self
    }

    /// `addSpnToEnvironment` — expose service principal details in override params.
    pub fn add_spn_to_environment(mut self, value: bool) -> Self {
        self.opts.add_spn_to_environment = Some(value);
        self
    }

    /// `useWithoutJSON` — return individual output values without JSON.Stringify.
    pub fn use_without_json(mut self, value: bool) -> Self {
        self.opts.use_without_json = Some(value);
        self
    }
}

/// Required inputs for `deploymentScope: "Management Group"`.
///
/// Note: `subscriptionId` is **not** used at management-group scope.
#[derive(Debug, Clone)]
pub struct ManagementGroupDeploy {
    connection: String,
    location: String,
    template: ArmTemplateSource,
    opts: DeployOptions,
}

impl ManagementGroupDeploy {
    pub fn new(
        connection: impl Into<String>,
        location: impl Into<String>,
        template: ArmTemplateSource,
    ) -> Self {
        Self {
            connection: connection.into(),
            location: location.into(),
            template,
            opts: DeployOptions::default(),
        }
    }

    /// `deploymentMode` — how the template is applied (default: `Incremental`).
    pub fn deployment_mode(mut self, mode: ArmDeploymentMode) -> Self {
        self.opts.deployment_mode = Some(mode);
        self
    }

    /// `overrideParameters` — space-separated `-name "value"` overrides.
    pub fn override_parameters(mut self, value: impl Into<String>) -> Self {
        self.opts.override_parameters = Some(value.into());
        self
    }

    /// `deploymentName` — name for this deployment resource.
    pub fn deployment_name(mut self, value: impl Into<String>) -> Self {
        self.opts.deployment_name = Some(value.into());
        self
    }

    /// `deploymentOutputs` — variable name to receive ARM output JSON.
    pub fn deployment_outputs(mut self, value: impl Into<String>) -> Self {
        self.opts.deployment_outputs = Some(value.into());
        self
    }

    /// `addSpnToEnvironment` — expose service principal details in override params.
    pub fn add_spn_to_environment(mut self, value: bool) -> Self {
        self.opts.add_spn_to_environment = Some(value);
        self
    }

    /// `useWithoutJSON` — return individual output values without JSON.Stringify.
    pub fn use_without_json(mut self, value: bool) -> Self {
        self.opts.use_without_json = Some(value);
        self
    }
}

/// Required inputs for `deploymentScope: "Tenant"`.
///
/// Note: `subscriptionId` is **not** used at tenant scope.
#[derive(Debug, Clone)]
pub struct TenantDeploy {
    connection: String,
    location: String,
    template: ArmTemplateSource,
    opts: DeployOptions,
}

impl TenantDeploy {
    pub fn new(
        connection: impl Into<String>,
        location: impl Into<String>,
        template: ArmTemplateSource,
    ) -> Self {
        Self {
            connection: connection.into(),
            location: location.into(),
            template,
            opts: DeployOptions::default(),
        }
    }

    /// `deploymentMode` — how the template is applied (default: `Incremental`).
    pub fn deployment_mode(mut self, mode: ArmDeploymentMode) -> Self {
        self.opts.deployment_mode = Some(mode);
        self
    }

    /// `overrideParameters` — space-separated `-name "value"` overrides.
    pub fn override_parameters(mut self, value: impl Into<String>) -> Self {
        self.opts.override_parameters = Some(value.into());
        self
    }

    /// `deploymentName` — name for this deployment resource.
    pub fn deployment_name(mut self, value: impl Into<String>) -> Self {
        self.opts.deployment_name = Some(value.into());
        self
    }

    /// `deploymentOutputs` — variable name to receive ARM output JSON.
    pub fn deployment_outputs(mut self, value: impl Into<String>) -> Self {
        self.opts.deployment_outputs = Some(value.into());
        self
    }

    /// `addSpnToEnvironment` — expose service principal details in override params.
    pub fn add_spn_to_environment(mut self, value: bool) -> Self {
        self.opts.add_spn_to_environment = Some(value);
        self
    }

    /// `useWithoutJSON` — return individual output values without JSON.Stringify.
    pub fn use_without_json(mut self, value: bool) -> Self {
        self.opts.use_without_json = Some(value);
        self
    }
}

// ── Command enum ──────────────────────────────────────────────────────────────

/// Deployment command selector for `AzureResourceManagerTemplateDeployment@3`.
#[derive(Debug, Clone)]
pub enum ArmDeploymentCommand {
    /// `deploymentScope: "Resource Group"`, `action: "Create Or Update Resource Group"`.
    ResourceGroupDeploy(ResourceGroupDeploy),
    /// `deploymentScope: "Resource Group"`, `action: "DeleteRG"`.
    ResourceGroupDelete(ResourceGroupDelete),
    /// `deploymentScope: "Subscription"`.
    SubscriptionDeploy(SubscriptionDeploy),
    /// `deploymentScope: "Management Group"`.
    ManagementGroupDeploy(ManagementGroupDeploy),
    /// `deploymentScope: "Tenant"`.
    TenantDeploy(TenantDeploy),
}

// ── Main builder ──────────────────────────────────────────────────────────────

/// Builder for a [`TaskStep`] invoking `AzureResourceManagerTemplateDeployment@3`.
///
/// Use the per-scope convenience constructors ([`Self::resource_group_deploy`],
/// [`Self::subscription_deploy`], etc.) or [`Self::new`] with an explicit
/// [`ArmDeploymentCommand`].
#[derive(Debug, Clone)]
pub struct ArmTemplateDeployment {
    command: ArmDeploymentCommand,
    display_name: Option<String>,
}

impl ArmTemplateDeployment {
    /// Construct from an explicit [`ArmDeploymentCommand`].
    pub fn new(command: ArmDeploymentCommand) -> Self {
        Self {
            command,
            display_name: None,
        }
    }

    /// Deploy to a Resource Group.
    pub fn resource_group_deploy(spec: ResourceGroupDeploy) -> Self {
        Self::new(ArmDeploymentCommand::ResourceGroupDeploy(spec))
    }

    /// Delete a Resource Group.
    pub fn resource_group_delete(spec: ResourceGroupDelete) -> Self {
        Self::new(ArmDeploymentCommand::ResourceGroupDelete(spec))
    }

    /// Deploy at subscription scope.
    pub fn subscription_deploy(spec: SubscriptionDeploy) -> Self {
        Self::new(ArmDeploymentCommand::SubscriptionDeploy(spec))
    }

    /// Deploy at management-group scope.
    pub fn management_group_deploy(spec: ManagementGroupDeploy) -> Self {
        Self::new(ArmDeploymentCommand::ManagementGroupDeploy(spec))
    }

    /// Deploy at tenant scope.
    pub fn tenant_deploy(spec: TenantDeploy) -> Self {
        Self::new(ArmDeploymentCommand::TenantDeploy(spec))
    }

    /// Override the default `displayName`.
    pub fn with_display_name(mut self, value: impl Into<String>) -> Self {
        self.display_name = Some(value.into());
        self
    }

    /// Lower into a [`TaskStep`].
    pub fn into_step(self) -> TaskStep {
        let (scope, action, default_display): (&str, Option<&str>, &str) = match &self.command {
            ArmDeploymentCommand::ResourceGroupDeploy(_) => (
                "Resource Group",
                Some("Create Or Update Resource Group"),
                "ARM Template Deployment",
            ),
            ArmDeploymentCommand::ResourceGroupDelete(_) => {
                ("Resource Group", Some("DeleteRG"), "Delete Resource Group")
            }
            ArmDeploymentCommand::SubscriptionDeploy(_) => {
                ("Subscription", None, "ARM Template Deployment")
            }
            ArmDeploymentCommand::ManagementGroupDeploy(_) => {
                ("Management Group", None, "ARM Template Deployment")
            }
            ArmDeploymentCommand::TenantDeploy(_) => ("Tenant", None, "ARM Template Deployment"),
        };

        let mut t = TaskStep::new(
            "AzureResourceManagerTemplateDeployment@3",
            self.display_name.unwrap_or_else(|| default_display.into()),
        )
        .with_input("deploymentScope", scope);
        if let Some(a) = action {
            t = t.with_input("action", a);
        }

        match self.command {
            ArmDeploymentCommand::ResourceGroupDeploy(s) => {
                t = t
                    .with_input("azureResourceManagerConnection", s.connection)
                    .with_input("subscriptionId", s.subscription_id)
                    .with_input("resourceGroupName", s.resource_group)
                    .with_input("location", s.location);
                push_template_source(&mut t, s.template);
                push_deploy_opts(&mut t, s.opts);
            }
            ArmDeploymentCommand::ResourceGroupDelete(s) => {
                t = t
                    .with_input("azureResourceManagerConnection", s.connection)
                    .with_input("subscriptionId", s.subscription_id)
                    .with_input("resourceGroupName", s.resource_group);
            }
            ArmDeploymentCommand::SubscriptionDeploy(s) => {
                t = t
                    .with_input("azureResourceManagerConnection", s.connection)
                    .with_input("subscriptionId", s.subscription_id)
                    .with_input("location", s.location);
                push_template_source(&mut t, s.template);
                push_deploy_opts(&mut t, s.opts);
            }
            ArmDeploymentCommand::ManagementGroupDeploy(s) => {
                t = t
                    .with_input("azureResourceManagerConnection", s.connection)
                    .with_input("location", s.location);
                push_template_source(&mut t, s.template);
                push_deploy_opts(&mut t, s.opts);
            }
            ArmDeploymentCommand::TenantDeploy(s) => {
                t = t
                    .with_input("azureResourceManagerConnection", s.connection)
                    .with_input("location", s.location);
                push_template_source(&mut t, s.template);
                push_deploy_opts(&mut t, s.opts);
            }
        }
        t
    }
}

// ── Private lowering helpers ──────────────────────────────────────────────────

fn push_template_source(t: &mut TaskStep, source: ArmTemplateSource) {
    match source {
        ArmTemplateSource::LinkedArtifact {
            csm_file,
            csm_parameters_file,
        } => {
            t.inputs
                .insert("templateLocation".into(), "Linked artifact".into());
            t.inputs.insert("csmFile".into(), csm_file);
            push_opt(t, "csmParametersFile", csm_parameters_file);
        }
        ArmTemplateSource::Url {
            csm_file_link,
            csm_parameters_file_link,
        } => {
            t.inputs
                .insert("templateLocation".into(), "URL of the file".into());
            t.inputs.insert("csmFileLink".into(), csm_file_link);
            push_opt(t, "csmParametersFileLink", csm_parameters_file_link);
        }
    }
}

fn push_deploy_opts(t: &mut TaskStep, opts: DeployOptions) {
    if let Some(mode) = opts.deployment_mode {
        t.inputs
            .insert("deploymentMode".into(), mode.as_ado_str().into());
    }
    push_opt(t, "overrideParameters", opts.override_parameters);
    push_opt(t, "deploymentName", opts.deployment_name);
    push_opt(t, "deploymentOutputs", opts.deployment_outputs);
    push_bool(t, "addSpnToEnvironment", opts.add_spn_to_environment);
    push_bool(t, "useWithoutJSON", opts.use_without_json);
}

// ── Advisory front-matter validation ─────────────────────────────────────────

/// Validate an authored `AzureResourceManagerTemplateDeployment@3` `inputs:`
/// mapping (advisory front-matter validation, see [`super::parse`]).
///
/// The task dispatches on `deploymentScope` (default `Resource Group`), and
/// within the Resource Group scope on `action` (default
/// `Create Or Update Resource Group`). Scope/action-specific required inputs are
/// checked and removed, then the remainder is validated against a
/// `deny_unknown_fields` schema so an input used for the wrong scope/action is
/// reported.
pub(crate) fn validate_inputs(inputs: Value) -> Result<(), String> {
    let mut map = match inputs {
        Value::Mapping(m) => m,
        Value::Null => Default::default(),
        other => return Err(format!("`inputs` must be a mapping, got {other:?}")),
    };

    // The ARM service connection is required (`azureResourceManagerConnection`,
    // alias of the `ConnectedServiceName` input).
    let has_connection = ["azureResourceManagerConnection", "ConnectedServiceName"]
        .into_iter()
        .any(|k| map.remove(k).is_some());
    if !has_connection {
        return Err("missing required input `azureResourceManagerConnection`".to_string());
    }

    let scope =
        take_string(&mut map, "deploymentScope")?.unwrap_or_else(|| "Resource Group".to_string());

    match scope.as_str() {
        "Resource Group" => {
            let action = take_string(&mut map, "action")?
                .unwrap_or_else(|| "Create Or Update Resource Group".to_string());
            match action.as_str() {
                "Create Or Update Resource Group" => {
                    require_subscription(&mut map)?;
                    require_string(&mut map, "resourceGroupName")?;
                    require_string(&mut map, "location")?;
                    validate_template_deploy(map)
                }
                "DeleteRG" => {
                    require_subscription(&mut map)?;
                    require_string(&mut map, "resourceGroupName")?;
                    serde_yaml::from_value::<DeleteSpec>(Value::Mapping(map))
                        .map(drop)
                        .map_err(|e| format!("action `DeleteRG`: {e}"))
                }
                other => Err(format!(
                    "unknown action `{other}` (expected Create Or Update Resource Group|DeleteRG)"
                )),
            }
        }
        "Subscription" => {
            require_subscription(&mut map)?;
            require_string(&mut map, "location")?;
            validate_template_deploy(map)
        }
        "Management Group" => {
            require_string(&mut map, "location")?;
            validate_template_deploy(map)
        }
        "Tenant" => {
            require_string(&mut map, "location")?;
            validate_template_deploy(map)
        }
        other => Err(format!(
            "unknown deploymentScope `{other}` \
             (expected Resource Group|Subscription|Management Group|Tenant)"
        )),
    }
}

fn validate_template_deploy(map: serde_yaml::Mapping) -> Result<(), String> {
    serde_yaml::from_value::<TemplateDeploySpec>(Value::Mapping(map))
        .map(drop)
        .map_err(|e| format!("template deployment inputs: {e}"))
}

/// Remove `key` and return its string value, erroring if present but non-string.
fn take_string(map: &mut serde_yaml::Mapping, key: &str) -> Result<Option<String>, String> {
    match map.remove(key) {
        Some(v) => v
            .as_str()
            .map(str::to_string)
            .map(Some)
            .ok_or_else(|| format!("`{key}` must be a string")),
        None => Ok(None),
    }
}

/// Remove a required string `key`, erroring if missing or non-string.
fn require_string(map: &mut serde_yaml::Mapping, key: &str) -> Result<(), String> {
    match take_string(map, key)? {
        Some(_) => Ok(()),
        None => Err(format!(
            "missing required input `{key}` for the selected scope"
        )),
    }
}

/// The subscription input accepts either the `subscriptionId` alias or the
/// canonical `subscriptionName` key; require one of them.
fn require_subscription(map: &mut serde_yaml::Mapping) -> Result<(), String> {
    let present = ["subscriptionId", "subscriptionName"]
        .into_iter()
        .any(|k| map.remove(k).is_some());
    if present {
        Ok(())
    } else {
        Err("missing required input `subscriptionId` for the selected scope".to_string())
    }
}

/// Template + advanced inputs shared by every "deploy template" command.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TemplateDeploySpec {
    #[serde(rename = "templateLocation", default)]
    _template_location: Option<String>,
    #[serde(rename = "csmFile", default)]
    _csm_file: Option<String>,
    #[serde(rename = "csmParametersFile", default)]
    _csm_parameters_file: Option<String>,
    #[serde(rename = "csmFileLink", default)]
    _csm_file_link: Option<String>,
    #[serde(rename = "csmParametersFileLink", default)]
    _csm_parameters_file_link: Option<String>,
    #[serde(rename = "overrideParameters", default)]
    _override_parameters: Option<String>,
    #[serde(rename = "deploymentMode", default)]
    _deployment_mode: Option<ArmDeploymentMode>,
    #[serde(rename = "deploymentName", default)]
    _deployment_name: Option<String>,
    #[serde(rename = "deploymentOutputs", default)]
    _deployment_outputs: Option<String>,
    #[serde(
        rename = "addSpnToEnvironment",
        default,
        deserialize_with = "de_opt_bool_flex"
    )]
    _add_spn_to_environment: Option<bool>,
    #[serde(
        rename = "useWithoutJSON",
        default,
        deserialize_with = "de_opt_bool_flex"
    )]
    _use_without_json: Option<bool>,
}

/// `action = DeleteRG` takes no template or advanced inputs.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeleteSpec {}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resource_group_deploy_linked_artifact() {
        let t = ArmTemplateDeployment::resource_group_deploy(ResourceGroupDeploy::new(
            "myAzureConnection",
            "00000000-0000-0000-0000-000000000000",
            "my-rg",
            "East US",
            ArmTemplateSource::linked_artifact("infra/main.bicep"),
        ))
        .into_step();

        assert_eq!(t.task, "AzureResourceManagerTemplateDeployment@3");
        assert_eq!(t.display_name, "ARM Template Deployment");
        assert_eq!(
            t.inputs.get("deploymentScope").map(String::as_str),
            Some("Resource Group")
        );
        assert_eq!(
            t.inputs.get("action").map(String::as_str),
            Some("Create Or Update Resource Group")
        );
        assert_eq!(
            t.inputs
                .get("azureResourceManagerConnection")
                .map(String::as_str),
            Some("myAzureConnection")
        );
        assert_eq!(
            t.inputs.get("subscriptionId").map(String::as_str),
            Some("00000000-0000-0000-0000-000000000000")
        );
        assert_eq!(
            t.inputs.get("resourceGroupName").map(String::as_str),
            Some("my-rg")
        );
        assert_eq!(
            t.inputs.get("location").map(String::as_str),
            Some("East US")
        );
        assert_eq!(
            t.inputs.get("templateLocation").map(String::as_str),
            Some("Linked artifact")
        );
        assert_eq!(
            t.inputs.get("csmFile").map(String::as_str),
            Some("infra/main.bicep")
        );
        assert!(t.inputs.get("csmParametersFile").is_none());
    }

    #[test]
    fn resource_group_deploy_with_parameters_and_options() {
        let t = ArmTemplateDeployment::resource_group_deploy(
            ResourceGroupDeploy::new(
                "conn",
                "sub-id",
                "rg",
                "West US",
                ArmTemplateSource::linked_artifact("main.json")
                    .with_parameters_file("main.parameters.json"),
            )
            .deployment_mode(ArmDeploymentMode::Complete)
            .deployment_name("my-deploy")
            .deployment_outputs("armOutputs")
            .add_spn_to_environment(true)
            .use_without_json(true),
        )
        .with_display_name("Deploy Infrastructure")
        .into_step();

        assert_eq!(t.display_name, "Deploy Infrastructure");
        assert_eq!(
            t.inputs.get("deploymentMode").map(String::as_str),
            Some("Complete")
        );
        assert_eq!(
            t.inputs.get("csmParametersFile").map(String::as_str),
            Some("main.parameters.json")
        );
        assert_eq!(
            t.inputs.get("deploymentName").map(String::as_str),
            Some("my-deploy")
        );
        assert_eq!(
            t.inputs.get("deploymentOutputs").map(String::as_str),
            Some("armOutputs")
        );
        assert_eq!(
            t.inputs.get("addSpnToEnvironment").map(String::as_str),
            Some("true")
        );
        assert_eq!(
            t.inputs.get("useWithoutJSON").map(String::as_str),
            Some("true")
        );
    }

    #[test]
    fn resource_group_delete() {
        let t = ArmTemplateDeployment::resource_group_delete(ResourceGroupDelete::new(
            "conn", "sub-id", "old-rg",
        ))
        .into_step();

        assert_eq!(t.task, "AzureResourceManagerTemplateDeployment@3");
        assert_eq!(t.display_name, "Delete Resource Group");
        assert_eq!(
            t.inputs.get("deploymentScope").map(String::as_str),
            Some("Resource Group")
        );
        assert_eq!(t.inputs.get("action").map(String::as_str), Some("DeleteRG"));
        assert_eq!(
            t.inputs.get("resourceGroupName").map(String::as_str),
            Some("old-rg")
        );
        assert!(t.inputs.get("templateLocation").is_none());
        assert!(t.inputs.get("csmFile").is_none());
    }

    #[test]
    fn subscription_deploy_url_source() {
        let url = "https://raw.githubusercontent.com/Azure/azure-quickstart-templates/master/101-vm-simple-windows/azuredeploy.json";
        let t = ArmTemplateDeployment::subscription_deploy(SubscriptionDeploy::new(
            "conn",
            "sub-id",
            "North Europe",
            ArmTemplateSource::url(url),
        ))
        .into_step();

        assert_eq!(
            t.inputs.get("deploymentScope").map(String::as_str),
            Some("Subscription")
        );
        assert!(t.inputs.get("action").is_none());
        assert_eq!(
            t.inputs.get("templateLocation").map(String::as_str),
            Some("URL of the file")
        );
        assert_eq!(t.inputs.get("csmFileLink").map(String::as_str), Some(url));
        assert!(t.inputs.get("csmFile").is_none());
        assert!(t.inputs.get("resourceGroupName").is_none());
    }

    #[test]
    fn management_group_deploy_no_subscription_id() {
        let t = ArmTemplateDeployment::management_group_deploy(ManagementGroupDeploy::new(
            "conn",
            "East US",
            ArmTemplateSource::linked_artifact("mg-policy.json"),
        ))
        .into_step();

        assert_eq!(
            t.inputs.get("deploymentScope").map(String::as_str),
            Some("Management Group")
        );
        assert!(t.inputs.get("subscriptionId").is_none());
        assert!(t.inputs.get("action").is_none());
        assert_eq!(
            t.inputs.get("csmFile").map(String::as_str),
            Some("mg-policy.json")
        );
    }

    #[test]
    fn tenant_deploy_validation_mode() {
        let t = ArmTemplateDeployment::tenant_deploy(
            TenantDeploy::new(
                "conn",
                "East US",
                ArmTemplateSource::linked_artifact("tenant-policy.json"),
            )
            .deployment_mode(ArmDeploymentMode::Validation),
        )
        .into_step();

        assert_eq!(
            t.inputs.get("deploymentScope").map(String::as_str),
            Some("Tenant")
        );
        assert_eq!(
            t.inputs.get("deploymentMode").map(String::as_str),
            Some("Validation")
        );
    }

    #[test]
    fn override_parameters_setter() {
        let t = ArmTemplateDeployment::resource_group_deploy(
            ResourceGroupDeploy::new(
                "conn",
                "sub",
                "rg",
                "East US",
                ArmTemplateSource::linked_artifact("template.json"),
            )
            .override_parameters("-appName \"my-app\" -env \"prod\""),
        )
        .into_step();

        assert_eq!(
            t.inputs.get("overrideParameters").map(String::as_str),
            Some("-appName \"my-app\" -env \"prod\"")
        );
    }
}
