//! Typed builder for `BicepDeploy@0`.
//!
//! `BicepDeploy@0` is a command/mode-dispatch task: the `type` input selects
//! between a standard deployment ([`BicepDeployment`]) and an Azure Deployment
//! Stack ([`BicepDeploymentStack`]), each with its own set of optional inputs.
//! The scope of the deployment is carried by the [`BicepScope`] enum, whose
//! variants hold the scope-specific required inputs (subscription ID, resource
//! group name, management group ID, or tenant ID).
//!
//! ADO task reference:
//! <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/bicep-deploy-v0>

use super::common::{push_bool, push_opt};
use crate::compile::ir::step::TaskStep;

// ── Deployment scope ──────────────────────────────────────────────────────────

/// Deployment scope for `BicepDeploy@0`.
///
/// Each variant carries the inputs that are required at that scope level.
/// The ADO input key is `scope`; additional required inputs differ per variant:
///
/// | Variant | ADO scope value | Additional required inputs |
/// |---------|-----------------|--------------------------|
/// | `ResourceGroup` | `resourceGroup` | `subscriptionId`, `resourceGroupName` |
/// | `Subscription` | `subscription` | `subscriptionId`, `location` |
/// | `ManagementGroup` | `managementGroup` | `managementGroupId`, `location` |
/// | `Tenant` | `tenant` | `tenantId`, `location` |
#[derive(Debug, Clone)]
pub enum BicepScope {
    /// Deploy to a resource group (`scope: resourceGroup`).
    ResourceGroup {
        /// Azure subscription ID.
        subscription_id: String,
        /// Resource group name.
        resource_group: String,
    },
    /// Deploy at subscription scope (`scope: subscription`).
    Subscription {
        /// Azure subscription ID.
        subscription_id: String,
        /// Location for deployment metadata storage.
        location: String,
    },
    /// Deploy at management group scope (`scope: managementGroup`).
    ManagementGroup {
        /// Management group ID.
        management_group_id: String,
        /// Location for deployment metadata storage.
        location: String,
    },
    /// Deploy at tenant scope (`scope: tenant`).
    Tenant {
        /// Tenant ID.
        tenant_id: String,
        /// Location for deployment metadata storage.
        location: String,
    },
}

impl BicepScope {
    fn as_ado_str(&self) -> &'static str {
        match self {
            BicepScope::ResourceGroup { .. } => "resourceGroup",
            BicepScope::Subscription { .. } => "subscription",
            BicepScope::ManagementGroup { .. } => "managementGroup",
            BicepScope::Tenant { .. } => "tenant",
        }
    }
}

// ── Operation enums ───────────────────────────────────────────────────────────

/// Operation for a standard `BicepDeploy@0` deployment (`type: deployment`).
///
/// ADO input: `operation`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BicepOperation {
    /// Create or update resources (`create`). This is the ADO default.
    Create,
    /// Validate the template without deploying (`validate`).
    Validate,
    /// Preview changes without deploying (`whatIf`).
    WhatIf,
}

impl BicepOperation {
    fn as_ado_str(self) -> &'static str {
        match self {
            BicepOperation::Create => "create",
            BicepOperation::Validate => "validate",
            BicepOperation::WhatIf => "whatIf",
        }
    }
}

/// Operation for a `BicepDeploy@0` deployment stack (`type: deploymentStack`).
///
/// ADO input: `operation`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BicepStackOperation {
    /// Create or update the deployment stack (`create`). This is the ADO default.
    Create,
    /// Validate the template without deploying (`validate`).
    Validate,
    /// Delete the deployment stack (`delete`).
    Delete,
}

impl BicepStackOperation {
    fn as_ado_str(self) -> &'static str {
        match self {
            BicepStackOperation::Create => "create",
            BicepStackOperation::Validate => "validate",
            BicepStackOperation::Delete => "delete",
        }
    }
}

// ── Deployment Stack enums ────────────────────────────────────────────────────

/// Action to take on resources not defined in the template when managing a
/// deployment stack.
///
/// ADO inputs: `actionOnUnmanageResources`, `actionOnUnmanageResourceGroups`,
/// `actionOnUnmanageManagementGroups`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BicepUnmanageAction {
    /// Delete the unmanaged resource (`delete`).
    Delete,
    /// Detach (remove from stack management) but do not delete (`detach`).
    /// This is the ADO default for `actionOnUnmanageResources`.
    Detach,
}

impl BicepUnmanageAction {
    fn as_ado_str(self) -> &'static str {
        match self {
            BicepUnmanageAction::Delete => "delete",
            BicepUnmanageAction::Detach => "detach",
        }
    }
}

/// Deny-settings mode for a deployment stack.
///
/// ADO input: `denySettingsMode`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BicepDenySettingsMode {
    /// No deny assignments (`none`). This is the ADO default.
    None,
    /// Deny delete operations only (`denyDelete`).
    DenyDelete,
    /// Deny all write and delete operations (`denyWriteAndDelete`).
    DenyWriteAndDelete,
}

impl BicepDenySettingsMode {
    fn as_ado_str(self) -> &'static str {
        match self {
            BicepDenySettingsMode::None => "none",
            BicepDenySettingsMode::DenyDelete => "denyDelete",
            BicepDenySettingsMode::DenyWriteAndDelete => "denyWriteAndDelete",
        }
    }
}

// ── Azure cloud environment ───────────────────────────────────────────────────

/// Azure cloud environment for `BicepDeploy@0`.
///
/// ADO input: `environment`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BicepEnvironment {
    /// Azure Public Cloud (`azureCloud`). This is the ADO default.
    Cloud,
    /// Azure China Cloud (`azureChinaCloud`).
    ChinaCloud,
    /// Azure German Cloud (`azureGermanCloud`).
    GermanCloud,
    /// Azure US Government (`azureUSGovernment`).
    UsGovernment,
}

impl BicepEnvironment {
    fn as_ado_str(self) -> &'static str {
        match self {
            BicepEnvironment::Cloud => "azureCloud",
            BicepEnvironment::ChinaCloud => "azureChinaCloud",
            BicepEnvironment::GermanCloud => "azureGermanCloud",
            BicepEnvironment::UsGovernment => "azureUSGovernment",
        }
    }
}

/// Validation level for deployment validate/whatIf operations.
///
/// ADO input: `validationLevel`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BicepValidationLevel {
    /// Validate using the resource provider, including RBAC checks (`provider`).
    Provider,
    /// Validate the template structure only (`template`).
    Template,
    /// Validate using the resource provider without RBAC checks (`providerNoRbac`).
    ProviderNoRbac,
}

impl BicepValidationLevel {
    fn as_ado_str(self) -> &'static str {
        match self {
            BicepValidationLevel::Provider => "provider",
            BicepValidationLevel::Template => "template",
            BicepValidationLevel::ProviderNoRbac => "providerNoRbac",
        }
    }
}

// ── Per-type option structs ───────────────────────────────────────────────────

/// Optional inputs for `BicepDeploy@0` when `type: deployment`.
#[derive(Debug, Clone, Default)]
pub struct BicepDeployment {
    /// ADO operation: `create` | `validate` | `whatIf`. Default: `create`.
    operation: Option<BicepOperation>,
    /// Validation level; only relevant for `validate` and `whatIf` operations.
    validation_level: Option<BicepValidationLevel>,
}

impl BicepDeployment {
    pub fn new() -> Self {
        Self::default()
    }

    /// `operation` — the deployment operation. Default: `create`.
    pub fn operation(mut self, value: BicepOperation) -> Self {
        self.operation = Some(value);
        self
    }

    /// `validationLevel` — relevant when `operation` is `validate` or `whatIf`.
    pub fn validation_level(mut self, value: BicepValidationLevel) -> Self {
        self.validation_level = Some(value);
        self
    }
}

/// Optional inputs for `BicepDeploy@0` when `type: deploymentStack`.
#[derive(Debug, Clone, Default)]
pub struct BicepDeploymentStack {
    /// `operation` — `create`, `validate`, or `delete`. Default: `create`.
    operation: Option<BicepStackOperation>,
    /// `actionOnUnmanageResources` — action on resources not in the template. Default: `detach`.
    action_on_unmanage_resources: Option<BicepUnmanageAction>,
    /// `actionOnUnmanageResourceGroups` — action on resource groups not in the template.
    action_on_unmanage_resource_groups: Option<BicepUnmanageAction>,
    /// `actionOnUnmanageManagementGroups` — action on management groups not in the template.
    action_on_unmanage_management_groups: Option<BicepUnmanageAction>,
    /// `denySettingsMode` — deny settings mode. Default: `none`.
    deny_settings_mode: Option<BicepDenySettingsMode>,
    /// `denySettingsExcludedActions` — comma-separated actions excluded from deny settings.
    deny_settings_excluded_actions: Option<String>,
    /// `denySettingsExcludedPrincipals` — comma-separated principal IDs excluded from deny settings.
    deny_settings_excluded_principals: Option<String>,
    /// `denySettingsApplyToChildScopes` — apply deny settings to child scopes.
    deny_settings_apply_to_child_scopes: Option<bool>,
    /// `bypassStackOutOfSyncError` — bypass errors when the stack is out of sync.
    bypass_stack_out_of_sync_error: Option<bool>,
    /// `tags` — tags to apply to the deployment stack.
    tags: Option<String>,
}

impl BicepDeploymentStack {
    pub fn new() -> Self {
        Self::default()
    }

    /// `operation` — `create`, `validate`, or `delete`. Default: `create`.
    pub fn operation(mut self, value: BicepStackOperation) -> Self {
        self.operation = Some(value);
        self
    }

    /// `actionOnUnmanageResources` — `delete` or `detach`. Default: `detach`.
    pub fn action_on_unmanage_resources(mut self, value: BicepUnmanageAction) -> Self {
        self.action_on_unmanage_resources = Some(value);
        self
    }

    /// `actionOnUnmanageResourceGroups` — `delete` or `detach`.
    pub fn action_on_unmanage_resource_groups(mut self, value: BicepUnmanageAction) -> Self {
        self.action_on_unmanage_resource_groups = Some(value);
        self
    }

    /// `actionOnUnmanageManagementGroups` — `delete` or `detach`.
    pub fn action_on_unmanage_management_groups(mut self, value: BicepUnmanageAction) -> Self {
        self.action_on_unmanage_management_groups = Some(value);
        self
    }

    /// `denySettingsMode` — `none`, `denyDelete`, or `denyWriteAndDelete`. Default: `none`.
    pub fn deny_settings_mode(mut self, value: BicepDenySettingsMode) -> Self {
        self.deny_settings_mode = Some(value);
        self
    }

    /// `denySettingsExcludedActions` — comma-separated list of excluded actions.
    pub fn deny_settings_excluded_actions(mut self, value: impl Into<String>) -> Self {
        self.deny_settings_excluded_actions = Some(value.into());
        self
    }

    /// `denySettingsExcludedPrincipals` — comma-separated list of principal object IDs.
    pub fn deny_settings_excluded_principals(mut self, value: impl Into<String>) -> Self {
        self.deny_settings_excluded_principals = Some(value.into());
        self
    }

    /// `denySettingsApplyToChildScopes` — extend deny settings to child scopes.
    pub fn deny_settings_apply_to_child_scopes(mut self, value: bool) -> Self {
        self.deny_settings_apply_to_child_scopes = Some(value);
        self
    }

    /// `bypassStackOutOfSyncError` — bypass stack-out-of-sync errors.
    pub fn bypass_stack_out_of_sync_error(mut self, value: bool) -> Self {
        self.bypass_stack_out_of_sync_error = Some(value);
        self
    }

    /// `tags` — tags for the deployment stack resource.
    pub fn tags(mut self, value: impl Into<String>) -> Self {
        self.tags = Some(value.into());
        self
    }
}

/// Command dispatch enum for `BicepDeploy@0`.
///
/// Selects the `type` input and carries the per-type optional inputs.
#[derive(Debug, Clone)]
pub enum BicepDeploymentType {
    /// Standard deployment (`type: deployment`).
    Deployment(BicepDeployment),
    /// Azure Deployment Stack (`type: deploymentStack`).
    DeploymentStack(BicepDeploymentStack),
}

impl BicepDeploymentType {
    fn as_ado_str(&self) -> &'static str {
        match self {
            BicepDeploymentType::Deployment(_) => "deployment",
            BicepDeploymentType::DeploymentStack(_) => "deploymentStack",
        }
    }
}

// ── Main builder ──────────────────────────────────────────────────────────────

/// Builder for a [`TaskStep`] invoking `BicepDeploy@0`.
///
/// Deploys Azure resources using Bicep templates. Supports standard deployments
/// and Azure Deployment Stacks across resource group, subscription, management
/// group, and tenant scopes.
///
/// Required inputs are the ARM service connection (positional) and the
/// deployment scope ([`BicepScope`], positional). The deployment type
/// ([`BicepDeploymentType`]) is also required and determines which optional
/// inputs are available.
///
/// ```rust,ignore
/// use crate::compile::ir::tasks::bicep_deploy::{
///     BicepDeploy, BicepDeployment, BicepDeploymentType, BicepScope,
/// };
///
/// let step = BicepDeploy::new(
///     "MyArmConnection",
///     BicepScope::ResourceGroup {
///         subscription_id: "$(SubscriptionId)".into(),
///         resource_group: "my-rg".into(),
///     },
///     BicepDeploymentType::Deployment(BicepDeployment::new()),
/// )
/// .template_file("infra/main.bicep")
/// .parameters_file("infra/main.parameters.json")
/// .into_step();
/// ```
///
/// ADO task reference:
/// <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/bicep-deploy-v0>
#[derive(Debug, Clone)]
pub struct BicepDeploy {
    connection: String,
    scope: BicepScope,
    kind: BicepDeploymentType,
    name: Option<String>,
    template_file: Option<String>,
    parameters_file: Option<String>,
    parameters: Option<String>,
    environment: Option<BicepEnvironment>,
    display_name: Option<String>,
}

impl BicepDeploy {
    /// Create a new `BicepDeploy` step.
    ///
    /// - `connection` — Azure Resource Manager service connection name.
    /// - `scope` — deployment scope (carries scope-specific required inputs).
    /// - `kind` — deployment type: standard [`BicepDeployment`] or
    ///   [`BicepDeploymentStack`] (carries per-type optional inputs).
    pub fn new(
        connection: impl Into<String>,
        scope: BicepScope,
        kind: BicepDeploymentType,
    ) -> Self {
        Self {
            connection: connection.into(),
            scope,
            kind,
            name: None,
            template_file: None,
            parameters_file: None,
            parameters: None,
            environment: None,
            display_name: None,
        }
    }

    /// `name` — name of the deployment or deployment stack. Auto-generated when absent.
    pub fn name(mut self, value: impl Into<String>) -> Self {
        self.name = Some(value.into());
        self
    }

    /// `templateFile` — path to the Bicep template file (`.bicep`).
    pub fn template_file(mut self, value: impl Into<String>) -> Self {
        self.template_file = Some(value.into());
        self
    }

    /// `parametersFile` — path to the parameters file (`.json` or `.bicepparam`).
    pub fn parameters_file(mut self, value: impl Into<String>) -> Self {
        self.parameters_file = Some(value.into());
        self
    }

    /// `parameters` — inline override parameters as a JSON or YAML object string.
    pub fn parameters(mut self, value: impl Into<String>) -> Self {
        self.parameters = Some(value.into());
        self
    }

    /// `environment` — target Azure cloud environment. Default: `azureCloud`.
    pub fn environment(mut self, value: BicepEnvironment) -> Self {
        self.environment = Some(value);
        self
    }

    /// Override the default `displayName`.
    pub fn with_display_name(mut self, value: impl Into<String>) -> Self {
        self.display_name = Some(value.into());
        self
    }

    /// Lower into a [`TaskStep`].
    pub fn into_step(self) -> TaskStep {
        let mut t = TaskStep::new(
            "BicepDeploy@0",
            self.display_name
                .unwrap_or_else(|| "Deploy Bicep template".into()),
        )
        .with_input("azureResourceManagerConnection", self.connection)
        .with_input("type", self.kind.as_ado_str())
        .with_input("scope", self.scope.as_ado_str());

        // Scope-specific required inputs.
        match &self.scope {
            BicepScope::ResourceGroup {
                subscription_id,
                resource_group,
            } => {
                t = t
                    .with_input("subscriptionId", subscription_id)
                    .with_input("resourceGroupName", resource_group);
            }
            BicepScope::Subscription {
                subscription_id,
                location,
            } => {
                t = t
                    .with_input("subscriptionId", subscription_id)
                    .with_input("location", location);
            }
            BicepScope::ManagementGroup {
                management_group_id,
                location,
            } => {
                t = t
                    .with_input("managementGroupId", management_group_id)
                    .with_input("location", location);
            }
            BicepScope::Tenant { tenant_id, location } => {
                t = t
                    .with_input("tenantId", tenant_id)
                    .with_input("location", location);
            }
        }

        // Shared optional inputs.
        push_opt(&mut t, "name", self.name);
        push_opt(&mut t, "templateFile", self.template_file);
        push_opt(&mut t, "parametersFile", self.parameters_file);
        push_opt(&mut t, "parameters", self.parameters);
        if let Some(env) = self.environment {
            t = t.with_input("environment", env.as_ado_str());
        }

        // Per-type inputs.
        match self.kind {
            BicepDeploymentType::Deployment(d) => {
                if let Some(op) = d.operation {
                    t = t.with_input("operation", op.as_ado_str());
                }
                if let Some(vl) = d.validation_level {
                    t = t.with_input("validationLevel", vl.as_ado_str());
                }
            }
            BicepDeploymentType::DeploymentStack(s) => {
                if let Some(op) = s.operation {
                    t = t.with_input("operation", op.as_ado_str());
                }
                if let Some(v) = s.action_on_unmanage_resources {
                    t = t.with_input("actionOnUnmanageResources", v.as_ado_str());
                }
                if let Some(v) = s.action_on_unmanage_resource_groups {
                    t = t.with_input("actionOnUnmanageResourceGroups", v.as_ado_str());
                }
                if let Some(v) = s.action_on_unmanage_management_groups {
                    t = t.with_input("actionOnUnmanageManagementGroups", v.as_ado_str());
                }
                if let Some(v) = s.deny_settings_mode {
                    t = t.with_input("denySettingsMode", v.as_ado_str());
                }
                push_opt(
                    &mut t,
                    "denySettingsExcludedActions",
                    s.deny_settings_excluded_actions,
                );
                push_opt(
                    &mut t,
                    "denySettingsExcludedPrincipals",
                    s.deny_settings_excluded_principals,
                );
                push_bool(
                    &mut t,
                    "denySettingsApplyToChildScopes",
                    s.deny_settings_apply_to_child_scopes,
                );
                push_bool(
                    &mut t,
                    "bypassStackOutOfSyncError",
                    s.bypass_stack_out_of_sync_error,
                );
                push_opt(&mut t, "tags", s.tags);
            }
        }

        t
    }
}

/// Convenience constructor: create a deployment to a resource group.
///
/// Equivalent to:
/// ```rust,ignore
/// BicepDeploy::new(
///     connection,
///     BicepScope::ResourceGroup { subscription_id, resource_group },
///     BicepDeploymentType::Deployment(BicepDeployment::new()),
/// )
/// ```
pub fn deploy_to_resource_group(
    connection: impl Into<String>,
    subscription_id: impl Into<String>,
    resource_group: impl Into<String>,
) -> BicepDeploy {
    BicepDeploy::new(
        connection,
        BicepScope::ResourceGroup {
            subscription_id: subscription_id.into(),
            resource_group: resource_group.into(),
        },
        BicepDeploymentType::Deployment(BicepDeployment::new()),
    )
}

/// Convenience constructor: create a deployment at subscription scope.
pub fn deploy_to_subscription(
    connection: impl Into<String>,
    subscription_id: impl Into<String>,
    location: impl Into<String>,
) -> BicepDeploy {
    BicepDeploy::new(
        connection,
        BicepScope::Subscription {
            subscription_id: subscription_id.into(),
            location: location.into(),
        },
        BicepDeploymentType::Deployment(BicepDeployment::new()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resource_group_scope_emits_required_inputs() {
        let t = BicepDeploy::new(
            "MyArmConnection",
            BicepScope::ResourceGroup {
                subscription_id: "00000000-0000-0000-0000-000000000000".into(),
                resource_group: "my-rg".into(),
            },
            BicepDeploymentType::Deployment(BicepDeployment::new()),
        )
        .into_step();

        assert_eq!(t.task, "BicepDeploy@0");
        assert_eq!(t.display_name, "Deploy Bicep template");
        assert_eq!(
            t.inputs.get("azureResourceManagerConnection").map(String::as_str),
            Some("MyArmConnection")
        );
        assert_eq!(t.inputs.get("type").map(String::as_str), Some("deployment"));
        assert_eq!(t.inputs.get("scope").map(String::as_str), Some("resourceGroup"));
        assert_eq!(
            t.inputs.get("subscriptionId").map(String::as_str),
            Some("00000000-0000-0000-0000-000000000000")
        );
        assert_eq!(t.inputs.get("resourceGroupName").map(String::as_str), Some("my-rg"));
        // No optional inputs emitted by default.
        assert!(t.inputs.get("operation").is_none());
        assert!(t.inputs.get("templateFile").is_none());
    }

    #[test]
    fn template_and_parameters_file_are_emitted_when_set() {
        let t = deploy_to_resource_group("conn", "sub-id", "rg-name")
            .template_file("infra/main.bicep")
            .parameters_file("infra/main.parameters.json")
            .into_step();

        assert_eq!(
            t.inputs.get("templateFile").map(String::as_str),
            Some("infra/main.bicep")
        );
        assert_eq!(
            t.inputs.get("parametersFile").map(String::as_str),
            Some("infra/main.parameters.json")
        );
    }

    #[test]
    fn deployment_operation_is_emitted_when_set() {
        let t = BicepDeploy::new(
            "conn",
            BicepScope::ResourceGroup {
                subscription_id: "sub".into(),
                resource_group: "rg".into(),
            },
            BicepDeploymentType::Deployment(
                BicepDeployment::new()
                    .operation(BicepOperation::WhatIf)
                    .validation_level(BicepValidationLevel::Provider),
            ),
        )
        .into_step();

        assert_eq!(t.inputs.get("operation").map(String::as_str), Some("whatIf"));
        assert_eq!(
            t.inputs.get("validationLevel").map(String::as_str),
            Some("provider")
        );
    }

    #[test]
    fn subscription_scope_emits_location_not_resource_group() {
        let t = deploy_to_subscription("conn", "sub-id", "eastus").into_step();

        assert_eq!(t.inputs.get("scope").map(String::as_str), Some("subscription"));
        assert_eq!(t.inputs.get("subscriptionId").map(String::as_str), Some("sub-id"));
        assert_eq!(t.inputs.get("location").map(String::as_str), Some("eastus"));
        assert!(t.inputs.get("resourceGroupName").is_none());
    }

    #[test]
    fn management_group_scope() {
        let t = BicepDeploy::new(
            "conn",
            BicepScope::ManagementGroup {
                management_group_id: "mg-root".into(),
                location: "westeurope".into(),
            },
            BicepDeploymentType::Deployment(BicepDeployment::new()),
        )
        .into_step();

        assert_eq!(t.inputs.get("scope").map(String::as_str), Some("managementGroup"));
        assert_eq!(
            t.inputs.get("managementGroupId").map(String::as_str),
            Some("mg-root")
        );
        assert_eq!(t.inputs.get("location").map(String::as_str), Some("westeurope"));
    }

    #[test]
    fn tenant_scope() {
        let t = BicepDeploy::new(
            "conn",
            BicepScope::Tenant {
                tenant_id: "tenant-id".into(),
                location: "northeurope".into(),
            },
            BicepDeploymentType::Deployment(BicepDeployment::new()),
        )
        .into_step();

        assert_eq!(t.inputs.get("scope").map(String::as_str), Some("tenant"));
        assert_eq!(t.inputs.get("tenantId").map(String::as_str), Some("tenant-id"));
        assert_eq!(t.inputs.get("location").map(String::as_str), Some("northeurope"));
    }

    #[test]
    fn deployment_stack_emits_type_and_stack_options() {
        let t = BicepDeploy::new(
            "conn",
            BicepScope::ResourceGroup {
                subscription_id: "sub".into(),
                resource_group: "rg".into(),
            },
            BicepDeploymentType::DeploymentStack(
                BicepDeploymentStack::new()
                    .action_on_unmanage_resources(BicepUnmanageAction::Delete)
                    .deny_settings_mode(BicepDenySettingsMode::DenyDelete)
                    .deny_settings_apply_to_child_scopes(true)
                    .bypass_stack_out_of_sync_error(false),
            ),
        )
        .template_file("infra/main.bicep")
        .into_step();

        assert_eq!(t.inputs.get("type").map(String::as_str), Some("deploymentStack"));
        assert_eq!(
            t.inputs.get("actionOnUnmanageResources").map(String::as_str),
            Some("delete")
        );
        assert_eq!(
            t.inputs.get("denySettingsMode").map(String::as_str),
            Some("denyDelete")
        );
        assert_eq!(
            t.inputs.get("denySettingsApplyToChildScopes").map(String::as_str),
            Some("true")
        );
        assert_eq!(
            t.inputs.get("bypassStackOutOfSyncError").map(String::as_str),
            Some("false")
        );
        assert_eq!(
            t.inputs.get("templateFile").map(String::as_str),
            Some("infra/main.bicep")
        );
    }

    #[test]
    fn deployment_stack_delete_operation() {
        let t = BicepDeploy::new(
            "conn",
            BicepScope::ResourceGroup {
                subscription_id: "sub".into(),
                resource_group: "rg".into(),
            },
            BicepDeploymentType::DeploymentStack(
                BicepDeploymentStack::new().operation(BicepStackOperation::Delete),
            ),
        )
        .name("my-stack")
        .into_step();

        assert_eq!(t.inputs.get("operation").map(String::as_str), Some("delete"));
        assert_eq!(t.inputs.get("name").map(String::as_str), Some("my-stack"));
    }

    #[test]
    fn display_name_override() {
        let t = deploy_to_resource_group("conn", "sub", "rg")
            .with_display_name("Deploy infra")
            .into_step();

        assert_eq!(t.display_name, "Deploy infra");
    }

    #[test]
    fn environment_emitted_when_set() {
        let t = deploy_to_resource_group("conn", "sub", "rg")
            .environment(BicepEnvironment::UsGovernment)
            .into_step();

        assert_eq!(
            t.inputs.get("environment").map(String::as_str),
            Some("azureUSGovernment")
        );
    }

    #[test]
    fn inline_parameters_override() {
        let t = deploy_to_resource_group("conn", "sub", "rg")
            .parameters(r#"{"param1": "value1"}"#)
            .into_step();

        assert_eq!(
            t.inputs.get("parameters").map(String::as_str),
            Some(r#"{"param1": "value1"}"#)
        );
    }
}
