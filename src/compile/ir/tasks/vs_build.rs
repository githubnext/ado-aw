//! Typed builder for `VSBuild@1`.
//!
//! ADO task reference:
//! <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/vsbuild-v1-task>

use super::common::{de_opt_bool_flex, push_bool, push_opt};
use crate::compile::ir::step::TaskStep;
use serde::Deserialize;

/// Visual Studio version constraint for [`VsBuild`].
///
/// Maps to the `vsVersion` input of `VSBuild@1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum VsVersion {
    /// Use the latest installed Visual Studio version.
    #[serde(rename = "latest")]
    Latest,
    /// Visual Studio 2022 (18.0).
    #[serde(rename = "18.0")]
    Vs2022,
    /// Visual Studio 2022 (17.0).
    #[serde(rename = "17.0")]
    Vs2019,
    /// Visual Studio 2019 (16.0).
    #[serde(rename = "16.0")]
    Vs2017,
    /// Visual Studio 2017 (15.0).
    #[serde(rename = "15.0")]
    Vs2015,
    /// Visual Studio 2015 (14.0).
    #[serde(rename = "14.0")]
    Vs2013,
}

impl VsVersion {
    /// Returns the exact token ADO expects for the `vsVersion` input.
    pub fn as_ado_str(&self) -> &'static str {
        match self {
            VsVersion::Latest => "latest",
            VsVersion::Vs2022 => "18.0",
            VsVersion::Vs2019 => "17.0",
            VsVersion::Vs2017 => "16.0",
            VsVersion::Vs2015 => "15.0",
            VsVersion::Vs2013 => "14.0",
        }
    }
}

/// MSBuild architecture for [`VsBuild`].
///
/// Maps to the `msbuildArchitecture` input of `VSBuild@1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum MsBuildArchitecture {
    /// 32-bit MSBuild.
    #[serde(rename = "x86")]
    X86,
    /// 64-bit MSBuild.
    #[serde(rename = "x64")]
    X64,
    /// ARM64 MSBuild.
    #[serde(rename = "arm64")]
    Arm64,
}

impl MsBuildArchitecture {
    /// Returns the exact token ADO expects for the `msbuildArchitecture` input.
    pub fn as_ado_str(&self) -> &'static str {
        match self {
            MsBuildArchitecture::X86 => "x86",
            MsBuildArchitecture::X64 => "x64",
            MsBuildArchitecture::Arm64 => "arm64",
        }
    }
}

/// MSBuild log verbosity for [`VsBuild`].
///
/// Maps to the `logFileVerbosity` input of `VSBuild@1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum LogFileVerbosity {
    /// Minimal output.
    #[serde(rename = "quiet")]
    Quiet,
    /// Minimal output plus errors and warnings.
    #[serde(rename = "minimal")]
    Minimal,
    /// Standard output (default).
    #[serde(rename = "normal")]
    Normal,
    /// Detailed output.
    #[serde(rename = "detailed")]
    Detailed,
    /// All available output.
    #[serde(rename = "diagnostic")]
    Diagnostic,
}

impl LogFileVerbosity {
    /// Returns the exact token ADO expects for the `logFileVerbosity` input.
    pub fn as_ado_str(&self) -> &'static str {
        match self {
            LogFileVerbosity::Quiet => "quiet",
            LogFileVerbosity::Minimal => "minimal",
            LogFileVerbosity::Normal => "normal",
            LogFileVerbosity::Detailed => "detailed",
            LogFileVerbosity::Diagnostic => "diagnostic",
        }
    }
}

/// Builder for a [`TaskStep`] invoking `VSBuild@1`.
///
/// Builds a Visual Studio solution using MSBuild. The `solution` argument
/// selects the solution file(s) to build (glob pattern accepted). All other
/// inputs are optional and only emitted when set.
///
/// ADO task reference:
/// <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/vsbuild-v1-task>
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VsBuild {
    #[serde(rename = "solution")]
    solution: String,
    #[serde(rename = "vsVersion", default)]
    vs_version: Option<VsVersion>,
    #[serde(rename = "msbuildArgs", default)]
    msbuild_args: Option<String>,
    #[serde(rename = "platform", default)]
    platform: Option<String>,
    #[serde(rename = "configuration", default)]
    configuration: Option<String>,
    #[serde(rename = "clean", default, deserialize_with = "de_opt_bool_flex")]
    clean: Option<bool>,
    #[serde(
        rename = "maximumCpuCount",
        default,
        deserialize_with = "de_opt_bool_flex"
    )]
    maximum_cpu_count: Option<bool>,
    #[serde(
        rename = "restoreNugetPackages",
        default,
        deserialize_with = "de_opt_bool_flex"
    )]
    restore_nuget_packages: Option<bool>,
    #[serde(rename = "msbuildArchitecture", default)]
    msbuild_architecture: Option<MsBuildArchitecture>,
    #[serde(
        rename = "logProjectEvents",
        default,
        deserialize_with = "de_opt_bool_flex"
    )]
    log_project_events: Option<bool>,
    #[serde(
        rename = "createLogFile",
        default,
        deserialize_with = "de_opt_bool_flex"
    )]
    create_log_file: Option<bool>,
    #[serde(rename = "logFileVerbosity", default)]
    log_file_verbosity: Option<LogFileVerbosity>,
    #[serde(
        rename = "enableDefaultLogger",
        default,
        deserialize_with = "de_opt_bool_flex"
    )]
    enable_default_logger: Option<bool>,
    #[serde(rename = "customVersion", default)]
    custom_version: Option<String>,
    #[serde(skip)]
    display_name: Option<String>,
}

impl VsBuild {
    /// Create a new `VSBuild@1` builder.
    ///
    /// `solution` is the solution file path or glob pattern
    /// (e.g. `**\\*.sln`).
    pub fn new(solution: impl Into<String>) -> Self {
        Self {
            solution: solution.into(),
            vs_version: None,
            msbuild_args: None,
            platform: None,
            configuration: None,
            clean: None,
            maximum_cpu_count: None,
            restore_nuget_packages: None,
            msbuild_architecture: None,
            log_project_events: None,
            create_log_file: None,
            log_file_verbosity: None,
            enable_default_logger: None,
            custom_version: None,
            display_name: None,
        }
    }

    /// `vsVersion` — Visual Studio version to use (default: latest).
    pub fn vs_version(mut self, value: VsVersion) -> Self {
        self.vs_version = Some(value);
        self
    }

    /// `msbuildArgs` — additional MSBuild arguments.
    pub fn msbuild_args(mut self, value: impl Into<String>) -> Self {
        self.msbuild_args = Some(value.into());
        self
    }

    /// `platform` — target platform (e.g. `x86`, `x64`, `Any CPU`).
    pub fn platform(mut self, value: impl Into<String>) -> Self {
        self.platform = Some(value.into());
        self
    }

    /// `configuration` — build configuration (e.g. `Debug`, `Release`).
    pub fn configuration(mut self, value: impl Into<String>) -> Self {
        self.configuration = Some(value.into());
        self
    }

    /// `clean` — run a clean build (default: false).
    pub fn clean(mut self, value: bool) -> Self {
        self.clean = Some(value);
        self
    }

    /// `maximumCpuCount` — use all available CPUs (default: false).
    pub fn maximum_cpu_count(mut self, value: bool) -> Self {
        self.maximum_cpu_count = Some(value);
        self
    }

    /// `restoreNugetPackages` — restore NuGet packages before building (default: false).
    pub fn restore_nuget_packages(mut self, value: bool) -> Self {
        self.restore_nuget_packages = Some(value);
        self
    }

    /// `msbuildArchitecture` — MSBuild process architecture (default: x86).
    pub fn msbuild_architecture(mut self, value: MsBuildArchitecture) -> Self {
        self.msbuild_architecture = Some(value);
        self
    }

    /// `logProjectEvents` — log MSBuild project-level events (default: true).
    pub fn log_project_events(mut self, value: bool) -> Self {
        self.log_project_events = Some(value);
        self
    }

    /// `createLogFile` — write a build log file (default: false).
    pub fn create_log_file(mut self, value: bool) -> Self {
        self.create_log_file = Some(value);
        self
    }

    /// `logFileVerbosity` — verbosity of the log file (default: normal).
    pub fn log_file_verbosity(mut self, value: LogFileVerbosity) -> Self {
        self.log_file_verbosity = Some(value);
        self
    }

    /// `enableDefaultLogger` — enable the default MSBuild logger (default: true).
    pub fn enable_default_logger(mut self, value: bool) -> Self {
        self.enable_default_logger = Some(value);
        self
    }

    /// `customVersion` — custom Visual Studio version string (overrides `vsVersion`).
    pub fn custom_version(mut self, value: impl Into<String>) -> Self {
        self.custom_version = Some(value.into());
        self
    }

    /// Override the default `displayName` (`"Build solution"`).
    pub fn with_display_name(mut self, value: impl Into<String>) -> Self {
        self.display_name = Some(value.into());
        self
    }

    /// Lower into a [`TaskStep`].
    pub fn into_step(self) -> TaskStep {
        let mut t = TaskStep::new(
            "VSBuild@1",
            self.display_name.unwrap_or_else(|| "Build solution".into()),
        )
        .with_input("solution", self.solution);
        if let Some(v) = self.vs_version {
            t = t.with_input("vsVersion", v.as_ado_str());
        }
        push_opt(&mut t, "msbuildArgs", self.msbuild_args);
        push_opt(&mut t, "platform", self.platform);
        push_opt(&mut t, "configuration", self.configuration);
        push_bool(&mut t, "clean", self.clean);
        push_bool(&mut t, "maximumCpuCount", self.maximum_cpu_count);
        push_bool(&mut t, "restoreNugetPackages", self.restore_nuget_packages);
        if let Some(v) = self.msbuild_architecture {
            t = t.with_input("msbuildArchitecture", v.as_ado_str());
        }
        push_bool(&mut t, "logProjectEvents", self.log_project_events);
        push_bool(&mut t, "createLogFile", self.create_log_file);
        if let Some(v) = self.log_file_verbosity {
            t = t.with_input("logFileVerbosity", v.as_ado_str());
        }
        push_bool(&mut t, "enableDefaultLogger", self.enable_default_logger);
        push_opt(&mut t, "customVersion", self.custom_version);
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimal_build_with_solution_glob() {
        let t = VsBuild::new("**\\*.sln").into_step();
        assert_eq!(t.task, "VSBuild@1");
        assert_eq!(t.display_name, "Build solution");
        assert_eq!(
            t.inputs.get("solution").map(String::as_str),
            Some("**\\*.sln")
        );
        // No optional inputs should be emitted.
        assert!(t.inputs.get("vsVersion").is_none());
        assert!(t.inputs.get("msbuildArgs").is_none());
        assert!(t.inputs.get("platform").is_none());
        assert!(t.inputs.get("configuration").is_none());
    }

    #[test]
    fn release_build_with_options() {
        let t = VsBuild::new("MyApp.sln")
            .configuration("Release")
            .platform("x64")
            .msbuild_args("/p:DeployOnBuild=true")
            .clean(true)
            .maximum_cpu_count(true)
            .into_step();
        assert_eq!(
            t.inputs.get("configuration").map(String::as_str),
            Some("Release")
        );
        assert_eq!(t.inputs.get("platform").map(String::as_str), Some("x64"));
        assert_eq!(
            t.inputs.get("msbuildArgs").map(String::as_str),
            Some("/p:DeployOnBuild=true")
        );
        assert_eq!(t.inputs.get("clean").map(String::as_str), Some("true"));
        assert_eq!(
            t.inputs.get("maximumCpuCount").map(String::as_str),
            Some("true")
        );
    }

    #[test]
    fn vs_version_enum_emits_correct_token() {
        let t = VsBuild::new("**\\*.sln")
            .vs_version(VsVersion::Vs2019)
            .into_step();
        assert_eq!(t.inputs.get("vsVersion").map(String::as_str), Some("17.0"));

        let t = VsBuild::new("**\\*.sln")
            .vs_version(VsVersion::Latest)
            .into_step();
        assert_eq!(
            t.inputs.get("vsVersion").map(String::as_str),
            Some("latest")
        );
    }

    #[test]
    fn msbuild_architecture_enum_emits_correct_token() {
        let t = VsBuild::new("**\\*.sln")
            .msbuild_architecture(MsBuildArchitecture::X64)
            .into_step();
        assert_eq!(
            t.inputs.get("msbuildArchitecture").map(String::as_str),
            Some("x64")
        );
    }

    #[test]
    fn log_verbosity_enum_emits_correct_token() {
        let t = VsBuild::new("**\\*.sln")
            .create_log_file(true)
            .log_file_verbosity(LogFileVerbosity::Detailed)
            .into_step();
        assert_eq!(
            t.inputs.get("createLogFile").map(String::as_str),
            Some("true")
        );
        assert_eq!(
            t.inputs.get("logFileVerbosity").map(String::as_str),
            Some("detailed")
        );
    }

    #[test]
    fn display_name_override() {
        let t = VsBuild::new("**\\*.sln")
            .with_display_name("Build MyApp")
            .into_step();
        assert_eq!(t.display_name, "Build MyApp");
    }

    #[test]
    fn untouched_optionals_are_absent() {
        let t = VsBuild::new("**\\*.sln").into_step();
        for key in &[
            "vsVersion",
            "msbuildArgs",
            "platform",
            "configuration",
            "clean",
            "maximumCpuCount",
            "restoreNugetPackages",
            "msbuildArchitecture",
            "logProjectEvents",
            "createLogFile",
            "logFileVerbosity",
            "enableDefaultLogger",
            "customVersion",
        ] {
            assert!(t.inputs.get(*key).is_none(), "expected {key} to be absent");
        }
    }
}
