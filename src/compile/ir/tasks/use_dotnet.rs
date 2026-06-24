//! Typed builder for `UseDotNet@2`.
//!
//! ADO task reference:
//! <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/use-dotnet-v2-task>

use super::common::{push_bool, push_opt};
use crate::compile::ir::step::TaskStep;

/// `packageType` input for [`UseDotNet`]: whether to install the SDK or only
/// the runtime.
///
/// ADO default: [`PackageType::Sdk`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageType {
    /// Install the full .NET SDK (`"sdk"`). This is the ADO task default.
    Sdk,
    /// Install only the .NET runtime (`"runtime"`).
    Runtime,
}

impl PackageType {
    /// Returns the exact string token ADO expects for the `packageType` input.
    pub fn as_ado_str(self) -> &'static str {
        match self {
            PackageType::Sdk => "sdk",
            PackageType::Runtime => "runtime",
        }
    }
}

/// Builder for a [`TaskStep`] invoking `UseDotNet@2`.
///
/// Acquires and caches a specific version of the .NET SDK or runtime and adds
/// it to the PATH. Supports two version-resolution modes:
///
/// * **Version spec** — call [`UseDotNet::with_version`] (or set via
///   [`UseDotNet::version`]) to pin a specific version spec (e.g. `"8.0.x"`).
/// * **global.json** — call [`UseDotNet::with_global_json`] (or set
///   [`UseDotNet::use_global_json`] to `true`) to read the version from a
///   `global.json` file in the workspace.
///
/// Both modes share the optional [`PackageType`], `installationPath`,
/// `performMultiLevelLookup`, and `failOnStandardError` inputs.
///
/// ADO task reference:
/// <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/use-dotnet-v2-task>
#[derive(Debug, Clone)]
pub struct UseDotNet {
    version: Option<String>,
    package_type: Option<PackageType>,
    use_global_json: Option<bool>,
    working_directory: Option<String>,
    installation_path: Option<String>,
    perform_multi_level_lookup: Option<bool>,
    fail_on_standard_error: Option<bool>,
    display_name: Option<String>,
}

impl UseDotNet {
    /// Create a builder with no inputs pre-set. Callers typically use the
    /// convenience constructors [`UseDotNet::with_version`] or
    /// [`UseDotNet::with_global_json`] instead.
    pub fn new() -> Self {
        Self {
            version: None,
            package_type: None,
            use_global_json: None,
            working_directory: None,
            installation_path: None,
            perform_multi_level_lookup: None,
            fail_on_standard_error: None,
            display_name: None,
        }
    }

    /// Convenience constructor: install a specific .NET version spec
    /// (e.g. `"8.0.x"`, `"6.0.x"`, `">=6.0.0"`).
    ///
    /// Equivalent to `UseDotNet::new().version(spec)`.
    pub fn with_version(spec: impl Into<String>) -> Self {
        Self::new().version(spec)
    }

    /// Convenience constructor: resolve the .NET version from a `global.json`
    /// file in the repository.
    ///
    /// Equivalent to `UseDotNet::new().use_global_json(true)`.
    pub fn with_global_json() -> Self {
        Self::new().use_global_json(true)
    }

    /// `version` — .NET version spec to install (e.g. `"8.0.x"`, `"6.0.x"`).
    /// Mutually exclusive with `useGlobalJson: true` in ADO (global.json
    /// takes precedence when both are set).
    pub fn version(mut self, value: impl Into<String>) -> Self {
        self.version = Some(value.into());
        self
    }

    /// `packageType` — whether to install the SDK (`"sdk"`, the default) or
    /// only the runtime (`"runtime"`).
    pub fn package_type(mut self, value: PackageType) -> Self {
        self.package_type = Some(value);
        self
    }

    /// `useGlobalJson` — read the .NET version from a `global.json` file.
    /// When `true`, the `version` input is ignored by ADO.
    pub fn use_global_json(mut self, value: bool) -> Self {
        self.use_global_json = Some(value);
        self
    }

    /// `workingDirectory` — directory to search for `global.json`. Relevant
    /// only when [`use_global_json`](Self::use_global_json) is `true`.
    pub fn working_directory(mut self, value: impl Into<String>) -> Self {
        self.working_directory = Some(value.into());
        self
    }

    /// `installationPath` — directory where the .NET SDK is installed.
    /// Default: `$(Agent.ToolsDirectory)/dotnet`.
    pub fn installation_path(mut self, value: impl Into<String>) -> Self {
        self.installation_path = Some(value.into());
        self
    }

    /// `performMultiLevelLookup` — search parent directories for
    /// `global.json`. Default: `false`.
    pub fn perform_multi_level_lookup(mut self, value: bool) -> Self {
        self.perform_multi_level_lookup = Some(value);
        self
    }

    /// `failOnStandardError` — fail the task if any output is written to
    /// stderr. Default: `false`.
    pub fn fail_on_standard_error(mut self, value: bool) -> Self {
        self.fail_on_standard_error = Some(value);
        self
    }

    /// Override the default `displayName`.
    ///
    /// Default display name:
    /// * Version spec set: `"Install .NET SDK <version>"` (or
    ///   `"Install .NET Runtime <version>"` when `packageType` is `Runtime`).
    /// * `useGlobalJson: true`: `"Install .NET SDK (from global.json)"` (or
    ///   `"Install .NET Runtime (from global.json)"` when `packageType` is
    ///   `Runtime`).
    /// * Neither set: `"Install .NET SDK"`.
    pub fn with_display_name(mut self, value: impl Into<String>) -> Self {
        self.display_name = Some(value.into());
        self
    }

    /// Lower into a [`TaskStep`].
    pub fn into_step(self) -> TaskStep {
        let package_label = match self.package_type {
            Some(PackageType::Runtime) => "Runtime",
            _ => "SDK",
        };

        let default_name = if self.use_global_json == Some(true) {
            format!("Install .NET {package_label} (from global.json)")
        } else if let Some(ref v) = self.version {
            format!("Install .NET {package_label} {v}")
        } else {
            format!("Install .NET {package_label}")
        };

        let mut t = TaskStep::new(
            "UseDotNet@2",
            self.display_name.unwrap_or(default_name),
        );
        push_opt(
            &mut t,
            "packageType",
            self.package_type.map(|p| p.as_ado_str().to_string()),
        );
        push_opt(&mut t, "version", self.version);
        push_bool(&mut t, "useGlobalJson", self.use_global_json);
        push_opt(&mut t, "workingDirectory", self.working_directory);
        push_opt(&mut t, "installationPath", self.installation_path);
        push_bool(
            &mut t,
            "performMultiLevelLookup",
            self.perform_multi_level_lookup,
        );
        push_bool(&mut t, "failOnStandardError", self.fail_on_standard_error);
        t
    }
}

impl Default for UseDotNet {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn with_version_sets_task_and_version() {
        let t = UseDotNet::with_version("8.0.x").into_step();
        assert_eq!(t.task, "UseDotNet@2");
        assert_eq!(t.display_name, "Install .NET SDK 8.0.x");
        assert_eq!(t.inputs.get("version").map(String::as_str), Some("8.0.x"));
        assert!(t.inputs.get("useGlobalJson").is_none());
        assert!(t.inputs.get("packageType").is_none());
    }

    #[test]
    fn with_global_json_sets_flag() {
        let t = UseDotNet::with_global_json().into_step();
        assert_eq!(t.task, "UseDotNet@2");
        assert_eq!(t.display_name, "Install .NET SDK (from global.json)");
        assert_eq!(
            t.inputs.get("useGlobalJson").map(String::as_str),
            Some("true")
        );
        assert!(t.inputs.get("version").is_none());
        assert!(t.inputs.get("packageType").is_none());
    }

    #[test]
    fn package_type_runtime_emits_input_and_adjusts_display_name() {
        let t = UseDotNet::with_version("8.0.x")
            .package_type(PackageType::Runtime)
            .into_step();
        assert_eq!(t.display_name, "Install .NET Runtime 8.0.x");
        assert_eq!(
            t.inputs.get("packageType").map(String::as_str),
            Some("runtime")
        );
    }

    #[test]
    fn package_type_sdk_emits_input_when_set_explicitly() {
        let t = UseDotNet::with_version("6.0.x")
            .package_type(PackageType::Sdk)
            .into_step();
        assert_eq!(
            t.inputs.get("packageType").map(String::as_str),
            Some("sdk")
        );
    }

    #[test]
    fn optional_inputs_emitted_when_set() {
        let t = UseDotNet::with_global_json()
            .working_directory("$(Build.SourcesDirectory)")
            .installation_path("/opt/dotnet")
            .perform_multi_level_lookup(true)
            .fail_on_standard_error(false)
            .into_step();
        assert_eq!(
            t.inputs.get("workingDirectory").map(String::as_str),
            Some("$(Build.SourcesDirectory)")
        );
        assert_eq!(
            t.inputs.get("installationPath").map(String::as_str),
            Some("/opt/dotnet")
        );
        assert_eq!(
            t.inputs.get("performMultiLevelLookup").map(String::as_str),
            Some("true")
        );
        assert_eq!(
            t.inputs.get("failOnStandardError").map(String::as_str),
            Some("false")
        );
    }

    #[test]
    fn optional_inputs_absent_when_not_set() {
        let t = UseDotNet::with_version("8.0.x").into_step();
        assert!(t.inputs.get("workingDirectory").is_none());
        assert!(t.inputs.get("installationPath").is_none());
        assert!(t.inputs.get("performMultiLevelLookup").is_none());
        assert!(t.inputs.get("failOnStandardError").is_none());
    }

    #[test]
    fn display_name_override() {
        let t = UseDotNet::with_version("8.0.x")
            .with_display_name("Install .NET SDK (from global.json)")
            .into_step();
        assert_eq!(
            t.display_name,
            "Install .NET SDK (from global.json)"
        );
        assert_eq!(t.inputs.get("version").map(String::as_str), Some("8.0.x"));
    }

    #[test]
    fn default_display_name_no_version_no_global_json() {
        let t = UseDotNet::new().into_step();
        assert_eq!(t.display_name, "Install .NET SDK");
    }

    #[test]
    fn global_json_runtime_adjusts_display_name() {
        let t = UseDotNet::with_global_json()
            .package_type(PackageType::Runtime)
            .into_step();
        assert_eq!(t.display_name, "Install .NET Runtime (from global.json)");
    }

    #[test]
    fn bool_input_false_emits_false_string() {
        let t = UseDotNet::new()
            .use_global_json(false)
            .into_step();
        assert_eq!(
            t.inputs.get("useGlobalJson").map(String::as_str),
            Some("false")
        );
    }
}
