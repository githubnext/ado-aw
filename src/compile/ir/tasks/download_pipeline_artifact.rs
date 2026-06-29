//! Typed builder for `DownloadPipelineArtifact@2`.

use super::common::bool_input;
use crate::compile::ir::step::TaskStep;

/// Run source for [`DownloadPipelineArtifact`] (`source` input).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactSource {
    Current,
    Specific,
}

impl ArtifactSource {
    /// The exact token the ADO task expects.
    pub fn as_ado_str(self) -> &'static str {
        match self {
            ArtifactSource::Current => "current",
            ArtifactSource::Specific => "specific",
        }
    }
}

/// Which run to download from for [`DownloadPipelineArtifact`] (`runVersion`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunVersion {
    Latest,
    LatestFromBranch,
    Specific,
}

impl RunVersion {
    /// The exact token the ADO task expects.
    pub fn as_ado_str(self) -> &'static str {
        match self {
            RunVersion::Latest => "latest",
            RunVersion::LatestFromBranch => "latestFromBranch",
            RunVersion::Specific => "specific",
        }
    }
}

/// Builder for a [`TaskStep`] invoking `DownloadPipelineArtifact@2`.
///
/// ADO task reference:
/// <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/download-pipeline-artifact-v2>
#[derive(Debug, Clone)]
pub struct DownloadPipelineArtifact {
    target_path: String,
    artifact: Option<String>,
    patterns: Option<String>,
    source: Option<ArtifactSource>,
    project: Option<String>,
    pipeline: Option<String>,
    run_version: Option<RunVersion>,
    branch_name: Option<String>,
    run_id: Option<String>,
    tags: Option<String>,
    allow_partially_succeeded_builds: Option<bool>,
    allow_failed_builds: Option<bool>,
    prefer_triggering_pipeline: Option<bool>,
    item_pattern: Option<String>,
    display_name: Option<String>,
}

impl DownloadPipelineArtifact {
    /// Required input: `targetPath` — download destination.
    pub fn new(target_path: impl Into<String>) -> Self {
        Self {
            target_path: target_path.into(),
            artifact: None,
            patterns: None,
            source: None,
            project: None,
            pipeline: None,
            run_version: None,
            branch_name: None,
            run_id: None,
            tags: None,
            allow_partially_succeeded_builds: None,
            allow_failed_builds: None,
            prefer_triggering_pipeline: None,
            item_pattern: None,
            display_name: None,
        }
    }

    /// `artifact` — name of the artifact to download (omit to download all).
    pub fn artifact(mut self, value: impl Into<String>) -> Self {
        self.artifact = Some(value.into());
        self
    }

    /// `patterns` — newline-separated glob filters for files to download.
    pub fn patterns(mut self, value: impl Into<String>) -> Self {
        self.patterns = Some(value.into());
        self
    }

    /// `source` — `current` (this run) or `specific` (another run).
    pub fn source(mut self, value: ArtifactSource) -> Self {
        self.source = Some(value);
        self
    }

    /// `project` — ADO project name or ID (`source = specific` only).
    pub fn project(mut self, value: impl Into<String>) -> Self {
        self.project = Some(value.into());
        self
    }

    /// `pipeline` — pipeline definition ID or name (`source = specific` only).
    pub fn pipeline(mut self, value: impl Into<String>) -> Self {
        self.pipeline = Some(value.into());
        self
    }

    /// `runVersion` — which run to download from (`source = specific` only).
    pub fn run_version(mut self, value: RunVersion) -> Self {
        self.run_version = Some(value);
        self
    }

    /// `branchName` — branch filter (`runVersion = latestFromBranch` only).
    pub fn branch_name(mut self, value: impl Into<String>) -> Self {
        self.branch_name = Some(value.into());
        self
    }

    /// `runId` — build ID to download from (`runVersion = specific` only).
    pub fn run_id(mut self, value: impl Into<String>) -> Self {
        self.run_id = Some(value.into());
        self
    }

    /// `tags` — comma-separated build tags to filter candidate runs.
    pub fn tags(mut self, value: impl Into<String>) -> Self {
        self.tags = Some(value.into());
        self
    }

    /// `allowPartiallySucceededBuilds` — also consider partially-succeeded runs.
    pub fn allow_partially_succeeded_builds(mut self, value: bool) -> Self {
        self.allow_partially_succeeded_builds = Some(value);
        self
    }

    /// `allowFailedBuilds` — also consider failed runs.
    pub fn allow_failed_builds(mut self, value: bool) -> Self {
        self.allow_failed_builds = Some(value);
        self
    }

    /// `preferTriggeringPipeline` — prefer the run that triggered this pipeline.
    pub fn prefer_triggering_pipeline(mut self, value: bool) -> Self {
        self.prefer_triggering_pipeline = Some(value);
        self
    }

    /// `itemPattern` — minimatch pattern applied after download.
    pub fn item_pattern(mut self, value: impl Into<String>) -> Self {
        self.item_pattern = Some(value.into());
        self
    }

    /// Override the default `displayName` (`"Download Pipeline Artifact"`).
    pub fn with_display_name(mut self, value: impl Into<String>) -> Self {
        self.display_name = Some(value.into());
        self
    }

    /// Lower into a [`TaskStep`].
    pub fn into_step(self) -> TaskStep {
        let mut t = TaskStep::new(
            "DownloadPipelineArtifact@2",
            self.display_name.unwrap_or_else(|| "Download Pipeline Artifact".into()),
        )
        .with_input("targetPath", self.target_path);
        if let Some(v) = self.artifact {
            t = t.with_input("artifact", v);
        }
        if let Some(v) = self.patterns {
            t = t.with_input("patterns", v);
        }
        if let Some(v) = self.source {
            t = t.with_input("source", v.as_ado_str());
        }
        if let Some(v) = self.project {
            t = t.with_input("project", v);
        }
        if let Some(v) = self.pipeline {
            t = t.with_input("pipeline", v);
        }
        if let Some(v) = self.run_version {
            t = t.with_input("runVersion", v.as_ado_str());
        }
        if let Some(v) = self.branch_name {
            t = t.with_input("branchName", v);
        }
        if let Some(v) = self.run_id {
            t = t.with_input("runId", v);
        }
        if let Some(v) = self.tags {
            t = t.with_input("tags", v);
        }
        if let Some(v) = self.allow_partially_succeeded_builds {
            t = t.with_input("allowPartiallySucceededBuilds", bool_input(v));
        }
        if let Some(v) = self.allow_failed_builds {
            t = t.with_input("allowFailedBuilds", bool_input(v));
        }
        if let Some(v) = self.prefer_triggering_pipeline {
            t = t.with_input("preferTriggeringPipeline", bool_input(v));
        }
        if let Some(v) = self.item_pattern {
            t = t.with_input("itemPattern", v);
        }
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sets_task_and_required_target() {
        let t = DownloadPipelineArtifact::new("$(Pipeline.Workspace)/drop").into_step();
        assert_eq!(t.task, "DownloadPipelineArtifact@2");
        assert_eq!(
            t.inputs.get("targetPath").map(String::as_str),
            Some("$(Pipeline.Workspace)/drop")
        );
    }

    #[test]
    fn specific_run_inputs() {
        let t = DownloadPipelineArtifact::new("$(Pipeline.Workspace)/in")
            .source(ArtifactSource::Specific)
            .project("$(System.TeamProject)")
            .pipeline("$(System.DefinitionId)")
            .run_version(RunVersion::LatestFromBranch)
            .branch_name("$(Build.SourceBranch)")
            .artifact("safe_outputs")
            .allow_partially_succeeded_builds(true)
            .into_step();
        assert_eq!(t.inputs.get("source").map(String::as_str), Some("specific"));
        assert_eq!(t.inputs.get("runVersion").map(String::as_str), Some("latestFromBranch"));
        assert_eq!(t.inputs.get("artifact").map(String::as_str), Some("safe_outputs"));
        assert_eq!(
            t.inputs.get("allowPartiallySucceededBuilds").map(String::as_str),
            Some("true")
        );
    }
}
