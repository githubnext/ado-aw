//! Typed builder for `KubernetesManifest@1`.
//!
//! Deploys, bakes, creates secrets, scales, patches, promotes, or rejects
//! Kubernetes resources in a cluster. This is a command-dispatch task: each
//! [`KubernetesManifestAction`] variant carries its own per-action optional
//! inputs; shared connection and namespace inputs live on the
//! [`KubernetesManifest`] builder itself.
//!
//! ADO task reference:
//! <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/kubernetes-manifest-v1>

use super::common::{de_opt_str_or_int, push_bool, push_opt};
use crate::compile::ir::step::TaskStep;
use serde::Deserialize;
use serde_yaml::Value;

/// Service-connection type for `KubernetesManifest@1` (all actions except `bake`).
#[derive(Debug, Clone, Deserialize)]
pub enum ConnectionType {
    /// Uses a KubeConfig file or Service Account.
    #[serde(rename = "kubernetesServiceConnection")]
    KubernetesServiceConnection,
    /// Uses an AKS instance via an Azure Resource Manager service connection;
    /// does not require cluster access at service-connection configuration time.
    #[serde(rename = "azureResourceManager")]
    AzureResourceManager,
}

impl ConnectionType {
    pub fn as_ado_str(&self) -> &'static str {
        match self {
            Self::KubernetesServiceConnection => "kubernetesServiceConnection",
            Self::AzureResourceManager => "azureResourceManager",
        }
    }
}

/// Deployment strategy for `deploy`, `promote`, and `reject` actions.
#[derive(Debug, Clone, Deserialize)]
pub enum DeploymentStrategy {
    #[serde(rename = "none")]
    None,
    #[serde(rename = "canary")]
    Canary,
}

impl DeploymentStrategy {
    pub fn as_ado_str(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Canary => "canary",
        }
    }
}

/// Traffic split method when `strategy = canary`.
#[derive(Debug, Clone, Deserialize)]
pub enum TrafficSplitMethod {
    /// Pod-level traffic splitting.
    #[serde(rename = "pod")]
    Pod,
    /// Service Mesh Interface (SMI) traffic splitting.
    #[serde(rename = "smi")]
    Smi,
}

impl TrafficSplitMethod {
    pub fn as_ado_str(&self) -> &'static str {
        match self {
            Self::Pod => "pod",
            Self::Smi => "smi",
        }
    }
}

/// Render engine for the `bake` action.
#[derive(Debug, Clone, Deserialize)]
pub enum RenderType {
    #[serde(rename = "helm")]
    Helm,
    #[serde(rename = "kompose")]
    Kompose,
    #[serde(rename = "kustomize")]
    Kustomize,
}

impl RenderType {
    pub fn as_ado_str(&self) -> &'static str {
        match self {
            Self::Helm => "helm",
            Self::Kompose => "kompose",
            Self::Kustomize => "kustomize",
        }
    }
}

/// Kubernetes resource kind, used by the `scale` and `patch` actions.
#[derive(Debug, Clone, Deserialize)]
pub enum ResourceKind {
    #[serde(rename = "deployment")]
    Deployment,
    #[serde(rename = "replicaset")]
    ReplicaSet,
    #[serde(rename = "statefulset")]
    StatefulSet,
}

impl ResourceKind {
    pub fn as_ado_str(&self) -> &'static str {
        match self {
            Self::Deployment => "deployment",
            Self::ReplicaSet => "replicaset",
            Self::StatefulSet => "statefulset",
        }
    }
}

/// Merge strategy for the `patch` action.
#[derive(Debug, Clone, Deserialize)]
pub enum MergeStrategy {
    #[serde(rename = "json")]
    Json,
    #[serde(rename = "merge")]
    Merge,
    #[serde(rename = "strategic")]
    Strategic,
}

impl MergeStrategy {
    pub fn as_ado_str(&self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Merge => "merge",
            Self::Strategic => "strategic",
        }
    }
}

/// Kubernetes secret type for the `createSecret` action.
#[derive(Debug, Clone, Deserialize)]
pub enum SecretType {
    #[serde(rename = "dockerRegistry")]
    DockerRegistry,
    #[serde(rename = "generic")]
    Generic,
}

impl SecretType {
    pub fn as_ado_str(&self) -> &'static str {
        match self {
            Self::DockerRegistry => "dockerRegistry",
            Self::Generic => "generic",
        }
    }
}

/// Resource-to-patch selector for the `patch` action.
#[derive(Debug, Clone, Deserialize)]
pub enum PatchTarget {
    /// Patch a resource described by a manifest file.
    #[serde(rename = "file")]
    File,
    /// Patch a resource identified by kind and name.
    #[serde(rename = "name")]
    Name,
}

impl PatchTarget {
    pub fn as_ado_str(&self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Name => "name",
        }
    }
}

// ─── Per-action data structs ──────────────────────────────────────────────────

/// Inputs specific to the `deploy` action.
#[derive(Debug, Clone)]
pub struct KubernetesManifestDeploy {
    manifests: String,
    strategy: Option<DeploymentStrategy>,
    traffic_split_method: Option<TrafficSplitMethod>,
    percentage: Option<String>,
    baseline_and_canary_replicas: Option<String>,
    containers: Option<String>,
    image_pull_secrets: Option<String>,
    rollout_status_timeout: Option<String>,
    resource_type: Option<String>,
}

impl KubernetesManifestDeploy {
    /// Required: `manifests` — newline-separated paths to Kubernetes manifest files.
    pub fn new(manifests: impl Into<String>) -> Self {
        Self {
            manifests: manifests.into(),
            strategy: None,
            traffic_split_method: None,
            percentage: None,
            baseline_and_canary_replicas: None,
            containers: None,
            image_pull_secrets: None,
            rollout_status_timeout: None,
            resource_type: None,
        }
    }

    /// `strategy` — deployment strategy (default `none`).
    pub fn strategy(mut self, value: DeploymentStrategy) -> Self {
        self.strategy = Some(value);
        self
    }

    /// `trafficSplitMethod` — traffic split method for canary strategy (default `pod`).
    pub fn traffic_split_method(mut self, value: TrafficSplitMethod) -> Self {
        self.traffic_split_method = Some(value);
        self
    }

    /// `percentage` — canary traffic percentage (default `"0"`).
    pub fn percentage(mut self, value: impl Into<String>) -> Self {
        self.percentage = Some(value.into());
        self
    }

    /// `baselineAndCanaryReplicas` — replica count for baseline/canary (SMI only, default `"1"`).
    pub fn baseline_and_canary_replicas(mut self, value: impl Into<String>) -> Self {
        self.baseline_and_canary_replicas = Some(value.into());
        self
    }

    /// `containers` — fully qualified container image names (newline-separated).
    pub fn containers(mut self, value: impl Into<String>) -> Self {
        self.containers = Some(value.into());
        self
    }

    /// `imagePullSecrets` — image pull secret names (newline-separated).
    pub fn image_pull_secrets(mut self, value: impl Into<String>) -> Self {
        self.image_pull_secrets = Some(value.into());
        self
    }

    /// `rolloutStatusTimeout` — seconds to wait for rollout status (default `"0"`).
    pub fn rollout_status_timeout(mut self, value: impl Into<String>) -> Self {
        self.rollout_status_timeout = Some(value.into());
        self
    }

    /// `resourceType` — AKS resource type (default `Microsoft.ContainerService/managedClusters`).
    pub fn resource_type(mut self, value: impl Into<String>) -> Self {
        self.resource_type = Some(value.into());
        self
    }
}

/// Inputs specific to the `bake` action.
///
/// Renders Helm charts, Kompose files, or Kustomize configurations into
/// plain Kubernetes manifest files.
#[derive(Debug, Clone, Default)]
pub struct KubernetesManifestBake {
    render_type: Option<RenderType>,
    /// Required when `renderType = helm`.
    helm_chart: Option<String>,
    /// Required when `renderType = kompose`.
    docker_compose_file: Option<String>,
    /// Used when `renderType = kustomize`.
    kustomization_path: Option<String>,
    release_name: Option<String>,
    override_files: Option<String>,
    overrides: Option<String>,
    containers: Option<String>,
}

impl KubernetesManifestBake {
    pub fn new() -> Self {
        Self::default()
    }

    /// `renderType` — render engine (default `helm`).
    pub fn render_type(mut self, value: RenderType) -> Self {
        self.render_type = Some(value);
        self
    }

    /// `helmChart` — path to Helm chart (required when `renderType = helm`).
    pub fn helm_chart(mut self, value: impl Into<String>) -> Self {
        self.helm_chart = Some(value.into());
        self
    }

    /// `dockerComposeFile` — path to docker-compose file (required when `renderType = kompose`).
    pub fn docker_compose_file(mut self, value: impl Into<String>) -> Self {
        self.docker_compose_file = Some(value.into());
        self
    }

    /// `kustomizationPath` — path to kustomization directory (used when `renderType = kustomize`).
    pub fn kustomization_path(mut self, value: impl Into<String>) -> Self {
        self.kustomization_path = Some(value.into());
        self
    }

    /// `releaseName` — Helm release name.
    pub fn release_name(mut self, value: impl Into<String>) -> Self {
        self.release_name = Some(value.into());
        self
    }

    /// `overrideFiles` — Helm values file paths (newline-separated).
    pub fn override_files(mut self, value: impl Into<String>) -> Self {
        self.override_files = Some(value.into());
        self
    }

    /// `overrides` — Helm `--set` overrides (newline-separated `key=value` pairs).
    pub fn overrides(mut self, value: impl Into<String>) -> Self {
        self.overrides = Some(value.into());
        self
    }

    /// `containers` — container images to substitute into the baked manifests.
    pub fn containers(mut self, value: impl Into<String>) -> Self {
        self.containers = Some(value.into());
        self
    }
}

/// Inputs specific to the `createSecret` action.
#[derive(Debug, Clone)]
pub struct KubernetesManifestCreateSecret {
    secret_type: SecretType,
    secret_name: Option<String>,
    secret_arguments: Option<String>,
    docker_registry_endpoint: Option<String>,
}

impl KubernetesManifestCreateSecret {
    /// Required: `secretType` — type of Kubernetes secret to create (default `dockerRegistry`).
    pub fn new(secret_type: SecretType) -> Self {
        Self {
            secret_type,
            secret_name: None,
            secret_arguments: None,
            docker_registry_endpoint: None,
        }
    }

    /// `secretName` — name for the secret.
    pub fn secret_name(mut self, value: impl Into<String>) -> Self {
        self.secret_name = Some(value.into());
        self
    }

    /// `secretArguments` — generic secret key-value arguments (used when `secretType = generic`).
    pub fn secret_arguments(mut self, value: impl Into<String>) -> Self {
        self.secret_arguments = Some(value.into());
        self
    }

    /// `dockerRegistryEndpoint` — Docker registry service connection (used when `secretType = dockerRegistry`).
    pub fn docker_registry_endpoint(mut self, value: impl Into<String>) -> Self {
        self.docker_registry_endpoint = Some(value.into());
        self
    }
}

/// Inputs specific to the `delete` action.
#[derive(Debug, Clone, Default)]
pub struct KubernetesManifestDelete {
    arguments: Option<String>,
}

impl KubernetesManifestDelete {
    pub fn new() -> Self {
        Self::default()
    }

    /// `arguments` — `kubectl delete` arguments.
    pub fn arguments(mut self, value: impl Into<String>) -> Self {
        self.arguments = Some(value.into());
        self
    }
}

/// Inputs specific to the `patch` action.
#[derive(Debug, Clone)]
pub struct KubernetesManifestPatch {
    resource_to_patch: PatchTarget,
    merge_strategy: MergeStrategy,
    patch: String,
    resource_file_to_patch: Option<String>,
    kind: Option<ResourceKind>,
    name: Option<String>,
    rollout_status_timeout: Option<String>,
}

impl KubernetesManifestPatch {
    /// Required: `resourceToPatch`, `mergeStrategy`, `patch`.
    pub fn new(
        resource_to_patch: PatchTarget,
        merge_strategy: MergeStrategy,
        patch: impl Into<String>,
    ) -> Self {
        Self {
            resource_to_patch,
            merge_strategy,
            patch: patch.into(),
            resource_file_to_patch: None,
            kind: None,
            name: None,
            rollout_status_timeout: None,
        }
    }

    /// `resourceFileToPatch` — path to manifest file (required when `resourceToPatch = file`).
    pub fn resource_file_to_patch(mut self, value: impl Into<String>) -> Self {
        self.resource_file_to_patch = Some(value.into());
        self
    }

    /// `kind` — resource kind (required when `resourceToPatch = name`).
    pub fn kind(mut self, value: ResourceKind) -> Self {
        self.kind = Some(value);
        self
    }

    /// `name` — resource name (required when `resourceToPatch = name`).
    pub fn name(mut self, value: impl Into<String>) -> Self {
        self.name = Some(value.into());
        self
    }

    /// `rolloutStatusTimeout` — seconds to wait for rollout status (default `"0"`).
    pub fn rollout_status_timeout(mut self, value: impl Into<String>) -> Self {
        self.rollout_status_timeout = Some(value.into());
        self
    }
}

/// Inputs specific to the `promote` action.
#[derive(Debug, Clone)]
pub struct KubernetesManifestPromote {
    manifests: String,
    strategy: Option<DeploymentStrategy>,
    containers: Option<String>,
    image_pull_secrets: Option<String>,
    rollout_status_timeout: Option<String>,
}

impl KubernetesManifestPromote {
    /// Required: `manifests` — paths to manifest files (newline-separated).
    pub fn new(manifests: impl Into<String>) -> Self {
        Self {
            manifests: manifests.into(),
            strategy: None,
            containers: None,
            image_pull_secrets: None,
            rollout_status_timeout: None,
        }
    }

    /// `strategy` — deployment strategy.
    pub fn strategy(mut self, value: DeploymentStrategy) -> Self {
        self.strategy = Some(value);
        self
    }

    /// `containers` — container image names to substitute (newline-separated).
    pub fn containers(mut self, value: impl Into<String>) -> Self {
        self.containers = Some(value.into());
        self
    }

    /// `imagePullSecrets` — image pull secret names (newline-separated).
    pub fn image_pull_secrets(mut self, value: impl Into<String>) -> Self {
        self.image_pull_secrets = Some(value.into());
        self
    }

    /// `rolloutStatusTimeout` — seconds to wait for rollout status (default `"0"`).
    pub fn rollout_status_timeout(mut self, value: impl Into<String>) -> Self {
        self.rollout_status_timeout = Some(value.into());
        self
    }
}

/// Inputs specific to the `scale` action.
#[derive(Debug, Clone)]
pub struct KubernetesManifestScale {
    kind: ResourceKind,
    name: String,
    replicas: String,
    rollout_status_timeout: Option<String>,
}

impl KubernetesManifestScale {
    /// Required: `kind`, `name`, `replicas`.
    pub fn new(
        kind: ResourceKind,
        name: impl Into<String>,
        replicas: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            name: name.into(),
            replicas: replicas.into(),
            rollout_status_timeout: None,
        }
    }

    /// `rolloutStatusTimeout` — seconds to wait for rollout status (default `"0"`).
    pub fn rollout_status_timeout(mut self, value: impl Into<String>) -> Self {
        self.rollout_status_timeout = Some(value.into());
        self
    }
}

/// Inputs specific to the `reject` action.
#[derive(Debug, Clone)]
pub struct KubernetesManifestReject {
    manifests: String,
    strategy: Option<DeploymentStrategy>,
}

impl KubernetesManifestReject {
    /// Required: `manifests` — paths to manifest files (newline-separated).
    pub fn new(manifests: impl Into<String>) -> Self {
        Self {
            manifests: manifests.into(),
            strategy: None,
        }
    }

    /// `strategy` — deployment strategy.
    pub fn strategy(mut self, value: DeploymentStrategy) -> Self {
        self.strategy = Some(value);
        self
    }
}

// ─── Action enum ─────────────────────────────────────────────────────────────

/// Action selector for [`KubernetesManifest`].
///
/// Each variant carries its own per-action inputs; shared connection and
/// namespace inputs live on the [`KubernetesManifest`] builder itself.
#[derive(Debug, Clone)]
pub enum KubernetesManifestAction {
    Deploy(KubernetesManifestDeploy),
    Bake(KubernetesManifestBake),
    CreateSecret(KubernetesManifestCreateSecret),
    Delete(KubernetesManifestDelete),
    Patch(KubernetesManifestPatch),
    Promote(KubernetesManifestPromote),
    Scale(KubernetesManifestScale),
    Reject(KubernetesManifestReject),
}

// ─── Main builder ─────────────────────────────────────────────────────────────

/// Builder for a [`TaskStep`] invoking `KubernetesManifest@1`.
///
/// Shared connection inputs (all actions except `bake`) are set on the builder
/// itself; per-action inputs are carried by the [`KubernetesManifestAction`] variant.
///
/// ```no_run
/// use ado_aw::compile::ir::tasks::kubernetes_manifest::{
///     KubernetesManifest, KubernetesManifestDeploy,
/// };
/// let step = KubernetesManifest::deploy(
///     KubernetesManifestDeploy::new("manifests/*.yaml"),
/// )
/// .kubernetes_service_connection("myCluster")
/// .namespace("production")
/// .into_step();
/// ```
#[derive(Debug, Clone)]
pub struct KubernetesManifest {
    action: KubernetesManifestAction,
    connection_type: Option<ConnectionType>,
    kubernetes_service_connection: Option<String>,
    azure_subscription_connection: Option<String>,
    azure_resource_group: Option<String>,
    kubernetes_cluster: Option<String>,
    use_cluster_admin: Option<bool>,
    namespace: Option<String>,
    display_name: Option<String>,
}

impl KubernetesManifest {
    /// Construct from an explicit [`KubernetesManifestAction`].
    pub fn new(action: KubernetesManifestAction) -> Self {
        Self {
            action,
            connection_type: None,
            kubernetes_service_connection: None,
            azure_subscription_connection: None,
            azure_resource_group: None,
            kubernetes_cluster: None,
            use_cluster_admin: None,
            namespace: None,
            display_name: None,
        }
    }

    /// `action: deploy` — apply manifest files to the cluster.
    pub fn deploy(spec: KubernetesManifestDeploy) -> Self {
        Self::new(KubernetesManifestAction::Deploy(spec))
    }

    /// `action: bake` — render Helm/Kompose/Kustomize sources into manifests.
    pub fn bake(spec: KubernetesManifestBake) -> Self {
        Self::new(KubernetesManifestAction::Bake(spec))
    }

    /// `action: createSecret` — create a Kubernetes secret.
    pub fn create_secret(spec: KubernetesManifestCreateSecret) -> Self {
        Self::new(KubernetesManifestAction::CreateSecret(spec))
    }

    /// `action: delete` — delete Kubernetes resources.
    pub fn delete(spec: KubernetesManifestDelete) -> Self {
        Self::new(KubernetesManifestAction::Delete(spec))
    }

    /// `action: patch` — patch a Kubernetes resource.
    pub fn patch(spec: KubernetesManifestPatch) -> Self {
        Self::new(KubernetesManifestAction::Patch(spec))
    }

    /// `action: promote` — promote a canary deployment to stable.
    pub fn promote(spec: KubernetesManifestPromote) -> Self {
        Self::new(KubernetesManifestAction::Promote(spec))
    }

    /// `action: scale` — scale a Kubernetes resource.
    pub fn scale(spec: KubernetesManifestScale) -> Self {
        Self::new(KubernetesManifestAction::Scale(spec))
    }

    /// `action: reject` — reject a canary deployment.
    pub fn reject(spec: KubernetesManifestReject) -> Self {
        Self::new(KubernetesManifestAction::Reject(spec))
    }

    /// `connectionType` — service connection type (default `kubernetesServiceConnection`).
    pub fn connection_type(mut self, value: ConnectionType) -> Self {
        self.connection_type = Some(value);
        self
    }

    /// `kubernetesServiceConnection` — Kubernetes service connection name.
    pub fn kubernetes_service_connection(mut self, value: impl Into<String>) -> Self {
        self.kubernetes_service_connection = Some(value.into());
        self
    }

    /// `azureSubscriptionConnection` — Azure subscription service connection name.
    pub fn azure_subscription_connection(mut self, value: impl Into<String>) -> Self {
        self.azure_subscription_connection = Some(value.into());
        self
    }

    /// `azureResourceGroup` — Azure resource group containing the AKS cluster.
    pub fn azure_resource_group(mut self, value: impl Into<String>) -> Self {
        self.azure_resource_group = Some(value.into());
        self
    }

    /// `kubernetesCluster` — AKS cluster name.
    pub fn kubernetes_cluster(mut self, value: impl Into<String>) -> Self {
        self.kubernetes_cluster = Some(value.into());
        self
    }

    /// `useClusterAdmin` — use cluster admin credentials (AzureResourceManager only).
    pub fn use_cluster_admin(mut self, value: bool) -> Self {
        self.use_cluster_admin = Some(value);
        self
    }

    /// `namespace` — Kubernetes namespace.
    pub fn namespace(mut self, value: impl Into<String>) -> Self {
        self.namespace = Some(value.into());
        self
    }

    /// Override the default per-action `displayName`.
    pub fn with_display_name(mut self, value: impl Into<String>) -> Self {
        self.display_name = Some(value.into());
        self
    }

    /// Lower into a [`TaskStep`].
    pub fn into_step(self) -> TaskStep {
        let (action_str, default_display): (&str, &str) = match &self.action {
            KubernetesManifestAction::Deploy(_) => ("deploy", "Deploy to Kubernetes"),
            KubernetesManifestAction::Bake(_) => ("bake", "Bake Kubernetes Manifests"),
            KubernetesManifestAction::CreateSecret(_) => {
                ("createSecret", "Create Kubernetes Secret")
            }
            KubernetesManifestAction::Delete(_) => ("delete", "Delete Kubernetes Resources"),
            KubernetesManifestAction::Patch(_) => ("patch", "Patch Kubernetes Resource"),
            KubernetesManifestAction::Promote(_) => ("promote", "Promote Kubernetes Canary"),
            KubernetesManifestAction::Scale(_) => ("scale", "Scale Kubernetes Resource"),
            KubernetesManifestAction::Reject(_) => ("reject", "Reject Kubernetes Canary"),
        };

        let mut t = TaskStep::new(
            "KubernetesManifest@1",
            self.display_name.unwrap_or_else(|| default_display.into()),
        )
        .with_input("action", action_str);

        // Shared connection inputs (applicable to all actions except bake).
        push_opt(
            &mut t,
            "connectionType",
            self.connection_type.map(|c| c.as_ado_str().to_string()),
        );
        push_opt(&mut t, "kubernetesServiceConnection", self.kubernetes_service_connection);
        push_opt(&mut t, "azureSubscriptionConnection", self.azure_subscription_connection);
        push_opt(&mut t, "azureResourceGroup", self.azure_resource_group);
        push_opt(&mut t, "kubernetesCluster", self.kubernetes_cluster);
        push_bool(&mut t, "useClusterAdmin", self.use_cluster_admin);
        push_opt(&mut t, "namespace", self.namespace);

        // Per-action inputs.
        match self.action {
            KubernetesManifestAction::Deploy(s) => {
                t = t.with_input("manifests", s.manifests);
                push_opt(&mut t, "strategy", s.strategy.map(|v| v.as_ado_str().to_string()));
                push_opt(
                    &mut t,
                    "trafficSplitMethod",
                    s.traffic_split_method.map(|v| v.as_ado_str().to_string()),
                );
                push_opt(&mut t, "percentage", s.percentage);
                push_opt(&mut t, "baselineAndCanaryReplicas", s.baseline_and_canary_replicas);
                push_opt(&mut t, "containers", s.containers);
                push_opt(&mut t, "imagePullSecrets", s.image_pull_secrets);
                push_opt(&mut t, "rolloutStatusTimeout", s.rollout_status_timeout);
                push_opt(&mut t, "resourceType", s.resource_type);
            }
            KubernetesManifestAction::Bake(s) => {
                push_opt(&mut t, "renderType", s.render_type.map(|v| v.as_ado_str().to_string()));
                push_opt(&mut t, "helmChart", s.helm_chart);
                push_opt(&mut t, "dockerComposeFile", s.docker_compose_file);
                push_opt(&mut t, "kustomizationPath", s.kustomization_path);
                push_opt(&mut t, "releaseName", s.release_name);
                push_opt(&mut t, "overrideFiles", s.override_files);
                push_opt(&mut t, "overrides", s.overrides);
                push_opt(&mut t, "containers", s.containers);
            }
            KubernetesManifestAction::CreateSecret(s) => {
                t = t.with_input("secretType", s.secret_type.as_ado_str());
                push_opt(&mut t, "secretName", s.secret_name);
                push_opt(&mut t, "secretArguments", s.secret_arguments);
                push_opt(&mut t, "dockerRegistryEndpoint", s.docker_registry_endpoint);
            }
            KubernetesManifestAction::Delete(s) => {
                push_opt(&mut t, "arguments", s.arguments);
            }
            KubernetesManifestAction::Patch(s) => {
                t = t
                    .with_input("resourceToPatch", s.resource_to_patch.as_ado_str())
                    .with_input("mergeStrategy", s.merge_strategy.as_ado_str())
                    .with_input("patch", s.patch);
                push_opt(&mut t, "resourceFileToPatch", s.resource_file_to_patch);
                push_opt(&mut t, "kind", s.kind.map(|v| v.as_ado_str().to_string()));
                push_opt(&mut t, "name", s.name);
                push_opt(&mut t, "rolloutStatusTimeout", s.rollout_status_timeout);
            }
            KubernetesManifestAction::Promote(s) => {
                t = t.with_input("manifests", s.manifests);
                push_opt(&mut t, "strategy", s.strategy.map(|v| v.as_ado_str().to_string()));
                push_opt(&mut t, "containers", s.containers);
                push_opt(&mut t, "imagePullSecrets", s.image_pull_secrets);
                push_opt(&mut t, "rolloutStatusTimeout", s.rollout_status_timeout);
            }
            KubernetesManifestAction::Scale(s) => {
                t = t
                    .with_input("kind", s.kind.as_ado_str())
                    .with_input("name", s.name)
                    .with_input("replicas", s.replicas);
                push_opt(&mut t, "rolloutStatusTimeout", s.rollout_status_timeout);
            }
            KubernetesManifestAction::Reject(s) => {
                t = t.with_input("manifests", s.manifests);
                push_opt(&mut t, "strategy", s.strategy.map(|v| v.as_ado_str().to_string()));
            }
        }
        t
    }
}

// ─── Advisory front-matter validation ────────────────────────────────────────

/// Validate an authored `KubernetesManifest@1` `inputs:` mapping (advisory
/// front-matter validation, see [`super::parse`]).
///
/// The task dispatches on `action` (default `deploy`). Shared connection inputs
/// (applicable to every action) are validated and removed first, then the
/// per-action inputs are validated against a `deny_unknown_fields` schema so an
/// input supplied for the wrong action is reported.
pub(crate) fn validate_inputs(inputs: Value) -> Result<(), String> {
    let mut map = match inputs {
        Value::Mapping(m) => m,
        Value::Null => Default::default(),
        other => return Err(format!("`inputs` must be a mapping, got {other:?}")),
    };

    strip_shared_inputs(&mut map)?;

    let action = match map.remove("action") {
        Some(v) => v
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| "`action` must be a string".to_string())?,
        None => "deploy".to_string(),
    };

    let rest = Value::Mapping(map);
    let result = match action.as_str() {
        "deploy" => serde_yaml::from_value::<DeploySpec>(rest).map(drop),
        "bake" => serde_yaml::from_value::<BakeSpec>(rest).map(drop),
        "createSecret" => serde_yaml::from_value::<CreateSecretSpec>(rest).map(drop),
        "delete" => serde_yaml::from_value::<DeleteSpec>(rest).map(drop),
        "patch" => serde_yaml::from_value::<PatchSpec>(rest).map(drop),
        "promote" => serde_yaml::from_value::<PromoteSpec>(rest).map(drop),
        "scale" => serde_yaml::from_value::<ScaleSpec>(rest).map(drop),
        "reject" => serde_yaml::from_value::<RejectSpec>(rest).map(drop),
        other => {
            return Err(format!(
                "unknown action `{other}` \
                 (expected deploy|bake|createSecret|delete|patch|promote|scale|reject)"
            ));
        }
    };
    result.map_err(|e| format!("action `{action}`: {e}"))
}

/// Validate and remove the connection/namespace inputs shared by every action.
fn strip_shared_inputs(map: &mut serde_yaml::Mapping) -> Result<(), String> {
    for key in [
        "kubernetesServiceConnection",
        "azureSubscriptionConnection",
        "azureResourceGroup",
        "kubernetesCluster",
        "namespace",
    ] {
        if let Some(v) = map.get(key)
            && !v.is_string()
        {
            return Err(format!("`{key}` must be a string"));
        }
        map.remove(key);
    }
    if let Some(v) = map.remove("connectionType") {
        serde_yaml::from_value::<ConnectionType>(v)
            .map(drop)
            .map_err(|e| format!("`connectionType`: {e}"))?;
    }
    if let Some(v) = map.remove("useClusterAdmin") {
        let ok = v.is_bool()
            || v
                .as_str()
                .is_some_and(|s| s.eq_ignore_ascii_case("true") || s.eq_ignore_ascii_case("false"));
        if !ok {
            return Err("`useClusterAdmin` must be a boolean".to_string());
        }
    }
    Ok(())
}

/// Inputs valid for `action = deploy`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeploySpec {
    #[serde(rename = "manifests")]
    _manifests: String,
    #[serde(rename = "strategy", default)]
    _strategy: Option<DeploymentStrategy>,
    #[serde(rename = "trafficSplitMethod", default)]
    _traffic_split_method: Option<TrafficSplitMethod>,
    #[serde(rename = "percentage", default, deserialize_with = "de_opt_str_or_int")]
    _percentage: Option<String>,
    #[serde(
        rename = "baselineAndCanaryReplicas",
        default,
        deserialize_with = "de_opt_str_or_int"
    )]
    _baseline_and_canary_replicas: Option<String>,
    #[serde(rename = "containers", default)]
    _containers: Option<String>,
    #[serde(rename = "imagePullSecrets", default)]
    _image_pull_secrets: Option<String>,
    #[serde(
        rename = "rolloutStatusTimeout",
        default,
        deserialize_with = "de_opt_str_or_int"
    )]
    _rollout_status_timeout: Option<String>,
    #[serde(rename = "resourceType", default)]
    _resource_type: Option<String>,
}

/// Inputs valid for `action = bake` (all optional).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BakeSpec {
    #[serde(rename = "renderType", default)]
    _render_type: Option<RenderType>,
    #[serde(rename = "helmChart", default)]
    _helm_chart: Option<String>,
    #[serde(rename = "dockerComposeFile", default)]
    _docker_compose_file: Option<String>,
    #[serde(rename = "kustomizationPath", default)]
    _kustomization_path: Option<String>,
    #[serde(rename = "releaseName", default)]
    _release_name: Option<String>,
    #[serde(rename = "overrideFiles", default)]
    _override_files: Option<String>,
    #[serde(rename = "overrides", default)]
    _overrides: Option<String>,
    #[serde(rename = "containers", default)]
    _containers: Option<String>,
}

/// Inputs valid for `action = createSecret`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateSecretSpec {
    #[serde(rename = "secretType")]
    _secret_type: SecretType,
    #[serde(rename = "secretName", default)]
    _secret_name: Option<String>,
    #[serde(rename = "secretArguments", default)]
    _secret_arguments: Option<String>,
    #[serde(rename = "dockerRegistryEndpoint", default)]
    _docker_registry_endpoint: Option<String>,
}

/// Inputs valid for `action = delete`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeleteSpec {
    #[serde(rename = "arguments", default)]
    _arguments: Option<String>,
}

/// Inputs valid for `action = patch`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PatchSpec {
    #[serde(rename = "resourceToPatch")]
    _resource_to_patch: PatchTarget,
    #[serde(rename = "mergeStrategy")]
    _merge_strategy: MergeStrategy,
    #[serde(rename = "patch")]
    _patch: String,
    #[serde(rename = "resourceFileToPatch", default)]
    _resource_file_to_patch: Option<String>,
    #[serde(rename = "kind", default)]
    _kind: Option<ResourceKind>,
    #[serde(rename = "name", default)]
    _name: Option<String>,
    #[serde(
        rename = "rolloutStatusTimeout",
        default,
        deserialize_with = "de_opt_str_or_int"
    )]
    _rollout_status_timeout: Option<String>,
}

/// Inputs valid for `action = promote`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PromoteSpec {
    #[serde(rename = "manifests")]
    _manifests: String,
    #[serde(rename = "strategy", default)]
    _strategy: Option<DeploymentStrategy>,
    #[serde(rename = "containers", default)]
    _containers: Option<String>,
    #[serde(rename = "imagePullSecrets", default)]
    _image_pull_secrets: Option<String>,
    #[serde(
        rename = "rolloutStatusTimeout",
        default,
        deserialize_with = "de_opt_str_or_int"
    )]
    _rollout_status_timeout: Option<String>,
}

/// Inputs valid for `action = scale`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ScaleSpec {
    #[serde(rename = "kind")]
    _kind: ResourceKind,
    #[serde(rename = "name")]
    _name: String,
    #[serde(rename = "replicas", deserialize_with = "de_str_or_int")]
    _replicas: String,
    #[serde(
        rename = "rolloutStatusTimeout",
        default,
        deserialize_with = "de_opt_str_or_int"
    )]
    _rollout_status_timeout: Option<String>,
}

/// Inputs valid for `action = reject`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RejectSpec {
    #[serde(rename = "manifests")]
    _manifests: String,
    #[serde(rename = "strategy", default)]
    _strategy: Option<DeploymentStrategy>,
}

/// Deserialize a required ADO input that may be authored as a string or an
/// integer (e.g. `replicas: 3` or `replicas: "3"`) into a `String`.
fn de_str_or_int<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    match de_opt_str_or_int(deserializer)? {
        Some(s) => Ok(s),
        None => Err(serde::de::Error::custom("expected a string or integer")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deploy_sets_action_and_manifests() {
        let t = KubernetesManifest::deploy(KubernetesManifestDeploy::new("k8s/deployment.yaml"))
            .kubernetes_service_connection("myCluster")
            .namespace("production")
            .into_step();
        assert_eq!(t.task, "KubernetesManifest@1");
        assert_eq!(t.display_name, "Deploy to Kubernetes");
        assert_eq!(t.inputs.get("action").map(String::as_str), Some("deploy"));
        assert_eq!(
            t.inputs.get("manifests").map(String::as_str),
            Some("k8s/deployment.yaml")
        );
        assert_eq!(
            t.inputs.get("kubernetesServiceConnection").map(String::as_str),
            Some("myCluster")
        );
        assert_eq!(t.inputs.get("namespace").map(String::as_str), Some("production"));
    }

    #[test]
    fn deploy_with_canary_strategy() {
        let t = KubernetesManifest::deploy(
            KubernetesManifestDeploy::new("k8s/*.yaml")
                .strategy(DeploymentStrategy::Canary)
                .percentage("25")
                .containers("myregistry.azurecr.io/myapp:latest"),
        )
        .into_step();
        assert_eq!(t.inputs.get("strategy").map(String::as_str), Some("canary"));
        assert_eq!(t.inputs.get("percentage").map(String::as_str), Some("25"));
        assert_eq!(
            t.inputs.get("containers").map(String::as_str),
            Some("myregistry.azurecr.io/myapp:latest")
        );
        // No traffic split method set → absent.
        assert!(t.inputs.get("trafficSplitMethod").is_none());
    }

    #[test]
    fn bake_helm_chart() {
        let t = KubernetesManifest::bake(
            KubernetesManifestBake::new()
                .render_type(RenderType::Helm)
                .helm_chart("./charts/myapp")
                .release_name("myapp-release"),
        )
        .into_step();
        assert_eq!(t.inputs.get("action").map(String::as_str), Some("bake"));
        assert_eq!(t.inputs.get("renderType").map(String::as_str), Some("helm"));
        assert_eq!(t.inputs.get("helmChart").map(String::as_str), Some("./charts/myapp"));
        assert_eq!(t.inputs.get("releaseName").map(String::as_str), Some("myapp-release"));
        // Bake has no connection inputs.
        assert!(t.inputs.get("kubernetesServiceConnection").is_none());
    }

    #[test]
    fn bake_kustomize() {
        let t = KubernetesManifest::bake(
            KubernetesManifestBake::new()
                .render_type(RenderType::Kustomize)
                .kustomization_path("./overlays/production"),
        )
        .into_step();
        assert_eq!(t.inputs.get("renderType").map(String::as_str), Some("kustomize"));
        assert_eq!(
            t.inputs.get("kustomizationPath").map(String::as_str),
            Some("./overlays/production")
        );
    }

    #[test]
    fn create_docker_registry_secret() {
        let t = KubernetesManifest::create_secret(
            KubernetesManifestCreateSecret::new(SecretType::DockerRegistry)
                .secret_name("myregistrysecret")
                .docker_registry_endpoint("myRegistry"),
        )
        .namespace("default")
        .into_step();
        assert_eq!(t.inputs.get("action").map(String::as_str), Some("createSecret"));
        assert_eq!(t.inputs.get("secretType").map(String::as_str), Some("dockerRegistry"));
        assert_eq!(
            t.inputs.get("secretName").map(String::as_str),
            Some("myregistrysecret")
        );
        assert_eq!(
            t.inputs.get("dockerRegistryEndpoint").map(String::as_str),
            Some("myRegistry")
        );
        assert_eq!(t.inputs.get("namespace").map(String::as_str), Some("default"));
    }

    #[test]
    fn create_generic_secret() {
        let t = KubernetesManifest::create_secret(
            KubernetesManifestCreateSecret::new(SecretType::Generic)
                .secret_name("my-secret")
                .secret_arguments("--from-literal=key=value"),
        )
        .into_step();
        assert_eq!(t.inputs.get("secretType").map(String::as_str), Some("generic"));
        assert_eq!(
            t.inputs.get("secretArguments").map(String::as_str),
            Some("--from-literal=key=value")
        );
    }

    #[test]
    fn scale_deployment() {
        let t = KubernetesManifest::scale(KubernetesManifestScale::new(
            ResourceKind::Deployment,
            "myapp",
            "3",
        ))
        .kubernetes_service_connection("myCluster")
        .into_step();
        assert_eq!(t.inputs.get("action").map(String::as_str), Some("scale"));
        assert_eq!(t.inputs.get("kind").map(String::as_str), Some("deployment"));
        assert_eq!(t.inputs.get("name").map(String::as_str), Some("myapp"));
        assert_eq!(t.inputs.get("replicas").map(String::as_str), Some("3"));
        assert_eq!(
            t.inputs.get("kubernetesServiceConnection").map(String::as_str),
            Some("myCluster")
        );
    }

    #[test]
    fn patch_by_file() {
        let t = KubernetesManifest::patch(KubernetesManifestPatch::new(
            PatchTarget::File,
            MergeStrategy::Strategic,
            r#"{"spec":{"replicas":2}}"#,
        ))
        .into_step();
        assert_eq!(t.inputs.get("action").map(String::as_str), Some("patch"));
        assert_eq!(t.inputs.get("resourceToPatch").map(String::as_str), Some("file"));
        assert_eq!(t.inputs.get("mergeStrategy").map(String::as_str), Some("strategic"));
        assert_eq!(
            t.inputs.get("patch").map(String::as_str),
            Some(r#"{"spec":{"replicas":2}}"#)
        );
    }

    #[test]
    fn delete_with_arguments() {
        let t = KubernetesManifest::delete(
            KubernetesManifestDelete::new().arguments("deployment/myapp"),
        )
        .namespace("staging")
        .into_step();
        assert_eq!(t.inputs.get("action").map(String::as_str), Some("delete"));
        assert_eq!(
            t.inputs.get("arguments").map(String::as_str),
            Some("deployment/myapp")
        );
        assert_eq!(t.inputs.get("namespace").map(String::as_str), Some("staging"));
    }

    #[test]
    fn promote_canary() {
        let t = KubernetesManifest::promote(
            KubernetesManifestPromote::new("k8s/deployment.yaml")
                .strategy(DeploymentStrategy::Canary),
        )
        .into_step();
        assert_eq!(t.inputs.get("action").map(String::as_str), Some("promote"));
        assert_eq!(t.inputs.get("manifests").map(String::as_str), Some("k8s/deployment.yaml"));
        assert_eq!(t.inputs.get("strategy").map(String::as_str), Some("canary"));
    }

    #[test]
    fn reject_canary() {
        let t = KubernetesManifest::reject(
            KubernetesManifestReject::new("k8s/*.yaml").strategy(DeploymentStrategy::Canary),
        )
        .into_step();
        assert_eq!(t.inputs.get("action").map(String::as_str), Some("reject"));
        assert_eq!(t.inputs.get("manifests").map(String::as_str), Some("k8s/*.yaml"));
        assert_eq!(t.inputs.get("strategy").map(String::as_str), Some("canary"));
    }

    #[test]
    fn azure_resource_manager_connection() {
        let t = KubernetesManifest::deploy(KubernetesManifestDeploy::new("*.yaml"))
            .connection_type(ConnectionType::AzureResourceManager)
            .azure_subscription_connection("mySubscription")
            .azure_resource_group("myRG")
            .kubernetes_cluster("myAKSCluster")
            .use_cluster_admin(true)
            .into_step();
        assert_eq!(
            t.inputs.get("connectionType").map(String::as_str),
            Some("azureResourceManager")
        );
        assert_eq!(
            t.inputs.get("azureSubscriptionConnection").map(String::as_str),
            Some("mySubscription")
        );
        assert_eq!(t.inputs.get("azureResourceGroup").map(String::as_str), Some("myRG"));
        assert_eq!(
            t.inputs.get("kubernetesCluster").map(String::as_str),
            Some("myAKSCluster")
        );
        assert_eq!(t.inputs.get("useClusterAdmin").map(String::as_str), Some("true"));
    }

    #[test]
    fn display_name_override() {
        let t = KubernetesManifest::deploy(KubernetesManifestDeploy::new("*.yaml"))
            .with_display_name("Deploy to staging cluster")
            .into_step();
        assert_eq!(t.display_name, "Deploy to staging cluster");
    }

    #[test]
    fn optionals_absent_by_default() {
        let t =
            KubernetesManifest::deploy(KubernetesManifestDeploy::new("k8s/deploy.yaml")).into_step();
        assert!(t.inputs.get("strategy").is_none());
        assert!(t.inputs.get("namespace").is_none());
        assert!(t.inputs.get("connectionType").is_none());
        assert!(t.inputs.get("containers").is_none());
        assert!(t.inputs.get("imagePullSecrets").is_none());
    }
}
