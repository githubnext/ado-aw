//! Typed builder for `npmAuthenticate@0`.

use crate::compile::ir::step::TaskStep;
use serde::Deserialize;

/// Builder for a [`TaskStep`] invoking `npmAuthenticate@0`.
///
/// Searches the specified `.npmrc` file for registry entries, then appends
/// authentication details for the discovered registries to the end of the
/// file. For registries in the current organization/collection, the build's
/// credentials are used automatically. For registries in a different
/// organization or hosted by a third party, the registry URIs are compared
/// against the URIs of npm service connections supplied via
/// [`custom_endpoint`](Self::custom_endpoint).
///
/// The `.npmrc` file is reverted to its original state at the end of pipeline
/// execution.
///
/// ADO task reference:
/// <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/npm-authenticate-v0>
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NpmAuthenticate {
    #[serde(rename = "workingFile")]
    working_file: String,
    #[serde(rename = "customEndpoint", default)]
    custom_endpoint: Option<String>,
    #[serde(rename = "azureDevOpsServiceConnection", default)]
    azure_devops_service_connection: Option<String>,
    #[serde(rename = "feedUrl", default)]
    feed_url: Option<String>,
    #[serde(skip)]
    display_name: Option<String>,
}

impl NpmAuthenticate {
    /// Create a new builder.
    ///
    /// `working_file` — path to the `.npmrc` file to authenticate
    /// (e.g. `".npmrc"` or `"$(Agent.TempDirectory)/.npmrc"`). The file must
    /// exist before this step runs; use a preceding bash step to create it if
    /// the repository does not already contain one.
    pub fn new(working_file: impl Into<String>) -> Self {
        Self {
            working_file: working_file.into(),
            custom_endpoint: None,
            azure_devops_service_connection: None,
            feed_url: None,
            display_name: None,
        }
    }

    /// `customEndpoint` — comma-separated names of npm service connections
    /// configured for registries outside this organization/collection.
    /// Registry URIs in the `.npmrc` file are matched against the service
    /// connection URIs, and the corresponding credentials are used.
    pub fn custom_endpoint(mut self, value: impl Into<String>) -> Self {
        self.custom_endpoint = Some(value.into());
        self
    }

    /// `azureDevOpsServiceConnection` (alias: `workloadIdentityServiceConnection`) —
    /// workload-identity service connection for authenticating against this
    /// Azure DevOps organization's Azure Artifacts feeds. When set,
    /// [`feed_url`](Self::feed_url) is required. Not compatible with
    /// [`custom_endpoint`](Self::custom_endpoint).
    pub fn azure_devops_service_connection(mut self, value: impl Into<String>) -> Self {
        self.azure_devops_service_connection = Some(value.into());
        self
    }

    /// `feedUrl` — Azure Artifacts feed URL in npm registry format:
    /// `https://pkgs.dev.azure.com/{ORG}/{PROJECT}/_packaging/{FEED}/npm/registry/`.
    /// Required when
    /// [`azure_devops_service_connection`](Self::azure_devops_service_connection)
    /// is set. Not compatible with [`custom_endpoint`](Self::custom_endpoint).
    pub fn feed_url(mut self, value: impl Into<String>) -> Self {
        self.feed_url = Some(value.into());
        self
    }

    /// Override the default `displayName` (`"npm Authenticate"`).
    pub fn with_display_name(mut self, value: impl Into<String>) -> Self {
        self.display_name = Some(value.into());
        self
    }

    /// Lower into a [`TaskStep`].
    pub fn into_step(self) -> TaskStep {
        let mut t = TaskStep::new(
            "npmAuthenticate@0",
            self.display_name
                .unwrap_or_else(|| "npm Authenticate".into()),
        )
        .with_input("workingFile", self.working_file);
        if let Some(v) = self.custom_endpoint {
            t = t.with_input("customEndpoint", v);
        }
        if let Some(v) = self.azure_devops_service_connection {
            t = t.with_input("azureDevOpsServiceConnection", v);
        }
        if let Some(v) = self.feed_url {
            t = t.with_input("feedUrl", v);
        }
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sets_task_identifier() {
        let t = NpmAuthenticate::new(".npmrc").into_step();
        assert_eq!(t.task, "npmAuthenticate@0");
    }

    #[test]
    fn working_file_is_always_emitted() {
        let t = NpmAuthenticate::new(".npmrc").into_step();
        assert_eq!(
            t.inputs.get("workingFile").map(String::as_str),
            Some(".npmrc")
        );
    }

    #[test]
    fn default_display_name() {
        let t = NpmAuthenticate::new(".npmrc").into_step();
        assert_eq!(t.display_name, "npm Authenticate");
    }

    #[test]
    fn no_optional_inputs_by_default() {
        let t = NpmAuthenticate::new(".npmrc").into_step();
        assert_eq!(t.inputs.len(), 1, "only workingFile should be set");
    }

    #[test]
    fn custom_endpoint() {
        let t = NpmAuthenticate::new(".npmrc")
            .custom_endpoint("OtherOrgNpmConn,ThirdPartyConn")
            .into_step();
        assert_eq!(
            t.inputs.get("customEndpoint").map(String::as_str),
            Some("OtherOrgNpmConn,ThirdPartyConn")
        );
    }

    #[test]
    fn azure_devops_service_connection_mode() {
        let t = NpmAuthenticate::new(".npmrc")
            .azure_devops_service_connection("my-wif-conn")
            .feed_url("https://pkgs.dev.azure.com/myorg/_packaging/myfeed/npm/registry/")
            .into_step();
        assert_eq!(
            t.inputs
                .get("azureDevOpsServiceConnection")
                .map(String::as_str),
            Some("my-wif-conn")
        );
        assert_eq!(
            t.inputs.get("feedUrl").map(String::as_str),
            Some("https://pkgs.dev.azure.com/myorg/_packaging/myfeed/npm/registry/")
        );
    }

    #[test]
    fn with_display_name_override() {
        let t = NpmAuthenticate::new(".npmrc")
            .with_display_name("Authenticate npm (build service identity)")
            .into_step();
        assert_eq!(t.display_name, "Authenticate npm (build service identity)");
    }

    #[test]
    fn temp_directory_working_file() {
        let t = NpmAuthenticate::new("$(Agent.TempDirectory)/.npmrc").into_step();
        assert_eq!(
            t.inputs.get("workingFile").map(String::as_str),
            Some("$(Agent.TempDirectory)/.npmrc")
        );
    }

    #[test]
    fn all_optional_inputs_together() {
        let t = NpmAuthenticate::new(".npmrc")
            .custom_endpoint("external-npm-conn")
            .azure_devops_service_connection("wif-conn")
            .feed_url("https://pkgs.dev.azure.com/myorg/_packaging/myfeed/npm/registry/")
            .with_display_name("Auth npm feeds")
            .into_step();
        assert_eq!(t.task, "npmAuthenticate@0");
        assert_eq!(t.display_name, "Auth npm feeds");
        assert_eq!(
            t.inputs.get("workingFile").map(String::as_str),
            Some(".npmrc")
        );
        assert_eq!(
            t.inputs.get("customEndpoint").map(String::as_str),
            Some("external-npm-conn")
        );
        assert_eq!(
            t.inputs
                .get("azureDevOpsServiceConnection")
                .map(String::as_str),
            Some("wif-conn")
        );
        assert_eq!(
            t.inputs.get("feedUrl").map(String::as_str),
            Some("https://pkgs.dev.azure.com/myorg/_packaging/myfeed/npm/registry/")
        );
    }
}
