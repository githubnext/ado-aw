//! Typed builder for `SonarQubeAnalyze@8`.
//!
//! ADO task reference:
//! <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/sonar-qube-analyze-v8>

use crate::compile::ir::step::TaskStep;
use serde::Deserialize;

/// JDK version source used by the SonarQube scanner during analysis.
///
/// Maps to the `jdkversion` input of `SonarQubeAnalyze@8`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum JdkVersion {
    /// Use the value of the `JAVA_HOME` environment variable.
    #[serde(rename = "JAVA_HOME")]
    JavaHome,
    /// Use the built-in `JAVA_HOME_17_X64` path available on hosted agents
    /// (ADO server default).
    #[serde(rename = "JAVA_HOME_17_X64")]
    JavaHome17X64,
    /// Use the built-in `JAVA_HOME_21_X64` path available on hosted agents.
    #[serde(rename = "JAVA_HOME_21_X64")]
    JavaHome21X64,
}

impl JdkVersion {
    /// Returns the exact ADO token for this value.
    pub fn as_ado_str(self) -> &'static str {
        match self {
            Self::JavaHome => "JAVA_HOME",
            Self::JavaHome17X64 => "JAVA_HOME_17_X64",
            Self::JavaHome21X64 => "JAVA_HOME_21_X64",
        }
    }
}

/// Builder for a [`TaskStep`] invoking `SonarQubeAnalyze@8`.
///
/// Runs the SonarQube scanner analysis and uploads results to the SonarQube
/// server. This is the second step in the three-task SonarQube workflow:
/// `SonarQubePrepare@8` → `SonarQubeAnalyze@8` → `SonarQubePublish@8`.
///
/// All inputs are optional; calling `into_step()` with no inputs set uses
/// the ADO server-side default JDK version (`JAVA_HOME_17_X64`).
///
/// ADO task reference:
/// <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/sonar-qube-analyze-v8>
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SonarQubeAnalyze {
    #[serde(rename = "jdkversion", default)]
    jdk_version: Option<JdkVersion>,
    #[serde(skip)]
    display_name: Option<String>,
}

impl SonarQubeAnalyze {
    /// Create a new builder; all inputs are optional.
    pub fn new() -> Self {
        Self {
            jdk_version: None,
            display_name: None,
        }
    }

    /// `jdkversion` — JDK version source for the analysis.
    ///
    /// Defaults to [`JdkVersion::JavaHome17X64`] on the ADO server side when
    /// not set. Only emit this input to override the default.
    pub fn jdk_version(mut self, version: JdkVersion) -> Self {
        self.jdk_version = Some(version);
        self
    }

    /// Override the default `displayName` (`"Run Code Analysis"`).
    pub fn with_display_name(mut self, value: impl Into<String>) -> Self {
        self.display_name = Some(value.into());
        self
    }

    /// Lower into a [`TaskStep`].
    pub fn into_step(self) -> TaskStep {
        let mut t = TaskStep::new(
            "SonarQubeAnalyze@8",
            self.display_name
                .unwrap_or_else(|| "Run Code Analysis".into()),
        );
        if let Some(v) = self.jdk_version {
            t = t.with_input("jdkversion", v.as_ado_str());
        }
        t
    }
}

impl Default for SonarQubeAnalyze {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sets_task_identifier() {
        let t = SonarQubeAnalyze::new().into_step();
        assert_eq!(t.task, "SonarQubeAnalyze@8");
    }

    #[test]
    fn default_display_name() {
        let t = SonarQubeAnalyze::new().into_step();
        assert_eq!(t.display_name, "Run Code Analysis");
    }

    #[test]
    fn no_inputs_by_default() {
        let t = SonarQubeAnalyze::new().into_step();
        assert!(
            t.inputs.is_empty(),
            "expected no inputs when none are set; got {:?}",
            t.inputs
        );
    }

    #[test]
    fn jdk_version_java_home() {
        let t = SonarQubeAnalyze::new()
            .jdk_version(JdkVersion::JavaHome)
            .into_step();
        assert_eq!(
            t.inputs.get("jdkversion").map(String::as_str),
            Some("JAVA_HOME")
        );
    }

    #[test]
    fn jdk_version_java_home_17() {
        let t = SonarQubeAnalyze::new()
            .jdk_version(JdkVersion::JavaHome17X64)
            .into_step();
        assert_eq!(
            t.inputs.get("jdkversion").map(String::as_str),
            Some("JAVA_HOME_17_X64")
        );
    }

    #[test]
    fn jdk_version_java_home_21() {
        let t = SonarQubeAnalyze::new()
            .jdk_version(JdkVersion::JavaHome21X64)
            .into_step();
        assert_eq!(
            t.inputs.get("jdkversion").map(String::as_str),
            Some("JAVA_HOME_21_X64")
        );
    }

    #[test]
    fn display_name_override() {
        let t = SonarQubeAnalyze::new()
            .with_display_name("SonarQube Analysis")
            .into_step();
        assert_eq!(t.display_name, "SonarQube Analysis");
    }

    #[test]
    fn default_impl_matches_new() {
        let t = SonarQubeAnalyze::default().into_step();
        assert_eq!(t.task, "SonarQubeAnalyze@8");
        assert!(t.inputs.is_empty());
    }
}
