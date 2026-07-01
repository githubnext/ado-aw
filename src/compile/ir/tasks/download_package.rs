//! Typed builder for `DownloadPackage@1`.

use super::common::{bool_input, de_opt_bool_flex, push_opt};
use crate::compile::ir::step::TaskStep;
use serde::Deserialize;

/// Package ecosystem for [`DownloadPackage`] (`packageType` input).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum PackageType {
    #[serde(rename = "nuget")]
    NuGet,
    #[serde(rename = "npm")]
    Npm,
    #[serde(rename = "pypi")]
    PyPi,
    #[serde(rename = "maven")]
    Maven,
    #[serde(rename = "upack")]
    UPack,
    #[serde(rename = "cargo")]
    Cargo,
}

impl PackageType {
    /// The exact token the ADO task expects.
    pub fn as_ado_str(self) -> &'static str {
        match self {
            PackageType::NuGet => "nuget",
            PackageType::Npm => "npm",
            PackageType::PyPi => "pypi",
            PackageType::Maven => "maven",
            PackageType::UPack => "upack",
            PackageType::Cargo => "cargo",
        }
    }
}

/// Builder for a [`TaskStep`] invoking `DownloadPackage@1`.
///
/// Downloads a package from an Azure Artifacts feed into `download_path`.
/// The required inputs are `package_type`, `feed`, `definition` (package
/// name), `version`, and `download_path`; optional inputs are applied through
/// typed setters and only emitted when set.
///
/// Use [`DownloadPackage::nuget`] as a convenience constructor when downloading
/// NuGet packages (the most common case in ado-aw supply-chain steps).
///
/// ADO task reference:
/// <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/download-package-v1-task>
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DownloadPackage {
    #[serde(rename = "packageType")]
    package_type: PackageType,
    feed: String,
    /// The package name (`definition` input).
    definition: String,
    version: String,
    #[serde(rename = "downloadPath")]
    download_path: String,
    #[serde(default)]
    view: Option<String>,
    #[serde(default)]
    files: Option<String>,
    #[serde(default, deserialize_with = "de_opt_bool_flex")]
    extract: Option<bool>,
    #[serde(skip)]
    display_name: Option<String>,
}

impl DownloadPackage {
    /// Required inputs: `packageType`, `feed`, package `definition`, `version`,
    /// and `downloadPath`.
    pub fn new(
        package_type: PackageType,
        feed: impl Into<String>,
        definition: impl Into<String>,
        version: impl Into<String>,
        download_path: impl Into<String>,
    ) -> Self {
        Self {
            package_type,
            feed: feed.into(),
            definition: definition.into(),
            version: version.into(),
            download_path: download_path.into(),
            view: None,
            files: None,
            extract: None,
            display_name: None,
        }
    }

    /// Convenience constructor for NuGet packages.
    pub fn nuget(
        feed: impl Into<String>,
        definition: impl Into<String>,
        version: impl Into<String>,
        download_path: impl Into<String>,
    ) -> Self {
        Self::new(PackageType::NuGet, feed, definition, version, download_path)
    }

    /// `view` — view within the feed to resolve the package from (e.g.
    /// `"Release"`, `"Prerelease"`). Omit to resolve from the feed directly.
    pub fn view(mut self, value: impl Into<String>) -> Self {
        self.view = Some(value.into());
        self
    }

    /// `files` — glob patterns selecting files to download from the package
    /// (default: `"**"` — all files).
    pub fn files(mut self, value: impl Into<String>) -> Self {
        self.files = Some(value.into());
        self
    }

    /// `extract` — whether to extract the package contents after download
    /// (default: `true`).
    pub fn extract(mut self, value: bool) -> Self {
        self.extract = Some(value);
        self
    }

    /// Override the default `displayName` (`"Download Package"`).
    pub fn with_display_name(mut self, value: impl Into<String>) -> Self {
        self.display_name = Some(value.into());
        self
    }

    /// Lower into a [`TaskStep`].
    pub fn into_step(self) -> TaskStep {
        let mut t = TaskStep::new(
            "DownloadPackage@1",
            self.display_name
                .unwrap_or_else(|| "Download Package".into()),
        )
        .with_input("packageType", self.package_type.as_ado_str())
        .with_input("feed", self.feed)
        .with_input("definition", self.definition)
        .with_input("version", self.version)
        .with_input("downloadPath", self.download_path);
        push_opt(&mut t, "view", self.view);
        push_opt(&mut t, "files", self.files);
        if let Some(v) = self.extract {
            t = t.with_input("extract", bool_input(v));
        }
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nuget_convenience_sets_required_inputs() {
        let t = DownloadPackage::nuget(
            "my-feed",
            "my-package",
            "1.2.3",
            "$(System.ArtifactsDirectory)",
        )
        .into_step();
        assert_eq!(t.task, "DownloadPackage@1");
        assert_eq!(t.display_name, "Download Package");
        assert_eq!(
            t.inputs.get("packageType").map(String::as_str),
            Some("nuget")
        );
        assert_eq!(t.inputs.get("feed").map(String::as_str), Some("my-feed"));
        assert_eq!(
            t.inputs.get("definition").map(String::as_str),
            Some("my-package")
        );
        assert_eq!(t.inputs.get("version").map(String::as_str), Some("1.2.3"));
        assert_eq!(
            t.inputs.get("downloadPath").map(String::as_str),
            Some("$(System.ArtifactsDirectory)")
        );
    }

    #[test]
    fn optional_inputs_emit_only_when_set() {
        let t = DownloadPackage::nuget("feed", "pkg", "2.0.0", "$(Pipeline.Workspace)/out")
            .view("Release")
            .files("**/*.dll")
            .extract(false)
            .into_step();
        assert_eq!(t.inputs.get("view").map(String::as_str), Some("Release"));
        assert_eq!(t.inputs.get("files").map(String::as_str), Some("**/*.dll"));
        assert_eq!(t.inputs.get("extract").map(String::as_str), Some("false"));
    }

    #[test]
    fn optional_inputs_absent_when_not_set() {
        let t = DownloadPackage::nuget("feed", "pkg", "1.0.0", "/tmp/out").into_step();
        assert!(t.inputs.get("view").is_none());
        assert!(t.inputs.get("files").is_none());
        assert!(t.inputs.get("extract").is_none());
    }

    #[test]
    fn display_name_override() {
        let t = DownloadPackage::nuget("feed", "pkg", "1.0.0", "/tmp/out")
            .with_display_name("Download my-package v1.0.0")
            .into_step();
        assert_eq!(t.display_name, "Download my-package v1.0.0");
    }

    #[test]
    fn all_package_types_round_trip() {
        let cases = [
            (PackageType::NuGet, "nuget"),
            (PackageType::Npm, "npm"),
            (PackageType::PyPi, "pypi"),
            (PackageType::Maven, "maven"),
            (PackageType::UPack, "upack"),
            (PackageType::Cargo, "cargo"),
        ];
        for (pt, expected) in cases {
            let t = DownloadPackage::new(pt, "feed", "pkg", "1.0.0", "/out").into_step();
            assert_eq!(
                t.inputs.get("packageType").map(String::as_str),
                Some(expected)
            );
        }
    }
}
