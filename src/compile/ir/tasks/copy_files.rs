//! Typed builder for `CopyFiles@2`.

use super::common::bool_input;
use crate::compile::ir::step::TaskStep;

/// Builder for a [`TaskStep`] invoking `CopyFiles@2`.
///
/// Copies files matching `contents` into `target_folder`. Optional inputs are
/// applied through the typed setters; only those that are set are emitted.
///
/// ADO task reference:
/// <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/copy-files-v2>
#[derive(Debug, Clone)]
pub struct CopyFiles {
    contents: String,
    target_folder: String,
    source_folder: Option<String>,
    clean_target_folder: Option<bool>,
    over_write: Option<bool>,
    flatten_folders: Option<bool>,
    preserve_timestamp: Option<bool>,
    retry_count: Option<String>,
    delay_between_retries: Option<String>,
    ignore_make_dir_errors: Option<bool>,
    display_name: Option<String>,
}

impl CopyFiles {
    /// Required inputs: `Contents` glob and `TargetFolder`.
    pub fn new(contents: impl Into<String>, target_folder: impl Into<String>) -> Self {
        Self {
            contents: contents.into(),
            target_folder: target_folder.into(),
            source_folder: None,
            clean_target_folder: None,
            over_write: None,
            flatten_folders: None,
            preserve_timestamp: None,
            retry_count: None,
            delay_between_retries: None,
            ignore_make_dir_errors: None,
            display_name: None,
        }
    }

    /// `SourceFolder` — root for glob evaluation (default `$(Build.SourcesDirectory)`).
    pub fn source_folder(mut self, value: impl Into<String>) -> Self {
        self.source_folder = Some(value.into());
        self
    }

    /// `CleanTargetFolder` — delete target folder contents before copy.
    pub fn clean_target_folder(mut self, value: bool) -> Self {
        self.clean_target_folder = Some(value);
        self
    }

    /// `OverWrite` — overwrite files in the target folder.
    pub fn over_write(mut self, value: bool) -> Self {
        self.over_write = Some(value);
        self
    }

    /// `flattenFolders` — flatten directory structure in the target.
    pub fn flatten_folders(mut self, value: bool) -> Self {
        self.flatten_folders = Some(value);
        self
    }

    /// `preserveTimestamp` — preserve source timestamps.
    pub fn preserve_timestamp(mut self, value: bool) -> Self {
        self.preserve_timestamp = Some(value);
        self
    }

    /// `retryCount` — number of retry attempts on failure.
    pub fn retry_count(mut self, value: impl Into<String>) -> Self {
        self.retry_count = Some(value.into());
        self
    }

    /// `delayBetweenRetries` — milliseconds between retries.
    pub fn delay_between_retries(mut self, value: impl Into<String>) -> Self {
        self.delay_between_retries = Some(value.into());
        self
    }

    /// `ignoreMakeDirErrors` — ignore errors when creating the target folder.
    pub fn ignore_make_dir_errors(mut self, value: bool) -> Self {
        self.ignore_make_dir_errors = Some(value);
        self
    }

    /// Override the default `displayName` (`"Copy Files"`).
    pub fn with_display_name(mut self, value: impl Into<String>) -> Self {
        self.display_name = Some(value.into());
        self
    }

    /// Lower into a [`TaskStep`].
    pub fn into_step(self) -> TaskStep {
        let mut t = TaskStep::new(
            "CopyFiles@2",
            self.display_name.unwrap_or_else(|| "Copy Files".into()),
        )
        .with_input("Contents", self.contents)
        .with_input("TargetFolder", self.target_folder);
        if let Some(v) = self.source_folder {
            t = t.with_input("SourceFolder", v);
        }
        if let Some(v) = self.clean_target_folder {
            t = t.with_input("CleanTargetFolder", bool_input(v));
        }
        if let Some(v) = self.over_write {
            t = t.with_input("OverWrite", bool_input(v));
        }
        if let Some(v) = self.flatten_folders {
            t = t.with_input("flattenFolders", bool_input(v));
        }
        if let Some(v) = self.preserve_timestamp {
            t = t.with_input("preserveTimestamp", bool_input(v));
        }
        if let Some(v) = self.retry_count {
            t = t.with_input("retryCount", v);
        }
        if let Some(v) = self.delay_between_retries {
            t = t.with_input("delayBetweenRetries", v);
        }
        if let Some(v) = self.ignore_make_dir_errors {
            t = t.with_input("ignoreMakeDirErrors", bool_input(v));
        }
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sets_task_and_required_inputs() {
        let t = CopyFiles::new("**/*.rs", "$(Build.ArtifactStagingDirectory)").into_step();
        assert_eq!(t.task, "CopyFiles@2");
        assert_eq!(t.display_name, "Copy Files");
        assert_eq!(t.inputs.get("Contents").map(String::as_str), Some("**/*.rs"));
        assert_eq!(
            t.inputs.get("TargetFolder").map(String::as_str),
            Some("$(Build.ArtifactStagingDirectory)")
        );
    }

    #[test]
    fn optional_inputs_emit_only_when_set() {
        let t = CopyFiles::new("**", "$(Build.ArtifactStagingDirectory)")
            .source_folder("$(Build.SourcesDirectory)/src")
            .clean_target_folder(true)
            .over_write(true)
            .flatten_folders(true)
            .into_step();
        assert_eq!(
            t.inputs.get("SourceFolder").map(String::as_str),
            Some("$(Build.SourcesDirectory)/src")
        );
        assert_eq!(t.inputs.get("CleanTargetFolder").map(String::as_str), Some("true"));
        assert_eq!(t.inputs.get("OverWrite").map(String::as_str), Some("true"));
        assert_eq!(t.inputs.get("flattenFolders").map(String::as_str), Some("true"));
        // Untouched optionals are absent.
        assert!(t.inputs.get("preserveTimestamp").is_none());
        assert!(t.inputs.get("retryCount").is_none());
    }

    #[test]
    fn display_name_override() {
        let t = CopyFiles::new("**", "out").with_display_name("Stage build output").into_step();
        assert_eq!(t.display_name, "Stage build output");
    }
}
