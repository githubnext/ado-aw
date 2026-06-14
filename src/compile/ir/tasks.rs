//! Typed factory helpers for ADO built-in pipeline tasks.
//!
//! Each public function returns a [`TaskStep`] pre-configured for a
//! specific ADO task. Required inputs are positional parameters;
//! optional inputs may be applied via `.with_input(…)` on the
//! returned value.
//!
//! These helpers eliminate hand-crafted `TaskStep::new(…)` + raw
//! string inputs at every call site, making task usage self-documenting
//! and the required/optional input boundary explicit.

use super::step::TaskStep;

/// Returns a [`TaskStep`] for `PublishTestResults@2`.
///
/// Publishes test results to the ADO build summary and timeline.
///
/// - `test_results_format` — the test result format. One of `"JUnit"`,
///   `"NUnit"`, `"VSTest"`, `"XUnit"`, or `"CTest"` (alias:
///   `testRunner`).
/// - `test_results_files` — glob pattern that selects the result files,
///   e.g. `"**/TEST-*.xml"` or `"**/*.trx"`.
///
/// Optional inputs (applied with `.with_input(…)` on the returned
/// value):
///
/// | Input key | Type | Default | Description |
/// |---|---|---|---|
/// | `testRunTitle` | string | — | Label shown in the build summary. |
/// | `searchFolder` | string | `$(System.DefaultWorkingDirectory)` | Root for glob expansion. |
/// | `mergeTestResults` | bool string | `"false"` | Combine results into one run. |
/// | `failTaskOnFailedTests` | bool string | `"false"` | Fail the step if tests failed. |
/// | `publishRunAttachments` | bool string | `"true"` | Upload result files. |
///
/// ADO task reference:
/// <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/publish-test-results-v2>
pub fn publish_test_results_step(
    test_results_format: impl Into<String>,
    test_results_files: impl Into<String>,
) -> TaskStep {
    TaskStep::new("PublishTestResults@2", "Publish Test Results")
        .with_input("testResultsFormat", test_results_format)
        .with_input("testResultsFiles", test_results_files)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publish_test_results_step_sets_task_and_required_inputs() {
        let t = publish_test_results_step("JUnit", "**/TEST-*.xml");
        assert_eq!(t.task, "PublishTestResults@2");
        assert_eq!(
            t.inputs.get("testResultsFormat").map(|s| s.as_str()),
            Some("JUnit")
        );
        assert_eq!(
            t.inputs.get("testResultsFiles").map(|s| s.as_str()),
            Some("**/TEST-*.xml")
        );
        // display name follows ADO convention
        assert_eq!(t.display_name, "Publish Test Results");
        // no optional inputs by default
        assert_eq!(t.inputs.len(), 2);
    }

    #[test]
    fn publish_test_results_step_accepts_all_supported_formats() {
        for format in &["JUnit", "NUnit", "VSTest", "XUnit", "CTest"] {
            let t = publish_test_results_step(*format, "**/results.xml");
            assert_eq!(t.task, "PublishTestResults@2");
            assert_eq!(
                t.inputs.get("testResultsFormat").map(|s| s.as_str()),
                Some(*format)
            );
        }
    }

    #[test]
    fn publish_test_results_step_optional_inputs_via_with_input() {
        let t = publish_test_results_step("VSTest", "**/*.trx")
            .with_input("testRunTitle", "Unit Tests")
            .with_input("mergeTestResults", "true")
            .with_input("searchFolder", "$(System.DefaultWorkingDirectory)");
        assert_eq!(t.task, "PublishTestResults@2");
        assert_eq!(
            t.inputs.get("testRunTitle").map(|s| s.as_str()),
            Some("Unit Tests")
        );
        assert_eq!(
            t.inputs.get("mergeTestResults").map(|s| s.as_str()),
            Some("true")
        );
        assert_eq!(
            t.inputs.get("searchFolder").map(|s| s.as_str()),
            Some("$(System.DefaultWorkingDirectory)")
        );
        assert_eq!(t.inputs.len(), 5);
    }
}
