//! Typed builder for `NuGetAuthenticate@1`.

use super::common::{bool_input, de_opt_bool_flex};
use crate::compile::ir::step::TaskStep;
use serde::Deserialize;

/// Builder for a [`TaskStep`] invoking `NuGetAuthenticate@1`.
///
/// Configures NuGet tooling (nuget.exe, dotnet, MSBuild) to authenticate with
/// Azure Artifacts feeds and other NuGet repositories. All inputs are optional;
/// calling `into_step()` with no inputs set authenticates the ADO build service
/// identity against any feeds discovered in `nuget.config` files in the
/// workspace.
///
/// ADO task reference:
/// <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/nuget-authenticate-v1>
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NuGetAuthenticate {
    #[serde(rename = "nuGetServiceConnections", default)]
    nuget_service_connections: Option<String>,
    #[serde(
        rename = "forceReinstallCredentialProvider",
        default,
        deserialize_with = "de_opt_bool_flex"
    )]
    force_reinstall_credential_provider: Option<bool>,
    #[serde(rename = "workloadIdentityServiceConnection", default)]
    workload_identity_service_connection: Option<String>,
    #[serde(rename = "feedUrl", default)]
    feed_url: Option<String>,
    #[serde(skip)]
    display_name: Option<String>,
}

impl NuGetAuthenticate {
    /// Create a new builder; all inputs are optional.
    pub fn new() -> Self {
        Self {
            nuget_service_connections: None,
            force_reinstall_credential_provider: None,
            workload_identity_service_connection: None,
            feed_url: None,
            display_name: None,
        }
    }

    /// `nuGetServiceConnections` — comma-separated service connections for
    /// feeds outside this organization.
    pub fn nuget_service_connections(mut self, value: impl Into<String>) -> Self {
        self.nuget_service_connections = Some(value.into());
        self
    }

    /// `forceReinstallCredentialProvider` — reinstall the credential provider
    /// even if already installed. Default: `false`.
    pub fn force_reinstall_credential_provider(mut self, value: bool) -> Self {
        self.force_reinstall_credential_provider = Some(value);
        self
    }

    /// `workloadIdentityServiceConnection` (alias `azureDevOpsServiceConnection`) —
    /// service connection for authenticating against this Azure DevOps
    /// organization's feeds via workload-identity federation.
    pub fn workload_identity_service_connection(mut self, value: impl Into<String>) -> Self {
        self.workload_identity_service_connection = Some(value.into());
        self
    }

    /// `feedUrl` — Azure Artifacts feed URL to authenticate against.
    /// Used together with [`workload_identity_service_connection`].
    pub fn feed_url(mut self, value: impl Into<String>) -> Self {
        self.feed_url = Some(value.into());
        self
    }

    /// Override the default `displayName` (`"NuGet Authenticate"`).
    pub fn with_display_name(mut self, value: impl Into<String>) -> Self {
        self.display_name = Some(value.into());
        self
    }

    /// Lower into a [`TaskStep`].
    pub fn into_step(self) -> TaskStep {
        let mut t = TaskStep::new(
            "NuGetAuthenticate@1",
            self.display_name
                .unwrap_or_else(|| "NuGet Authenticate".into()),
        );
        if let Some(v) = self.nuget_service_connections {
            t = t.with_input("nuGetServiceConnections", v);
        }
        if let Some(v) = self.force_reinstall_credential_provider {
            t = t.with_input("forceReinstallCredentialProvider", bool_input(v));
        }
        if let Some(v) = self.workload_identity_service_connection {
            t = t.with_input("workloadIdentityServiceConnection", v);
        }
        if let Some(v) = self.feed_url {
            t = t.with_input("feedUrl", v);
        }
        t
    }
}

impl Default for NuGetAuthenticate {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sets_task_identifier() {
        let t = NuGetAuthenticate::new().into_step();
        assert_eq!(t.task, "NuGetAuthenticate@1");
    }

    #[test]
    fn no_inputs_by_default() {
        let t = NuGetAuthenticate::new().into_step();
        assert!(t.inputs.is_empty(), "expected no inputs when none are set");
    }

    #[test]
    fn default_display_name() {
        let t = NuGetAuthenticate::new().into_step();
        assert_eq!(t.display_name, "NuGet Authenticate");
    }

    #[test]
    fn nuget_service_connections() {
        let t = NuGetAuthenticate::new()
            .nuget_service_connections("my-service-connection")
            .into_step();
        assert_eq!(
            t.inputs.get("nuGetServiceConnections").map(String::as_str),
            Some("my-service-connection")
        );
    }

    #[test]
    fn force_reinstall_credential_provider() {
        let t = NuGetAuthenticate::new()
            .force_reinstall_credential_provider(true)
            .into_step();
        assert_eq!(
            t.inputs
                .get("forceReinstallCredentialProvider")
                .map(String::as_str),
            Some("true")
        );
    }

    #[test]
    fn workload_identity_and_feed_url() {
        let t = NuGetAuthenticate::new()
            .workload_identity_service_connection("my-wif-conn")
            .feed_url("https://pkgs.dev.azure.com/myorg/_packaging/myfeed/nuget/v3/index.json")
            .into_step();
        assert_eq!(
            t.inputs
                .get("workloadIdentityServiceConnection")
                .map(String::as_str),
            Some("my-wif-conn")
        );
        assert_eq!(
            t.inputs.get("feedUrl").map(String::as_str),
            Some("https://pkgs.dev.azure.com/myorg/_packaging/myfeed/nuget/v3/index.json")
        );
    }

    #[test]
    fn with_display_name_override() {
        let t = NuGetAuthenticate::new()
            .with_display_name("Authenticate to internal feed")
            .into_step();
        assert_eq!(t.display_name, "Authenticate to internal feed");
    }
}
