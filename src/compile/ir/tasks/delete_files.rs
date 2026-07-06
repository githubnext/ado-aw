//! Typed builder for `DeleteFiles@1`.

use super::common::{bool_input, de_opt_bool_flex};
use crate::compile::ir::step::TaskStep;
use serde::Deserialize;

/// Builder for a [`TaskStep`] invoking `DeleteFiles@1`.
///
/// Deletes files or folders matching `contents` from a source folder.
///
/// ADO task reference:
/// <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/delete-files-v1>
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeleteFiles {
    #[serde(rename = "Contents")]
    contents: String,
    #[serde(rename = "SourceFolder", default)]
    source_folder: Option<String>,
    #[serde(
        rename = "RemoveSourceFolder",
        default,
        deserialize_with = "de_opt_bool_flex"
    )]
    remove_source_folder: Option<bool>,
    #[serde(
        rename = "RemoveDotFiles",
        default,
        deserialize_with = "de_opt_bool_flex"
    )]
    remove_dot_files: Option<bool>,
    #[serde(skip)]
    display_name: Option<String>,
}

impl DeleteFiles {
    /// Required input: `Contents` — newline-separated glob patterns.
    pub fn new(contents: impl Into<String>) -> Self {
        Self {
            contents: contents.into(),
            source_folder: None,
            remove_source_folder: None,
            remove_dot_files: None,
            display_name: None,
        }
    }

    /// `SourceFolder` — root folder to delete from.
    pub fn source_folder(mut self, value: impl Into<String>) -> Self {
        self.source_folder = Some(value.into());
        self
    }

    /// `RemoveSourceFolder` — remove the source folder itself after deletion.
    pub fn remove_source_folder(mut self, value: bool) -> Self {
        self.remove_source_folder = Some(value);
        self
    }

    /// `RemoveDotFiles` — also delete files whose name starts with a dot.
    pub fn remove_dot_files(mut self, value: bool) -> Self {
        self.remove_dot_files = Some(value);
        self
    }

    /// Override the default `displayName` (`"Delete Files"`).
    pub fn with_display_name(mut self, value: impl Into<String>) -> Self {
        self.display_name = Some(value.into());
        self
    }

    /// Lower into a [`TaskStep`].
    pub fn into_step(self) -> TaskStep {
        let mut t = TaskStep::new(
            "DeleteFiles@1",
            self.display_name.unwrap_or_else(|| "Delete Files".into()),
        )
        .with_input("Contents", self.contents);
        if let Some(v) = self.source_folder {
            t = t.with_input("SourceFolder", v);
        }
        if let Some(v) = self.remove_source_folder {
            t = t.with_input("RemoveSourceFolder", bool_input(v));
        }
        if let Some(v) = self.remove_dot_files {
            t = t.with_input("RemoveDotFiles", bool_input(v));
        }
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sets_task_and_required_contents() {
        let t = DeleteFiles::new("**/*.tmp").into_step();
        assert_eq!(t.task, "DeleteFiles@1");
        assert_eq!(
            t.inputs.get("Contents").map(String::as_str),
            Some("**/*.tmp")
        );
    }

    #[test]
    fn optional_inputs() {
        let t = DeleteFiles::new("*")
            .source_folder("$(Build.ArtifactStagingDirectory)")
            .remove_source_folder(true)
            .remove_dot_files(true)
            .into_step();
        assert_eq!(
            t.inputs.get("SourceFolder").map(String::as_str),
            Some("$(Build.ArtifactStagingDirectory)")
        );
        assert_eq!(
            t.inputs.get("RemoveSourceFolder").map(String::as_str),
            Some("true")
        );
        assert_eq!(
            t.inputs.get("RemoveDotFiles").map(String::as_str),
            Some("true")
        );
    }
}
