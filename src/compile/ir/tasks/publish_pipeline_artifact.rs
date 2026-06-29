//! Typed builder for `PublishPipelineArtifact@1`.

use super::common::bool_input;
use crate::compile::ir::step::TaskStep;

/// Storage location for [`PublishPipelineArtifact`] (`publishLocation` input).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublishLocation {
    Pipeline,
    Filepath,
}

impl PublishLocation {
    /// The exact token the ADO task expects.
    pub fn as_ado_str(self) -> &'static str {
        match self {
            PublishLocation::Pipeline => "pipeline",
            PublishLocation::Filepath => "filepath",
        }
    }
}

/// Builder for a [`TaskStep`] invoking `PublishPipelineArtifact@1`.
///
/// ADO task reference:
/// <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/publish-pipeline-artifact-v1>
#[derive(Debug, Clone)]
pub struct PublishPipelineArtifact {
    target_path: String,
    artifact: Option<String>,
    publish_location: Option<PublishLocation>,
    file_share_path: Option<String>,
    parallel: Option<bool>,
    parallel_count: Option<String>,
    properties: Option<String>,
    display_name: Option<String>,
}

impl PublishPipelineArtifact {
    /// Required input: `targetPath` — the file or directory to publish.
    pub fn new(target_path: impl Into<String>) -> Self {
        Self {
            target_path: target_path.into(),
            artifact: None,
            publish_location: None,
            file_share_path: None,
            parallel: None,
            parallel_count: None,
            properties: None,
            display_name: None,
        }
    }

    /// `artifact` — name of the published artifact (e.g. `"drop"`).
    pub fn artifact(mut self, value: impl Into<String>) -> Self {
        self.artifact = Some(value.into());
        self
    }

    /// `publishLocation` — where to store the artifact (default `pipeline`).
    pub fn publish_location(mut self, value: PublishLocation) -> Self {
        self.publish_location = Some(value);
        self
    }

    /// `fileSharePath` — UNC path (required when `publishLocation = filepath`).
    pub fn file_share_path(mut self, value: impl Into<String>) -> Self {
        self.file_share_path = Some(value.into());
        self
    }

    /// `parallel` — multi-threaded copy when `publishLocation = filepath`.
    pub fn parallel(mut self, value: bool) -> Self {
        self.parallel = Some(value);
        self
    }

    /// `parallelCount` — thread count for parallel copy (1–128).
    pub fn parallel_count(mut self, value: impl Into<String>) -> Self {
        self.parallel_count = Some(value.into());
        self
    }

    /// `properties` — JSON string of custom artifact properties.
    pub fn properties(mut self, value: impl Into<String>) -> Self {
        self.properties = Some(value.into());
        self
    }

    /// Override the default `displayName` (`"Publish Pipeline Artifact"`).
    pub fn with_display_name(mut self, value: impl Into<String>) -> Self {
        self.display_name = Some(value.into());
        self
    }

    /// Lower into a [`TaskStep`].
    pub fn into_step(self) -> TaskStep {
        let mut t = TaskStep::new(
            "PublishPipelineArtifact@1",
            self.display_name.unwrap_or_else(|| "Publish Pipeline Artifact".into()),
        )
        .with_input("targetPath", self.target_path);
        if let Some(v) = self.artifact {
            t = t.with_input("artifact", v);
        }
        if let Some(v) = self.publish_location {
            t = t.with_input("publishLocation", v.as_ado_str());
        }
        if let Some(v) = self.file_share_path {
            t = t.with_input("fileSharePath", v);
        }
        if let Some(v) = self.parallel {
            t = t.with_input("parallel", bool_input(v));
        }
        if let Some(v) = self.parallel_count {
            t = t.with_input("parallelCount", v);
        }
        if let Some(v) = self.properties {
            t = t.with_input("properties", v);
        }
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sets_task_and_required_target() {
        let t = PublishPipelineArtifact::new("$(Build.ArtifactStagingDirectory)").into_step();
        assert_eq!(t.task, "PublishPipelineArtifact@1");
        assert_eq!(
            t.inputs.get("targetPath").map(String::as_str),
            Some("$(Build.ArtifactStagingDirectory)")
        );
    }

    #[test]
    fn filepath_location() {
        let t = PublishPipelineArtifact::new("$(Build.ArtifactStagingDirectory)")
            .artifact("binaries")
            .publish_location(PublishLocation::Filepath)
            .file_share_path("\\\\myserver\\share\\$(Build.DefinitionName)")
            .into_step();
        assert_eq!(t.inputs.get("artifact").map(String::as_str), Some("binaries"));
        assert_eq!(t.inputs.get("publishLocation").map(String::as_str), Some("filepath"));
        assert_eq!(
            t.inputs.get("fileSharePath").map(String::as_str),
            Some("\\\\myserver\\share\\$(Build.DefinitionName)")
        );
    }
}
