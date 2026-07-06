//! Typed builder for `MavenAuthenticate@0`.

use crate::compile::ir::step::TaskStep;
use serde::Deserialize;

/// Builder for a [`TaskStep`] invoking `MavenAuthenticate@0`.
///
/// Provides credentials for Azure Artifacts feeds and external Maven
/// repositories by writing authenticated server entries into the current
/// user's `settings.xml` file. All inputs are optional; calling
/// `into_step()` with no inputs set is valid and lets the task authenticate
/// the ADO build service identity with any feeds it discovers.
///
/// Inputs overview:
/// - [`artifact_feeds`](Self::artifact_feeds) — comma-separated Azure Artifacts
///   feed names to authenticate.
/// - [`maven_service_connections`](Self::maven_service_connections) — comma-
///   separated Maven service connection names for repositories outside the
///   current organization.
/// - [`workload_identity_service_connection`](Self::workload_identity_service_connection)
///   — Entra Workload Identity-backed service connection; when set,
///   `mavenServiceConnections` is ignored by the task.
///
/// ADO task reference:
/// <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/maven-authenticate-v0>
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MavenAuthenticate {
    #[serde(rename = "artifactsFeeds", default)]
    artifacts_feeds: Option<String>,
    #[serde(rename = "mavenServiceConnections", default)]
    maven_service_connections: Option<String>,
    #[serde(rename = "workloadIdentityServiceConnection", default)]
    workload_identity_service_connection: Option<String>,
    #[serde(skip)]
    display_name: Option<String>,
}

impl MavenAuthenticate {
    /// Create a new builder; all inputs are optional.
    pub fn new() -> Self {
        Self {
            artifacts_feeds: None,
            maven_service_connections: None,
            workload_identity_service_connection: None,
            display_name: None,
        }
    }

    /// `artifactsFeeds` — comma-separated list of Azure Artifacts feed names
    /// to authenticate with Maven. Leave blank when only external repository
    /// authentication is needed.
    pub fn artifact_feeds(mut self, value: impl Into<String>) -> Self {
        self.artifacts_feeds = Some(value.into());
        self
    }

    /// `mavenServiceConnections` — comma-separated list of Maven service
    /// connection names from external organizations to authenticate with Maven.
    /// Leave blank when only Azure Artifacts feed authentication is needed.
    /// Ignored when [`workload_identity_service_connection`](Self::workload_identity_service_connection)
    /// is set.
    pub fn maven_service_connections(mut self, value: impl Into<String>) -> Self {
        self.maven_service_connections = Some(value.into());
        self
    }

    /// `workloadIdentityServiceConnection` — Entra Workload Identity-backed
    /// Azure DevOps service connection. When set, `mavenServiceConnections` is
    /// ignored by the task.
    pub fn workload_identity_service_connection(mut self, value: impl Into<String>) -> Self {
        self.workload_identity_service_connection = Some(value.into());
        self
    }

    /// Override the default `displayName` (`"Maven Authenticate"`).
    pub fn with_display_name(mut self, value: impl Into<String>) -> Self {
        self.display_name = Some(value.into());
        self
    }

    /// Lower into a [`TaskStep`].
    pub fn into_step(self) -> TaskStep {
        let mut t = TaskStep::new(
            "MavenAuthenticate@0",
            self.display_name
                .unwrap_or_else(|| "Maven Authenticate".into()),
        );
        if let Some(v) = self.artifacts_feeds {
            t = t.with_input("artifactsFeeds", v);
        }
        if let Some(v) = self.maven_service_connections {
            t = t.with_input("mavenServiceConnections", v);
        }
        if let Some(v) = self.workload_identity_service_connection {
            t = t.with_input("workloadIdentityServiceConnection", v);
        }
        t
    }
}

impl Default for MavenAuthenticate {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sets_task_identifier() {
        let t = MavenAuthenticate::new().into_step();
        assert_eq!(t.task, "MavenAuthenticate@0");
    }

    #[test]
    fn no_inputs_by_default() {
        let t = MavenAuthenticate::new().into_step();
        assert!(t.inputs.is_empty(), "expected no inputs when none are set");
    }

    #[test]
    fn default_display_name() {
        let t = MavenAuthenticate::new().into_step();
        assert_eq!(t.display_name, "Maven Authenticate");
    }

    #[test]
    fn artifact_feeds_single() {
        let t = MavenAuthenticate::new()
            .artifact_feeds("MyFeedInOrg")
            .into_step();
        assert_eq!(
            t.inputs.get("artifactsFeeds").map(String::as_str),
            Some("MyFeedInOrg")
        );
    }

    #[test]
    fn artifact_feeds_multiple() {
        let t = MavenAuthenticate::new()
            .artifact_feeds("MyFeedInOrg1,MyFeedInOrg2")
            .into_step();
        assert_eq!(
            t.inputs.get("artifactsFeeds").map(String::as_str),
            Some("MyFeedInOrg1,MyFeedInOrg2")
        );
    }

    #[test]
    fn maven_service_connections() {
        let t = MavenAuthenticate::new()
            .maven_service_connections("central,MavenOrg")
            .into_step();
        assert_eq!(
            t.inputs
                .get("mavenServiceConnections")
                .map(String::as_str),
            Some("central,MavenOrg")
        );
    }

    #[test]
    fn workload_identity_service_connection() {
        let t = MavenAuthenticate::new()
            .workload_identity_service_connection("my-wif-conn")
            .into_step();
        assert_eq!(
            t.inputs
                .get("workloadIdentityServiceConnection")
                .map(String::as_str),
            Some("my-wif-conn")
        );
    }

    #[test]
    fn with_display_name_override() {
        let t = MavenAuthenticate::new()
            .with_display_name("Authenticate to internal Maven feed")
            .into_step();
        assert_eq!(t.display_name, "Authenticate to internal Maven feed");
    }

    #[test]
    fn artifact_feeds_and_service_connections_together() {
        let t = MavenAuthenticate::new()
            .artifact_feeds("internal-feed")
            .maven_service_connections("external-maven-conn")
            .with_display_name("Auth Maven feeds")
            .into_step();
        assert_eq!(t.task, "MavenAuthenticate@0");
        assert_eq!(t.display_name, "Auth Maven feeds");
        assert_eq!(
            t.inputs.get("artifactsFeeds").map(String::as_str),
            Some("internal-feed")
        );
        assert_eq!(
            t.inputs
                .get("mavenServiceConnections")
                .map(String::as_str),
            Some("external-maven-conn")
        );
    }
}
