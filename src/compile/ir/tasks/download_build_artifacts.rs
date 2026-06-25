//! Typed builder for `DownloadBuildArtifacts@1`.
//!
//! Downloads build artifacts from the current build or a specific build
//! definition. This is the legacy build-artifact download task; for
//! pipeline-artifact downloads use [`super::download_pipeline_artifact`].
//!
//! The `buildType` input selects *which build* to download from
//! (`current` or `specific`). The `downloadType` input selects *how many
//! artifacts* to download (`single` for one named artifact, `specific` for
//! all artifacts matching an item pattern).
//!
//! ADO task reference:
//! <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/download-build-artifacts-v1>

use super::common::{bool_input, push_bool, push_opt};
use crate::compile::ir::step::TaskStep;

/// Which build to download artifacts from (`buildType` input).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildType {
    /// Download from the current pipeline run (default).
    Current,
    /// Download from a specific pipeline definition / run.
    Specific,
}

impl BuildType {
    /// The exact token the ADO task expects.
    pub fn as_ado_str(self) -> &'static str {
        match self {
            BuildType::Current => "current",
            BuildType::Specific => "specific",
        }
    }
}

/// Which build version to retrieve when `buildType = specific`
/// (`buildVersionToDownload` input).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildVersionToDownload {
    /// Latest successful build (default).
    Latest,
    /// Latest successful build from a specific branch.
    LatestFromBranch,
    /// A specific build identified by its build ID.
    Specific,
}

impl BuildVersionToDownload {
    /// The exact token the ADO task expects.
    pub fn as_ado_str(self) -> &'static str {
        match self {
            BuildVersionToDownload::Latest => "latest",
            BuildVersionToDownload::LatestFromBranch => "latestFromBranch",
            BuildVersionToDownload::Specific => "specific",
        }
    }
}

/// How many artifacts to download (`downloadType` input).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadType {
    /// Download a single artifact by name (default).
    Single,
    /// Download all artifacts whose paths match `itemPattern`.
    Specific,
}

impl DownloadType {
    /// The exact token the ADO task expects.
    pub fn as_ado_str(self) -> &'static str {
        match self {
            DownloadType::Single => "single",
            DownloadType::Specific => "specific",
        }
    }
}

/// Builder for a [`TaskStep`] invoking `DownloadBuildArtifacts@1`.
///
/// The only required input is `download_path` (the local destination directory).
/// All other inputs default to the task's own defaults — most importantly
/// `buildType` defaults to `current` (download from the current build) and
/// `downloadType` defaults to `single` (download one named artifact).
///
/// When downloading from a **specific** build (`buildType = specific`), also set
/// [`project`](DownloadBuildArtifacts::project),
/// [`definition`](DownloadBuildArtifacts::definition), and
/// [`build_version_to_download`](DownloadBuildArtifacts::build_version_to_download).
/// If the version is `latestFromBranch`, also set
/// [`branch_name`](DownloadBuildArtifacts::branch_name).
/// If the version is `specific`, also set
/// [`build_id`](DownloadBuildArtifacts::build_id).
///
/// ADO task reference:
/// <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/download-build-artifacts-v1>
#[derive(Debug, Clone)]
pub struct DownloadBuildArtifacts {
    /// `downloadPath` — destination directory on the agent.
    download_path: String,
    /// `buildType` — which build to download from.
    build_type: Option<BuildType>,
    /// `downloadType` — single artifact or all matching a pattern.
    download_type: Option<DownloadType>,
    /// `artifactName` — name of the artifact to download (`downloadType = single`).
    artifact_name: Option<String>,
    /// `itemPattern` — minimatch glob applied to artifact contents (`downloadType = specific`).
    item_pattern: Option<String>,
    // --- buildType=specific inputs ---
    /// `project` — ADO project name or ID.
    project: Option<String>,
    /// `definition` — pipeline definition ID or name.
    definition: Option<String>,
    /// `buildVersionToDownload` — which build to select.
    build_version_to_download: Option<BuildVersionToDownload>,
    /// `branchName` — branch filter (`buildVersionToDownload = latestFromBranch`).
    branch_name: Option<String>,
    /// `buildId` — build ID to download from (`buildVersionToDownload = specific`).
    build_id: Option<String>,
    /// `tags` — comma-separated build tags to filter candidate builds.
    tags: Option<String>,
    /// `specificBuildWithTriggering` — use the triggering build when it matches.
    specific_build_with_triggering: Option<bool>,
    /// `allowPartiallySucceededBuilds` — include partially-succeeded builds.
    allow_partially_succeeded_builds: Option<bool>,
    // --- misc optionals ---
    /// `cleanDestinationFolder` — clean the destination folder before downloading.
    clean_destination_folder: Option<bool>,
    /// `parallelizationLimit` — number of parallel download threads (default `"8"`).
    parallelization_limit: Option<String>,
    /// `checkDownloadedFiles` — verify downloaded files are not corrupt.
    check_downloaded_files: Option<bool>,
    /// `retryDownloadCount` — number of retries on download failure (default `"4"`).
    retry_download_count: Option<String>,
    /// `extractTars` — automatically extract downloaded tar archives.
    extract_tars: Option<bool>,
    display_name: Option<String>,
}

impl DownloadBuildArtifacts {
    /// Required input: `downloadPath` — local destination directory.
    ///
    /// Defaults to `$(System.ArtifactsDirectory)` when not set by the task.
    pub fn new(download_path: impl Into<String>) -> Self {
        Self {
            download_path: download_path.into(),
            build_type: None,
            download_type: None,
            artifact_name: None,
            item_pattern: None,
            project: None,
            definition: None,
            build_version_to_download: None,
            branch_name: None,
            build_id: None,
            tags: None,
            specific_build_with_triggering: None,
            allow_partially_succeeded_builds: None,
            clean_destination_folder: None,
            parallelization_limit: None,
            check_downloaded_files: None,
            retry_download_count: None,
            extract_tars: None,
            display_name: None,
        }
    }

    /// `buildType` — which build to download from (`current` or `specific`).
    pub fn build_type(mut self, value: BuildType) -> Self {
        self.build_type = Some(value);
        self
    }

    /// `downloadType` — single artifact or all matching a pattern.
    pub fn download_type(mut self, value: DownloadType) -> Self {
        self.download_type = Some(value);
        self
    }

    /// `artifactName` — artifact name to download (`downloadType = single`).
    pub fn artifact_name(mut self, value: impl Into<String>) -> Self {
        self.artifact_name = Some(value.into());
        self
    }

    /// `itemPattern` — minimatch pattern applied to artifact paths (`downloadType = specific`).
    pub fn item_pattern(mut self, value: impl Into<String>) -> Self {
        self.item_pattern = Some(value.into());
        self
    }

    /// `project` — ADO project name or ID (`buildType = specific`).
    pub fn project(mut self, value: impl Into<String>) -> Self {
        self.project = Some(value.into());
        self
    }

    /// `definition` — pipeline definition ID or name (`buildType = specific`).
    pub fn definition(mut self, value: impl Into<String>) -> Self {
        self.definition = Some(value.into());
        self
    }

    /// `buildVersionToDownload` — which build to select (`buildType = specific`).
    pub fn build_version_to_download(mut self, value: BuildVersionToDownload) -> Self {
        self.build_version_to_download = Some(value);
        self
    }

    /// `branchName` — branch filter for `buildVersionToDownload = latestFromBranch`.
    pub fn branch_name(mut self, value: impl Into<String>) -> Self {
        self.branch_name = Some(value.into());
        self
    }

    /// `buildId` — build ID for `buildVersionToDownload = specific`.
    pub fn build_id(mut self, value: impl Into<String>) -> Self {
        self.build_id = Some(value.into());
        self
    }

    /// `tags` — comma-separated build tags to filter candidate builds.
    pub fn tags(mut self, value: impl Into<String>) -> Self {
        self.tags = Some(value.into());
        self
    }

    /// `specificBuildWithTriggering` — prefer the triggering build when it matches.
    pub fn specific_build_with_triggering(mut self, value: bool) -> Self {
        self.specific_build_with_triggering = Some(value);
        self
    }

    /// `allowPartiallySucceededBuilds` — include partially-succeeded builds.
    pub fn allow_partially_succeeded_builds(mut self, value: bool) -> Self {
        self.allow_partially_succeeded_builds = Some(value);
        self
    }

    /// `cleanDestinationFolder` — remove existing files in the download directory first.
    pub fn clean_destination_folder(mut self, value: bool) -> Self {
        self.clean_destination_folder = Some(value);
        self
    }

    /// `parallelizationLimit` — number of parallel download threads (default `"8"`).
    pub fn parallelization_limit(mut self, value: impl Into<String>) -> Self {
        self.parallelization_limit = Some(value.into());
        self
    }

    /// `checkDownloadedFiles` — verify downloaded files are not corrupt.
    pub fn check_downloaded_files(mut self, value: bool) -> Self {
        self.check_downloaded_files = Some(value);
        self
    }

    /// `retryDownloadCount` — number of retries on transient download failure (default `"4"`).
    pub fn retry_download_count(mut self, value: impl Into<String>) -> Self {
        self.retry_download_count = Some(value.into());
        self
    }

    /// `extractTars` — automatically extract downloaded tar archives.
    pub fn extract_tars(mut self, value: bool) -> Self {
        self.extract_tars = Some(value);
        self
    }

    /// Override the default `displayName` (`"Download Build Artifacts"`).
    pub fn with_display_name(mut self, value: impl Into<String>) -> Self {
        self.display_name = Some(value.into());
        self
    }

    /// Lower into a [`TaskStep`].
    pub fn into_step(self) -> TaskStep {
        let mut t = TaskStep::new(
            "DownloadBuildArtifacts@1",
            self.display_name
                .unwrap_or_else(|| "Download Build Artifacts".into()),
        )
        .with_input("downloadPath", self.download_path);

        if let Some(v) = self.build_type {
            t = t.with_input("buildType", v.as_ado_str());
        }
        if let Some(v) = self.download_type {
            t = t.with_input("downloadType", v.as_ado_str());
        }
        push_opt(&mut t, "artifactName", self.artifact_name);
        push_opt(&mut t, "itemPattern", self.item_pattern);
        push_opt(&mut t, "project", self.project);
        push_opt(&mut t, "definition", self.definition);
        if let Some(v) = self.build_version_to_download {
            t = t.with_input("buildVersionToDownload", v.as_ado_str());
        }
        push_opt(&mut t, "branchName", self.branch_name);
        push_opt(&mut t, "buildId", self.build_id);
        push_opt(&mut t, "tags", self.tags);
        push_bool(&mut t, "specificBuildWithTriggering", self.specific_build_with_triggering);
        push_bool(
            &mut t,
            "allowPartiallySucceededBuilds",
            self.allow_partially_succeeded_builds,
        );
        push_bool(&mut t, "cleanDestinationFolder", self.clean_destination_folder);
        push_opt(&mut t, "parallelizationLimit", self.parallelization_limit);
        push_bool(&mut t, "checkDownloadedFiles", self.check_downloaded_files);
        push_opt(&mut t, "retryDownloadCount", self.retry_download_count);
        if let Some(v) = self.extract_tars {
            t = t.with_input("extractTars", bool_input(v));
        }
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sets_task_and_required_download_path() {
        let t = DownloadBuildArtifacts::new("$(System.ArtifactsDirectory)").into_step();
        assert_eq!(t.task, "DownloadBuildArtifacts@1");
        assert_eq!(t.display_name, "Download Build Artifacts");
        assert_eq!(
            t.inputs.get("downloadPath").map(String::as_str),
            Some("$(System.ArtifactsDirectory)")
        );
        // No optional inputs emitted when not set
        assert!(t.inputs.get("buildType").is_none());
        assert!(t.inputs.get("downloadType").is_none());
        assert!(t.inputs.get("artifactName").is_none());
    }

    #[test]
    fn single_artifact_from_current_build() {
        let t = DownloadBuildArtifacts::new("$(Build.ArtifactStagingDirectory)")
            .build_type(BuildType::Current)
            .download_type(DownloadType::Single)
            .artifact_name("drop")
            .into_step();
        assert_eq!(t.inputs.get("buildType").map(String::as_str), Some("current"));
        assert_eq!(t.inputs.get("downloadType").map(String::as_str), Some("single"));
        assert_eq!(t.inputs.get("artifactName").map(String::as_str), Some("drop"));
    }

    #[test]
    fn specific_build_latest_version() {
        let t = DownloadBuildArtifacts::new("$(Pipeline.Workspace)/artifacts")
            .build_type(BuildType::Specific)
            .project("$(System.TeamProject)")
            .definition("42")
            .build_version_to_download(BuildVersionToDownload::Latest)
            .artifact_name("binaries")
            .allow_partially_succeeded_builds(true)
            .into_step();
        assert_eq!(t.inputs.get("buildType").map(String::as_str), Some("specific"));
        assert_eq!(
            t.inputs.get("project").map(String::as_str),
            Some("$(System.TeamProject)")
        );
        assert_eq!(t.inputs.get("definition").map(String::as_str), Some("42"));
        assert_eq!(
            t.inputs.get("buildVersionToDownload").map(String::as_str),
            Some("latest")
        );
        assert_eq!(
            t.inputs.get("allowPartiallySucceededBuilds").map(String::as_str),
            Some("true")
        );
    }

    #[test]
    fn specific_build_from_branch() {
        let t = DownloadBuildArtifacts::new("$(System.ArtifactsDirectory)")
            .build_type(BuildType::Specific)
            .project("my-project")
            .definition("99")
            .build_version_to_download(BuildVersionToDownload::LatestFromBranch)
            .branch_name("refs/heads/main")
            .artifact_name("packages")
            .into_step();
        assert_eq!(
            t.inputs.get("buildVersionToDownload").map(String::as_str),
            Some("latestFromBranch")
        );
        assert_eq!(
            t.inputs.get("branchName").map(String::as_str),
            Some("refs/heads/main")
        );
    }

    #[test]
    fn specific_build_by_id() {
        let t = DownloadBuildArtifacts::new("$(System.ArtifactsDirectory)")
            .build_type(BuildType::Specific)
            .project("my-project")
            .definition("7")
            .build_version_to_download(BuildVersionToDownload::Specific)
            .build_id("12345")
            .into_step();
        assert_eq!(
            t.inputs.get("buildVersionToDownload").map(String::as_str),
            Some("specific")
        );
        assert_eq!(t.inputs.get("buildId").map(String::as_str), Some("12345"));
    }

    #[test]
    fn all_artifacts_with_item_pattern() {
        let t = DownloadBuildArtifacts::new("$(System.ArtifactsDirectory)")
            .download_type(DownloadType::Specific)
            .item_pattern("**/*.nupkg")
            .clean_destination_folder(true)
            .parallelization_limit("16")
            .check_downloaded_files(true)
            .retry_download_count("3")
            .extract_tars(false)
            .into_step();
        assert_eq!(t.inputs.get("downloadType").map(String::as_str), Some("specific"));
        assert_eq!(t.inputs.get("itemPattern").map(String::as_str), Some("**/*.nupkg"));
        assert_eq!(
            t.inputs.get("cleanDestinationFolder").map(String::as_str),
            Some("true")
        );
        assert_eq!(
            t.inputs.get("parallelizationLimit").map(String::as_str),
            Some("16")
        );
        assert_eq!(
            t.inputs.get("checkDownloadedFiles").map(String::as_str),
            Some("true")
        );
        assert_eq!(t.inputs.get("retryDownloadCount").map(String::as_str), Some("3"));
        assert_eq!(t.inputs.get("extractTars").map(String::as_str), Some("false"));
    }

    #[test]
    fn display_name_override() {
        let t = DownloadBuildArtifacts::new("$(System.ArtifactsDirectory)")
            .with_display_name("Download release binaries")
            .into_step();
        assert_eq!(t.display_name, "Download release binaries");
    }

    #[test]
    fn omits_unset_optionals() {
        let t = DownloadBuildArtifacts::new("$(System.ArtifactsDirectory)").into_step();
        for key in &[
            "buildType",
            "downloadType",
            "artifactName",
            "itemPattern",
            "project",
            "definition",
            "buildVersionToDownload",
            "branchName",
            "buildId",
            "tags",
            "specificBuildWithTriggering",
            "allowPartiallySucceededBuilds",
            "cleanDestinationFolder",
            "parallelizationLimit",
            "checkDownloadedFiles",
            "retryDownloadCount",
            "extractTars",
        ] {
            assert!(
                t.inputs.get(*key).is_none(),
                "unexpected input emitted: {key}"
            );
        }
    }
}
