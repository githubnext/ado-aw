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
pub fn copy_files_step(contents: impl Into<String>, target_folder: impl Into<String>) -> TaskStep {
    TaskStep::new("CopyFiles@2", "Copy Files")
        .with_input("Contents", contents)
        .with_input("TargetFolder", target_folder)
}

/// Returns a [`TaskStep`] for `DockerInstaller@0`.
///
/// Installs a specific version of Docker Engine on the agent.
///
/// - `docker_version` — the Docker Engine version to install (e.g.
///   `"26.1.4"`). Maps to the `dockerVersion` ADO task input, which
///   is **required** by the task.
///
/// Optional inputs (applied with `.with_input(…)` on the returned
/// value):
///
/// | Input key | Type | Default | Description |
/// |---|---|---|---|
/// | `releaseType` | string | `"stable"` | Release channel: `"stable"`, `"edge"`, `"test"`, or `"nightly"`. |
///
/// ADO task reference:
/// <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/docker-installer-v0>
pub fn docker_installer_step(docker_version: impl Into<String>) -> TaskStep {
    TaskStep::new("DockerInstaller@0", "Install Docker").with_input("dockerVersion", docker_version)
}

/// Returns a [`TaskStep`] for `DotNetCoreCLI@2`.
///
/// Runs a .NET CLI command against .NET projects or solutions.
///
/// - `command` — the dotnet CLI sub-command. One of `"build"`, `"test"`,
///   `"publish"`, `"restore"`, `"pack"`, `"run"`, `"push"`, or
///   `"custom"`. This is the only required input; maps to the `command`
///   ADO task input.
///
/// Optional inputs (applied with `.with_input(…)` on the returned value):
///
/// | Input key | Applies to | Default | Description |
/// |---|---|---|---|
/// | `projects` | build, test, publish, restore, run, custom | — | Glob for `.csproj`/`.sln` files. |
/// | `arguments` | build, publish, run, test, custom | — | Extra CLI args (e.g. `"--configuration Release"`). |
/// | `workingDirectory` | build, publish, run, test, custom | — | Working directory for the command. |
/// | `publishTestResults` | test | `"true"` | Publish test results to the pipeline. |
/// | `testRunTitle` | test | — | Title shown in the build summary. |
/// | `zipAfterPublish` | publish | `"true"` | Zip output after publish. |
/// | `modifyOutputPath` | publish | `"true"` | Append project folder name to publish path. |
/// | `publishWebProjects` | publish | `"true"` | Publish all web projects. |
/// | `custom` | custom | — | The custom dotnet sub-command word. |
/// | `packagesToPush` | push | `"$(Build.ArtifactStagingDirectory)/*.nupkg"` | NuGet package glob to publish. |
/// | `packagesToPack` | pack | `"**/*.csproj"` | `.csproj`/`.nuspec` glob to pack. |
///
/// ADO task reference:
/// <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/dotnet-core-cli-v2>
pub fn dot_net_core_cli_step(command: impl Into<String>) -> TaskStep {
    let cmd: String = command.into();
    TaskStep::new("DotNetCoreCLI@2", format!("dotnet {}", cmd)).with_input("command", cmd)
}

/// Returns a [`TaskStep`] for `ArchiveFiles@2`.
///
/// Creates an archive from `root_folder_or_file` and writes it to
/// `archive_file`. The archive type defaults to `zip`; override with
/// `.with_input("archiveType", "7z")` (or `"tar"` / `"wim"`) when needed.
///
/// Required inputs are positional parameters. Optional inputs (applied
/// via `.with_input(…)` on the returned value):
///
/// | Input key | Type | Default | Description |
/// |---|---|---|---|
/// | `archiveType` | string | `"zip"` | Archive format: `"zip"`, `"7z"`, `"tar"`, `"wim"`. |
/// | `includeRootFolder` | bool string | `"true"` | Prepend root folder name to archive paths. |
/// | `replaceExistingArchive` | bool string | `"true"` | Replace existing archive. |
/// | `sevenZipCompression` | string | `"normal"` | 7z compression level (when `archiveType = 7z`). |
/// | `tarCompression` | string | `"gz"` | Tar compression (when `archiveType = tar`): `"gz"`, `"bz2"`, `"xz"`, `"none"`. |
/// | `verbose` | bool string | `"false"` | Force verbose output. |
/// | `quiet` | bool string | `"false"` | Force quiet output. |
///
/// ADO task reference:
/// <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/archive-files-v2>
pub fn archive_files_step(
    root_folder_or_file: impl Into<String>,
    archive_file: impl Into<String>,
) -> TaskStep {
    TaskStep::new("ArchiveFiles@2", "Archive Files")
        .with_input("rootFolderOrFile", root_folder_or_file)
        .with_input("archiveFile", archive_file)
}

/// Returns a [`TaskStep`] for `ExtractFiles@1`.
///
/// Extracts archives matching `archive_file_patterns` into `destination_folder`.
/// Supports `.zip`, `.tar.gz`, `.tar.bz2`, and 7-Zip formats (`.7z`, `.tar`,
/// `.rar`, etc.) via the 7z utility bundled with the task on Windows agents,
/// or the system 7z on Linux/macOS.
///
/// - `archive_file_patterns` — glob pattern(s) that match the archives to
///   extract. Patterns are evaluated from the root of the repository
///   (equivalent to `$(Build.SourcesDirectory)`). Multiple patterns can be
///   separated by newlines. Default: `**/*.zip`.
/// - `destination_folder` — path to the folder where files will be extracted.
///   **Required** — the task has no default for this input.
///
/// Optional inputs (applied with `.with_input(…)` on the returned value):
///
/// | Input key | Type | Default | Description |
/// |---|---|---|---|
/// | `cleanDestinationFolder` | bool string | `"true"` | Delete destination folder contents before extracting. |
/// | `overwriteExistingFiles` | bool string | `"false"` | Overwrite files that already exist in the destination. |
/// | `pathToSevenZipTool` | string | — | Absolute path to a custom `7z` binary (e.g. `/usr/local/bin/7z`). |
///
/// ADO task reference:
/// <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/extract-files-v1>
pub fn extract_files_step(
    archive_file_patterns: impl Into<String>,
    destination_folder: impl Into<String>,
) -> TaskStep {
    TaskStep::new("ExtractFiles@1", "Extract Files")
        .with_input("archiveFilePatterns", archive_file_patterns)
        .with_input("destinationFolder", destination_folder)
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
        assert_eq!(
            t.inputs.get("Contents").map(|s| s.as_str()),
            Some("**/*.rs")
        );
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

    // ── DotNetCoreCLI@2 ──────────────────────────────────────────────────

    #[test]
    fn dot_net_core_cli_step_build_sets_task_and_command() {
        let t = dot_net_core_cli_step("build");
        assert_eq!(t.task, "DotNetCoreCLI@2");
        assert_eq!(t.display_name, "dotnet build");
        assert_eq!(t.inputs.get("command").map(|s| s.as_str()), Some("build"));
        // only the required input is set by default
        assert_eq!(t.inputs.len(), 1);
    }

    #[test]
    fn dot_net_core_cli_step_test_command_with_optional_inputs() {
        let t = dot_net_core_cli_step("test")
            .with_input("projects", "**/*Tests.csproj")
            .with_input("arguments", "--configuration Release")
            .with_input("publishTestResults", "true")
            .with_input("testRunTitle", "Unit Tests");
        assert_eq!(t.task, "DotNetCoreCLI@2");
        assert_eq!(t.display_name, "dotnet test");
        assert_eq!(t.inputs.get("command").map(|s| s.as_str()), Some("test"));
        assert_eq!(
            t.inputs.get("projects").map(|s| s.as_str()),
            Some("**/*Tests.csproj")
        );
        assert_eq!(
            t.inputs.get("arguments").map(|s| s.as_str()),
            Some("--configuration Release")
        );
        assert_eq!(
            t.inputs.get("publishTestResults").map(|s| s.as_str()),
            Some("true")
        );
        assert_eq!(
            t.inputs.get("testRunTitle").map(|s| s.as_str()),
            Some("Unit Tests")
        );
        assert_eq!(t.inputs.len(), 5);
    }

    #[test]
    fn dot_net_core_cli_step_accepts_all_supported_commands() {
        for cmd in &[
            "build", "test", "publish", "restore", "pack", "run", "push", "custom",
        ] {
            let t = dot_net_core_cli_step(*cmd);
            assert_eq!(t.task, "DotNetCoreCLI@2");
            assert_eq!(t.display_name, format!("dotnet {}", cmd));
            assert_eq!(t.inputs.get("command").map(|s| s.as_str()), Some(*cmd));
        }
    }

    #[test]
    fn dot_net_core_cli_step_publish_optional_inputs() {
        let t = dot_net_core_cli_step("publish")
            .with_input("projects", "src/MyApp/MyApp.csproj")
            .with_input(
                "arguments",
                "--configuration Release --output $(Build.ArtifactStagingDirectory)",
            )
            .with_input("zipAfterPublish", "false")
            .with_input("modifyOutputPath", "false");
        assert_eq!(t.inputs.get("command").map(|s| s.as_str()), Some("publish"));
        assert_eq!(
            t.inputs.get("zipAfterPublish").map(|s| s.as_str()),
            Some("false")
        );
        assert_eq!(
            t.inputs.get("modifyOutputPath").map(|s| s.as_str()),
            Some("false")
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

    #[test]
    fn docker_installer_step_sets_task_and_required_input() {
        let t = docker_installer_step("26.1.4");
        assert_eq!(t.task, "DockerInstaller@0");
        assert_eq!(t.display_name, "Install Docker");
        assert_eq!(
            t.inputs.get("dockerVersion").map(|s| s.as_str()),
            Some("26.1.4")
        );
        // only the required input is set by default
        assert_eq!(t.inputs.len(), 1);
    }

    #[test]
    fn docker_installer_step_optional_release_type_via_with_input() {
        let t = docker_installer_step("26.1.4").with_input("releaseType", "edge");
        assert_eq!(t.task, "DockerInstaller@0");
        assert_eq!(
            t.inputs.get("dockerVersion").map(|s| s.as_str()),
            Some("26.1.4")
        );
        assert_eq!(
            t.inputs.get("releaseType").map(|s| s.as_str()),
            Some("edge")
        );
        assert_eq!(t.inputs.len(), 2);
    }

    #[test]
    fn docker_installer_step_accepts_different_versions() {
        for version in &["17.09.0-ce", "20.10.0", "26.1.4"] {
            let t = docker_installer_step(*version);
            assert_eq!(
                t.inputs.get("dockerVersion").map(|s| s.as_str()),
                Some(*version)
            );
        }
    }

    // ── ArchiveFiles@2 ───────────────────────────────────────────────────

    #[test]
    fn archive_files_step_sets_task_and_required_inputs() {
        let t = archive_files_step(
            "$(Build.BinariesDirectory)",
            "$(Build.ArtifactStagingDirectory)/output.zip",
        );
        assert_eq!(t.task, "ArchiveFiles@2");
        assert_eq!(t.display_name, "Archive Files");
        assert_eq!(
            t.inputs.get("rootFolderOrFile").map(|s| s.as_str()),
            Some("$(Build.BinariesDirectory)")
        );
        assert_eq!(
            t.inputs.get("archiveFile").map(|s| s.as_str()),
            Some("$(Build.ArtifactStagingDirectory)/output.zip")
        );
        // no optional inputs by default
        assert_eq!(t.inputs.len(), 2);
    }

    #[test]
    fn archive_files_step_accepts_archive_type_override() {
        let t = archive_files_step("$(Build.BinariesDirectory)", "$(Build.ArtifactStagingDirectory)/output.tar.gz")
            .with_input("archiveType", "tar")
            .with_input("tarCompression", "gz");
        assert_eq!(t.task, "ArchiveFiles@2");
        assert_eq!(
            t.inputs.get("archiveType").map(|s| s.as_str()),
            Some("tar")
        );
        assert_eq!(
            t.inputs.get("tarCompression").map(|s| s.as_str()),
            Some("gz")
        );
        assert_eq!(t.inputs.len(), 4);
    }

    #[test]
    fn archive_files_step_accepts_optional_flags() {
        let t = archive_files_step("$(Build.SourcesDirectory)", "$(Build.ArtifactStagingDirectory)/src.zip")
            .with_input("includeRootFolder", "false")
            .with_input("replaceExistingArchive", "true");
        assert_eq!(
            t.inputs.get("includeRootFolder").map(|s| s.as_str()),
            Some("false")
        );
        assert_eq!(
            t.inputs.get("replaceExistingArchive").map(|s| s.as_str()),
            Some("true")
        );
        assert_eq!(t.inputs.len(), 4);
    }

    #[test]
    fn archive_files_step_seven_zip_compression() {
        let t = archive_files_step("$(Build.BinariesDirectory)", "$(Build.ArtifactStagingDirectory)/output.7z")
            .with_input("archiveType", "7z")
            .with_input("sevenZipCompression", "maximum");
        assert_eq!(t.task, "ArchiveFiles@2");
        assert_eq!(
            t.inputs.get("archiveType").map(|s| s.as_str()),
            Some("7z")
        );
        assert_eq!(
            t.inputs.get("sevenZipCompression").map(|s| s.as_str()),
            Some("maximum")
        );
        assert_eq!(t.inputs.len(), 4);
    }

    // ── ExtractFiles@1 ───────────────────────────────────────────────────

    #[test]
    fn extract_files_step_sets_task_and_required_inputs() {
        let t = extract_files_step("**/*.zip", "$(Build.BinariesDirectory)");
        assert_eq!(t.task, "ExtractFiles@1");
        assert_eq!(t.display_name, "Extract Files");
        assert_eq!(
            t.inputs.get("archiveFilePatterns").map(|s| s.as_str()),
            Some("**/*.zip")
        );
        assert_eq!(
            t.inputs.get("destinationFolder").map(|s| s.as_str()),
            Some("$(Build.BinariesDirectory)")
        );
        // no optional inputs by default
        assert_eq!(t.inputs.len(), 2);
    }

    #[test]
    fn extract_files_step_accepts_optional_clean_and_overwrite() {
        let t = extract_files_step("**/*.tar.gz", "$(Agent.TempDirectory)/extracted")
            .with_input("cleanDestinationFolder", "false")
            .with_input("overwriteExistingFiles", "true");
        assert_eq!(t.task, "ExtractFiles@1");
        assert_eq!(
            t.inputs.get("cleanDestinationFolder").map(|s| s.as_str()),
            Some("false")
        );
        assert_eq!(
            t.inputs.get("overwriteExistingFiles").map(|s| s.as_str()),
            Some("true")
        );
        assert_eq!(t.inputs.len(), 4);
    }

    #[test]
    fn extract_files_step_accepts_custom_seven_zip_path() {
        let t = extract_files_step("artifacts/**/*.7z", "$(Build.BinariesDirectory)")
            .with_input("pathToSevenZipTool", "/usr/local/bin/7z");
        assert_eq!(t.task, "ExtractFiles@1");
        assert_eq!(
            t.inputs.get("pathToSevenZipTool").map(|s| s.as_str()),
            Some("/usr/local/bin/7z")
        );
        assert_eq!(t.inputs.len(), 3);
    }

    #[test]
    fn extract_files_step_multiline_patterns() {
        let patterns = "**/*.zip\n**/*.tar.gz";
        let t = extract_files_step(patterns, "$(Build.BinariesDirectory)");
        assert_eq!(
            t.inputs.get("archiveFilePatterns").map(|s| s.as_str()),
            Some("**/*.zip\n**/*.tar.gz")
        );
    }
}
