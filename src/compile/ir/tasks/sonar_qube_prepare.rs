//! Typed builder for `SonarQubePrepare@8`.
//!
//! This is a mode-dispatch task: the `scannerMode` input selects between three
//! scanner integration strategies (`dotnet`, `cli`, `other`), each exposing a
//! different set of inputs. Per-mode data is carried in [`ScannerMode`] variants
//! so that applying an input to the wrong mode is unrepresentable.
//!
//! ADO task reference:
//! <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/sonar-qube-prepare-v8>

use super::common::push_opt;
use crate::compile::ir::step::TaskStep;
use serde::Deserialize;
use serde_yaml::Value;

/// Scanner integration mode for `SonarQubePrepare@8`.
#[derive(Debug, Clone)]
pub enum ScannerMode {
    /// `scannerMode: dotnet` — integrate with .NET builds via the MSBuild
    /// SonarScanner. Use with [`DotNetMode`].
    DotNet(DotNetMode),
    /// `scannerMode: cli` — use the standalone SonarScanner CLI. Configuration
    /// can be sourced from a properties file ([`CliMode::File`]) or provided
    /// inline ([`CliMode::Manual`]).
    Cli(CliMode),
    /// `scannerMode: other` — integrate via Maven or Gradle analysis plugins;
    /// no extra scanner inputs needed.
    Other,
}

/// Per-mode inputs for `scannerMode = dotnet`.
#[derive(Debug, Clone)]
pub struct DotNetMode {
    /// `projectKey` — the SonarQube project key. **Required.**
    pub project_key: String,
    /// `projectName` — the SonarQube project name. Optional.
    pub project_name: Option<String>,
    /// `projectVersion` — the SonarQube project version. Optional.
    pub project_version: Option<String>,
    /// `msBuildVersion` (alias `dotnetScannerVersion`) — the .NET scanner
    /// version to use. Optional; omitting uses the task default.
    pub ms_build_version: Option<String>,
}

impl DotNetMode {
    /// Create a `dotnet` mode configuration with the required project key.
    pub fn new(project_key: impl Into<String>) -> Self {
        Self {
            project_key: project_key.into(),
            project_name: None,
            project_version: None,
            ms_build_version: None,
        }
    }

    /// `projectName` — SonarQube project name displayed in the UI.
    pub fn project_name(mut self, value: impl Into<String>) -> Self {
        self.project_name = Some(value.into());
        self
    }

    /// `projectVersion` — SonarQube project version (e.g. `"1.0"`,
    /// `"$(Build.BuildNumber)"`).
    pub fn project_version(mut self, value: impl Into<String>) -> Self {
        self.project_version = Some(value.into());
        self
    }

    /// `msBuildVersion` — pin the .NET SonarScanner version. Optional.
    pub fn ms_build_version(mut self, value: impl Into<String>) -> Self {
        self.ms_build_version = Some(value.into());
        self
    }
}

/// Configuration sub-mode for `scannerMode = cli`.
#[derive(Debug, Clone)]
pub enum CliMode {
    /// `configMode: file` — read analysis settings from a
    /// `sonar-project.properties` file (or a custom path).
    File(CliFileMode),
    /// `configMode: manual` — specify project identity and sources inline.
    Manual(CliManualMode),
}

/// Inputs for `scannerMode = cli, configMode = file`.
#[derive(Debug, Clone, Default)]
pub struct CliFileMode {
    /// `cliVersion` (alias `cliScannerVersion`) — pin the CLI scanner version.
    /// Optional; omitting uses the task default.
    pub cli_version: Option<String>,
    /// `configFile` — path to the properties file. Optional; ADO default is
    /// `sonar-project.properties`.
    pub config_file: Option<String>,
}

impl CliFileMode {
    pub fn new() -> Self {
        Self::default()
    }

    /// `cliVersion` — pin the SonarScanner CLI version.
    pub fn cli_version(mut self, value: impl Into<String>) -> Self {
        self.cli_version = Some(value.into());
        self
    }

    /// `configFile` — path to the `sonar-project.properties` file.
    pub fn config_file(mut self, value: impl Into<String>) -> Self {
        self.config_file = Some(value.into());
        self
    }
}

/// Inputs for `scannerMode = cli, configMode = manual`.
#[derive(Debug, Clone)]
pub struct CliManualMode {
    /// `cliProjectKey` — the SonarQube project key. **Required.**
    pub project_key: String,
    /// `cliVersion` (alias `cliScannerVersion`) — pin the CLI scanner version.
    pub cli_version: Option<String>,
    /// `cliProjectName` — the SonarQube project name. Optional.
    pub project_name: Option<String>,
    /// `cliProjectVersion` — the SonarQube project version. Optional.
    pub project_version: Option<String>,
    /// `cliSources` — sources directory root. Optional; ADO default is `"."`.
    pub sources: Option<String>,
}

impl CliManualMode {
    /// Create a manual CLI mode configuration with the required project key.
    pub fn new(project_key: impl Into<String>) -> Self {
        Self {
            project_key: project_key.into(),
            cli_version: None,
            project_name: None,
            project_version: None,
            sources: None,
        }
    }

    /// `cliVersion` — pin the SonarScanner CLI version.
    pub fn cli_version(mut self, value: impl Into<String>) -> Self {
        self.cli_version = Some(value.into());
        self
    }

    /// `cliProjectName` — project name displayed in SonarQube UI.
    pub fn project_name(mut self, value: impl Into<String>) -> Self {
        self.project_name = Some(value.into());
        self
    }

    /// `cliProjectVersion` — project version string (e.g. `"1.0"`).
    pub fn project_version(mut self, value: impl Into<String>) -> Self {
        self.project_version = Some(value.into());
        self
    }

    /// `cliSources` — path to the sources directory. ADO default is `"."`.
    pub fn sources(mut self, value: impl Into<String>) -> Self {
        self.sources = Some(value.into());
        self
    }
}

/// Builder for a [`TaskStep`] invoking `SonarQubePrepare@8`.
///
/// Prepares the SonarQube analysis configuration before build steps run. Pair
/// with [`SonarQubeAnalyze@8`] (post-build) and [`SonarQubePublish@8`] to
/// complete a full analysis run.
///
/// The `scannerMode` input determines which ADO inputs are emitted; each mode
/// carries its own typed data so invalid mode/input combinations are
/// unrepresentable.
///
/// ADO task reference:
/// <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/sonar-qube-prepare-v8>
#[derive(Debug, Clone)]
pub struct SonarQubePrepare {
    /// `SonarQube` — the SonarQube service endpoint (service connection name).
    sonar_qube: String,
    mode: ScannerMode,
    extra_properties: Option<String>,
    display_name: Option<String>,
}

impl SonarQubePrepare {
    /// Construct with the `dotnet` scanner mode. `sonar_qube` is the service
    /// connection name; `mode` carries the required project key and optional
    /// `.NET`-specific inputs.
    pub fn dotnet(sonar_qube: impl Into<String>, mode: DotNetMode) -> Self {
        Self::new(sonar_qube, ScannerMode::DotNet(mode))
    }

    /// Construct with the `cli` scanner mode. `sonar_qube` is the service
    /// connection name; `mode` selects file-based or manual configuration.
    pub fn cli(sonar_qube: impl Into<String>, mode: CliMode) -> Self {
        Self::new(sonar_qube, ScannerMode::Cli(mode))
    }

    /// Construct with the `other` scanner mode (Maven / Gradle). No extra
    /// scanner inputs are needed; the build plugin handles analysis internally.
    pub fn other(sonar_qube: impl Into<String>) -> Self {
        Self::new(sonar_qube, ScannerMode::Other)
    }

    fn new(sonar_qube: impl Into<String>, mode: ScannerMode) -> Self {
        Self {
            sonar_qube: sonar_qube.into(),
            mode,
            extra_properties: None,
            display_name: None,
        }
    }

    /// `extraProperties` — additional SonarQube analysis properties, one
    /// `key=value` per line (e.g. `"sonar.exclusions=**/*.bin"`).
    pub fn extra_properties(mut self, value: impl Into<String>) -> Self {
        self.extra_properties = Some(value.into());
        self
    }

    /// Override the default `displayName`.
    pub fn with_display_name(mut self, value: impl Into<String>) -> Self {
        self.display_name = Some(value.into());
        self
    }

    /// Lower into a [`TaskStep`].
    pub fn into_step(self) -> TaskStep {
        let default_display = match &self.mode {
            ScannerMode::DotNet(_) => "Prepare SonarQube Analysis (.NET)",
            ScannerMode::Cli(_) => "Prepare SonarQube Analysis (CLI)",
            ScannerMode::Other => "Prepare SonarQube Analysis",
        };
        let scanner_mode_str = match &self.mode {
            ScannerMode::DotNet(_) => "dotnet",
            ScannerMode::Cli(_) => "cli",
            ScannerMode::Other => "other",
        };
        let mut t = TaskStep::new(
            "SonarQubePrepare@8",
            self.display_name.unwrap_or_else(|| default_display.into()),
        )
        .with_input("SonarQube", self.sonar_qube)
        .with_input("scannerMode", scanner_mode_str);

        match self.mode {
            ScannerMode::DotNet(m) => {
                t = t.with_input("projectKey", m.project_key);
                push_opt(&mut t, "projectName", m.project_name);
                push_opt(&mut t, "projectVersion", m.project_version);
                push_opt(&mut t, "msBuildVersion", m.ms_build_version);
            }
            ScannerMode::Cli(CliMode::File(m)) => {
                t = t.with_input("configMode", "file");
                push_opt(&mut t, "cliVersion", m.cli_version);
                push_opt(&mut t, "configFile", m.config_file);
            }
            ScannerMode::Cli(CliMode::Manual(m)) => {
                t = t.with_input("configMode", "manual");
                t = t.with_input("cliProjectKey", m.project_key);
                push_opt(&mut t, "cliVersion", m.cli_version);
                push_opt(&mut t, "cliProjectName", m.project_name);
                push_opt(&mut t, "cliProjectVersion", m.project_version);
                push_opt(&mut t, "cliSources", m.sources);
            }
            ScannerMode::Other => {}
        }

        push_opt(&mut t, "extraProperties", self.extra_properties);
        t
    }
}

/// Validate an authored `SonarQubePrepare@8` `inputs:` mapping (advisory
/// front-matter validation, see [`super::parse`]).
///
/// This is a mode-dispatch task: `scannerMode` (default `dotnet`) selects the
/// applicable inputs, and within `cli` mode `configMode` (default `file`)
/// selects a further sub-schema. Each mode is validated against a
/// `deny_unknown_fields` schema so an input supplied for the wrong mode is
/// reported.
pub(crate) fn validate_inputs(inputs: Value) -> Result<(), String> {
    let mut map = match inputs {
        Value::Mapping(m) => m,
        Value::Null => Default::default(),
        other => return Err(format!("`inputs` must be a mapping, got {other:?}")),
    };

    // Common inputs shared by every mode.
    match map.get("SonarQube") {
        Some(v) if v.is_string() => {}
        Some(_) => return Err("`SonarQube` must be a string".to_string()),
        None => return Err("missing required input `SonarQube`".to_string()),
    }
    map.remove("SonarQube");
    if let Some(v) = map.get("extraProperties")
        && !v.is_string()
    {
        return Err("`extraProperties` must be a string".to_string());
    }
    map.remove("extraProperties");

    let scanner_mode = match map.remove("scannerMode") {
        Some(v) => v
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| "`scannerMode` must be a string".to_string())?,
        None => "dotnet".to_string(),
    };

    match scanner_mode.as_str() {
        "dotnet" => serde_yaml::from_value::<DotNetSpec>(Value::Mapping(map))
            .map(drop)
            .map_err(|e| format!("scannerMode `dotnet`: {e}")),
        "other" => serde_yaml::from_value::<OtherSpec>(Value::Mapping(map))
            .map(drop)
            .map_err(|e| format!("scannerMode `other`: {e}")),
        "cli" => {
            let config_mode = match map.remove("configMode") {
                Some(v) => v
                    .as_str()
                    .map(str::to_string)
                    .ok_or_else(|| "`configMode` must be a string".to_string())?,
                None => "file".to_string(),
            };
            let rest = Value::Mapping(map);
            match config_mode.as_str() {
                "file" => serde_yaml::from_value::<CliFileSpec>(rest)
                    .map(drop)
                    .map_err(|e| format!("scannerMode `cli`, configMode `file`: {e}")),
                "manual" => serde_yaml::from_value::<CliManualSpec>(rest)
                    .map(drop)
                    .map_err(|e| format!("scannerMode `cli`, configMode `manual`: {e}")),
                other => Err(format!("unknown configMode `{other}` (expected file|manual)")),
            }
        }
        other => Err(format!(
            "unknown scannerMode `{other}` (expected dotnet|cli|other)"
        )),
    }
}

/// Inputs valid for `scannerMode = dotnet`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DotNetSpec {
    #[serde(rename = "projectKey")]
    _project_key: String,
    #[serde(rename = "projectName", default)]
    _project_name: Option<String>,
    #[serde(rename = "projectVersion", default)]
    _project_version: Option<String>,
    #[serde(rename = "dotnetScannerVersion", alias = "msBuildVersion", default)]
    _scanner_version: Option<String>,
}

/// Inputs valid for `scannerMode = cli, configMode = file`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CliFileSpec {
    #[serde(rename = "cliScannerVersion", alias = "cliVersion", default)]
    _cli_version: Option<String>,
    #[serde(rename = "configFile", default)]
    _config_file: Option<String>,
}

/// Inputs valid for `scannerMode = cli, configMode = manual`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CliManualSpec {
    #[serde(rename = "cliProjectKey")]
    _project_key: String,
    #[serde(rename = "cliScannerVersion", alias = "cliVersion", default)]
    _cli_version: Option<String>,
    #[serde(rename = "cliProjectName", default)]
    _project_name: Option<String>,
    #[serde(rename = "cliProjectVersion", default)]
    _project_version: Option<String>,
    #[serde(rename = "cliSources", default)]
    _sources: Option<String>,
}

/// Inputs valid for `scannerMode = other` (no extra scanner inputs).
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct OtherSpec {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dotnet_mode_required_inputs() {        let t = SonarQubePrepare::dotnet(
            "MySonarQubeConnection",
            DotNetMode::new("my-project-key"),
        )
        .into_step();
        assert_eq!(t.task, "SonarQubePrepare@8");
        assert_eq!(t.display_name, "Prepare SonarQube Analysis (.NET)");
        assert_eq!(
            t.inputs.get("SonarQube").map(String::as_str),
            Some("MySonarQubeConnection")
        );
        assert_eq!(
            t.inputs.get("scannerMode").map(String::as_str),
            Some("dotnet")
        );
        assert_eq!(
            t.inputs.get("projectKey").map(String::as_str),
            Some("my-project-key")
        );
        assert!(t.inputs.get("projectName").is_none());
        assert!(t.inputs.get("projectVersion").is_none());
        assert!(t.inputs.get("msBuildVersion").is_none());
        assert!(t.inputs.get("configMode").is_none());
    }

    #[test]
    fn dotnet_mode_all_optionals() {
        let t = SonarQubePrepare::dotnet(
            "SonarEndpoint",
            DotNetMode::new("proj-key")
                .project_name("My Project")
                .project_version("$(Build.BuildNumber)")
                .ms_build_version("5.13.0"),
        )
        .extra_properties("sonar.exclusions=**/*.bin")
        .into_step();
        assert_eq!(
            t.inputs.get("projectName").map(String::as_str),
            Some("My Project")
        );
        assert_eq!(
            t.inputs.get("projectVersion").map(String::as_str),
            Some("$(Build.BuildNumber)")
        );
        assert_eq!(
            t.inputs.get("msBuildVersion").map(String::as_str),
            Some("5.13.0")
        );
        assert_eq!(
            t.inputs.get("extraProperties").map(String::as_str),
            Some("sonar.exclusions=**/*.bin")
        );
    }

    #[test]
    fn cli_file_mode_defaults() {
        let t = SonarQubePrepare::cli(
            "SonarEndpoint",
            CliMode::File(CliFileMode::new()),
        )
        .into_step();
        assert_eq!(t.display_name, "Prepare SonarQube Analysis (CLI)");
        assert_eq!(
            t.inputs.get("scannerMode").map(String::as_str),
            Some("cli")
        );
        assert_eq!(
            t.inputs.get("configMode").map(String::as_str),
            Some("file")
        );
        assert!(t.inputs.get("configFile").is_none());
        assert!(t.inputs.get("cliVersion").is_none());
    }

    #[test]
    fn cli_file_mode_with_custom_config_file() {
        let t = SonarQubePrepare::cli(
            "SonarEndpoint",
            CliMode::File(
                CliFileMode::new()
                    .config_file("my-sonar.properties")
                    .cli_version("5.0.1"),
            ),
        )
        .into_step();
        assert_eq!(
            t.inputs.get("configFile").map(String::as_str),
            Some("my-sonar.properties")
        );
        assert_eq!(
            t.inputs.get("cliVersion").map(String::as_str),
            Some("5.0.1")
        );
    }

    #[test]
    fn cli_manual_mode_required_inputs() {
        let t = SonarQubePrepare::cli(
            "SonarEndpoint",
            CliMode::Manual(CliManualMode::new("my-project-key")),
        )
        .into_step();
        assert_eq!(
            t.inputs.get("configMode").map(String::as_str),
            Some("manual")
        );
        assert_eq!(
            t.inputs.get("cliProjectKey").map(String::as_str),
            Some("my-project-key")
        );
        assert!(t.inputs.get("cliProjectName").is_none());
        assert!(t.inputs.get("cliProjectVersion").is_none());
        assert!(t.inputs.get("cliSources").is_none());
    }

    #[test]
    fn cli_manual_mode_all_optionals() {
        let t = SonarQubePrepare::cli(
            "SonarEndpoint",
            CliMode::Manual(
                CliManualMode::new("proj-key")
                    .project_name("My Project")
                    .project_version("2.0")
                    .sources("src/")
                    .cli_version("5.0.1"),
            ),
        )
        .into_step();
        assert_eq!(
            t.inputs.get("cliProjectName").map(String::as_str),
            Some("My Project")
        );
        assert_eq!(
            t.inputs.get("cliProjectVersion").map(String::as_str),
            Some("2.0")
        );
        assert_eq!(
            t.inputs.get("cliSources").map(String::as_str),
            Some("src/")
        );
        assert_eq!(
            t.inputs.get("cliVersion").map(String::as_str),
            Some("5.0.1")
        );
    }

    #[test]
    fn other_mode_no_extra_inputs() {
        let t = SonarQubePrepare::other("SonarEndpoint").into_step();
        assert_eq!(t.display_name, "Prepare SonarQube Analysis");
        assert_eq!(
            t.inputs.get("scannerMode").map(String::as_str),
            Some("other")
        );
        assert!(t.inputs.get("projectKey").is_none());
        assert!(t.inputs.get("configMode").is_none());
    }

    #[test]
    fn display_name_override() {
        let t = SonarQubePrepare::dotnet(
            "SonarEndpoint",
            DotNetMode::new("my-key"),
        )
        .with_display_name("Configure SonarQube")
        .into_step();
        assert_eq!(t.display_name, "Configure SonarQube");
    }
}
