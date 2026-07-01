//! Typed builder for `Gradle@3`.
//!
//! `Gradle@3` runs a Gradle build using the Gradle wrapper script. It covers
//! the full range of Gradle invocations: plain builds, test execution with
//! JUnit result publishing, and optional code-coverage reporting.
//!
//! ADO task reference:
//! <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/gradle-v3>

use super::common::{de_opt_bool_flex, push_bool, push_opt};
use crate::compile::ir::step::TaskStep;
use serde::Deserialize;

/// How JAVA_HOME is determined (`javaHomeOption` / `javaHomeSelection` input).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum JavaHomeOption {
    /// `"JDKVersion"` — resolve the JDK install for a specific version.
    #[serde(rename = "JDKVersion")]
    JdkVersion,
    /// `"Path"` — set JAVA_HOME to a user-supplied directory path.
    #[serde(rename = "Path")]
    Path,
}

impl JavaHomeOption {
    /// The exact token the ADO task expects for `javaHomeOption`.
    pub fn as_ado_str(self) -> &'static str {
        match self {
            JavaHomeOption::JdkVersion => "JDKVersion",
            JavaHomeOption::Path => "Path",
        }
    }
}

/// JDK version to install when `javaHomeOption = JDKVersion`
/// (`jdkVersionOption` / `jdkVersion` input).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum JdkVersion {
    /// `"default"` — use whatever JDK is already on PATH.
    #[serde(rename = "default")]
    Default,
    /// `"1.21"` — JDK 21.
    #[serde(rename = "1.21")]
    V1_21,
    /// `"1.17"` — JDK 17.
    #[serde(rename = "1.17")]
    V1_17,
    /// `"1.11"` — JDK 11.
    #[serde(rename = "1.11")]
    V1_11,
    /// `"1.10"` — JDK 10.
    #[serde(rename = "1.10")]
    V1_10,
    /// `"1.9"` — JDK 9.
    #[serde(rename = "1.9")]
    V1_9,
    /// `"1.8"` — JDK 8.
    #[serde(rename = "1.8")]
    V1_8,
    /// `"1.7"` — JDK 7.
    #[serde(rename = "1.7")]
    V1_7,
    /// `"1.6"` — JDK 6.
    #[serde(rename = "1.6")]
    V1_6,
}

impl JdkVersion {
    /// The exact token the ADO task expects for `jdkVersionOption`.
    pub fn as_ado_str(self) -> &'static str {
        match self {
            JdkVersion::Default => "default",
            JdkVersion::V1_21 => "1.21",
            JdkVersion::V1_17 => "1.17",
            JdkVersion::V1_11 => "1.11",
            JdkVersion::V1_10 => "1.10",
            JdkVersion::V1_9 => "1.9",
            JdkVersion::V1_8 => "1.8",
            JdkVersion::V1_7 => "1.7",
            JdkVersion::V1_6 => "1.6",
        }
    }
}

/// JDK CPU architecture (`jdkArchitectureOption` / `jdkArchitecture` input).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum JdkArchitecture {
    /// `"x86"` — 32-bit x86.
    #[serde(rename = "x86")]
    X86,
    /// `"x64"` — 64-bit x64 (default).
    #[serde(rename = "x64")]
    X64,
    /// `"arm64"` — ARM 64-bit.
    #[serde(rename = "arm64")]
    Arm64,
}

impl JdkArchitecture {
    /// The exact token the ADO task expects for `jdkArchitectureOption`.
    pub fn as_ado_str(self) -> &'static str {
        match self {
            JdkArchitecture::X86 => "x86",
            JdkArchitecture::X64 => "x64",
            JdkArchitecture::Arm64 => "arm64",
        }
    }
}

/// Code coverage tool (`codeCoverageToolOption` / `codeCoverageTool` input).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum CodeCoverageTool {
    /// `"None"` — no code coverage (default).
    #[serde(rename = "None")]
    None,
    /// `"Cobertura"` — Cobertura XML coverage.
    #[serde(rename = "Cobertura")]
    Cobertura,
    /// `"JaCoCo"` — JaCoCo XML coverage.
    #[serde(rename = "JaCoCo")]
    JaCoCo,
}

impl CodeCoverageTool {
    /// The exact token the ADO task expects for `codeCoverageToolOption`.
    pub fn as_ado_str(self) -> &'static str {
        match self {
            CodeCoverageTool::None => "None",
            CodeCoverageTool::Cobertura => "Cobertura",
            CodeCoverageTool::JaCoCo => "JaCoCo",
        }
    }
}

/// Builder for a [`TaskStep`] invoking `Gradle@3`.
///
/// Runs a Gradle build using the Gradle wrapper. Both required ADO inputs
/// (`gradleWrapperFile` and `tasks`) are positional parameters of [`new`]; all
/// optional inputs have typed chained setters and are only emitted when set.
///
/// # Example
///
/// ```rust
/// use ado_aw::compile::ir::tasks::gradle::{Gradle, JdkVersion};
///
/// let step = Gradle::new("gradlew", "build test")
///     .jdk_version(JdkVersion::V1_17)
///     .publish_junit_results(true)
///     .into_step();
/// assert_eq!(step.task, "Gradle@3");
/// ```
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Gradle {
    /// `gradleWrapperFile` — path to the Gradle wrapper script (e.g. `gradlew`).
    #[serde(rename = "gradleWrapperFile")]
    gradle_wrapper_file: String,
    /// `tasks` — space-separated list of Gradle tasks to execute (e.g. `build test`).
    #[serde(rename = "tasks")]
    tasks: String,
    /// `options` — additional command-line options passed to Gradle.
    #[serde(rename = "options", default)]
    options: Option<String>,
    /// `publishJUnitResults` — publish JUnit XML results to Azure Pipelines.
    #[serde(
        rename = "publishJUnitResults",
        default,
        deserialize_with = "de_opt_bool_flex"
    )]
    publish_junit_results: Option<bool>,
    /// `testResultsFiles` — glob for JUnit XML files (required when `publishJUnitResults = true`).
    #[serde(rename = "testResultsFiles", default)]
    test_results_files: Option<String>,
    /// `codeCoverageToolOption` — optional code coverage tool.
    #[serde(rename = "codeCoverageToolOption", default)]
    code_coverage_tool: Option<CodeCoverageTool>,
    /// `codeCoverageClassFilesDirectories` — class file paths for coverage (required when
    /// `codeCoverageToolOption != None`).
    #[serde(rename = "codeCoverageClassFilesDirectories", default)]
    code_coverage_class_files_dirs: Option<String>,
    /// `codeCoverageFailIfEmpty` — fail the build when coverage results are missing.
    #[serde(
        rename = "codeCoverageFailIfEmpty",
        default,
        deserialize_with = "de_opt_bool_flex"
    )]
    code_coverage_fail_if_empty: Option<bool>,
    /// `javaHomeOption` — how to resolve `JAVA_HOME`.
    #[serde(rename = "javaHomeOption", default)]
    java_home_option: Option<JavaHomeOption>,
    /// `jdkVersionOption` — JDK version (only when `javaHomeOption = JDKVersion`).
    #[serde(rename = "jdkVersionOption", default)]
    jdk_version_option: Option<JdkVersion>,
    /// `jdkDirectory` — path to JDK home (only when `javaHomeOption = Path`).
    #[serde(rename = "jdkDirectory", default)]
    jdk_directory: Option<String>,
    /// `jdkArchitectureOption` — JDK architecture (only when `jdkVersionOption != default`).
    #[serde(rename = "jdkArchitectureOption", default)]
    jdk_architecture: Option<JdkArchitecture>,
    /// `gradleOptions` — value for the `GRADLE_OPTS` environment variable.
    #[serde(rename = "gradleOptions", default)]
    gradle_options: Option<String>,
    /// Override for the step's `displayName`.
    #[serde(skip)]
    display_name: Option<String>,
}

impl Gradle {
    /// Create a new `Gradle@3` builder.
    ///
    /// - `gradle_wrapper_file` — path to the Gradle wrapper script
    ///   (typically `"gradlew"` on Linux/macOS, `"gradlew.bat"` on Windows).
    /// - `tasks` — space-separated list of Gradle tasks to run (e.g. `"build"`,
    ///   `"build test"`, `"clean build"`).
    pub fn new(gradle_wrapper_file: impl Into<String>, tasks: impl Into<String>) -> Self {
        Self {
            gradle_wrapper_file: gradle_wrapper_file.into(),
            tasks: tasks.into(),
            options: None,
            publish_junit_results: None,
            test_results_files: None,
            code_coverage_tool: None,
            code_coverage_class_files_dirs: None,
            code_coverage_fail_if_empty: None,
            java_home_option: None,
            jdk_version_option: None,
            jdk_directory: None,
            jdk_architecture: None,
            gradle_options: None,
            display_name: None,
        }
    }

    /// `options` — additional Gradle command-line options (e.g. `"--no-daemon"`).
    pub fn options(mut self, value: impl Into<String>) -> Self {
        self.options = Some(value.into());
        self
    }

    /// `publishJUnitResults` — whether to publish JUnit XML results to Azure Pipelines.
    pub fn publish_junit_results(mut self, value: bool) -> Self {
        self.publish_junit_results = Some(value);
        self
    }

    /// `testResultsFiles` — glob pattern for JUnit XML result files.
    /// Relevant when [`publish_junit_results`](Self::publish_junit_results) is `true`.
    pub fn test_results_files(mut self, value: impl Into<String>) -> Self {
        self.test_results_files = Some(value.into());
        self
    }

    /// `codeCoverageToolOption` — code coverage tool to use.
    pub fn code_coverage_tool(mut self, value: CodeCoverageTool) -> Self {
        self.code_coverage_tool = Some(value);
        self
    }

    /// `codeCoverageClassFilesDirectories` — comma-separated directories containing
    /// class files for coverage (required when code coverage tool is set).
    pub fn code_coverage_class_files_dirs(mut self, value: impl Into<String>) -> Self {
        self.code_coverage_class_files_dirs = Some(value.into());
        self
    }

    /// `codeCoverageFailIfEmpty` — fail the step when coverage reports are missing.
    pub fn code_coverage_fail_if_empty(mut self, value: bool) -> Self {
        self.code_coverage_fail_if_empty = Some(value);
        self
    }

    /// `javaHomeOption` — how `JAVA_HOME` is determined.
    pub fn java_home_option(mut self, value: JavaHomeOption) -> Self {
        self.java_home_option = Some(value);
        self
    }

    /// `jdkVersionOption` — JDK version to use (only when
    /// [`java_home_option`](Self::java_home_option) is [`JavaHomeOption::JdkVersion`]).
    pub fn jdk_version(mut self, value: JdkVersion) -> Self {
        self.jdk_version_option = Some(value);
        self
    }

    /// `jdkDirectory` — custom path to the JDK home directory (only when
    /// [`java_home_option`](Self::java_home_option) is [`JavaHomeOption::Path`]).
    pub fn jdk_directory(mut self, value: impl Into<String>) -> Self {
        self.jdk_directory = Some(value.into());
        self
    }

    /// `jdkArchitectureOption` — CPU architecture of the JDK to use.
    pub fn jdk_architecture(mut self, value: JdkArchitecture) -> Self {
        self.jdk_architecture = Some(value);
        self
    }

    /// `gradleOptions` — value for the `GRADLE_OPTS` environment variable
    /// (e.g. `"-Xmx2048m"`).
    pub fn gradle_options(mut self, value: impl Into<String>) -> Self {
        self.gradle_options = Some(value.into());
        self
    }

    /// Override the step's `displayName`.
    pub fn with_display_name(mut self, value: impl Into<String>) -> Self {
        self.display_name = Some(value.into());
        self
    }

    /// Build the [`TaskStep`]. Required inputs are always emitted; optional
    /// inputs are only emitted when explicitly set.
    pub fn into_step(self) -> TaskStep {
        let display = self
            .display_name
            .unwrap_or_else(|| format!("Gradle {}", self.tasks));
        let mut t = TaskStep::new("Gradle@3", display)
            .with_input("gradleWrapperFile", self.gradle_wrapper_file)
            .with_input("tasks", self.tasks);

        push_opt(&mut t, "options", self.options);
        push_bool(&mut t, "publishJUnitResults", self.publish_junit_results);
        push_opt(&mut t, "testResultsFiles", self.test_results_files);
        if let Some(v) = self.code_coverage_tool {
            t.inputs.insert(
                "codeCoverageToolOption".to_string(),
                v.as_ado_str().to_string(),
            );
        }
        push_opt(
            &mut t,
            "codeCoverageClassFilesDirectories",
            self.code_coverage_class_files_dirs,
        );
        push_bool(
            &mut t,
            "codeCoverageFailIfEmpty",
            self.code_coverage_fail_if_empty,
        );
        if let Some(v) = self.java_home_option {
            t.inputs
                .insert("javaHomeOption".to_string(), v.as_ado_str().to_string());
        }
        if let Some(v) = self.jdk_version_option {
            t.inputs
                .insert("jdkVersionOption".to_string(), v.as_ado_str().to_string());
        }
        push_opt(&mut t, "jdkDirectory", self.jdk_directory);
        if let Some(v) = self.jdk_architecture {
            t.inputs.insert(
                "jdkArchitectureOption".to_string(),
                v.as_ado_str().to_string(),
            );
        }
        push_opt(&mut t, "gradleOptions", self.gradle_options);

        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimal_build() {
        let t = Gradle::new("gradlew", "build").into_step();
        assert_eq!(t.task, "Gradle@3");
        assert_eq!(
            t.inputs.get("gradleWrapperFile").map(String::as_str),
            Some("gradlew")
        );
        assert_eq!(t.inputs.get("tasks").map(String::as_str), Some("build"));
        // No optional inputs emitted
        assert!(!t.inputs.contains_key("options"));
        assert!(!t.inputs.contains_key("publishJUnitResults"));
        assert!(!t.inputs.contains_key("javaHomeOption"));
    }

    #[test]
    fn display_name_defaults_to_tasks() {
        let t = Gradle::new("gradlew", "build test").into_step();
        assert_eq!(t.display_name, "Gradle build test");
    }

    #[test]
    fn display_name_override() {
        let t = Gradle::new("gradlew", "build")
            .with_display_name("Build project")
            .into_step();
        assert_eq!(t.display_name, "Build project");
    }

    #[test]
    fn with_options_and_junit() {
        let t = Gradle::new("gradlew", "build test")
            .options("--no-daemon --parallel")
            .publish_junit_results(true)
            .test_results_files("**/TEST-*.xml")
            .into_step();
        assert_eq!(
            t.inputs.get("options").map(String::as_str),
            Some("--no-daemon --parallel")
        );
        assert_eq!(
            t.inputs.get("publishJUnitResults").map(String::as_str),
            Some("true")
        );
        assert_eq!(
            t.inputs.get("testResultsFiles").map(String::as_str),
            Some("**/TEST-*.xml")
        );
    }

    #[test]
    fn with_jdk_version() {
        let t = Gradle::new("gradlew", "build")
            .java_home_option(JavaHomeOption::JdkVersion)
            .jdk_version(JdkVersion::V1_17)
            .jdk_architecture(JdkArchitecture::X64)
            .into_step();
        assert_eq!(
            t.inputs.get("javaHomeOption").map(String::as_str),
            Some("JDKVersion")
        );
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
    fn with_jdk_path() {
        let t = Gradle::new("gradlew", "build")
            .java_home_option(JavaHomeOption::Path)
            .jdk_directory("/opt/java/17")
            .into_step();
        assert_eq!(
            t.inputs.get("javaHomeOption").map(String::as_str),
            Some("Path")
        );
        assert_eq!(
            t.inputs.get("jdkDirectory").map(String::as_str),
            Some("/opt/java/17")
        );
    }

    #[test]
    fn with_code_coverage() {
        let t = Gradle::new("gradlew", "build")
            .code_coverage_tool(CodeCoverageTool::JaCoCo)
            .code_coverage_class_files_dirs("build/classes/java/main")
            .code_coverage_fail_if_empty(true)
            .into_step();
        assert_eq!(
            t.inputs.get("codeCoverageToolOption").map(String::as_str),
            Some("JaCoCo")
        );
        assert_eq!(
            t.inputs
                .get("codeCoverageClassFilesDirectories")
                .map(String::as_str),
            Some("build/classes/java/main")
        );
        assert_eq!(
            t.inputs.get("codeCoverageFailIfEmpty").map(String::as_str),
            Some("true")
        );
    }

    #[test]
    fn with_gradle_options() {
        let t = Gradle::new("gradlew", "build")
            .gradle_options("-Xmx2048m")
            .into_step();
        assert_eq!(
            t.inputs.get("gradleOptions").map(String::as_str),
            Some("-Xmx2048m")
        );
    }

    #[test]
    fn jdk_version_ado_tokens() {
        assert_eq!(JdkVersion::Default.as_ado_str(), "default");
        assert_eq!(JdkVersion::V1_21.as_ado_str(), "1.21");
        assert_eq!(JdkVersion::V1_17.as_ado_str(), "1.17");
        assert_eq!(JdkVersion::V1_11.as_ado_str(), "1.11");
        assert_eq!(JdkVersion::V1_8.as_ado_str(), "1.8");
    }

    #[test]
    fn code_coverage_ado_tokens() {
        assert_eq!(CodeCoverageTool::None.as_ado_str(), "None");
        assert_eq!(CodeCoverageTool::Cobertura.as_ado_str(), "Cobertura");
        assert_eq!(CodeCoverageTool::JaCoCo.as_ado_str(), "JaCoCo");
    }
}
