//! Typed builder for `CargoAuthenticate@0`.

use crate::compile::ir::step::TaskStep;

/// Builder for a [`TaskStep`] invoking `CargoAuthenticate@0`.
///
/// Configures Cargo (the Rust package manager) to authenticate with Azure
/// Artifacts feeds and other Cargo registries declared in a `config.toml`
/// file. The `configFile` input is required; all other inputs are optional.
///
/// For registries inside the organization/collection, the build service
/// identity is used automatically. For external registries, provide their
/// credentials via [`cargo_service_connections`](CargoAuthenticate::cargo_service_connections).
///
/// ADO task reference:
/// <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/cargo-authenticate-v0>
#[derive(Debug, Clone)]
pub struct CargoAuthenticate {
    config_file: String,
    cargo_service_connections: Option<String>,
    registry_names: Option<String>,
    azure_devops_service_connection: Option<String>,
    display_name: Option<String>,
}

impl CargoAuthenticate {
    /// Create a new builder.
    ///
    /// `config_file` — path to the `config.toml` file that specifies the
    /// registries to authenticate (e.g. `".cargo/config.toml"`).
    pub fn new(config_file: impl Into<String>) -> Self {
        Self {
            config_file: config_file.into(),
            cargo_service_connections: None,
            registry_names: None,
            azure_devops_service_connection: None,
            display_name: None,
        }
    }

    /// `cargoServiceConnections` — comma-separated service connection names
    /// for registries outside this organization/collection.
    pub fn cargo_service_connections(mut self, value: impl Into<String>) -> Self {
        self.cargo_service_connections = Some(value.into());
        self
    }

    /// `registryNames` — comma-separated registry names from `config.toml` to
    /// authenticate. When set, `azureDevOpsServiceConnection` is required.
    /// Not compatible with [`cargo_service_connections`](Self::cargo_service_connections).
    pub fn registry_names(mut self, value: impl Into<String>) -> Self {
        self.registry_names = Some(value.into());
        self
    }

    /// `azureDevOpsServiceConnection` (alias: `workloadIdentityServiceConnection`) —
    /// service connection for authenticating against this Azure DevOps
    /// organization's feeds via workload-identity federation.
    pub fn azure_devops_service_connection(mut self, value: impl Into<String>) -> Self {
        self.azure_devops_service_connection = Some(value.into());
        self
    }

    /// Override the default `displayName` (`"Cargo Authenticate"`).
    pub fn with_display_name(mut self, value: impl Into<String>) -> Self {
        self.display_name = Some(value.into());
        self
    }

    /// Lower into a [`TaskStep`].
    pub fn into_step(self) -> TaskStep {
        let mut t = TaskStep::new(
            "CargoAuthenticate@0",
            self.display_name
                .unwrap_or_else(|| "Cargo Authenticate".into()),
        )
        .with_input("configFile", self.config_file);
        if let Some(v) = self.cargo_service_connections {
            t = t.with_input("cargoServiceConnections", v);
        }
        if let Some(v) = self.registry_names {
            t = t.with_input("registryNames", v);
        }
        if let Some(v) = self.azure_devops_service_connection {
            t = t.with_input("azureDevOpsServiceConnection", v);
        }
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sets_task_identifier() {
        let t = CargoAuthenticate::new(".cargo/config.toml").into_step();
        assert_eq!(t.task, "CargoAuthenticate@0");
    }

    #[test]
    fn config_file_is_always_emitted() {
        let t = CargoAuthenticate::new(".cargo/config.toml").into_step();
        assert_eq!(
            t.inputs.get("configFile").map(String::as_str),
            Some(".cargo/config.toml")
        );
    }

    #[test]
    fn default_display_name() {
        let t = CargoAuthenticate::new(".cargo/config.toml").into_step();
        assert_eq!(t.display_name, "Cargo Authenticate");
    }

    #[test]
    fn no_optional_inputs_by_default() {
        let t = CargoAuthenticate::new(".cargo/config.toml").into_step();
        assert_eq!(t.inputs.len(), 1, "only configFile should be set");
    }

    #[test]
    fn cargo_service_connections() {
        let t = CargoAuthenticate::new(".cargo/config.toml")
            .cargo_service_connections("my-crates-io-conn")
            .into_step();
        assert_eq!(
            t.inputs
                .get("cargoServiceConnections")
                .map(String::as_str),
            Some("my-crates-io-conn")
        );
    }

    #[test]
    fn registry_names() {
        let t = CargoAuthenticate::new(".cargo/config.toml")
            .registry_names("internal-feed,public-feed")
            .into_step();
        assert_eq!(
            t.inputs.get("registryNames").map(String::as_str),
            Some("internal-feed,public-feed")
        );
    }

    #[test]
    fn azure_devops_service_connection() {
        let t = CargoAuthenticate::new(".cargo/config.toml")
            .azure_devops_service_connection("my-ado-wif-conn")
            .into_step();
        assert_eq!(
            t.inputs
                .get("azureDevOpsServiceConnection")
                .map(String::as_str),
            Some("my-ado-wif-conn")
        );
    }

    #[test]
    fn all_inputs_together() {
        let t = CargoAuthenticate::new(".cargo/config.toml")
            .cargo_service_connections("external-conn")
            .registry_names("my-registry")
            .azure_devops_service_connection("wif-conn")
            .with_display_name("Auth Cargo feeds")
            .into_step();
        assert_eq!(t.task, "CargoAuthenticate@0");
        assert_eq!(t.display_name, "Auth Cargo feeds");
        assert_eq!(
            t.inputs.get("configFile").map(String::as_str),
            Some(".cargo/config.toml")
        );
        assert_eq!(
            t.inputs
                .get("cargoServiceConnections")
                .map(String::as_str),
            Some("external-conn")
        );
        assert_eq!(
            t.inputs.get("registryNames").map(String::as_str),
            Some("my-registry")
        );
        assert_eq!(
            t.inputs
                .get("azureDevOpsServiceConnection")
                .map(String::as_str),
            Some("wif-conn")
        );
    }

    #[test]
    fn with_display_name_override() {
        let t = CargoAuthenticate::new(".cargo/config.toml")
            .with_display_name("Authenticate to internal Cargo registry")
            .into_step();
        assert_eq!(t.display_name, "Authenticate to internal Cargo registry");
    }
}
