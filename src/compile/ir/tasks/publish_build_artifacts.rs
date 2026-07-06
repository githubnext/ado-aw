//! Typed builder for `PublishBuildArtifacts@1`.
//!
//! This is a location-dispatch task: the `publishLocation` input (`Container`
//! or `FilePath`) determines which optional inputs are meaningful. Because
//! file-share-only inputs (`TargetPath`, `Parallel`, `ParallelCount`,
//! `FileCopyOptions`) live inside [`FilePathLocation`], applying them when
//! `publishLocation` is `Container` is unrepresentable.
//!
//! ADO task reference:
//! <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/publish-build-artifacts-v1>

use super::common::{de_opt_bool_flex, de_opt_str_or_int, push_bool, push_opt};
use crate::compile::ir::step::TaskStep;
use serde::Deserialize;
use serde_yaml::Value;

/// Validate an authored `PublishBuildArtifacts@1` `inputs:` mapping (advisory
/// front-matter validation, see [`super::parse`]).
pub(crate) fn validate_inputs(inputs: Value) -> Result<(), String> {
    let mut map = match inputs {
        Value::Mapping(m) => m,
        Value::Null => Default::default(),
        other => return Err(format!("`inputs` must be a mapping, got {other:?}")),
    };
    let publish_location = match (map.remove("publishLocation"), map.remove("ArtifactType")) {
        (Some(primary), Some(alias)) => {
            let primary = primary
                .as_str()
                .ok_or_else(|| "`publishLocation` must be a string".to_string())?;
            let alias = alias
                .as_str()
                .ok_or_else(|| "`ArtifactType` must be a string".to_string())?;
            if primary != alias {
                return Err(format!(
                    "PublishBuildArtifacts@1: conflicting publishLocation `{primary}` and ArtifactType `{alias}`"
                ));
            }
            primary.to_string()
        }
        (Some(value), None) => value
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| "`publishLocation` must be a string".to_string())?,
        (None, Some(value)) => value
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| "`ArtifactType` must be a string".to_string())?,
        (None, None) => "Container".to_string(),
    };

    validate_common(&Value::Mapping(map.clone())).map_err(|e| format!("common inputs: {e}"))?;
    remove_common_inputs(&mut map);
    let rest = Value::Mapping(map);

    let result = match publish_location.as_str() {
        "Container" => serde_yaml::from_value::<PublishBuildArtifactsContainer>(rest).map(drop),
        "FilePath" => serde_yaml::from_value::<FilePathLocation>(rest).map(drop),
        other => {
            return Err(format!(
                "PublishBuildArtifacts@1: unknown publishLocation `{other}`"
            ))
        }
    };
    result.map_err(|e| format!("publishLocation `{publish_location}`: {e}"))
}

fn validate_common(inputs: &Value) -> Result<(), serde_yaml::Error> {
    serde_yaml::from_value::<PublishBuildArtifactsCommonInputs>(inputs.clone()).map(drop)
}

fn remove_common_inputs(map: &mut serde_yaml::Mapping) {
    for key in [
        "PathtoPublish",
        "ArtifactName",
        "MaxArtifactSize",
        "StoreAsTar",
    ] {
        map.remove(key);
    }
}

#[derive(Debug, Deserialize)]
struct PublishBuildArtifactsCommonInputs {
    #[serde(rename = "PathtoPublish")]
    _path_to_publish: String,
    #[serde(rename = "ArtifactName")]
    _artifact_name: String,
    #[serde(rename = "MaxArtifactSize", default)]
    _max_artifact_size: Option<String>,
    #[serde(rename = "StoreAsTar", default, deserialize_with = "de_opt_bool_flex")]
    _store_as_tar: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct PublishBuildArtifactsContainer {}

/// `PublishBuildArtifacts@1` `publishLocation` selector, carrying
/// per-location optional inputs.
#[derive(Debug, Clone)]
pub enum PublishLocation {
    /// Publish to Azure Pipelines artifact storage (default).
    Container,
    /// Publish to a file share; carries file-share-specific optionals.
    FilePath(FilePathLocation),
}

/// Per-location optionals for `publishLocation: FilePath`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FilePathLocation {
    /// `TargetPath` — UNC file share path. Required for the `FilePath` location.
    #[serde(rename = "TargetPath")]
    target_path: String,
    /// `Parallel` — copy files in parallel using multiple threads.
    #[serde(rename = "Parallel", default, deserialize_with = "de_opt_bool_flex")]
    parallel: Option<bool>,
    /// `ParallelCount` — number of threads for parallel copy (1–128, default 8).
    #[serde(
        rename = "ParallelCount",
        default,
        deserialize_with = "de_opt_str_or_int"
    )]
    parallel_count: Option<String>,
    /// `FileCopyOptions` — additional robocopy arguments.
    #[serde(rename = "FileCopyOptions", default)]
    file_copy_options: Option<String>,
}

impl FilePathLocation {
    /// Required: `TargetPath` — UNC path to the file share.
    pub fn new(target_path: impl Into<String>) -> Self {
        Self {
            target_path: target_path.into(),
            parallel: None,
            parallel_count: None,
            file_copy_options: None,
        }
    }

    /// `Parallel` — enable parallel copy.
    pub fn parallel(mut self, value: bool) -> Self {
        self.parallel = Some(value);
        self
    }

    /// `ParallelCount` — thread count for parallel copy (`"1"` – `"128"`, default `"8"`).
    pub fn parallel_count(mut self, value: impl Into<String>) -> Self {
        self.parallel_count = Some(value.into());
        self
    }

    /// `FileCopyOptions` — additional robocopy-style options.
    pub fn file_copy_options(mut self, value: impl Into<String>) -> Self {
        self.file_copy_options = Some(value.into());
        self
    }
}

/// Builder for a [`TaskStep`] invoking `PublishBuildArtifacts@1`.
///
/// Use [`PublishBuildArtifacts::container`] to publish to Azure Pipelines
/// artifact storage and [`PublishBuildArtifacts::file_path`] to publish to a
/// file share. Shared optionals (`max_artifact_size`, `store_as_tar`) are
/// available on both.
///
/// ADO task reference:
/// <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/publish-build-artifacts-v1>
#[derive(Debug, Clone)]
pub struct PublishBuildArtifacts {
    path_to_publish: String,
    artifact_name: String,
    location: PublishLocation,
    /// `MaxArtifactSize` — artifact size limit in bytes; `"0"` disables the limit.
    max_artifact_size: Option<String>,
    /// `StoreAsTar` — tar the artifact directory before uploading.
    store_as_tar: Option<bool>,
    display_name: Option<String>,
}

impl PublishBuildArtifacts {
    /// Construct from an explicit [`PublishLocation`].
    pub fn new(
        path_to_publish: impl Into<String>,
        artifact_name: impl Into<String>,
        location: PublishLocation,
    ) -> Self {
        Self {
            path_to_publish: path_to_publish.into(),
            artifact_name: artifact_name.into(),
            location,
            max_artifact_size: None,
            store_as_tar: None,
            display_name: None,
        }
    }

    /// `publishLocation: Container` — publish to Azure Pipelines artifact storage.
    pub fn container(path_to_publish: impl Into<String>, artifact_name: impl Into<String>) -> Self {
        Self::new(path_to_publish, artifact_name, PublishLocation::Container)
    }

    /// `publishLocation: FilePath` — publish to a file share.
    pub fn file_path(
        path_to_publish: impl Into<String>,
        artifact_name: impl Into<String>,
        location: FilePathLocation,
    ) -> Self {
        Self::new(
            path_to_publish,
            artifact_name,
            PublishLocation::FilePath(location),
        )
    }

    /// `MaxArtifactSize` — maximum artifact size in bytes; `"0"` means no limit.
    pub fn max_artifact_size(mut self, value: impl Into<String>) -> Self {
        self.max_artifact_size = Some(value.into());
        self
    }

    /// `StoreAsTar` — tar the artifact directory before uploading.
    pub fn store_as_tar(mut self, value: bool) -> Self {
        self.store_as_tar = Some(value);
        self
    }

    /// Override the default `displayName` (`"Publish Artifact"`).
    pub fn with_display_name(mut self, value: impl Into<String>) -> Self {
        self.display_name = Some(value.into());
        self
    }

    /// Lower into a [`TaskStep`].
    pub fn into_step(self) -> TaskStep {
        let mut t = TaskStep::new(
            "PublishBuildArtifacts@1",
            self.display_name
                .unwrap_or_else(|| "Publish Artifact".into()),
        )
        .with_input("PathtoPublish", self.path_to_publish)
        .with_input("ArtifactName", self.artifact_name);

        match self.location {
            PublishLocation::Container => {
                t = t.with_input("publishLocation", "Container");
            }
            PublishLocation::FilePath(loc) => {
                t = t
                    .with_input("publishLocation", "FilePath")
                    .with_input("TargetPath", loc.target_path);
                push_bool(&mut t, "Parallel", loc.parallel);
                push_opt(&mut t, "ParallelCount", loc.parallel_count);
                push_opt(&mut t, "FileCopyOptions", loc.file_copy_options);
            }
        }

        push_opt(&mut t, "MaxArtifactSize", self.max_artifact_size);
        push_bool(&mut t, "StoreAsTar", self.store_as_tar);
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drift guard for the two-pass validator: `remove_common_inputs` must stay
    /// in sync with `PublishBuildArtifactsCommonInputs`. A step that sets every
    /// common input must validate for both locations — a common field added to
    /// the struct but forgotten in the removal list would survive into the
    /// variant dispatch and trip `deny_unknown_fields`, failing this test.
    #[test]
    fn all_common_inputs_validate_across_both_locations() {
        let common = concat!(
            "\nPathtoPublish: $(Build.ArtifactStagingDirectory)",
            "\nArtifactName: drop",
            "\nMaxArtifactSize: '0'",
            "\nStoreAsTar: true",
        );

        let container =
            serde_yaml::from_str(&format!("publishLocation: Container{common}")).unwrap();
        assert!(
            validate_inputs(container).is_ok(),
            "common inputs must validate for Container"
        );

        let file_path = serde_yaml::from_str(&format!(
            "publishLocation: FilePath\nTargetPath: \\\\share\\drops{common}"
        ))
        .unwrap();
        assert!(
            validate_inputs(file_path).is_ok(),
            "common inputs must validate for FilePath"
        );
    }

    #[test]
    fn container_defaults() {
        let t = PublishBuildArtifacts::container("$(Build.ArtifactStagingDirectory)", "drop")
            .into_step();
        assert_eq!(t.task, "PublishBuildArtifacts@1");
        assert_eq!(t.display_name, "Publish Artifact");
        assert_eq!(
            t.inputs.get("PathtoPublish").map(String::as_str),
            Some("$(Build.ArtifactStagingDirectory)")
        );
        assert_eq!(
            t.inputs.get("ArtifactName").map(String::as_str),
            Some("drop")
        );
        assert_eq!(
            t.inputs.get("publishLocation").map(String::as_str),
            Some("Container")
        );
        assert!(t.inputs.get("TargetPath").is_none());
    }

    #[test]
    fn container_with_shared_optionals() {
        let t = PublishBuildArtifacts::container("$(Build.ArtifactStagingDirectory)", "binaries")
            .max_artifact_size("1073741824")
            .store_as_tar(true)
            .with_display_name("Publish build binaries")
            .into_step();
        assert_eq!(t.display_name, "Publish build binaries");
        assert_eq!(
            t.inputs.get("MaxArtifactSize").map(String::as_str),
            Some("1073741824")
        );
        assert_eq!(t.inputs.get("StoreAsTar").map(String::as_str), Some("true"));
    }

    #[test]
    fn file_path_with_location_optionals() {
        let t = PublishBuildArtifacts::file_path(
            "$(Build.ArtifactStagingDirectory)",
            "drop",
            FilePathLocation::new(r"\\myshare\builds")
                .parallel(true)
                .parallel_count("16")
                .file_copy_options("/MIR"),
        )
        .into_step();
        assert_eq!(
            t.inputs.get("publishLocation").map(String::as_str),
            Some("FilePath")
        );
        assert_eq!(
            t.inputs.get("TargetPath").map(String::as_str),
            Some(r"\\myshare\builds")
        );
        assert_eq!(t.inputs.get("Parallel").map(String::as_str), Some("true"));
        assert_eq!(
            t.inputs.get("ParallelCount").map(String::as_str),
            Some("16")
        );
        assert_eq!(
            t.inputs.get("FileCopyOptions").map(String::as_str),
            Some("/MIR")
        );
    }

    #[test]
    fn file_path_omits_absent_optionals() {
        let t = PublishBuildArtifacts::file_path(
            "$(Build.ArtifactStagingDirectory)",
            "drop",
            FilePathLocation::new(r"\\myshare\builds"),
        )
        .into_step();
        assert_eq!(
            t.inputs.get("publishLocation").map(String::as_str),
            Some("FilePath")
        );
        assert!(t.inputs.get("Parallel").is_none());
        assert!(t.inputs.get("ParallelCount").is_none());
        assert!(t.inputs.get("FileCopyOptions").is_none());
        assert!(t.inputs.get("MaxArtifactSize").is_none());
        assert!(t.inputs.get("StoreAsTar").is_none());
    }

    #[test]
    fn display_name_override() {
        let t = PublishBuildArtifacts::container("out/", "my-artifact")
            .with_display_name("Stage and publish")
            .into_step();
        assert_eq!(t.display_name, "Stage and publish");
    }
}
