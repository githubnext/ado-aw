//! Typed builder for `GitHubRelease@1`.
//!
//! `GitHubRelease@1` is a command-dispatch task: the three [`GitHubReleaseAction`]
//! variants (`Create`, `Edit`, `Delete`) carry different required and optional inputs.
//! Because each action's data lives inside its own variant, applying an input to the
//! wrong action (e.g. `tag_source` on a `delete`) is unrepresentable at the type level.
//!
//! ADO task reference:
//! <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/github-release-v1>

use super::common::{de_opt_bool_flex, push_bool, push_opt};
use crate::compile::ir::step::TaskStep;
use serde::Deserialize;
use serde_yaml::Value;

/// Validate an authored `GitHubRelease@1` `inputs:` mapping (advisory
/// front-matter validation, see [`super::parse`]).
pub(crate) fn validate_inputs(inputs: Value) -> Result<(), String> {
    let mut map = match inputs {
        Value::Mapping(m) => m,
        Value::Null => Default::default(),
        other => return Err(format!("`inputs` must be a mapping, got {other:?}")),
    };
    let action = match map.remove("action") {
        Some(value) => value
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| "`action` must be a string".to_string())?,
        None => "create".to_string(),
    };

    validate_common(&Value::Mapping(map.clone())).map_err(|e| format!("common inputs: {e}"))?;
    remove_common_inputs(&mut map);
    let rest = Value::Mapping(map);

    let result = match action.as_str() {
        "create" => serde_yaml::from_value::<GitHubReleaseCreate>(rest).map(drop),
        "edit" => serde_yaml::from_value::<GitHubReleaseEdit>(rest).map(drop),
        "delete" => serde_yaml::from_value::<GitHubReleaseDelete>(rest).map(drop),
        other => return Err(format!("GitHubRelease@1: unknown action `{other}`")),
    };
    result.map_err(|e| format!("action `{action}`: {e}"))
}

fn validate_common(inputs: &Value) -> Result<(), serde_yaml::Error> {
    serde_yaml::from_value::<GitHubReleaseCommonInputs>(inputs.clone()).map(drop)
}

fn remove_common_inputs(map: &mut serde_yaml::Mapping) {
    for key in ["gitHubConnection", "repositoryName"] {
        map.remove(key);
    }
}

#[derive(Debug, Deserialize)]
struct GitHubReleaseCommonInputs {
    #[serde(rename = "gitHubConnection")]
    _git_hub_connection: String,
    #[serde(rename = "repositoryName")]
    _repository_name: String,
}

// ─── Constrained input enums ──────────────────────────────────────────────────

/// Where the release tag comes from (`tagSource` input). Only applies to
/// `action: create`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum TagSource {
    /// Use the git tag that triggered the pipeline run (default).
    #[serde(rename = "gitTag")]
    GitTag,
    /// Use a user-specified tag string.
    #[serde(rename = "userSpecifiedTag")]
    UserSpecifiedTag,
}

impl TagSource {
    pub fn as_ado_str(self) -> &'static str {
        match self {
            Self::GitTag => "gitTag",
            Self::UserSpecifiedTag => "userSpecifiedTag",
        }
    }
}

/// Where the release notes come from (`releaseNotesSource` input). Applies to
/// `action: create` and `action: edit`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum ReleaseNotesSource {
    /// Read release notes from a file (`releaseNotesFilePath`).
    #[serde(rename = "filePath")]
    FilePath,
    /// Provide release notes inline (`releaseNotesInline`).
    #[serde(rename = "inline")]
    Inline,
}

impl ReleaseNotesSource {
    pub fn as_ado_str(self) -> &'static str {
        match self {
            Self::FilePath => "filePath",
            Self::Inline => "inline",
        }
    }
}

/// Upload mode for assets when editing a release (`assetUploadMode` input).
/// Only applies to `action: edit`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum AssetUploadMode {
    /// Delete all existing assets before uploading (default).
    #[serde(rename = "delete")]
    Delete,
    /// Replace individual matching assets.
    #[serde(rename = "replace")]
    Replace,
}

impl AssetUploadMode {
    pub fn as_ado_str(self) -> &'static str {
        match self {
            Self::Delete => "delete",
            Self::Replace => "replace",
        }
    }
}

/// Whether to mark the release as the latest GitHub release (`makeLatest`
/// input). This is a three-way enum, not a plain bool: `legacy` preserves the
/// pre-2022 comparison logic based on `isDraft` / `isPreRelease`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum MakeLatest {
    #[serde(rename = "true")]
    True,
    #[serde(rename = "false")]
    False,
    /// Use the legacy `isPreRelease` / `isDraft` comparison.
    #[serde(rename = "legacy")]
    Legacy,
}

impl MakeLatest {
    pub fn as_ado_str(self) -> &'static str {
        match self {
            Self::True => "true",
            Self::False => "false",
            Self::Legacy => "legacy",
        }
    }
}

/// Which prior release to compare against when generating the changelog
/// (`changeLogCompareToRelease` input).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum ChangeLogCompareToRelease {
    /// Compare against the last full (non-draft, non-pre-release) release (default).
    #[serde(rename = "lastFullRelease")]
    FullRelease,
    /// Compare against the last non-draft release.
    #[serde(rename = "lastNonDraftRelease")]
    NonDraftRelease,
    /// Compare against the last non-draft release matching a given tag.
    #[serde(rename = "lastNonDraftReleaseByTag")]
    NonDraftReleaseByTag,
}

impl ChangeLogCompareToRelease {
    pub fn as_ado_str(self) -> &'static str {
        match self {
            Self::FullRelease => "lastFullRelease",
            Self::NonDraftRelease => "lastNonDraftRelease",
            Self::NonDraftReleaseByTag => "lastNonDraftReleaseByTag",
        }
    }
}

/// Changelog entry format (`changeLogType` input).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum ChangeLogType {
    /// Entries based on commit messages (default).
    #[serde(rename = "commitBased")]
    CommitBased,
    /// Entries based on closed issues.
    #[serde(rename = "issueBased")]
    IssueBased,
}

impl ChangeLogType {
    pub fn as_ado_str(self) -> &'static str {
        match self {
            Self::CommitBased => "commitBased",
            Self::IssueBased => "issueBased",
        }
    }
}

// ─── Per-action option structs ────────────────────────────────────────────────

/// Optional inputs for `GitHubRelease@1` `action: create`.
///
/// All fields are optional because the ADO task provides sensible defaults. Set
/// `tag_source` to [`TagSource::UserSpecifiedTag`] and call
/// [`GitHubReleaseCreate::tag`] when you need to pin the tag string explicitly.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitHubReleaseCreate {
    #[serde(default)]
    target: Option<String>,
    #[serde(rename = "tagSource", default)]
    tag_source: Option<TagSource>,
    #[serde(rename = "tagPattern", default)]
    tag_pattern: Option<String>,
    #[serde(default)]
    tag: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(rename = "releaseNotesSource", default)]
    release_notes_source: Option<ReleaseNotesSource>,
    #[serde(rename = "releaseNotesFilePath", default)]
    release_notes_file_path: Option<String>,
    #[serde(rename = "releaseNotesInline", default)]
    release_notes_inline: Option<String>,
    #[serde(default)]
    assets: Option<String>,
    #[serde(rename = "isDraft", default, deserialize_with = "de_opt_bool_flex")]
    is_draft: Option<bool>,
    #[serde(
        rename = "isPreRelease",
        default,
        deserialize_with = "de_opt_bool_flex"
    )]
    is_pre_release: Option<bool>,
    #[serde(rename = "makeLatest", default)]
    make_latest: Option<MakeLatest>,
    #[serde(
        rename = "addChangeLog",
        default,
        deserialize_with = "de_opt_bool_flex"
    )]
    add_change_log: Option<bool>,
    #[serde(rename = "changeLogCompareToRelease", default)]
    change_log_compare_to_release: Option<ChangeLogCompareToRelease>,
    #[serde(rename = "changeLogCompareToReleaseTag", default)]
    change_log_compare_to_release_tag: Option<String>,
    #[serde(rename = "changeLogType", default)]
    change_log_type: Option<ChangeLogType>,
    #[serde(rename = "changeLogLabels", default)]
    change_log_labels: Option<String>,
}

impl GitHubReleaseCreate {
    pub fn new() -> Self {
        Self::default()
    }

    /// `target` — commit SHA or branch name to create the tag against.
    /// Defaults to `$(Build.SourceVersion)`.
    pub fn target(mut self, value: impl Into<String>) -> Self {
        self.target = Some(value.into());
        self
    }

    /// `tagSource` — use an existing git tag (`GitTag`, the ADO default) or a
    /// user-specified string (`UserSpecifiedTag`). When `UserSpecifiedTag`,
    /// also call [`Self::tag`] with the tag value.
    pub fn tag_source(mut self, value: TagSource) -> Self {
        self.tag_source = Some(value);
        self
    }

    /// `tagPattern` — regex pattern to match the triggering git tag.
    /// Only used when `tagSource = gitTag`.
    pub fn tag_pattern(mut self, value: impl Into<String>) -> Self {
        self.tag_pattern = Some(value.into());
        self
    }

    /// `tag` — tag string to apply. Required when `tagSource = userSpecifiedTag`.
    pub fn tag(mut self, value: impl Into<String>) -> Self {
        self.tag = Some(value.into());
        self
    }

    /// `title` — release title displayed on GitHub.
    pub fn title(mut self, value: impl Into<String>) -> Self {
        self.title = Some(value.into());
        self
    }

    /// `releaseNotesSource` — where to read release notes from.
    pub fn release_notes_source(mut self, value: ReleaseNotesSource) -> Self {
        self.release_notes_source = Some(value);
        self
    }

    /// `releaseNotesFilePath` — path to the release notes file.
    /// Only used when `releaseNotesSource = filePath`.
    pub fn release_notes_file_path(mut self, value: impl Into<String>) -> Self {
        self.release_notes_file_path = Some(value.into());
        self
    }

    /// `releaseNotesInline` — inline release notes content.
    /// Only used when `releaseNotesSource = inline`.
    pub fn release_notes_inline(mut self, value: impl Into<String>) -> Self {
        self.release_notes_inline = Some(value.into());
        self
    }

    /// `assets` — glob pattern(s) for files to attach to the release.
    /// Defaults to `$(Build.ArtifactStagingDirectory)/*`.
    pub fn assets(mut self, value: impl Into<String>) -> Self {
        self.assets = Some(value.into());
        self
    }

    /// `isDraft` — create the release as a draft (not publicly visible).
    pub fn draft(mut self, value: bool) -> Self {
        self.is_draft = Some(value);
        self
    }

    /// `isPreRelease` — mark the release as a pre-release.
    pub fn pre_release(mut self, value: bool) -> Self {
        self.is_pre_release = Some(value);
        self
    }

    /// `makeLatest` — whether to mark this as the latest GitHub release.
    pub fn make_latest(mut self, value: MakeLatest) -> Self {
        self.make_latest = Some(value);
        self
    }

    /// `addChangeLog` — append an auto-generated changelog to the release notes.
    pub fn add_change_log(mut self, value: bool) -> Self {
        self.add_change_log = Some(value);
        self
    }

    /// `changeLogCompareToRelease` — which previous release to diff against.
    pub fn change_log_compare_to_release(mut self, value: ChangeLogCompareToRelease) -> Self {
        self.change_log_compare_to_release = Some(value);
        self
    }

    /// `changeLogCompareToReleaseTag` — tag of the baseline release.
    /// Only used when `changeLogCompareToRelease = lastNonDraftReleaseByTag`.
    pub fn change_log_compare_to_release_tag(mut self, value: impl Into<String>) -> Self {
        self.change_log_compare_to_release_tag = Some(value.into());
        self
    }

    /// `changeLogType` — format of the generated changelog entries.
    pub fn change_log_type(mut self, value: ChangeLogType) -> Self {
        self.change_log_type = Some(value);
        self
    }

    /// `changeLogLabels` — JSON array of label→category mappings.
    /// Only used when `changeLogType = issueBased`.
    pub fn change_log_labels(mut self, value: impl Into<String>) -> Self {
        self.change_log_labels = Some(value.into());
        self
    }
}

/// Required and optional inputs for `GitHubRelease@1` `action: edit`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitHubReleaseEdit {
    /// `tag` — tag identifying the release to edit. Required.
    #[serde(rename = "tag")]
    tag: String,
    #[serde(default)]
    target: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(rename = "releaseNotesSource", default)]
    release_notes_source: Option<ReleaseNotesSource>,
    #[serde(rename = "releaseNotesFilePath", default)]
    release_notes_file_path: Option<String>,
    #[serde(rename = "releaseNotesInline", default)]
    release_notes_inline: Option<String>,
    #[serde(default)]
    assets: Option<String>,
    #[serde(rename = "assetUploadMode", default)]
    asset_upload_mode: Option<AssetUploadMode>,
    #[serde(rename = "isDraft", default, deserialize_with = "de_opt_bool_flex")]
    is_draft: Option<bool>,
    #[serde(
        rename = "isPreRelease",
        default,
        deserialize_with = "de_opt_bool_flex"
    )]
    is_pre_release: Option<bool>,
    #[serde(rename = "makeLatest", default)]
    make_latest: Option<MakeLatest>,
    #[serde(
        rename = "addChangeLog",
        default,
        deserialize_with = "de_opt_bool_flex"
    )]
    add_change_log: Option<bool>,
    #[serde(rename = "changeLogCompareToRelease", default)]
    change_log_compare_to_release: Option<ChangeLogCompareToRelease>,
    #[serde(rename = "changeLogCompareToReleaseTag", default)]
    change_log_compare_to_release_tag: Option<String>,
    #[serde(rename = "changeLogType", default)]
    change_log_type: Option<ChangeLogType>,
    #[serde(rename = "changeLogLabels", default)]
    change_log_labels: Option<String>,
}

impl GitHubReleaseEdit {
    /// `tag` is the only required input for `action: edit`.
    pub fn new(tag: impl Into<String>) -> Self {
        Self {
            tag: tag.into(),
            target: None,
            title: None,
            release_notes_source: None,
            release_notes_file_path: None,
            release_notes_inline: None,
            assets: None,
            asset_upload_mode: None,
            is_draft: None,
            is_pre_release: None,
            make_latest: None,
            add_change_log: None,
            change_log_compare_to_release: None,
            change_log_compare_to_release_tag: None,
            change_log_type: None,
            change_log_labels: None,
        }
    }

    /// `target` — commit SHA or branch the tag points to. Defaults to
    /// `$(Build.SourceVersion)`.
    pub fn target(mut self, value: impl Into<String>) -> Self {
        self.target = Some(value.into());
        self
    }

    /// `title` — updated release title.
    pub fn title(mut self, value: impl Into<String>) -> Self {
        self.title = Some(value.into());
        self
    }

    /// `releaseNotesSource` — where to read the updated release notes from.
    pub fn release_notes_source(mut self, value: ReleaseNotesSource) -> Self {
        self.release_notes_source = Some(value);
        self
    }

    /// `releaseNotesFilePath` — path to the release notes file.
    /// Only used when `releaseNotesSource = filePath`.
    pub fn release_notes_file_path(mut self, value: impl Into<String>) -> Self {
        self.release_notes_file_path = Some(value.into());
        self
    }

    /// `releaseNotesInline` — inline release notes content.
    /// Only used when `releaseNotesSource = inline`.
    pub fn release_notes_inline(mut self, value: impl Into<String>) -> Self {
        self.release_notes_inline = Some(value.into());
        self
    }

    /// `assets` — glob pattern(s) for files to attach.
    /// Defaults to `$(Build.ArtifactStagingDirectory)/*`.
    pub fn assets(mut self, value: impl Into<String>) -> Self {
        self.assets = Some(value.into());
        self
    }

    /// `assetUploadMode` — how to handle existing assets: delete all then
    /// re-upload (`Delete`, the default) or replace matching files (`Replace`).
    pub fn asset_upload_mode(mut self, value: AssetUploadMode) -> Self {
        self.asset_upload_mode = Some(value);
        self
    }

    /// `isDraft` — update the draft status of the release.
    pub fn draft(mut self, value: bool) -> Self {
        self.is_draft = Some(value);
        self
    }

    /// `isPreRelease` — update the pre-release flag.
    pub fn pre_release(mut self, value: bool) -> Self {
        self.is_pre_release = Some(value);
        self
    }

    /// `makeLatest` — whether to mark this as the latest GitHub release.
    pub fn make_latest(mut self, value: MakeLatest) -> Self {
        self.make_latest = Some(value);
        self
    }

    /// `addChangeLog` — append an auto-generated changelog.
    pub fn add_change_log(mut self, value: bool) -> Self {
        self.add_change_log = Some(value);
        self
    }

    /// `changeLogCompareToRelease` — which prior release to diff against.
    pub fn change_log_compare_to_release(mut self, value: ChangeLogCompareToRelease) -> Self {
        self.change_log_compare_to_release = Some(value);
        self
    }

    /// `changeLogCompareToReleaseTag` — tag of the baseline release.
    /// Only used when `changeLogCompareToRelease = lastNonDraftReleaseByTag`.
    pub fn change_log_compare_to_release_tag(mut self, value: impl Into<String>) -> Self {
        self.change_log_compare_to_release_tag = Some(value.into());
        self
    }

    /// `changeLogType` — format of the generated changelog entries.
    pub fn change_log_type(mut self, value: ChangeLogType) -> Self {
        self.change_log_type = Some(value);
        self
    }

    /// `changeLogLabels` — JSON array of label→category mappings.
    /// Only used when `changeLogType = issueBased`.
    pub fn change_log_labels(mut self, value: impl Into<String>) -> Self {
        self.change_log_labels = Some(value.into());
        self
    }
}

/// Required inputs for `GitHubRelease@1` `action: delete`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitHubReleaseDelete {
    /// `tag` — tag identifying the release to delete. Required.
    #[serde(rename = "tag")]
    tag: String,
}

impl GitHubReleaseDelete {
    /// `tag` is the only required input for `action: delete`.
    pub fn new(tag: impl Into<String>) -> Self {
        Self { tag: tag.into() }
    }
}

// ─── Action enum ──────────────────────────────────────────────────────────────

/// `GitHubRelease@1` action selector with per-action data.
#[derive(Debug, Clone)]
pub enum GitHubReleaseAction {
    Create(GitHubReleaseCreate),
    Edit(GitHubReleaseEdit),
    Delete(GitHubReleaseDelete),
}

// ─── Outer builder ────────────────────────────────────────────────────────────

/// Builder for a [`TaskStep`] invoking `GitHubRelease@1`.
///
/// The task creates, edits, or deletes a GitHub release. `gitHubConnection` and
/// `repositoryName` are required for all three actions; per-action inputs live
/// inside [`GitHubReleaseAction`] variants so invalid combinations are
/// unrepresentable.
///
/// # Examples
///
/// ```rust,ignore
/// use tasks::github_release::{GitHubRelease, GitHubReleaseCreate, TagSource};
///
/// // Create a release from a user-specified tag
/// let step = GitHubRelease::create(
///     "myGitHubServiceConnection",
///     "$(Build.Repository.Name)",
///     GitHubReleaseCreate::new()
///         .tag_source(TagSource::UserSpecifiedTag)
///         .tag("$(Build.BuildNumber)")
///         .assets("$(Build.ArtifactStagingDirectory)/*.tar.gz"),
/// )
/// .with_display_name("Publish GitHub Release")
/// .into_step();
/// ```
#[derive(Debug, Clone)]
pub struct GitHubRelease {
    git_hub_connection: String,
    repository_name: String,
    action: GitHubReleaseAction,
    display_name: Option<String>,
}

impl GitHubRelease {
    /// Construct from an explicit [`GitHubReleaseAction`].
    pub fn new(
        git_hub_connection: impl Into<String>,
        repository_name: impl Into<String>,
        action: GitHubReleaseAction,
    ) -> Self {
        Self {
            git_hub_connection: git_hub_connection.into(),
            repository_name: repository_name.into(),
            action,
            display_name: None,
        }
    }

    /// `action: create` — create a new release.
    pub fn create(
        git_hub_connection: impl Into<String>,
        repository_name: impl Into<String>,
        spec: GitHubReleaseCreate,
    ) -> Self {
        Self::new(
            git_hub_connection,
            repository_name,
            GitHubReleaseAction::Create(spec),
        )
    }

    /// `action: edit` — update an existing release.
    pub fn edit(
        git_hub_connection: impl Into<String>,
        repository_name: impl Into<String>,
        spec: GitHubReleaseEdit,
    ) -> Self {
        Self::new(
            git_hub_connection,
            repository_name,
            GitHubReleaseAction::Edit(spec),
        )
    }

    /// `action: delete` — delete an existing release.
    pub fn delete(
        git_hub_connection: impl Into<String>,
        repository_name: impl Into<String>,
        spec: GitHubReleaseDelete,
    ) -> Self {
        Self::new(
            git_hub_connection,
            repository_name,
            GitHubReleaseAction::Delete(spec),
        )
    }

    /// Override the default per-action `displayName`.
    pub fn with_display_name(mut self, value: impl Into<String>) -> Self {
        self.display_name = Some(value.into());
        self
    }

    /// Lower into a [`TaskStep`].
    pub fn into_step(self) -> TaskStep {
        let (action_str, default_display): (&str, &str) = match &self.action {
            GitHubReleaseAction::Create(_) => ("create", "Create GitHub Release"),
            GitHubReleaseAction::Edit(_) => ("edit", "Edit GitHub Release"),
            GitHubReleaseAction::Delete(_) => ("delete", "Delete GitHub Release"),
        };
        let mut t = TaskStep::new(
            "GitHubRelease@1",
            self.display_name.unwrap_or_else(|| default_display.into()),
        )
        .with_input("gitHubConnection", self.git_hub_connection)
        .with_input("repositoryName", self.repository_name)
        .with_input("action", action_str);

        match self.action {
            GitHubReleaseAction::Create(s) => {
                push_opt(&mut t, "target", s.target);
                push_opt(
                    &mut t,
                    "tagSource",
                    s.tag_source.map(|v| v.as_ado_str().to_string()),
                );
                push_opt(&mut t, "tagPattern", s.tag_pattern);
                push_opt(&mut t, "tag", s.tag);
                push_opt(&mut t, "title", s.title);
                push_opt(
                    &mut t,
                    "releaseNotesSource",
                    s.release_notes_source.map(|v| v.as_ado_str().to_string()),
                );
                push_opt(&mut t, "releaseNotesFilePath", s.release_notes_file_path);
                push_opt(&mut t, "releaseNotesInline", s.release_notes_inline);
                push_opt(&mut t, "assets", s.assets);
                push_bool(&mut t, "isDraft", s.is_draft);
                push_bool(&mut t, "isPreRelease", s.is_pre_release);
                push_opt(
                    &mut t,
                    "makeLatest",
                    s.make_latest.map(|v| v.as_ado_str().to_string()),
                );
                push_bool(&mut t, "addChangeLog", s.add_change_log);
                push_opt(
                    &mut t,
                    "changeLogCompareToRelease",
                    s.change_log_compare_to_release
                        .map(|v| v.as_ado_str().to_string()),
                );
                push_opt(
                    &mut t,
                    "changeLogCompareToReleaseTag",
                    s.change_log_compare_to_release_tag,
                );
                push_opt(
                    &mut t,
                    "changeLogType",
                    s.change_log_type.map(|v| v.as_ado_str().to_string()),
                );
                push_opt(&mut t, "changeLogLabels", s.change_log_labels);
            }
            GitHubReleaseAction::Edit(s) => {
                t = t.with_input("tag", s.tag);
                push_opt(&mut t, "target", s.target);
                push_opt(&mut t, "title", s.title);
                push_opt(
                    &mut t,
                    "releaseNotesSource",
                    s.release_notes_source.map(|v| v.as_ado_str().to_string()),
                );
                push_opt(&mut t, "releaseNotesFilePath", s.release_notes_file_path);
                push_opt(&mut t, "releaseNotesInline", s.release_notes_inline);
                push_opt(&mut t, "assets", s.assets);
                push_opt(
                    &mut t,
                    "assetUploadMode",
                    s.asset_upload_mode.map(|v| v.as_ado_str().to_string()),
                );
                push_bool(&mut t, "isDraft", s.is_draft);
                push_bool(&mut t, "isPreRelease", s.is_pre_release);
                push_opt(
                    &mut t,
                    "makeLatest",
                    s.make_latest.map(|v| v.as_ado_str().to_string()),
                );
                push_bool(&mut t, "addChangeLog", s.add_change_log);
                push_opt(
                    &mut t,
                    "changeLogCompareToRelease",
                    s.change_log_compare_to_release
                        .map(|v| v.as_ado_str().to_string()),
                );
                push_opt(
                    &mut t,
                    "changeLogCompareToReleaseTag",
                    s.change_log_compare_to_release_tag,
                );
                push_opt(
                    &mut t,
                    "changeLogType",
                    s.change_log_type.map(|v| v.as_ado_str().to_string()),
                );
                push_opt(&mut t, "changeLogLabels", s.change_log_labels);
            }
            GitHubReleaseAction::Delete(s) => {
                t = t.with_input("tag", s.tag);
            }
        }
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_defaults_emits_required_inputs_only() {
        let t = GitHubRelease::create(
            "myServiceConnection",
            "$(Build.Repository.Name)",
            GitHubReleaseCreate::new(),
        )
        .into_step();
        assert_eq!(t.task, "GitHubRelease@1");
        assert_eq!(t.display_name, "Create GitHub Release");
        assert_eq!(
            t.inputs.get("gitHubConnection").map(String::as_str),
            Some("myServiceConnection")
        );
        assert_eq!(
            t.inputs.get("repositoryName").map(String::as_str),
            Some("$(Build.Repository.Name)")
        );
        assert_eq!(t.inputs.get("action").map(String::as_str), Some("create"));
        // Optional inputs absent when not set
        assert!(t.inputs.get("tagSource").is_none());
        assert!(t.inputs.get("tag").is_none());
    }

    #[test]
    fn create_with_user_specified_tag() {
        let t = GitHubRelease::create(
            "myServiceConnection",
            "myorg/myrepo",
            GitHubReleaseCreate::new()
                .tag_source(TagSource::UserSpecifiedTag)
                .tag("v1.2.3")
                .title("Release 1.2.3")
                .release_notes_source(ReleaseNotesSource::Inline)
                .release_notes_inline("Bug fixes and improvements.")
                .assets("$(Build.ArtifactStagingDirectory)/*.tar.gz")
                .pre_release(false)
                .make_latest(MakeLatest::True),
        )
        .with_display_name("Publish Release")
        .into_step();
        assert_eq!(t.display_name, "Publish Release");
        assert_eq!(t.inputs.get("action").map(String::as_str), Some("create"));
        assert_eq!(
            t.inputs.get("tagSource").map(String::as_str),
            Some("userSpecifiedTag")
        );
        assert_eq!(t.inputs.get("tag").map(String::as_str), Some("v1.2.3"));
        assert_eq!(
            t.inputs.get("title").map(String::as_str),
            Some("Release 1.2.3")
        );
        assert_eq!(
            t.inputs.get("releaseNotesSource").map(String::as_str),
            Some("inline")
        );
        assert_eq!(
            t.inputs.get("releaseNotesInline").map(String::as_str),
            Some("Bug fixes and improvements.")
        );
        assert_eq!(
            t.inputs.get("assets").map(String::as_str),
            Some("$(Build.ArtifactStagingDirectory)/*.tar.gz")
        );
        assert_eq!(
            t.inputs.get("isPreRelease").map(String::as_str),
            Some("false")
        );
        assert_eq!(t.inputs.get("makeLatest").map(String::as_str), Some("true"));
    }

    #[test]
    fn create_git_tag_source_with_pattern() {
        let t = GitHubRelease::create(
            "conn",
            "org/repo",
            GitHubReleaseCreate::new()
                .tag_source(TagSource::GitTag)
                .tag_pattern(r"^v\d+\.\d+\.\d+$"),
        )
        .into_step();
        assert_eq!(
            t.inputs.get("tagSource").map(String::as_str),
            Some("gitTag")
        );
        assert_eq!(
            t.inputs.get("tagPattern").map(String::as_str),
            Some(r"^v\d+\.\d+\.\d+$")
        );
    }

    #[test]
    fn create_with_changelog_options() {
        let t = GitHubRelease::create(
            "conn",
            "org/repo",
            GitHubReleaseCreate::new()
                .add_change_log(true)
                .change_log_compare_to_release(ChangeLogCompareToRelease::NonDraftReleaseByTag)
                .change_log_compare_to_release_tag("v1.0.0")
                .change_log_type(ChangeLogType::IssueBased)
                .change_log_labels(r#"[{"label":"bug","displayName":"Bugs","state":"closed"}]"#),
        )
        .into_step();
        assert_eq!(
            t.inputs.get("addChangeLog").map(String::as_str),
            Some("true")
        );
        assert_eq!(
            t.inputs
                .get("changeLogCompareToRelease")
                .map(String::as_str),
            Some("lastNonDraftReleaseByTag")
        );
        assert_eq!(
            t.inputs
                .get("changeLogCompareToReleaseTag")
                .map(String::as_str),
            Some("v1.0.0")
        );
        assert_eq!(
            t.inputs.get("changeLogType").map(String::as_str),
            Some("issueBased")
        );
    }

    #[test]
    fn edit_emits_required_tag() {
        let t = GitHubRelease::edit(
            "conn",
            "org/repo",
            GitHubReleaseEdit::new("v2.0.0").draft(false),
        )
        .into_step();
        assert_eq!(t.task, "GitHubRelease@1");
        assert_eq!(t.display_name, "Edit GitHub Release");
        assert_eq!(t.inputs.get("action").map(String::as_str), Some("edit"));
        assert_eq!(t.inputs.get("tag").map(String::as_str), Some("v2.0.0"));
        assert_eq!(t.inputs.get("isDraft").map(String::as_str), Some("false"));
    }

    #[test]
    fn edit_with_asset_replace_mode() {
        let t = GitHubRelease::edit(
            "conn",
            "org/repo",
            GitHubReleaseEdit::new("v2.0.0")
                .assets("$(Build.ArtifactStagingDirectory)/*.zip")
                .asset_upload_mode(AssetUploadMode::Replace),
        )
        .into_step();
        assert_eq!(
            t.inputs.get("assetUploadMode").map(String::as_str),
            Some("replace")
        );
        // tag_source should NOT be emitted for edit
        assert!(t.inputs.get("tagSource").is_none());
    }

    #[test]
    fn delete_emits_required_tag_only() {
        let t = GitHubRelease::delete("conn", "org/repo", GitHubReleaseDelete::new("v0.1.0-rc.1"))
            .into_step();
        assert_eq!(t.task, "GitHubRelease@1");
        assert_eq!(t.display_name, "Delete GitHub Release");
        assert_eq!(t.inputs.get("action").map(String::as_str), Some("delete"));
        assert_eq!(t.inputs.get("tag").map(String::as_str), Some("v0.1.0-rc.1"));
        // No optional inputs
        assert!(t.inputs.get("title").is_none());
        assert!(t.inputs.get("assets").is_none());
    }

    #[test]
    fn make_latest_legacy_variant() {
        let t = GitHubRelease::create(
            "conn",
            "org/repo",
            GitHubReleaseCreate::new().make_latest(MakeLatest::Legacy),
        )
        .into_step();
        assert_eq!(
            t.inputs.get("makeLatest").map(String::as_str),
            Some("legacy")
        );
    }
}
