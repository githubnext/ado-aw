//! Typed builder for `ExtractFiles@1`.

use super::common::{bool_input, de_opt_bool_flex};
use crate::compile::ir::step::TaskStep;
use serde::Deserialize;

/// Builder for a [`TaskStep`] invoking `ExtractFiles@1`.
///
/// Extracts archives matching `archive_file_patterns` into `destination_folder`.
///
/// ADO task reference:
/// <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/extract-files-v1>
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtractFiles {
    #[serde(rename = "archiveFilePatterns")]
    archive_file_patterns: String,
    #[serde(rename = "destinationFolder")]
    destination_folder: String,
    #[serde(
        rename = "cleanDestinationFolder",
        default,
        deserialize_with = "de_opt_bool_flex"
    )]
    clean_destination_folder: Option<bool>,
    #[serde(
        rename = "overwriteExistingFiles",
        default,
        deserialize_with = "de_opt_bool_flex"
    )]
    overwrite_existing_files: Option<bool>,
    #[serde(rename = "pathToSevenZipTool", default)]
    path_to_seven_zip_tool: Option<String>,
    #[serde(skip)]
    display_name: Option<String>,
}

impl ExtractFiles {
    /// Required inputs: `archiveFilePatterns` glob and `destinationFolder`.
    pub fn new(
        archive_file_patterns: impl Into<String>,
        destination_folder: impl Into<String>,
    ) -> Self {
        Self {
            archive_file_patterns: archive_file_patterns.into(),
            destination_folder: destination_folder.into(),
            clean_destination_folder: None,
            overwrite_existing_files: None,
            path_to_seven_zip_tool: None,
            display_name: None,
        }
    }

    /// `cleanDestinationFolder` — delete destination contents before extracting.
    pub fn clean_destination_folder(mut self, value: bool) -> Self {
        self.clean_destination_folder = Some(value);
        self
    }

    /// `overwriteExistingFiles` — overwrite files already in the destination.
    pub fn overwrite_existing_files(mut self, value: bool) -> Self {
        self.overwrite_existing_files = Some(value);
        self
    }

    /// `pathToSevenZipTool` — absolute path to a custom `7z` binary.
    pub fn path_to_seven_zip_tool(mut self, value: impl Into<String>) -> Self {
        self.path_to_seven_zip_tool = Some(value.into());
        self
    }

    /// Override the default `displayName` (`"Extract Files"`).
    pub fn with_display_name(mut self, value: impl Into<String>) -> Self {
        self.display_name = Some(value.into());
        self
    }

    /// Lower into a [`TaskStep`].
    pub fn into_step(self) -> TaskStep {
        let mut t = TaskStep::new(
            "ExtractFiles@1",
            self.display_name.unwrap_or_else(|| "Extract Files".into()),
        )
        .with_input("archiveFilePatterns", self.archive_file_patterns)
        .with_input("destinationFolder", self.destination_folder);
        if let Some(v) = self.clean_destination_folder {
            t = t.with_input("cleanDestinationFolder", bool_input(v));
        }
        if let Some(v) = self.overwrite_existing_files {
            t = t.with_input("overwriteExistingFiles", bool_input(v));
        }
        if let Some(v) = self.path_to_seven_zip_tool {
            t = t.with_input("pathToSevenZipTool", v);
        }
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sets_task_and_required_inputs() {
        let t = ExtractFiles::new("**/*.zip", "$(Build.SourcesDirectory)/unpacked").into_step();
        assert_eq!(t.task, "ExtractFiles@1");
        assert_eq!(t.inputs.get("archiveFilePatterns").map(String::as_str), Some("**/*.zip"));
        assert_eq!(
            t.inputs.get("destinationFolder").map(String::as_str),
            Some("$(Build.SourcesDirectory)/unpacked")
        );
    }

    #[test]
    fn optional_inputs() {
        let t = ExtractFiles::new("**/*.tar.gz", "out")
            .clean_destination_folder(false)
            .overwrite_existing_files(true)
            .path_to_seven_zip_tool("/usr/local/bin/7z")
            .into_step();
        assert_eq!(t.inputs.get("cleanDestinationFolder").map(String::as_str), Some("false"));
        assert_eq!(t.inputs.get("overwriteExistingFiles").map(String::as_str), Some("true"));
        assert_eq!(
            t.inputs.get("pathToSevenZipTool").map(String::as_str),
            Some("/usr/local/bin/7z")
        );
    }
}
