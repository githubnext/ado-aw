//! Typed builder for `TwineAuthenticate@1`.

use crate::compile::ir::step::TaskStep;
use serde::Deserialize;

/// Builder for a [`TaskStep`] invoking `TwineAuthenticate@1`.
///
/// Authenticates the twine Python package upload tool by writing a `.pypirc`
/// file and setting the `PYPIRC_PATH` environment variable. After this step
/// runs, pass `--config-file $(PYPIRC_PATH)` to `twine upload` and use the
/// feed name or endpoint name as the repository flag (`-r FeedName`).
///
/// Both inputs are optional; in practice exactly one should be set — either
/// [`artifact_feed`](Self::artifact_feed) for an Azure Artifacts feed in this
/// organization, or [`python_upload_service_connection`](Self::python_upload_service_connection)
/// for an external PyPI-compatible endpoint.
///
/// Note: only one feed or one service connection can be authorized per task
/// invocation. Add a second task step if two upload targets are needed.
///
/// ADO task reference:
/// <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/twine-authenticate-v1>
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TwineAuthenticate {
    #[serde(rename = "artifactFeed", default)]
    artifact_feed: Option<String>,
    #[serde(rename = "pythonUploadServiceConnection", default)]
    python_upload_service_connection: Option<String>,
    #[serde(skip)]
    display_name: Option<String>,
}

impl TwineAuthenticate {
    /// Create a new builder; all inputs are optional.
    pub fn new() -> Self {
        Self {
            artifact_feed: None,
            python_upload_service_connection: None,
            display_name: None,
        }
    }

    /// `artifactFeed` — name of an Azure Artifacts feed in this organization
    /// to authenticate. Use the feed name (or `project/feed` for project-scoped
    /// feeds) as the `-r` argument to `twine upload`.
    pub fn artifact_feed(mut self, value: impl Into<String>) -> Self {
        self.artifact_feed = Some(value.into());
        self
    }

    /// `pythonUploadServiceConnection` — name of a service connection for a
    /// feed outside this organization. The service connection must have package
    /// upload permissions.
    pub fn python_upload_service_connection(mut self, value: impl Into<String>) -> Self {
        self.python_upload_service_connection = Some(value.into());
        self
    }

    /// Override the default `displayName` (`"Twine Authenticate"`).
    pub fn with_display_name(mut self, value: impl Into<String>) -> Self {
        self.display_name = Some(value.into());
        self
    }

    /// Lower into a [`TaskStep`].
    pub fn into_step(self) -> TaskStep {
        let mut t = TaskStep::new(
            "TwineAuthenticate@1",
            self.display_name
                .unwrap_or_else(|| "Twine Authenticate".into()),
        );
        if let Some(v) = self.artifact_feed {
            t = t.with_input("artifactFeed", v);
        }
        if let Some(v) = self.python_upload_service_connection {
            t = t.with_input("pythonUploadServiceConnection", v);
        }
        t
    }
}

impl Default for TwineAuthenticate {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sets_task_identifier() {
        let t = TwineAuthenticate::new().into_step();
        assert_eq!(t.task, "TwineAuthenticate@1");
    }

    #[test]
    fn no_inputs_by_default() {
        let t = TwineAuthenticate::new().into_step();
        assert!(t.inputs.is_empty(), "expected no inputs when none are set");
    }

    #[test]
    fn default_display_name() {
        let t = TwineAuthenticate::new().into_step();
        assert_eq!(t.display_name, "Twine Authenticate");
    }

    #[test]
    fn artifact_feed() {
        let t = TwineAuthenticate::new()
            .artifact_feed("my-pypi-feed")
            .into_step();
        assert_eq!(
            t.inputs.get("artifactFeed").map(String::as_str),
            Some("my-pypi-feed")
        );
        assert!(!t.inputs.contains_key("pythonUploadServiceConnection"));
    }

    #[test]
    fn artifact_feed_project_scoped() {
        let t = TwineAuthenticate::new()
            .artifact_feed("my-project/my-pypi-feed")
            .into_step();
        assert_eq!(
            t.inputs.get("artifactFeed").map(String::as_str),
            Some("my-project/my-pypi-feed")
        );
    }

    #[test]
    fn python_upload_service_connection() {
        let t = TwineAuthenticate::new()
            .python_upload_service_connection("external-pypi-conn")
            .into_step();
        assert_eq!(
            t.inputs
                .get("pythonUploadServiceConnection")
                .map(String::as_str),
            Some("external-pypi-conn")
        );
        assert!(!t.inputs.contains_key("artifactFeed"));
    }

    #[test]
    fn with_display_name_override() {
        let t = TwineAuthenticate::new()
            .with_display_name("Authenticate twine (build service identity)")
            .into_step();
        assert_eq!(
            t.display_name,
            "Authenticate twine (build service identity)"
        );
    }

    #[test]
    fn all_inputs_together() {
        let t = TwineAuthenticate::new()
            .artifact_feed("internal-feed")
            .python_upload_service_connection("external-conn")
            .with_display_name("Auth twine feeds")
            .into_step();
        assert_eq!(t.task, "TwineAuthenticate@1");
        assert_eq!(t.display_name, "Auth twine feeds");
        assert_eq!(
            t.inputs.get("artifactFeed").map(String::as_str),
            Some("internal-feed")
        );
        assert_eq!(
            t.inputs
                .get("pythonUploadServiceConnection")
                .map(String::as_str),
            Some("external-conn")
        );
    }
}
