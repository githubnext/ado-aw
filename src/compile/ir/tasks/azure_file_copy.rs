//! Typed builder for `AzureFileCopy@6`.
//!
//! `AzureFileCopy@6` copies files from a source path to either **Azure Blob
//! Storage** or **Azure VMs** via AzCopy. The destination type is modeled as
//! a [`AzureFileCopyDestination`] enum whose variants carry the per-destination
//! required and optional inputs, so providing blob-only inputs to a VM copy (or
//! vice versa) is unrepresentable.
//!
//! ADO task reference:
//! <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/azure-file-copy-v6>

use super::common::{de_opt_bool_flex, push_bool, push_opt};
use crate::compile::ir::step::TaskStep;
use serde::Deserialize;
use serde_yaml::Value;

/// Validate an authored `AzureFileCopy@6` `inputs:` mapping (advisory
/// front-matter validation, see [`super::parse`]).
pub(crate) fn validate_inputs(inputs: Value) -> Result<(), String> {
    let mut map = match inputs {
        Value::Mapping(m) => m,
        Value::Null => Default::default(),
        other => return Err(format!("`inputs` must be a mapping, got {other:?}")),
    };
    let destination = match map.remove("Destination") {
        Some(value) => value
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| "`Destination` must be a string".to_string())?,
        None => "AzureBlob".to_string(),
    };

    validate_common(&Value::Mapping(map.clone())).map_err(|e| format!("common inputs: {e}"))?;
    remove_common_inputs(&mut map);
    let rest = Value::Mapping(map);

    let result = match destination.as_str() {
        "AzureBlob" => serde_yaml::from_value::<AzureFileCopyToBlob>(rest).map(drop),
        "AzureVMs" => serde_yaml::from_value::<AzureFileCopyToVMs>(rest).map(drop),
        other => return Err(format!("AzureFileCopy@6: unknown Destination `{other}`")),
    };
    result.map_err(|e| format!("Destination `{destination}`: {e}"))
}

fn validate_common(inputs: &Value) -> Result<(), serde_yaml::Error> {
    serde_yaml::from_value::<AzureFileCopyCommonInputs>(inputs.clone()).map(drop)
}

fn remove_common_inputs(map: &mut serde_yaml::Mapping) {
    for key in [
        "SourcePath",
        "azureSubscription",
        "storage",
        "CleanTargetBeforeCopy",
    ] {
        map.remove(key);
    }
}

#[derive(Debug, Deserialize)]
struct AzureFileCopyCommonInputs {
    #[serde(rename = "SourcePath")]
    _source_path: String,
    #[serde(rename = "azureSubscription")]
    _azure_subscription: String,
    #[serde(rename = "storage")]
    _storage: String,
    #[serde(
        rename = "CleanTargetBeforeCopy",
        default,
        deserialize_with = "de_opt_bool_flex"
    )]
    _clean_target_before_copy: Option<bool>,
}

/// Resource filtering method for Azure VM targets.
///
/// Controls how the `MachineNames` filter is interpreted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum ResourceFilteringMethod {
    /// Filter by machine name (`machineNames`). ADO default.
    #[serde(rename = "machineNames")]
    MachineNames,
    /// Filter by tag (`tags`).
    #[serde(rename = "tags")]
    Tags,
}

impl ResourceFilteringMethod {
    /// Return the exact ADO token for this filtering method.
    pub fn as_ado_str(self) -> &'static str {
        match self {
            ResourceFilteringMethod::MachineNames => "machineNames",
            ResourceFilteringMethod::Tags => "tags",
        }
    }
}

/// Per-destination inputs for [`AzureFileCopy`].
///
/// Each variant carries the inputs that are required and/or applicable only for
/// that destination type. Choose the variant that matches your copy target:
///
/// - [`AzureFileCopyDestination::AzureBlob`] — uploads via AzCopy to a blob
///   container.
/// - [`AzureFileCopyDestination::AzureVMs`] — copies to Azure VMs over WinRM.
#[derive(Debug, Clone)]
pub enum AzureFileCopyDestination {
    /// Copy files to Azure Blob Storage.
    AzureBlob(AzureFileCopyToBlob),
    /// Copy files to Azure VMs via WinRM.
    AzureVMs(AzureFileCopyToVMs),
}

/// Inputs for [`AzureFileCopyDestination::AzureBlob`].
///
/// `container_name` is required; all other inputs are optional.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AzureFileCopyToBlob {
    #[serde(rename = "ContainerName")]
    container_name: String,
    #[serde(rename = "BlobPrefix", default)]
    blob_prefix: Option<String>,
    #[serde(rename = "AdditionalArgumentsForBlobCopy", default)]
    additional_arguments: Option<String>,
}

impl AzureFileCopyToBlob {
    /// Create a new blob destination.
    ///
    /// `container_name` — the Azure Blob container to copy files into; created
    /// automatically if it does not exist.
    pub fn new(container_name: impl Into<String>) -> Self {
        Self {
            container_name: container_name.into(),
            blob_prefix: None,
            additional_arguments: None,
        }
    }

    /// `BlobPrefix` — virtual directory prefix within the container.
    ///
    /// When `SourcePath` contains a wildcard, all matched files are placed
    /// under this prefix. When `SourcePath` is a single file, the prefix
    /// becomes the destination blob name.
    pub fn blob_prefix(mut self, value: impl Into<String>) -> Self {
        self.blob_prefix = Some(value.into());
        self
    }

    /// `AdditionalArgumentsForBlobCopy` — extra `AzCopy.exe` flags for the
    /// upload, e.g. `--blob-type=PageBlob` for Premium storage.
    pub fn additional_arguments(mut self, value: impl Into<String>) -> Self {
        self.additional_arguments = Some(value.into());
        self
    }
}

/// Inputs for [`AzureFileCopyDestination::AzureVMs`].
///
/// The four positional parameters (`resource_group`, `admin_user`,
/// `admin_password`, `target_path`) are required; all other inputs are
/// optional.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AzureFileCopyToVMs {
    #[serde(rename = "resourceGroup")]
    resource_group: String,
    #[serde(rename = "vmsAdminUserName")]
    admin_user: String,
    #[serde(rename = "vmsAdminPassword")]
    admin_password: String,
    #[serde(rename = "TargetPath")]
    target_path: String,
    #[serde(rename = "ResourceFilteringMethod", default)]
    resource_filtering_method: Option<ResourceFilteringMethod>,
    #[serde(rename = "MachineNames", default)]
    machine_names: Option<String>,
    #[serde(rename = "AdditionalArgumentsForVMCopy", default)]
    additional_arguments: Option<String>,
    #[serde(
        rename = "enableCopyPrerequisites",
        default,
        deserialize_with = "de_opt_bool_flex"
    )]
    enable_copy_prerequisites: Option<bool>,
    #[serde(
        rename = "CopyFilesInParallel",
        default,
        deserialize_with = "de_opt_bool_flex"
    )]
    copy_files_in_parallel: Option<bool>,
    #[serde(rename = "skipCACheck", default, deserialize_with = "de_opt_bool_flex")]
    skip_ca_check: Option<bool>,
}

impl AzureFileCopyToVMs {
    /// Create a new VM destination.
    ///
    /// - `resource_group` — (`resourceGroup`) Azure Resource Group containing
    ///   the target VMs.
    /// - `admin_user` — (`vmsAdminUserName`) administrative account on the
    ///   VMs; e.g. `username` or `domain\\username`.
    /// - `admin_password` — (`vmsAdminPassword`) password for `admin_user`;
    ///   use a secret variable.
    /// - `target_path` — (`TargetPath`) destination folder on the VMs; e.g.
    ///   `c:\\FabrikamFiber` or `$env:windir\\Temp`.
    pub fn new(
        resource_group: impl Into<String>,
        admin_user: impl Into<String>,
        admin_password: impl Into<String>,
        target_path: impl Into<String>,
    ) -> Self {
        Self {
            resource_group: resource_group.into(),
            admin_user: admin_user.into(),
            admin_password: admin_password.into(),
            target_path: target_path.into(),
            resource_filtering_method: None,
            machine_names: None,
            additional_arguments: None,
            enable_copy_prerequisites: None,
            copy_files_in_parallel: None,
            skip_ca_check: None,
        }
    }

    /// `ResourceFilteringMethod` — how `MachineNames` is interpreted.
    /// ADO default: `machineNames`.
    pub fn resource_filtering_method(mut self, value: ResourceFilteringMethod) -> Self {
        self.resource_filtering_method = Some(value);
        self
    }

    /// `MachineNames` — comma-separated VM hostnames, IPs, or tag filters
    /// (e.g. `Role:DB;OS:Win8.1`) that limit which VMs are targeted.
    pub fn machine_names(mut self, value: impl Into<String>) -> Self {
        self.machine_names = Some(value.into());
        self
    }

    /// `AdditionalArgumentsForVMCopy` — extra `AzCopy.exe` flags for the
    /// VM download phase, e.g. `--check-length=true`.
    pub fn additional_arguments(mut self, value: impl Into<String>) -> Self {
        self.additional_arguments = Some(value.into());
        self
    }

    /// `enableCopyPrerequisites` — configure WinRM over HTTPS on port 5986.
    ///
    /// Required for ARM VMs when WinRM HTTPS is not already enabled. ADO
    /// default: `false`.
    pub fn enable_copy_prerequisites(mut self, value: bool) -> Self {
        self.enable_copy_prerequisites = Some(value);
        self
    }

    /// `CopyFilesInParallel` — copy files to all target VMs in parallel.
    /// ADO default: `true`.
    pub fn copy_files_in_parallel(mut self, value: bool) -> Self {
        self.copy_files_in_parallel = Some(value);
        self
    }

    /// `skipCACheck` — skip certificate validation against a trusted CA when
    /// connecting to VMs over WinRM HTTPS. ADO default: `true`.
    pub fn skip_ca_check(mut self, value: bool) -> Self {
        self.skip_ca_check = Some(value);
        self
    }
}

/// Builder for a [`TaskStep`] invoking `AzureFileCopy@6`.
///
/// Copies files from a local or pipeline source path to Azure Blob Storage or
/// Azure VMs using `AzCopy`. All destination-specific inputs (container name,
/// resource group, VM credentials, …) are captured in the
/// [`AzureFileCopyDestination`] enum so that blob-only and VM-only inputs are
/// not accidentally mixed.
///
/// ## Examples
///
/// ### Upload to Azure Blob Storage
///
/// ```rust
/// # use ado_aw::compile::ir::tasks::azure_file_copy::{
/// #     AzureFileCopy, AzureFileCopyDestination, AzureFileCopyToBlob,
/// # };
/// let step = AzureFileCopy::new(
///     "$(Build.ArtifactStagingDirectory)/**",
///     "my-arm-connection",
///     "myaccount",
///     AzureFileCopyDestination::AzureBlob(
///         AzureFileCopyToBlob::new("releases")
///             .blob_prefix("$(Build.BuildNumber)"),
///     ),
/// )
/// .into_step();
/// assert_eq!(step.task, "AzureFileCopy@6");
/// ```
///
/// ADO task reference:
/// <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/azure-file-copy-v6>
#[derive(Debug, Clone)]
pub struct AzureFileCopy {
    source_path: String,
    azure_subscription: String,
    storage: String,
    destination: AzureFileCopyDestination,
    clean_target_before_copy: Option<bool>,
    display_name: Option<String>,
}

impl AzureFileCopy {
    /// Create a new `AzureFileCopy@6` builder.
    ///
    /// - `source_path` — (`SourcePath`) local files or folder to copy;
    ///   supports wildcards, e.g. `$(Build.ArtifactStagingDirectory)/**`.
    /// - `azure_subscription` — (`azureSubscription`) name of the Azure
    ///   Resource Manager service connection.
    /// - `storage` — (`storage`) pre-existing ARM storage account name used
    ///   for the upload (and as intermediary for VM copies).
    /// - `destination` — where to copy; carries all destination-specific
    ///   required and optional inputs.
    pub fn new(
        source_path: impl Into<String>,
        azure_subscription: impl Into<String>,
        storage: impl Into<String>,
        destination: AzureFileCopyDestination,
    ) -> Self {
        Self {
            source_path: source_path.into(),
            azure_subscription: azure_subscription.into(),
            storage: storage.into(),
            destination,
            clean_target_before_copy: None,
            display_name: None,
        }
    }

    /// `CleanTargetBeforeCopy` — remove all files in the destination before
    /// copying. ADO default: `false`.
    pub fn clean_target_before_copy(mut self, value: bool) -> Self {
        self.clean_target_before_copy = Some(value);
        self
    }

    /// Override the default `displayName` (`"Azure File Copy"`).
    pub fn with_display_name(mut self, value: impl Into<String>) -> Self {
        self.display_name = Some(value.into());
        self
    }

    /// Lower into a [`TaskStep`].
    pub fn into_step(self) -> TaskStep {
        let destination_str = match &self.destination {
            AzureFileCopyDestination::AzureBlob(_) => "AzureBlob",
            AzureFileCopyDestination::AzureVMs(_) => "AzureVMs",
        };

        let mut t = TaskStep::new(
            "AzureFileCopy@6",
            self.display_name
                .unwrap_or_else(|| "Azure File Copy".into()),
        )
        .with_input("SourcePath", self.source_path)
        .with_input("azureSubscription", self.azure_subscription)
        .with_input("Destination", destination_str)
        .with_input("storage", self.storage);

        push_bool(
            &mut t,
            "CleanTargetBeforeCopy",
            self.clean_target_before_copy,
        );

        match self.destination {
            AzureFileCopyDestination::AzureBlob(blob) => {
                t = t.with_input("ContainerName", blob.container_name);
                push_opt(&mut t, "BlobPrefix", blob.blob_prefix);
                push_opt(
                    &mut t,
                    "AdditionalArgumentsForBlobCopy",
                    blob.additional_arguments,
                );
            }
            AzureFileCopyDestination::AzureVMs(vms) => {
                t = t.with_input("resourceGroup", vms.resource_group);
                t = t.with_input("vmsAdminUserName", vms.admin_user);
                t = t.with_input("vmsAdminPassword", vms.admin_password);
                t = t.with_input("TargetPath", vms.target_path);
                if let Some(v) = vms.resource_filtering_method {
                    t = t.with_input("ResourceFilteringMethod", v.as_ado_str());
                }
                push_opt(&mut t, "MachineNames", vms.machine_names);
                push_opt(
                    &mut t,
                    "AdditionalArgumentsForVMCopy",
                    vms.additional_arguments,
                );
                push_bool(
                    &mut t,
                    "enableCopyPrerequisites",
                    vms.enable_copy_prerequisites,
                );
                push_bool(&mut t, "CopyFilesInParallel", vms.copy_files_in_parallel);
                push_bool(&mut t, "skipCACheck", vms.skip_ca_check);
            }
        }

        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blob_sets_task_and_required_inputs() {
        let step = AzureFileCopy::new(
            "$(Build.ArtifactStagingDirectory)/**",
            "my-arm-connection",
            "mystorageaccount",
            AzureFileCopyDestination::AzureBlob(AzureFileCopyToBlob::new("my-container")),
        )
        .into_step();

        assert_eq!(step.task, "AzureFileCopy@6");
        assert_eq!(step.display_name, "Azure File Copy");
        assert_eq!(
            step.inputs.get("SourcePath").map(String::as_str),
            Some("$(Build.ArtifactStagingDirectory)/**")
        );
        assert_eq!(
            step.inputs.get("azureSubscription").map(String::as_str),
            Some("my-arm-connection")
        );
        assert_eq!(
            step.inputs.get("Destination").map(String::as_str),
            Some("AzureBlob")
        );
        assert_eq!(
            step.inputs.get("storage").map(String::as_str),
            Some("mystorageaccount")
        );
        assert_eq!(
            step.inputs.get("ContainerName").map(String::as_str),
            Some("my-container")
        );
    }

    #[test]
    fn blob_optional_inputs_emit_only_when_set() {
        let step = AzureFileCopy::new(
            "$(Build.ArtifactStagingDirectory)",
            "conn",
            "storage",
            AzureFileCopyDestination::AzureBlob(
                AzureFileCopyToBlob::new("drops")
                    .blob_prefix("$(Build.BuildNumber)")
                    .additional_arguments("--blob-type=PageBlob"),
            ),
        )
        .clean_target_before_copy(true)
        .into_step();

        assert_eq!(
            step.inputs.get("BlobPrefix").map(String::as_str),
            Some("$(Build.BuildNumber)")
        );
        assert_eq!(
            step.inputs
                .get("AdditionalArgumentsForBlobCopy")
                .map(String::as_str),
            Some("--blob-type=PageBlob")
        );
        assert_eq!(
            step.inputs.get("CleanTargetBeforeCopy").map(String::as_str),
            Some("true")
        );
        // VM-only inputs must be absent
        assert!(step.inputs.get("resourceGroup").is_none());
        assert!(step.inputs.get("TargetPath").is_none());
    }

    #[test]
    fn vm_sets_required_inputs_and_destination() {
        let step = AzureFileCopy::new(
            "$(Build.ArtifactStagingDirectory)",
            "arm-conn",
            "storage",
            AzureFileCopyDestination::AzureVMs(AzureFileCopyToVMs::new(
                "MyResourceGroup",
                "adminuser",
                "$(AdminPassword)",
                "c:\\Fabrikam",
            )),
        )
        .into_step();

        assert_eq!(
            step.inputs.get("Destination").map(String::as_str),
            Some("AzureVMs")
        );
        assert_eq!(
            step.inputs.get("resourceGroup").map(String::as_str),
            Some("MyResourceGroup")
        );
        assert_eq!(
            step.inputs.get("vmsAdminUserName").map(String::as_str),
            Some("adminuser")
        );
        assert_eq!(
            step.inputs.get("vmsAdminPassword").map(String::as_str),
            Some("$(AdminPassword)")
        );
        assert_eq!(
            step.inputs.get("TargetPath").map(String::as_str),
            Some("c:\\Fabrikam")
        );
        // Blob-only inputs must be absent
        assert!(step.inputs.get("ContainerName").is_none());
        assert!(step.inputs.get("BlobPrefix").is_none());
    }

    #[test]
    fn vm_optional_inputs_emit_only_when_set() {
        let step = AzureFileCopy::new(
            "src",
            "conn",
            "storage",
            AzureFileCopyDestination::AzureVMs(
                AzureFileCopyToVMs::new("RG", "user", "pass", "c:\\dest")
                    .resource_filtering_method(ResourceFilteringMethod::Tags)
                    .machine_names("Role:Web")
                    .enable_copy_prerequisites(true)
                    .copy_files_in_parallel(false)
                    .skip_ca_check(false),
            ),
        )
        .into_step();

        assert_eq!(
            step.inputs
                .get("ResourceFilteringMethod")
                .map(String::as_str),
            Some("tags")
        );
        assert_eq!(
            step.inputs.get("MachineNames").map(String::as_str),
            Some("Role:Web")
        );
        assert_eq!(
            step.inputs
                .get("enableCopyPrerequisites")
                .map(String::as_str),
            Some("true")
        );
        assert_eq!(
            step.inputs.get("CopyFilesInParallel").map(String::as_str),
            Some("false")
        );
        assert_eq!(
            step.inputs.get("skipCACheck").map(String::as_str),
            Some("false")
        );
        // Unset optional must be absent
        assert!(step.inputs.get("AdditionalArgumentsForVMCopy").is_none());
    }

    #[test]
    fn display_name_override() {
        let step = AzureFileCopy::new(
            "src",
            "conn",
            "storage",
            AzureFileCopyDestination::AzureBlob(AzureFileCopyToBlob::new("c")),
        )
        .with_display_name("Upload release artifacts")
        .into_step();

        assert_eq!(step.display_name, "Upload release artifacts");
    }
}
