//! Typed builder for `PipAuthenticate@1`.

use super::common::bool_input;
use crate::compile::ir::step::TaskStep;

/// Builder for a [`TaskStep`] invoking `PipAuthenticate@1`.
///
/// Configures pip to authenticate with Azure Artifacts feeds and external pip
/// repositories. All inputs are optional; calling `into_step()` with no inputs
/// set (or only [`artifact_feeds`](Self::artifact_feeds)) authenticates the ADO
/// build service identity against the specified internal feeds.
///
/// Two modes are available (mutually exclusive per task docs):
///
/// 1. **Service-connection mode**: set both
///    [`azure_devops_service_connection`](Self::azure_devops_service_connection)
///    and [`feed_url`](Self::feed_url) together. All other inputs are ignored.
/// 2. **Feed-list mode**: set [`artifact_feeds`](Self::artifact_feeds) and/or
///    [`python_download_service_connections`](Self::python_download_service_connections).
///
/// ADO task reference:
/// <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/pip-authenticate-v1>
#[derive(Debug, Clone)]
pub struct PipAuthenticate {
    azure_devops_service_connection: Option<String>,
    feed_url: Option<String>,
    artifact_feeds: Option<String>,
    python_download_service_connections: Option<String>,
    only_add_extra_index: Option<bool>,
    display_name: Option<String>,
}

impl PipAuthenticate {
    /// Create a new builder; all inputs are optional.
    pub fn new() -> Self {
        Self {
            azure_devops_service_connection: None,
            feed_url: None,
            artifact_feeds: None,
            python_download_service_connections: None,
            only_add_extra_index: None,
            display_name: None,
        }
    }

    /// `azureDevOpsServiceConnection` (alias: `workloadIdentityServiceConnection`) —
    /// service connection for authenticating against this organization's feeds
    /// via workload-identity federation. When set, [`feed_url`](Self::feed_url)
    /// is required and all other inputs are ignored.
    pub fn azure_devops_service_connection(mut self, value: impl Into<String>) -> Self {
        self.azure_devops_service_connection = Some(value.into());
        self
    }

    /// `feedUrl` — Azure Artifacts feed URL to authenticate against.
    /// Required when [`azure_devops_service_connection`](Self::azure_devops_service_connection)
    /// is set.
    pub fn feed_url(mut self, value: impl Into<String>) -> Self {
        self.feed_url = Some(value.into());
        self
    }

    /// `artifactFeeds` — comma-separated list of Azure Artifacts feeds (by
    /// name or URL) to authenticate with pip. Use an empty string to
    /// authenticate against all feeds accessible by the build service identity.
    pub fn artifact_feeds(mut self, value: impl Into<String>) -> Self {
        self.artifact_feeds = Some(value.into());
        self
    }

    /// `pythonDownloadServiceConnections` — comma-separated list of pip service
    /// connection names from external organizations to authenticate with pip.
    pub fn python_download_service_connections(mut self, value: impl Into<String>) -> Self {
        self.python_download_service_connections = Some(value.into());
        self
    }

    /// `onlyAddExtraIndex` — when `true`, no feed is set as the primary index
    /// URL; all configured feeds/endpoints are set as extra index URLs only.
    /// Default: `false`.
    pub fn only_add_extra_index(mut self, value: bool) -> Self {
        self.only_add_extra_index = Some(value);
        self
    }

    /// Override the default `displayName` (`"Pip Authenticate"`).
    pub fn with_display_name(mut self, value: impl Into<String>) -> Self {
        self.display_name = Some(value.into());
        self
    }

    /// Lower into a [`TaskStep`].
    pub fn into_step(self) -> TaskStep {
        let mut t = TaskStep::new(
            "PipAuthenticate@1",
            self.display_name
                .unwrap_or_else(|| "Pip Authenticate".into()),
        );
        if let Some(v) = self.azure_devops_service_connection {
            t = t.with_input("azureDevOpsServiceConnection", v);
        }
        if let Some(v) = self.feed_url {
            t = t.with_input("feedUrl", v);
        }
        if let Some(v) = self.artifact_feeds {
            t = t.with_input("artifactFeeds", v);
        }
        if let Some(v) = self.python_download_service_connections {
            t = t.with_input("pythonDownloadServiceConnections", v);
        }
        if let Some(v) = self.only_add_extra_index {
            t = t.with_input("onlyAddExtraIndex", bool_input(v));
        }
        t
    }
}

impl Default for PipAuthenticate {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sets_task_identifier() {
        let t = PipAuthenticate::new().into_step();
        assert_eq!(t.task, "PipAuthenticate@1");
    }

    #[test]
    fn no_inputs_by_default() {
        let t = PipAuthenticate::new().into_step();
        assert!(t.inputs.is_empty(), "expected no inputs when none are set");
    }

    #[test]
    fn default_display_name() {
        let t = PipAuthenticate::new().into_step();
        assert_eq!(t.display_name, "Pip Authenticate");
    }

    #[test]
    fn artifact_feeds_empty_string() {
        let t = PipAuthenticate::new().artifact_feeds("").into_step();
        assert_eq!(
            t.inputs.get("artifactFeeds").map(String::as_str),
            Some("")
        );
    }

    #[test]
    fn artifact_feeds_named() {
        let t = PipAuthenticate::new()
            .artifact_feeds("my-feed,other-feed")
            .into_step();
        assert_eq!(
            t.inputs.get("artifactFeeds").map(String::as_str),
            Some("my-feed,other-feed")
        );
    }

    #[test]
    fn python_download_service_connections() {
        let t = PipAuthenticate::new()
            .python_download_service_connections("external-pypi-conn")
            .into_step();
        assert_eq!(
            t.inputs
                .get("pythonDownloadServiceConnections")
                .map(String::as_str),
            Some("external-pypi-conn")
        );
    }

    #[test]
    fn service_connection_mode() {
        let t = PipAuthenticate::new()
            .azure_devops_service_connection("my-wif-conn")
            .feed_url("https://pkgs.dev.azure.com/myorg/_packaging/myfeed/pypi/simple/")
            .into_step();
        assert_eq!(
            t.inputs
                .get("azureDevOpsServiceConnection")
                .map(String::as_str),
            Some("my-wif-conn")
        );
        assert_eq!(
            t.inputs.get("feedUrl").map(String::as_str),
            Some("https://pkgs.dev.azure.com/myorg/_packaging/myfeed/pypi/simple/")
        );
    }

    #[test]
    fn only_add_extra_index_true() {
        let t = PipAuthenticate::new()
            .only_add_extra_index(true)
            .into_step();
        assert_eq!(
            t.inputs.get("onlyAddExtraIndex").map(String::as_str),
            Some("true")
        );
    }

    #[test]
    fn only_add_extra_index_false_is_emitted() {
        let t = PipAuthenticate::new()
            .only_add_extra_index(false)
            .into_step();
        assert_eq!(
            t.inputs.get("onlyAddExtraIndex").map(String::as_str),
            Some("false")
        );
    }

    #[test]
    fn with_display_name_override() {
        let t = PipAuthenticate::new()
            .with_display_name("Authenticate pip (build service identity)")
            .into_step();
        assert_eq!(
            t.display_name,
            "Authenticate pip (build service identity)"
        );
    }

    #[test]
    fn all_inputs_together() {
        let t = PipAuthenticate::new()
            .artifact_feeds("internal-feed")
            .python_download_service_connections("external-conn")
            .only_add_extra_index(true)
            .with_display_name("Auth pip feeds")
            .into_step();
        assert_eq!(t.task, "PipAuthenticate@1");
        assert_eq!(t.display_name, "Auth pip feeds");
        assert_eq!(
            t.inputs.get("artifactFeeds").map(String::as_str),
            Some("internal-feed")
        );
        assert_eq!(
            t.inputs
                .get("pythonDownloadServiceConnections")
                .map(String::as_str),
            Some("external-conn")
        );
        assert_eq!(
            t.inputs.get("onlyAddExtraIndex").map(String::as_str),
            Some("true")
        );
    }
}
