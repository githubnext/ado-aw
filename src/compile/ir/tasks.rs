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

/// Returns a [`TaskStep`] for `CopyFiles@2`.
///
/// Copies files matching `contents` into `target_folder`. The optional
/// `source_folder` narrows the root from which the glob is evaluated;
/// when omitted ADO defaults to `$(Build.SourcesDirectory)`.
///
/// Required inputs are positional parameters. Optional inputs (applied
/// via `.with_input(…)` on the returned value):
///
/// | Input key | Type | Default | Description |
/// |---|---|---|---|
/// | `SourceFolder` | string | `$(Build.SourcesDirectory)` | Root for glob evaluation. |
/// | `CleanTargetFolder` | bool string | `"false"` | Delete target folder contents before copy. |
/// | `OverWrite` | bool string | `"false"` | Overwrite files in target folder. |
/// | `flattenFolders` | bool string | `"false"` | Flatten directory structure in target. |
/// | `preserveTimestamp` | bool string | `"false"` | Preserve source timestamps. |
/// | `retryCount` | string | `"0"` | Number of retry attempts on failure. |
/// | `delayBetweenRetries` | string | `"1000"` | Milliseconds between retries. |
/// | `ignoreMakeDirErrors` | bool string | `"false"` | Ignore errors when creating target folder. |
///
/// ADO task reference:
/// <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/copy-files-v2>
pub fn copy_files_step(
    contents: impl Into<String>,
    target_folder: impl Into<String>,
) -> TaskStep {
    TaskStep::new("CopyFiles@2", "Copy Files")
        .with_input("Contents", contents)
        .with_input("TargetFolder", target_folder)
}

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

    // ── CopyFiles@2 ──────────────────────────────────────────────────────

    #[test]
    fn copy_files_step_sets_task_and_required_inputs() {
        let t = copy_files_step("**/*.rs", "$(Build.ArtifactStagingDirectory)");
        assert_eq!(t.task, "CopyFiles@2");
        assert_eq!(t.display_name, "Copy Files");
        assert_eq!(t.inputs.get("Contents").map(|s| s.as_str()), Some("**/*.rs"));
        assert_eq!(
            t.inputs.get("TargetFolder").map(|s| s.as_str()),
            Some("$(Build.ArtifactStagingDirectory)")
        );
        // no optional inputs by default
        assert_eq!(t.inputs.len(), 2);
    }

    #[test]
    fn copy_files_step_accepts_source_folder_via_with_input() {
        let t = copy_files_step("**", "$(Build.ArtifactStagingDirectory)")
            .with_input("SourceFolder", "$(Build.SourcesDirectory)/src");
        assert_eq!(t.task, "CopyFiles@2");
        assert_eq!(
            t.inputs.get("SourceFolder").map(|s| s.as_str()),
            Some("$(Build.SourcesDirectory)/src")
        );
        assert_eq!(t.inputs.len(), 3);
    }

    #[test]
    fn copy_files_step_accepts_optional_flags() {
        let t = copy_files_step("**", "$(Build.ArtifactStagingDirectory)")
            .with_input("CleanTargetFolder", "true")
            .with_input("OverWrite", "true")
            .with_input("flattenFolders", "true");
        assert_eq!(
            t.inputs.get("CleanTargetFolder").map(|s| s.as_str()),
            Some("true")
        );
        assert_eq!(t.inputs.get("OverWrite").map(|s| s.as_str()), Some("true"));
        assert_eq!(
            t.inputs.get("flattenFolders").map(|s| s.as_str()),
            Some("true")
        );
        assert_eq!(t.inputs.len(), 5);
    }

    // ── PublishTestResults@2 ─────────────────────────────────────────────

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
