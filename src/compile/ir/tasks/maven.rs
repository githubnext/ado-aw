//! Typed builder for `Maven@3`.

use super::common::{de_opt_bool_flex, push_bool, push_opt};
use crate::compile::ir::step::TaskStep;
use serde::Deserialize;

/// Code-coverage tool selection for `Maven@3`.
///
/// Controls the `codeCoverageToolOption` input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum CodeCoverageTool {
    /// No code-coverage instrumentation (ADO default).
    #[serde(rename = "None")]
    None,
    /// Cobertura coverage format.
    #[serde(rename = "Cobertura")]
    Cobertura,
    /// JaCoCo coverage format.
    #[serde(rename = "JaCoCo")]
    JaCoCo,
}

impl CodeCoverageTool {
    /// Return the exact ADO token for this value.
    pub fn as_ado_str(self) -> &'static str {
        match self {
            CodeCoverageTool::None => "None",
            CodeCoverageTool::Cobertura => "Cobertura",
            CodeCoverageTool::JaCoCo => "JaCoCo",
        }
    }
}

/// JDK version selection for `Maven@3`.
///
/// Controls the `jdkVersionOption` input (used when `javaHomeOption` is
/// `JDKVersion`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum JdkVersion {
    /// Use the JDK already on PATH (ADO default).
    #[serde(rename = "default")]
    Default,
    /// JDK 21.
    #[serde(rename = "1.21")]
    V21,
    /// JDK 17.
    #[serde(rename = "1.17")]
    V17,
    /// JDK 11.
    #[serde(rename = "1.11")]
    V11,
    /// JDK 10.
    #[serde(rename = "1.10")]
    V10,
    /// JDK 9.
    #[serde(rename = "1.9")]
    V9,
    /// JDK 8.
    #[serde(rename = "1.8")]
    V8,
    /// JDK 7.
    #[serde(rename = "1.7")]
    V7,
    /// JDK 6.
    #[serde(rename = "1.6")]
    V6,
}

impl JdkVersion {
    /// Return the exact ADO token for this value.
    pub fn as_ado_str(self) -> &'static str {
        match self {
            JdkVersion::Default => "default",
            JdkVersion::V21 => "1.21",
            JdkVersion::V17 => "1.17",
            JdkVersion::V11 => "1.11",
            JdkVersion::V10 => "1.10",
            JdkVersion::V9 => "1.9",
            JdkVersion::V8 => "1.8",
            JdkVersion::V7 => "1.7",
            JdkVersion::V6 => "1.6",
        }
    }
}

/// JDK architecture for `Maven@3`.
///
/// Controls the `jdkArchitectureOption` input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum JdkArchitecture {
    /// 32-bit JDK.
    #[serde(rename = "x86")]
    X86,
    /// 64-bit JDK (ADO default).
    #[serde(rename = "x64")]
    X64,
    /// ARM 64-bit JDK.
    #[serde(rename = "arm64")]
    Arm64,
}

impl JdkArchitecture {
    /// Return the exact ADO token for this value.
    pub fn as_ado_str(self) -> &'static str {
        match self {
            JdkArchitecture::X86 => "x86",
            JdkArchitecture::X64 => "x64",
            JdkArchitecture::Arm64 => "arm64",
        }
    }
}

/// Builder for a [`TaskStep`] invoking `Maven@3`.
///
/// Builds, tests, and deploys with Apache Maven. Supply the path to the POM
/// file and optionally override goals, JDK version, code-coverage tool, etc.
/// Only inputs that are explicitly set are emitted, so the generated YAML stays
/// minimal and matches the task's own defaults.
///
/// ADO task reference:
/// <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/maven-v3>
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Maven {
    #[serde(rename = "mavenPOMFile")]
    maven_pom_file: String,
    // Build
    #[serde(rename = "goals", default)]
    goals: Option<String>,
    #[serde(rename = "options", default)]
    options: Option<String>,
    // JUnit test results
    #[serde(
        rename = "publishJUnitResults",
        default,
        deserialize_with = "de_opt_bool_flex"
    )]
    publish_junit_results: Option<bool>,
    #[serde(rename = "testResultsFiles", default)]
    test_results_files: Option<String>,
    #[serde(rename = "testRunTitle", default)]
    test_run_title: Option<String>,
    #[serde(
        rename = "allowBrokenSymlinks",
        default,
        deserialize_with = "de_opt_bool_flex"
    )]
    allow_broken_symlinks: Option<bool>,
    // Code coverage
    #[serde(rename = "codeCoverageToolOption", default)]
    code_coverage_tool: Option<CodeCoverageTool>,
    #[serde(rename = "codeCoverageClassFilter", default)]
    code_coverage_class_filter: Option<String>,
    #[serde(rename = "codeCoverageClassFilesDirectories", default)]
    code_coverage_class_files_directories: Option<String>,
    #[serde(rename = "codeCoverageSourceDirectories", default)]
    code_coverage_source_directories: Option<String>,
    #[serde(
        rename = "codeCoverageFailIfEmpty",
        default,
        deserialize_with = "de_opt_bool_flex"
    )]
    code_coverage_fail_if_empty: Option<bool>,
    #[serde(
        rename = "codeCoverageRestoreOriginalPomXml",
        default,
        deserialize_with = "de_opt_bool_flex"
    )]
    code_coverage_restore_original_pom_xml: Option<bool>,
    // JDK / advanced
    #[serde(rename = "jdkVersionOption", default)]
    jdk_version: Option<JdkVersion>,
    #[serde(rename = "jdkArchitectureOption", default)]
    jdk_architecture: Option<JdkArchitecture>,
    #[serde(rename = "mavenOptions", default)]
    maven_options: Option<String>,
    #[serde(
        rename = "mavenAuthenticateFeed",
        default,
        deserialize_with = "de_opt_bool_flex"
    )]
    maven_authenticate_feed: Option<bool>,
    // Code analysis
    #[serde(
        rename = "checkStyleRunAnalysis",
        default,
        deserialize_with = "de_opt_bool_flex"
    )]
    checkstyle_run_analysis: Option<bool>,
    #[serde(
        rename = "pmdRunAnalysis",
        default,
        deserialize_with = "de_opt_bool_flex"
    )]
    pmd_run_analysis: Option<bool>,
    #[serde(
        rename = "spotBugsRunAnalysis",
        default,
        deserialize_with = "de_opt_bool_flex"
    )]
    spot_bugs_run_analysis: Option<bool>,

    #[serde(skip)]
    display_name: Option<String>,
}

impl Maven {
    /// Required input: path to the Maven POM file (e.g. `"pom.xml"`).
    pub fn new(maven_pom_file: impl Into<String>) -> Self {
        Self {
            maven_pom_file: maven_pom_file.into(),
            goals: None,
            options: None,
            publish_junit_results: None,
            test_results_files: None,
            test_run_title: None,
            allow_broken_symlinks: None,
            code_coverage_tool: None,
            code_coverage_class_filter: None,
            code_coverage_class_files_directories: None,
            code_coverage_source_directories: None,
            code_coverage_fail_if_empty: None,
            code_coverage_restore_original_pom_xml: None,
            jdk_version: None,
            jdk_architecture: None,
            maven_options: None,
            maven_authenticate_feed: None,
            checkstyle_run_analysis: None,
            pmd_run_analysis: None,
            spot_bugs_run_analysis: None,
            display_name: None,
        }
    }

    /// `goals` — space-separated Maven goals (e.g. `"test"`, `"package"`,
    /// `"verify"`). ADO default: `"package"`.
    pub fn goals(mut self, value: impl Into<String>) -> Self {
        self.goals = Some(value.into());
        self
    }

    /// `options` — additional Maven command-line options.
    pub fn options(mut self, value: impl Into<String>) -> Self {
        self.options = Some(value.into());
        self
    }

    /// `publishJUnitResults` — publish JUnit XML results to Azure Pipelines
    /// (ADO default: `true`).
    pub fn publish_junit_results(mut self, value: bool) -> Self {
        self.publish_junit_results = Some(value);
        self
    }

    /// `testResultsFiles` — glob for JUnit XML files (ADO default:
    /// `"**/surefire-reports/TEST-*.xml"`).
    pub fn test_results_files(mut self, value: impl Into<String>) -> Self {
        self.test_results_files = Some(value.into());
        self
    }

    /// `testRunTitle` — name shown in the test-run summary.
    pub fn test_run_title(mut self, value: impl Into<String>) -> Self {
        self.test_run_title = Some(value.into());
        self
    }

    /// `allowBrokenSymlinks` — tolerate broken symlinks when scanning for test
    /// results (ADO default: `true`).
    pub fn allow_broken_symlinks(mut self, value: bool) -> Self {
        self.allow_broken_symlinks = Some(value);
        self
    }

    /// `codeCoverageToolOption` — code-coverage instrumentation tool (ADO
    /// default: [`CodeCoverageTool::None`]).
    pub fn code_coverage_tool(mut self, value: CodeCoverageTool) -> Self {
        self.code_coverage_tool = Some(value);
        self
    }

    /// `codeCoverageClassFilter` — inclusion/exclusion filter for coverage
    /// classes (`:` separated patterns).
    pub fn code_coverage_class_filter(mut self, value: impl Into<String>) -> Self {
        self.code_coverage_class_filter = Some(value.into());
        self
    }

    /// `codeCoverageClassFilesDirectories` — directories containing compiled
    /// `.class` files (JaCoCo only).
    pub fn code_coverage_class_files_directories(mut self, value: impl Into<String>) -> Self {
        self.code_coverage_class_files_directories = Some(value.into());
        self
    }

    /// `codeCoverageSourceDirectories` — source directories for JaCoCo
    /// source-level coverage (JaCoCo only).
    pub fn code_coverage_source_directories(mut self, value: impl Into<String>) -> Self {
        self.code_coverage_source_directories = Some(value.into());
        self
    }

    /// `codeCoverageFailIfEmpty` — fail the build if coverage results are
    /// missing (ADO default: `false`).
    pub fn code_coverage_fail_if_empty(mut self, value: bool) -> Self {
        self.code_coverage_fail_if_empty = Some(value);
        self
    }

    /// `codeCoverageRestoreOriginalPomXml` — restore the original `pom.xml`
    /// after the task modifies it for coverage (ADO default: `false`).
    pub fn code_coverage_restore_original_pom_xml(mut self, value: bool) -> Self {
        self.code_coverage_restore_original_pom_xml = Some(value);
        self
    }

    /// `jdkVersionOption` — JDK version to use (ADO default:
    /// [`JdkVersion::Default`]).
    pub fn jdk_version(mut self, value: JdkVersion) -> Self {
        self.jdk_version = Some(value);
        self
    }

    /// `jdkArchitectureOption` — JDK architecture (ADO default:
    /// [`JdkArchitecture::X64`]).
    pub fn jdk_architecture(mut self, value: JdkArchitecture) -> Self {
        self.jdk_architecture = Some(value);
        self
    }

    /// `mavenOptions` — value for the `MAVEN_OPTS` environment variable (ADO
    /// default: `"-Xmx1024m"`).
    pub fn maven_options(mut self, value: impl Into<String>) -> Self {
        self.maven_options = Some(value.into());
        self
    }

    /// `mavenAuthenticateFeed` — authenticate with Azure Artifacts feeds
    /// referenced in `pom.xml` (ADO default: `false`).
    pub fn maven_authenticate_feed(mut self, value: bool) -> Self {
        self.maven_authenticate_feed = Some(value);
        self
    }

    /// `checkStyleRunAnalysis` — run Checkstyle static analysis (ADO default:
    /// `false`).
    pub fn checkstyle_run_analysis(mut self, value: bool) -> Self {
        self.checkstyle_run_analysis = Some(value);
        self
    }

    /// `pmdRunAnalysis` — run PMD static analysis (ADO default: `false`).
    pub fn pmd_run_analysis(mut self, value: bool) -> Self {
        self.pmd_run_analysis = Some(value);
        self
    }

    /// `spotBugsRunAnalysis` — run SpotBugs static analysis (ADO default:
    /// `false`).
    pub fn spot_bugs_run_analysis(mut self, value: bool) -> Self {
        self.spot_bugs_run_analysis = Some(value);
        self
    }

    /// Override the default `displayName` (`"Maven"`).
    pub fn with_display_name(mut self, value: impl Into<String>) -> Self {
        self.display_name = Some(value.into());
        self
    }

    /// Lower into a [`TaskStep`].
    pub fn into_step(self) -> TaskStep {
        let mut t = TaskStep::new(
            "Maven@3",
            self.display_name.unwrap_or_else(|| "Maven".into()),
        )
        .with_input("mavenPOMFile", self.maven_pom_file);

        push_opt(&mut t, "goals", self.goals);
        push_opt(&mut t, "options", self.options);
        push_bool(&mut t, "publishJUnitResults", self.publish_junit_results);
        push_opt(&mut t, "testResultsFiles", self.test_results_files);
        push_opt(&mut t, "testRunTitle", self.test_run_title);
        push_bool(&mut t, "allowBrokenSymlinks", self.allow_broken_symlinks);

        if let Some(v) = self.code_coverage_tool {
            t = t.with_input("codeCoverageToolOption", v.as_ado_str());
        }
        push_opt(
            &mut t,
            "codeCoverageClassFilter",
            self.code_coverage_class_filter,
        );
        push_opt(
            &mut t,
            "codeCoverageClassFilesDirectories",
            self.code_coverage_class_files_directories,
        );
        push_opt(
            &mut t,
            "codeCoverageSourceDirectories",
            self.code_coverage_source_directories,
        );
        push_bool(
            &mut t,
            "codeCoverageFailIfEmpty",
            self.code_coverage_fail_if_empty,
        );
        push_bool(
            &mut t,
            "codeCoverageRestoreOriginalPomXml",
            self.code_coverage_restore_original_pom_xml,
        );

        if let Some(v) = self.jdk_version {
            t = t.with_input("jdkVersionOption", v.as_ado_str());
        }
        if let Some(v) = self.jdk_architecture {
            t = t.with_input("jdkArchitectureOption", v.as_ado_str());
        }
        push_opt(&mut t, "mavenOptions", self.maven_options);
        push_bool(
            &mut t,
            "mavenAuthenticateFeed",
            self.maven_authenticate_feed,
        );
        push_bool(
            &mut t,
            "checkStyleRunAnalysis",
            self.checkstyle_run_analysis,
        );
        push_bool(&mut t, "pmdRunAnalysis", self.pmd_run_analysis);
        push_bool(&mut t, "spotBugsRunAnalysis", self.spot_bugs_run_analysis);

        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimal_build_sets_task_and_pom() {
        let t = Maven::new("pom.xml").into_step();
        assert_eq!(t.task, "Maven@3");
        assert_eq!(t.display_name, "Maven");
        assert_eq!(
            t.inputs.get("mavenPOMFile").map(String::as_str),
            Some("pom.xml")
        );
        // Optional inputs absent by default.
        assert!(t.inputs.get("goals").is_none());
        assert!(t.inputs.get("options").is_none());
        assert!(t.inputs.get("publishJUnitResults").is_none());
    }

    #[test]
    fn goals_and_options_set_correctly() {
        let t = Maven::new("pom.xml")
            .goals("test")
            .options("-DskipTests=false -pl module-a")
            .into_step();
        assert_eq!(t.inputs.get("goals").map(String::as_str), Some("test"));
        assert_eq!(
            t.inputs.get("options").map(String::as_str),
            Some("-DskipTests=false -pl module-a")
        );
    }

    #[test]
    fn junit_results_inputs() {
        let t = Maven::new("pom.xml")
            .publish_junit_results(true)
            .test_results_files("**/surefire-reports/TEST-*.xml")
            .test_run_title("Unit Tests")
            .allow_broken_symlinks(false)
            .into_step();
        assert_eq!(
            t.inputs.get("publishJUnitResults").map(String::as_str),
            Some("true")
        );
        assert_eq!(
            t.inputs.get("testResultsFiles").map(String::as_str),
            Some("**/surefire-reports/TEST-*.xml")
        );
        assert_eq!(
            t.inputs.get("testRunTitle").map(String::as_str),
            Some("Unit Tests")
        );
        assert_eq!(
            t.inputs.get("allowBrokenSymlinks").map(String::as_str),
            Some("false")
        );
    }

    #[test]
    fn jacoco_coverage_inputs() {
        let t = Maven::new("pom.xml")
            .code_coverage_tool(CodeCoverageTool::JaCoCo)
            .code_coverage_class_filter("+:com.example.*,-:com.example.generated.*")
            .code_coverage_fail_if_empty(true)
            .into_step();
        assert_eq!(
            t.inputs.get("codeCoverageToolOption").map(String::as_str),
            Some("JaCoCo")
        );
        assert_eq!(
            t.inputs.get("codeCoverageClassFilter").map(String::as_str),
            Some("+:com.example.*,-:com.example.generated.*")
        );
        assert_eq!(
            t.inputs.get("codeCoverageFailIfEmpty").map(String::as_str),
            Some("true")
        );
    }

    #[test]
    fn jdk_version_and_architecture() {
        let t = Maven::new("pom.xml")
            .jdk_version(JdkVersion::V17)
            .jdk_architecture(JdkArchitecture::X64)
            .into_step();
        assert_eq!(
            t.inputs.get("jdkVersionOption").map(String::as_str),
            Some("1.17")
        );
        assert_eq!(
            t.inputs.get("jdkArchitectureOption").map(String::as_str),
            Some("x64")
        );
    }

    #[test]
    fn maven_opts_and_feed_auth() {
        let t = Maven::new("pom.xml")
            .maven_options("-Xmx2048m")
            .maven_authenticate_feed(true)
            .into_step();
        assert_eq!(
            t.inputs.get("mavenOptions").map(String::as_str),
            Some("-Xmx2048m")
        );
        assert_eq!(
            t.inputs.get("mavenAuthenticateFeed").map(String::as_str),
            Some("true")
        );
    }

    #[test]
    fn code_analysis_flags() {
        let t = Maven::new("pom.xml")
            .checkstyle_run_analysis(true)
            .pmd_run_analysis(true)
            .spot_bugs_run_analysis(false)
            .into_step();
        assert_eq!(
            t.inputs.get("checkStyleRunAnalysis").map(String::as_str),
            Some("true")
        );
        assert_eq!(
            t.inputs.get("pmdRunAnalysis").map(String::as_str),
            Some("true")
        );
        assert_eq!(
            t.inputs.get("spotBugsRunAnalysis").map(String::as_str),
            Some("false")
        );
    }

    #[test]
    fn display_name_override() {
        let t = Maven::new("pom.xml")
            .with_display_name("Build and Test")
            .into_step();
        assert_eq!(t.display_name, "Build and Test");
    }

    #[test]
    fn code_coverage_tool_ado_strings() {
        assert_eq!(CodeCoverageTool::None.as_ado_str(), "None");
        assert_eq!(CodeCoverageTool::Cobertura.as_ado_str(), "Cobertura");
        assert_eq!(CodeCoverageTool::JaCoCo.as_ado_str(), "JaCoCo");
    }

    #[test]
    fn jdk_version_ado_strings() {
        assert_eq!(JdkVersion::Default.as_ado_str(), "default");
        assert_eq!(JdkVersion::V21.as_ado_str(), "1.21");
        assert_eq!(JdkVersion::V17.as_ado_str(), "1.17");
        assert_eq!(JdkVersion::V11.as_ado_str(), "1.11");
        assert_eq!(JdkVersion::V8.as_ado_str(), "1.8");
    }

    #[test]
    fn jdk_architecture_ado_strings() {
        assert_eq!(JdkArchitecture::X86.as_ado_str(), "x86");
        assert_eq!(JdkArchitecture::X64.as_ado_str(), "x64");
        assert_eq!(JdkArchitecture::Arm64.as_ado_str(), "arm64");
    }

    #[test]
    fn unset_optional_inputs_absent() {
        // Verify that none of the optional inputs appear when not set.
        let t = Maven::new("submodule/pom.xml").into_step();
        let absent = [
            "goals",
            "options",
            "publishJUnitResults",
            "testResultsFiles",
            "testRunTitle",
            "allowBrokenSymlinks",
            "codeCoverageToolOption",
            "jdkVersionOption",
            "jdkArchitectureOption",
            "mavenOptions",
            "mavenAuthenticateFeed",
            "checkStyleRunAnalysis",
            "pmdRunAnalysis",
            "spotBugsRunAnalysis",
        ];
        for key in absent {
            assert!(t.inputs.get(key).is_none(), "expected {key} to be absent");
        }
    }
}
