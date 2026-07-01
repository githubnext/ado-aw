//! Typed builder for `DockerInstaller@0`.

use crate::compile::ir::step::TaskStep;
use serde::Deserialize;

/// Docker release channel for [`DockerInstaller`] (`releaseType` input).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum ReleaseType {
    #[serde(rename = "stable")]
    Stable,
    #[serde(rename = "edge")]
    Edge,
    #[serde(rename = "test")]
    Test,
    #[serde(rename = "nightly")]
    Nightly,
}

impl ReleaseType {
    /// The exact token the ADO task expects.
    pub fn as_ado_str(self) -> &'static str {
        match self {
            ReleaseType::Stable => "stable",
            ReleaseType::Edge => "edge",
            ReleaseType::Test => "test",
            ReleaseType::Nightly => "nightly",
        }
    }
}

/// Builder for a [`TaskStep`] invoking `DockerInstaller@0`.
///
/// Installs a specific version of Docker Engine on the agent.
///
/// ADO task reference:
/// <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/docker-installer-v0>
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DockerInstaller {
    #[serde(rename = "dockerVersion")]
    docker_version: String,
    #[serde(rename = "releaseType", default)]
    release_type: Option<ReleaseType>,
    #[serde(skip)]
    display_name: Option<String>,
}

impl DockerInstaller {
    /// Required input: `dockerVersion` (e.g. `"26.1.4"`).
    pub fn new(docker_version: impl Into<String>) -> Self {
        Self {
            docker_version: docker_version.into(),
            release_type: None,
            display_name: None,
        }
    }

    /// `releaseType` — release channel (default `stable`).
    pub fn release_type(mut self, value: ReleaseType) -> Self {
        self.release_type = Some(value);
        self
    }

    /// Override the default `displayName` (`"Install Docker"`).
    pub fn with_display_name(mut self, value: impl Into<String>) -> Self {
        self.display_name = Some(value.into());
        self
    }

    /// Lower into a [`TaskStep`].
    pub fn into_step(self) -> TaskStep {
        let mut t = TaskStep::new(
            "DockerInstaller@0",
            self.display_name.unwrap_or_else(|| "Install Docker".into()),
        )
        .with_input("dockerVersion", self.docker_version);
        if let Some(v) = self.release_type {
            t = t.with_input("releaseType", v.as_ado_str());
        }
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sets_task_and_required_version() {
        let t = DockerInstaller::new("26.1.4").into_step();
        assert_eq!(t.task, "DockerInstaller@0");
        assert_eq!(t.display_name, "Install Docker");
        assert_eq!(
            t.inputs.get("dockerVersion").map(String::as_str),
            Some("26.1.4")
        );
        assert!(t.inputs.get("releaseType").is_none());
    }

    #[test]
    fn release_type_is_typed() {
        let t = DockerInstaller::new("26.1.4")
            .release_type(ReleaseType::Edge)
            .into_step();
        assert_eq!(
            t.inputs.get("releaseType").map(String::as_str),
            Some("edge")
        );
    }
}
