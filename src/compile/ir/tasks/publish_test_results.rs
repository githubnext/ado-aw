//! Typed builder for `PublishTestResults@2`.

use super::common::{bool_input, de_opt_bool_flex};
use crate::compile::ir::step::TaskStep;
use serde::Deserialize;

/// Test result format for [`PublishTestResults`] (`testResultsFormat` input).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum TestResultsFormat {
    #[serde(rename = "JUnit")]
    JUnit,
    #[serde(rename = "NUnit")]
    NUnit,
    #[serde(rename = "VSTest")]
    VSTest,
    #[serde(rename = "XUnit")]
    XUnit,
    #[serde(rename = "CTest")]
    CTest,
}

impl TestResultsFormat {
    /// The exact token the ADO task expects.
    pub fn as_ado_str(self) -> &'static str {
        match self {
            TestResultsFormat::JUnit => "JUnit",
            TestResultsFormat::NUnit => "NUnit",
            TestResultsFormat::VSTest => "VSTest",
            TestResultsFormat::XUnit => "XUnit",
            TestResultsFormat::CTest => "CTest",
        }
    }
}

/// Builder for a [`TaskStep`] invoking `PublishTestResults@2`.
///
/// ADO task reference:
/// <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/publish-test-results-v2>
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublishTestResults {
    #[serde(rename = "testResultsFormat")]
    test_results_format: TestResultsFormat,
    #[serde(rename = "testResultsFiles")]
    test_results_files: String,
    #[serde(rename = "testRunTitle", default)]
    test_run_title: Option<String>,
    #[serde(rename = "searchFolder", default)]
    search_folder: Option<String>,
    #[serde(
        rename = "mergeTestResults",
        default,
        deserialize_with = "de_opt_bool_flex"
    )]
    merge_test_results: Option<bool>,
    #[serde(
        rename = "failTaskOnFailedTests",
        default,
        deserialize_with = "de_opt_bool_flex"
    )]
    fail_task_on_failed_tests: Option<bool>,
    #[serde(
        rename = "publishRunAttachments",
        default,
        deserialize_with = "de_opt_bool_flex"
    )]
    publish_run_attachments: Option<bool>,
    #[serde(skip)]
    display_name: Option<String>,
}

impl PublishTestResults {
    /// Required inputs: `testResultsFormat` and `testResultsFiles` glob.
    pub fn new(
        test_results_format: TestResultsFormat,
        test_results_files: impl Into<String>,
    ) -> Self {
        Self {
            test_results_format,
            test_results_files: test_results_files.into(),
            test_run_title: None,
            search_folder: None,
            merge_test_results: None,
            fail_task_on_failed_tests: None,
            publish_run_attachments: None,
            display_name: None,
        }
    }

    /// `testRunTitle` — label shown in the build summary.
    pub fn test_run_title(mut self, value: impl Into<String>) -> Self {
        self.test_run_title = Some(value.into());
        self
    }

    /// `searchFolder` — root for glob expansion.
    pub fn search_folder(mut self, value: impl Into<String>) -> Self {
        self.search_folder = Some(value.into());
        self
    }

    /// `mergeTestResults` — combine results into a single run.
    pub fn merge_test_results(mut self, value: bool) -> Self {
        self.merge_test_results = Some(value);
        self
    }

    /// `failTaskOnFailedTests` — fail the step if tests failed.
    pub fn fail_task_on_failed_tests(mut self, value: bool) -> Self {
        self.fail_task_on_failed_tests = Some(value);
        self
    }

    /// `publishRunAttachments` — upload result files.
    pub fn publish_run_attachments(mut self, value: bool) -> Self {
        self.publish_run_attachments = Some(value);
        self
    }

    /// Override the default `displayName` (`"Publish Test Results"`).
    pub fn with_display_name(mut self, value: impl Into<String>) -> Self {
        self.display_name = Some(value.into());
        self
    }

    /// Lower into a [`TaskStep`].
    pub fn into_step(self) -> TaskStep {
        let mut t = TaskStep::new(
            "PublishTestResults@2",
            self.display_name
                .unwrap_or_else(|| "Publish Test Results".into()),
        )
        .with_input("testResultsFormat", self.test_results_format.as_ado_str())
        .with_input("testResultsFiles", self.test_results_files);
        if let Some(v) = self.test_run_title {
            t = t.with_input("testRunTitle", v);
        }
        if let Some(v) = self.search_folder {
            t = t.with_input("searchFolder", v);
        }
        if let Some(v) = self.merge_test_results {
            t = t.with_input("mergeTestResults", bool_input(v));
        }
        if let Some(v) = self.fail_task_on_failed_tests {
            t = t.with_input("failTaskOnFailedTests", bool_input(v));
        }
        if let Some(v) = self.publish_run_attachments {
            t = t.with_input("publishRunAttachments", bool_input(v));
        }
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sets_task_and_required_inputs() {
        let t = PublishTestResults::new(TestResultsFormat::JUnit, "**/TEST-*.xml").into_step();
        assert_eq!(t.task, "PublishTestResults@2");
        assert_eq!(
            t.inputs.get("testResultsFormat").map(String::as_str),
            Some("JUnit")
        );
        assert_eq!(
            t.inputs.get("testResultsFiles").map(String::as_str),
            Some("**/TEST-*.xml")
        );
    }

    #[test]
    fn optional_inputs() {
        let t = PublishTestResults::new(TestResultsFormat::VSTest, "**/*.trx")
            .test_run_title("Unit Tests")
            .merge_test_results(true)
            .search_folder("$(System.DefaultWorkingDirectory)")
            .into_step();
        assert_eq!(
            t.inputs.get("testRunTitle").map(String::as_str),
            Some("Unit Tests")
        );
        assert_eq!(
            t.inputs.get("mergeTestResults").map(String::as_str),
            Some("true")
        );
        assert_eq!(
            t.inputs.get("searchFolder").map(String::as_str),
            Some("$(System.DefaultWorkingDirectory)")
        );
    }
}
