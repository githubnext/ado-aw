//! Typed builder for `JavaToolInstaller@0`.
//!
//! Models the three JDK source modes as an enum (`PreInstalled`,
//! `LocalDirectory`, `AzureStorage`) so that required per-source inputs are
//! positional and inapplicable inputs are unrepresentable at compile time.
//!
//! ADO task reference:
//! <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/java-tool-installer-v0>

use super::common::{de_opt_bool_flex, push_bool, push_opt};
use crate::compile::ir::step::TaskStep;
use serde::Deserialize;
use serde_yaml::Value;

/// Validate an authored `JavaToolInstaller@0` `inputs:` mapping (advisory
/// front-matter validation, see [`super::parse`]).
pub(crate) fn validate_inputs(inputs: Value) -> Result<(), String> {
    let mut map = match inputs {
        Value::Mapping(m) => m,
        Value::Null => Default::default(),
        other => return Err(format!("`inputs` must be a mapping, got {other:?}")),
    };
    let source = match map.remove("jdkSourceOption") {
        Some(value) => value
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| "`jdkSourceOption` must be a string".to_string())?,
        None => "PreInstalled".to_string(),
    };

    validate_common(&Value::Mapping(map.clone())).map_err(|e| format!("common inputs: {e}"))?;
    remove_common_inputs(&mut map);
    let rest = Value::Mapping(map);

    let result = match source.as_str() {
        "PreInstalled" => serde_yaml::from_value::<PreInstalledSpec>(rest).map(drop),
        "LocalDirectory" => serde_yaml::from_value::<LocalDirectorySpec>(rest).map(drop),
        "AzureStorage" => serde_yaml::from_value::<AzureStorageSpec>(rest).map(drop),
        other => {
            return Err(format!(
                "JavaToolInstaller@0: unknown jdkSourceOption `{other}`"
            ));
        }
    };
    result.map_err(|e| format!("jdkSourceOption `{source}`: {e}"))
}

fn validate_common(inputs: &Value) -> Result<(), serde_yaml::Error> {
    serde_yaml::from_value::<JavaToolInstallerCommonInputs>(inputs.clone()).map(drop)
}

fn remove_common_inputs(map: &mut serde_yaml::Mapping) {
    for key in ["versionSpec", "jdkArchitectureOption"] {
        map.remove(key);
    }
}

#[derive(Debug, Deserialize)]
struct JavaToolInstallerCommonInputs {
    #[serde(rename = "versionSpec")]
    _version_spec: String,
    #[serde(rename = "jdkArchitectureOption")]
    _architecture: JdkArchitecture,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct PreInstalledSpec {}

/// JDK CPU architecture for [`JavaToolInstaller`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum JdkArchitecture {
    #[serde(rename = "x64")]
    X64,
    #[serde(rename = "x86")]
    X86,
    #[serde(rename = "arm64")]
    Arm64,
}

impl JdkArchitecture {
    /// Returns the exact string token that `jdkArchitectureOption` expects.
    pub fn as_ado_str(self) -> &'static str {
        match self {
            JdkArchitecture::X64 => "x64",
            JdkArchitecture::X86 => "x86",
            JdkArchitecture::Arm64 => "arm64",
        }
    }
}

/// JDK source selection for [`JavaToolInstaller`], carrying per-source inputs.
///
/// Each variant holds the inputs that are required and optional for that
/// source, so applying an input to the wrong source is unrepresentable.
#[derive(Debug, Clone)]
pub enum JdkSource {
    /// Use a JDK that is already installed on the build agent.
    PreInstalled,
    /// Copy and extract a JDK archive from a local directory on the build agent.
    LocalDirectory(LocalDirectorySpec),
    /// Download and extract a JDK archive from Azure Blob Storage.
    AzureStorage(AzureStorageSpec),
}

/// Per-source required and optional inputs for `jdkSourceOption: LocalDirectory`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalDirectorySpec {
    /// `jdkFile` — path to the JDK archive on the build agent.
    #[serde(rename = "jdkFile")]
    pub jdk_file: String,
    /// `jdkDestinationDirectory` — directory where the JDK is installed.
    #[serde(rename = "jdkDestinationDirectory")]
    pub jdk_destination_directory: String,
    #[serde(
        rename = "cleanDestinationDirectory",
        default,
        deserialize_with = "de_opt_bool_flex"
    )]
    clean_destination_directory: Option<bool>,
    #[serde(
        rename = "createExtractDirectory",
        default,
        deserialize_with = "de_opt_bool_flex"
    )]
    create_extract_directory: Option<bool>,
}

impl LocalDirectorySpec {
    /// Required: `jdk_file` path and `jdk_destination_directory`.
    pub fn new(jdk_file: impl Into<String>, jdk_destination_directory: impl Into<String>) -> Self {
        Self {
            jdk_file: jdk_file.into(),
            jdk_destination_directory: jdk_destination_directory.into(),
            clean_destination_directory: None,
            create_extract_directory: None,
        }
    }

    /// `cleanDestinationDirectory` — delete destination directory contents before extraction.
    pub fn clean_destination_directory(mut self, value: bool) -> Self {
        self.clean_destination_directory = Some(value);
        self
    }

    /// `createExtractDirectory` — create a sub-directory for extraction.
    pub fn create_extract_directory(mut self, value: bool) -> Self {
        self.create_extract_directory = Some(value);
        self
    }
}

/// Per-source required and optional inputs for `jdkSourceOption: AzureStorage`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AzureStorageSpec {
    /// `azureStorageAccountName` — Azure Storage account containing the JDK archive.
    #[serde(rename = "azureStorageAccountName")]
    pub azure_storage_account_name: String,
    /// `azureContainerName` — Blob container name.
    #[serde(rename = "azureContainerName")]
    pub azure_container_name: String,
    /// `azureCommonVirtualFile` — common virtual path to the JDK archive.
    #[serde(rename = "azureCommonVirtualFile")]
    pub azure_common_virtual_file: String,
    /// `jdkDestinationDirectory` — directory where the JDK is installed.
    #[serde(rename = "jdkDestinationDirectory")]
    pub jdk_destination_directory: String,
    #[serde(rename = "azureResourceGroupName", default)]
    azure_resource_group_name: Option<String>,
    #[serde(
        rename = "cleanDestinationDirectory",
        default,
        deserialize_with = "de_opt_bool_flex"
    )]
    clean_destination_directory: Option<bool>,
    #[serde(
        rename = "createExtractDirectory",
        default,
        deserialize_with = "de_opt_bool_flex"
    )]
    create_extract_directory: Option<bool>,
}

impl AzureStorageSpec {
    /// Required: storage account name, container name, virtual file path, and destination directory.
    pub fn new(
        azure_storage_account_name: impl Into<String>,
        azure_container_name: impl Into<String>,
        azure_common_virtual_file: impl Into<String>,
        jdk_destination_directory: impl Into<String>,
    ) -> Self {
        Self {
            azure_storage_account_name: azure_storage_account_name.into(),
            azure_container_name: azure_container_name.into(),
            azure_common_virtual_file: azure_common_virtual_file.into(),
            jdk_destination_directory: jdk_destination_directory.into(),
            azure_resource_group_name: None,
            clean_destination_directory: None,
            create_extract_directory: None,
        }
    }

    /// `azureResourceGroupName` — resource group of the storage account.
    pub fn azure_resource_group_name(mut self, value: impl Into<String>) -> Self {
        self.azure_resource_group_name = Some(value.into());
        self
    }

    /// `cleanDestinationDirectory` — delete destination directory contents before extraction.
    pub fn clean_destination_directory(mut self, value: bool) -> Self {
        self.clean_destination_directory = Some(value);
        self
    }

    /// `createExtractDirectory` — create a sub-directory for extraction.
    pub fn create_extract_directory(mut self, value: bool) -> Self {
        self.create_extract_directory = Some(value);
        self
    }
}

/// Builder for a [`TaskStep`] invoking `JavaToolInstaller@0`.
///
/// Selects the appropriate JDK based on `versionSpec` and `architecture`.
/// The [`JdkSource`] enum determines whether the JDK comes from a
/// pre-installed location, a local directory archive, or Azure Blob Storage.
///
/// ```
/// use ado_aw::compile::ir::tasks::java_tool_installer::{
///     JavaToolInstaller, JdkArchitecture, JdkSource,
/// };
///
/// // Use a JDK already on the agent:
/// let step = JavaToolInstaller::pre_installed("17", JdkArchitecture::X64).into_step();
/// assert_eq!(step.task, "JavaToolInstaller@0");
/// ```
#[derive(Debug, Clone)]
pub struct JavaToolInstaller {
    version_spec: String,
    architecture: JdkArchitecture,
    source: JdkSource,
    display_name: Option<String>,
}

impl JavaToolInstaller {
    /// Construct from explicit `version_spec`, `architecture`, and `source`.
    pub fn new(
        version_spec: impl Into<String>,
        architecture: JdkArchitecture,
        source: JdkSource,
    ) -> Self {
        Self {
            version_spec: version_spec.into(),
            architecture,
            source,
            display_name: None,
        }
    }

    /// `jdkSourceOption: PreInstalled` — use a JDK already on the build agent.
    pub fn pre_installed(version_spec: impl Into<String>, architecture: JdkArchitecture) -> Self {
        Self::new(version_spec, architecture, JdkSource::PreInstalled)
    }

    /// `jdkSourceOption: LocalDirectory` — install from an archive on the agent.
    pub fn local_directory(
        version_spec: impl Into<String>,
        architecture: JdkArchitecture,
        spec: LocalDirectorySpec,
    ) -> Self {
        Self::new(version_spec, architecture, JdkSource::LocalDirectory(spec))
    }

    /// `jdkSourceOption: AzureStorage` — download and install from Azure Blob Storage.
    pub fn azure_storage(
        version_spec: impl Into<String>,
        architecture: JdkArchitecture,
        spec: AzureStorageSpec,
    ) -> Self {
        Self::new(version_spec, architecture, JdkSource::AzureStorage(spec))
    }

    /// Override the default `displayName` (`"Use Java <versionSpec>"`).
    pub fn with_display_name(mut self, value: impl Into<String>) -> Self {
        self.display_name = Some(value.into());
        self
    }

    /// Lower into a [`TaskStep`].
    pub fn into_step(self) -> TaskStep {
        let source_str = match &self.source {
            JdkSource::PreInstalled => "PreInstalled",
            JdkSource::LocalDirectory(_) => "LocalDirectory",
            JdkSource::AzureStorage(_) => "AzureStorage",
        };
        let default_display = format!("Use Java {}", self.version_spec);
        let mut t = TaskStep::new(
            "JavaToolInstaller@0",
            self.display_name.unwrap_or(default_display),
        )
        .with_input("versionSpec", self.version_spec)
        .with_input("jdkArchitectureOption", self.architecture.as_ado_str())
        .with_input("jdkSourceOption", source_str);
        match self.source {
            JdkSource::PreInstalled => {}
            JdkSource::LocalDirectory(s) => {
                t = t
                    .with_input("jdkFile", s.jdk_file)
                    .with_input("jdkDestinationDirectory", s.jdk_destination_directory);
                push_bool(
                    &mut t,
                    "cleanDestinationDirectory",
                    s.clean_destination_directory,
                );
                push_bool(&mut t, "createExtractDirectory", s.create_extract_directory);
            }
            JdkSource::AzureStorage(s) => {
                t = t
                    .with_input("azureStorageAccountName", s.azure_storage_account_name)
                    .with_input("azureContainerName", s.azure_container_name)
                    .with_input("azureCommonVirtualFile", s.azure_common_virtual_file)
                    .with_input("jdkDestinationDirectory", s.jdk_destination_directory);
                push_opt(
                    &mut t,
                    "azureResourceGroupName",
                    s.azure_resource_group_name,
                );
                push_bool(
                    &mut t,
                    "cleanDestinationDirectory",
                    s.clean_destination_directory,
                );
                push_bool(&mut t, "createExtractDirectory", s.create_extract_directory);
            }
        }
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pre_installed_emits_required_inputs() {
        let t = JavaToolInstaller::pre_installed("17", JdkArchitecture::X64).into_step();
        assert_eq!(t.task, "JavaToolInstaller@0");
        assert_eq!(t.display_name, "Use Java 17");
        assert_eq!(t.inputs.get("versionSpec").map(String::as_str), Some("17"));
        assert_eq!(
            t.inputs.get("jdkArchitectureOption").map(String::as_str),
            Some("x64")
        );
        assert_eq!(
            t.inputs.get("jdkSourceOption").map(String::as_str),
            Some("PreInstalled")
        );
        // No per-source extras emitted for PreInstalled.
        assert!(t.inputs.get("jdkFile").is_none());
        assert!(t.inputs.get("azureStorageAccountName").is_none());
    }

    #[test]
    fn pre_installed_arm64() {
        let t = JavaToolInstaller::pre_installed("21", JdkArchitecture::Arm64).into_step();
        assert_eq!(
            t.inputs.get("jdkArchitectureOption").map(String::as_str),
            Some("arm64")
        );
    }

    #[test]
    fn local_directory_emits_required_and_optional_inputs() {
        let spec = LocalDirectorySpec::new("/agent/tools/jdk17.tar.gz", "/tools/jdk17")
            .clean_destination_directory(true)
            .create_extract_directory(false);
        let t = JavaToolInstaller::local_directory("17", JdkArchitecture::X64, spec).into_step();
        assert_eq!(
            t.inputs.get("jdkSourceOption").map(String::as_str),
            Some("LocalDirectory")
        );
        assert_eq!(
            t.inputs.get("jdkFile").map(String::as_str),
            Some("/agent/tools/jdk17.tar.gz")
        );
        assert_eq!(
            t.inputs.get("jdkDestinationDirectory").map(String::as_str),
            Some("/tools/jdk17")
        );
        assert_eq!(
            t.inputs
                .get("cleanDestinationDirectory")
                .map(String::as_str),
            Some("true")
        );
        assert_eq!(
            t.inputs.get("createExtractDirectory").map(String::as_str),
            Some("false")
        );
        // Azure Storage keys must not be emitted.
        assert!(t.inputs.get("azureStorageAccountName").is_none());
    }

    #[test]
    fn local_directory_optional_inputs_absent_when_unset() {
        let spec = LocalDirectorySpec::new("/tools/jdk.tar.gz", "/tools/jdk");
        let t = JavaToolInstaller::local_directory("11", JdkArchitecture::X86, spec).into_step();
        assert!(t.inputs.get("cleanDestinationDirectory").is_none());
        assert!(t.inputs.get("createExtractDirectory").is_none());
    }

    #[test]
    fn azure_storage_emits_required_and_optional_inputs() {
        let spec = AzureStorageSpec::new(
            "myaccount",
            "jdk-binaries",
            "openjdk17/jdk17.tar.gz",
            "/tools/jdk17",
        )
        .azure_resource_group_name("my-rg")
        .clean_destination_directory(false);
        let t = JavaToolInstaller::azure_storage("17", JdkArchitecture::X64, spec).into_step();
        assert_eq!(
            t.inputs.get("jdkSourceOption").map(String::as_str),
            Some("AzureStorage")
        );
        assert_eq!(
            t.inputs.get("azureStorageAccountName").map(String::as_str),
            Some("myaccount")
        );
        assert_eq!(
            t.inputs.get("azureContainerName").map(String::as_str),
            Some("jdk-binaries")
        );
        assert_eq!(
            t.inputs.get("azureCommonVirtualFile").map(String::as_str),
            Some("openjdk17/jdk17.tar.gz")
        );
        assert_eq!(
            t.inputs.get("jdkDestinationDirectory").map(String::as_str),
            Some("/tools/jdk17")
        );
        assert_eq!(
            t.inputs.get("azureResourceGroupName").map(String::as_str),
            Some("my-rg")
        );
        assert_eq!(
            t.inputs
                .get("cleanDestinationDirectory")
                .map(String::as_str),
            Some("false")
        );
        // LocalDirectory keys must not be emitted.
        assert!(t.inputs.get("jdkFile").is_none());
    }

    #[test]
    fn azure_storage_optional_absent_when_unset() {
        let spec = AzureStorageSpec::new("acct", "container", "path/jdk.tar.gz", "/jdk");
        let t = JavaToolInstaller::azure_storage("11", JdkArchitecture::X64, spec).into_step();
        assert!(t.inputs.get("azureResourceGroupName").is_none());
        assert!(t.inputs.get("cleanDestinationDirectory").is_none());
        assert!(t.inputs.get("createExtractDirectory").is_none());
    }

    #[test]
    fn display_name_override() {
        let t = JavaToolInstaller::pre_installed("17", JdkArchitecture::X64)
            .with_display_name("Install Java 17 LTS")
            .into_step();
        assert_eq!(t.display_name, "Install Java 17 LTS");
    }

    #[test]
    fn explicit_new_with_pre_installed_source() {
        let t =
            JavaToolInstaller::new("8", JdkArchitecture::X86, JdkSource::PreInstalled).into_step();
        assert_eq!(t.inputs.get("versionSpec").map(String::as_str), Some("8"));
        assert_eq!(
            t.inputs.get("jdkArchitectureOption").map(String::as_str),
            Some("x86")
        );
    }
}
