//! Typed builder for `ArchiveFiles@2`.

use super::common::{bool_input, de_opt_bool_flex};
use crate::compile::ir::step::TaskStep;
use serde::Deserialize;

/// Archive format for [`ArchiveFiles`] (`archiveType` input).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum ArchiveType {
    #[serde(rename = "zip")]
    Zip,
    #[serde(rename = "7z")]
    SevenZip,
    #[serde(rename = "tar")]
    Tar,
    #[serde(rename = "wim")]
    Wim,
}

impl ArchiveType {
    /// The exact token the ADO task expects.
    pub fn as_ado_str(self) -> &'static str {
        match self {
            ArchiveType::Zip => "zip",
            ArchiveType::SevenZip => "7z",
            ArchiveType::Tar => "tar",
            ArchiveType::Wim => "wim",
        }
    }
}

/// Tar compression for [`ArchiveFiles`] (`tarCompression` input, only when
/// `archiveType = tar`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum TarCompression {
    #[serde(rename = "gz")]
    Gz,
    #[serde(rename = "bz2")]
    Bz2,
    #[serde(rename = "xz")]
    Xz,
    #[serde(rename = "none")]
    None,
}

impl TarCompression {
    /// The exact token the ADO task expects.
    pub fn as_ado_str(self) -> &'static str {
        match self {
            TarCompression::Gz => "gz",
            TarCompression::Bz2 => "bz2",
            TarCompression::Xz => "xz",
            TarCompression::None => "none",
        }
    }
}

/// 7-Zip compression level for [`ArchiveFiles`] (`sevenZipCompression` input,
/// only when `archiveType = 7z`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum SevenZipCompression {
    #[serde(rename = "ultra")]
    Ultra,
    #[serde(rename = "maximum")]
    Maximum,
    #[serde(rename = "normal")]
    Normal,
    #[serde(rename = "fast")]
    Fast,
    #[serde(rename = "fastest")]
    Fastest,
    #[serde(rename = "none")]
    None,
}

impl SevenZipCompression {
    /// The exact token the ADO task expects.
    pub fn as_ado_str(self) -> &'static str {
        match self {
            SevenZipCompression::Ultra => "ultra",
            SevenZipCompression::Maximum => "maximum",
            SevenZipCompression::Normal => "normal",
            SevenZipCompression::Fast => "fast",
            SevenZipCompression::Fastest => "fastest",
            SevenZipCompression::None => "none",
        }
    }
}

/// Builder for a [`TaskStep`] invoking `ArchiveFiles@2`.
///
/// Creates an archive from `root_folder_or_file` and writes it to `archive_file`.
///
/// ADO task reference:
/// <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/archive-files-v2>
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchiveFiles {
    #[serde(rename = "rootFolderOrFile")]
    root_folder_or_file: String,
    #[serde(rename = "archiveFile")]
    archive_file: String,
    #[serde(rename = "archiveType", default)]
    archive_type: Option<ArchiveType>,
    #[serde(
        rename = "includeRootFolder",
        default,
        deserialize_with = "de_opt_bool_flex"
    )]
    include_root_folder: Option<bool>,
    #[serde(
        rename = "replaceExistingArchive",
        default,
        deserialize_with = "de_opt_bool_flex"
    )]
    replace_existing_archive: Option<bool>,
    #[serde(rename = "sevenZipCompression", default)]
    seven_zip_compression: Option<SevenZipCompression>,
    #[serde(rename = "tarCompression", default)]
    tar_compression: Option<TarCompression>,
    #[serde(default, deserialize_with = "de_opt_bool_flex")]
    verbose: Option<bool>,
    #[serde(default, deserialize_with = "de_opt_bool_flex")]
    quiet: Option<bool>,
    #[serde(skip)]
    display_name: Option<String>,
}

impl ArchiveFiles {
    /// Required inputs: `rootFolderOrFile` and `archiveFile`.
    pub fn new(root_folder_or_file: impl Into<String>, archive_file: impl Into<String>) -> Self {
        Self {
            root_folder_or_file: root_folder_or_file.into(),
            archive_file: archive_file.into(),
            archive_type: None,
            include_root_folder: None,
            replace_existing_archive: None,
            seven_zip_compression: None,
            tar_compression: None,
            verbose: None,
            quiet: None,
            display_name: None,
        }
    }

    /// `archiveType` — archive format (default `zip`).
    pub fn archive_type(mut self, value: ArchiveType) -> Self {
        self.archive_type = Some(value);
        self
    }

    /// `includeRootFolder` — prepend the root folder name to archive paths.
    pub fn include_root_folder(mut self, value: bool) -> Self {
        self.include_root_folder = Some(value);
        self
    }

    /// `replaceExistingArchive` — replace an existing archive.
    pub fn replace_existing_archive(mut self, value: bool) -> Self {
        self.replace_existing_archive = Some(value);
        self
    }

    /// `sevenZipCompression` — 7z compression level (when `archiveType = 7z`).
    pub fn seven_zip_compression(mut self, value: SevenZipCompression) -> Self {
        self.seven_zip_compression = Some(value);
        self
    }

    /// `tarCompression` — tar compression (when `archiveType = tar`).
    pub fn tar_compression(mut self, value: TarCompression) -> Self {
        self.tar_compression = Some(value);
        self
    }

    /// `verbose` — force verbose output.
    pub fn verbose(mut self, value: bool) -> Self {
        self.verbose = Some(value);
        self
    }

    /// `quiet` — force quiet output.
    pub fn quiet(mut self, value: bool) -> Self {
        self.quiet = Some(value);
        self
    }

    /// Override the default `displayName` (`"Archive Files"`).
    pub fn with_display_name(mut self, value: impl Into<String>) -> Self {
        self.display_name = Some(value.into());
        self
    }

    /// Lower into a [`TaskStep`].
    pub fn into_step(self) -> TaskStep {
        let mut t = TaskStep::new(
            "ArchiveFiles@2",
            self.display_name.unwrap_or_else(|| "Archive Files".into()),
        )
        .with_input("rootFolderOrFile", self.root_folder_or_file)
        .with_input("archiveFile", self.archive_file);
        if let Some(v) = self.archive_type {
            t = t.with_input("archiveType", v.as_ado_str());
        }
        if let Some(v) = self.include_root_folder {
            t = t.with_input("includeRootFolder", bool_input(v));
        }
        if let Some(v) = self.replace_existing_archive {
            t = t.with_input("replaceExistingArchive", bool_input(v));
        }
        if let Some(v) = self.seven_zip_compression {
            t = t.with_input("sevenZipCompression", v.as_ado_str());
        }
        if let Some(v) = self.tar_compression {
            t = t.with_input("tarCompression", v.as_ado_str());
        }
        if let Some(v) = self.verbose {
            t = t.with_input("verbose", bool_input(v));
        }
        if let Some(v) = self.quiet {
            t = t.with_input("quiet", bool_input(v));
        }
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sets_task_and_required_inputs() {
        let t = ArchiveFiles::new(
            "$(Build.ArtifactStagingDirectory)",
            "$(Build.ArtifactStagingDirectory)/drop.zip",
        )
        .into_step();
        assert_eq!(t.task, "ArchiveFiles@2");
        assert_eq!(
            t.inputs.get("rootFolderOrFile").map(String::as_str),
            Some("$(Build.ArtifactStagingDirectory)")
        );
        assert_eq!(
            t.inputs.get("archiveFile").map(String::as_str),
            Some("$(Build.ArtifactStagingDirectory)/drop.zip")
        );
    }

    #[test]
    fn typed_archive_and_tar_compression() {
        let t = ArchiveFiles::new("src", "out.tar.gz")
            .archive_type(ArchiveType::Tar)
            .tar_compression(TarCompression::Gz)
            .include_root_folder(false)
            .into_step();
        assert_eq!(t.inputs.get("archiveType").map(String::as_str), Some("tar"));
        assert_eq!(
            t.inputs.get("tarCompression").map(String::as_str),
            Some("gz")
        );
        assert_eq!(
            t.inputs.get("includeRootFolder").map(String::as_str),
            Some("false")
        );
    }

    #[test]
    fn seven_zip_token() {
        let t = ArchiveFiles::new("src", "out.7z")
            .archive_type(ArchiveType::SevenZip)
            .seven_zip_compression(SevenZipCompression::Ultra)
            .into_step();
        assert_eq!(t.inputs.get("archiveType").map(String::as_str), Some("7z"));
        assert_eq!(
            t.inputs.get("sevenZipCompression").map(String::as_str),
            Some("ultra")
        );
    }
}
