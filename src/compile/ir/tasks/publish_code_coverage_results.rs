//! Typed builder for `PublishCodeCoverageResults@2`.

use super::common::bool_input;
use crate::compile::ir::step::TaskStep;

/// Builder for a [`TaskStep`] invoking `PublishCodeCoverageResults@2`.
///
/// Publishes code coverage results produced by a build (e.g. Cobertura or JaCoCo XML files)
/// so they appear in the build summary and trend charts.
///
/// ADO task reference:
/// <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/publish-code-coverage-results-v2>
#[derive(Debug, Clone)]
pub struct PublishCodeCoverageResults {
    /// `summaryFileLocation` — path (minimatch patterns accepted) to the coverage summary file(s).
    summary_file_location: String,
    /// `pathToSources` — path to source files, required when coverage XML lacks absolute paths
    /// (e.g. JaCoCo for Java).
    path_to_sources: Option<String>,
    /// `failIfCoverageEmpty` — fail the step when no coverage data was produced.
    fail_if_coverage_empty: Option<bool>,
    display_name: Option<String>,
}

impl PublishCodeCoverageResults {
    /// `summaryFileLocation` is the only required input.
    ///
    /// Accepts minimatch glob patterns, e.g.
    /// `"$(System.DefaultWorkingDirectory)/**/cobertura/coverage.xml"`.
    pub fn new(summary_file_location: impl Into<String>) -> Self {
        Self {
            summary_file_location: summary_file_location.into(),
            path_to_sources: None,
            fail_if_coverage_empty: None,
            display_name: None,
        }
    }

    /// `pathToSources` — absolute path to source files on the host.
    ///
    /// Required for coverage formats that embed relative source paths (JaCoCo).
    pub fn path_to_sources(mut self, value: impl Into<String>) -> Self {
        self.path_to_sources = Some(value.into());
        self
    }

    /// `failIfCoverageEmpty` — fail the step if no coverage results were found.
    pub fn fail_if_coverage_empty(mut self, value: bool) -> Self {
        self.fail_if_coverage_empty = Some(value);
        self
    }

    /// Override the default `displayName` (`"Publish code coverage results"`).
    pub fn with_display_name(mut self, value: impl Into<String>) -> Self {
        self.display_name = Some(value.into());
        self
    }

    /// Lower into a [`TaskStep`].
    pub fn into_step(self) -> TaskStep {
        let mut t = TaskStep::new(
            "PublishCodeCoverageResults@2",
            self.display_name
                .unwrap_or_else(|| "Publish code coverage results".into()),
        )
        .with_input("summaryFileLocation", self.summary_file_location);
        if let Some(v) = self.path_to_sources {
            t = t.with_input("pathToSources", v);
        }
        if let Some(v) = self.fail_if_coverage_empty {
            t = t.with_input("failIfCoverageEmpty", bool_input(v));
        }
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sets_task_and_required_input() {
        let t = PublishCodeCoverageResults::new(
            "$(System.DefaultWorkingDirectory)/**/coverage.xml",
        )
        .into_step();
        assert_eq!(t.task, "PublishCodeCoverageResults@2");
        assert_eq!(
            t.inputs.get("summaryFileLocation").map(String::as_str),
            Some("$(System.DefaultWorkingDirectory)/**/coverage.xml")
        );
        assert!(!t.inputs.contains_key("pathToSources"));
        assert!(!t.inputs.contains_key("failIfCoverageEmpty"));
    }

    #[test]
    fn optional_inputs_emitted_only_when_set() {
        let t = PublishCodeCoverageResults::new("**/cobertura/coverage.xml")
            .path_to_sources("$(System.DefaultWorkingDirectory)/MyApp/src/main/java/")
            .fail_if_coverage_empty(true)
            .into_step();
        assert_eq!(
            t.inputs.get("pathToSources").map(String::as_str),
            Some("$(System.DefaultWorkingDirectory)/MyApp/src/main/java/")
        );
        assert_eq!(
            t.inputs.get("failIfCoverageEmpty").map(String::as_str),
            Some("true")
        );
    }

    #[test]
    fn display_name_override() {
        let t = PublishCodeCoverageResults::new("**/coverage.xml")
            .with_display_name("Publish JaCoCo Coverage")
            .into_step();
        assert_eq!(t.display_name, "Publish JaCoCo Coverage");
    }

    #[test]
    fn default_display_name() {
        let t = PublishCodeCoverageResults::new("**/coverage.xml").into_step();
        assert_eq!(t.display_name, "Publish code coverage results");
    }
}
