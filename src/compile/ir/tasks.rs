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

/// Returns a [`TaskStep`] for `NuGetCommand@2`.
///
/// Runs a NuGet command. The `command` parameter selects the operation mode;
/// each mode exposes a different set of optional inputs.
///
/// - `command` — the NuGet operation: `"restore"`, `"push"`, `"pack"`, or
///   `"custom"`. This is the only required input.
///
/// **`restore` optional inputs** (applied with `.with_input(…)`):
///
/// | Input key | Default | Description |
/// |---|---|---|
/// | `solution` | `"**/*.sln"` | Path to solution, `packages.config`, or `project.json`. |
/// | `feedsToUse` | `"select"` | `"select"` (dropdown) or `"config"` (NuGet.config). |
/// | `vstsFeed` | — | Azure Artifacts feed name or ID (when `feedsToUse = select`). |
/// | `includeNuGetOrg` | `"true"` | Include NuGet.org as a package source. |
/// | `nugetConfigPath` | — | Path to `NuGet.config` (when `feedsToUse = config`). |
/// | `externalFeedCredentials` | — | Credentials for external feeds outside the org. |
/// | `noCache` | `"false"` | Disable the local NuGet cache. |
/// | `disableParallelProcessing` | `"false"` | Disable parallel package restore. |
/// | `restoreDirectory` | — | Destination directory for restored packages. |
/// | `verbosityRestore` | `"Detailed"` | Verbosity: `"Quiet"`, `"Normal"`, or `"Detailed"`. |
///
/// **`push` optional inputs**:
///
/// | Input key | Default | Description |
/// |---|---|---|
/// | `packagesToPush` | `"$(Build.ArtifactStagingDirectory)/**/*.nupkg;…"` | Glob for `.nupkg` files to publish. |
/// | `nuGetFeedType` | `"internal"` | Feed location: `"internal"` (Azure Artifacts) or `"external"`. |
/// | `publishVstsFeed` | — | Target Azure Artifacts feed (when `nuGetFeedType = internal`). |
/// | `allowPackageConflicts` | `"false"` | Skip duplicate packages instead of failing. |
/// | `publishFeedCredentials` | — | External NuGet server endpoint (when `nuGetFeedType = external`). |
/// | `publishPackageMetadata` | `"true"` | Publish pipeline metadata alongside the package. |
/// | `verbosityPush` | `"Detailed"` | Verbosity: `"Quiet"`, `"Normal"`, or `"Detailed"`. |
///
/// **`pack` optional inputs**:
///
/// | Input key | Default | Description |
/// |---|---|---|
/// | `packagesToPack` | `"**/*.csproj"` | Glob for `.csproj` or `.nuspec` files to pack. |
/// | `configuration` | — | Build configuration (e.g. `"Release"`). |
/// | `versioningScheme` | `"off"` | Version strategy: `"off"`, `"byPrereleaseNumber"`, `"byEnvVar"`, `"byBuildNumber"`. |
/// | `verbosityPack` | `"Detailed"` | Verbosity: `"Quiet"`, `"Normal"`, or `"Detailed"`. |
///
/// **`custom` optional inputs**:
///
/// | Input key | Default | Description |
/// |---|---|---|
/// | `arguments` | — | Full NuGet command-line arguments (e.g. `"install Foo -Version 1.0 -Source ..."`). **Required** for `custom`. |
///
/// ADO task reference:
/// <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/nuget-command-v2>
pub fn nuget_command_step(command: impl Into<String>) -> TaskStep {
    let cmd: String = command.into();
    TaskStep::new("NuGetCommand@2", format!("NuGet {cmd}")).with_input("command", cmd)
}

/// Returns a [`TaskStep`] for `PowerShell@2` in file-path mode.
///
/// Runs the PowerShell script at `file_path` on Linux, macOS, or Windows.
/// `file_path` must be a fully qualified path or relative to
/// `$(System.DefaultWorkingDirectory)`.
///
/// Optional inputs (applied via `.with_input(…)` on the returned value):
///
/// | Input key | Type | Default | Description |
/// |---|---|---|---|
/// | `arguments` | string | — | Arguments passed to the script. |
/// | `errorActionPreference` | string | `"stop"` | Non-terminating error behaviour: `"stop"`, `"continue"`, `"silentlyContinue"`. |
/// | `failOnStderr` | bool string | `"false"` | Fail the step if anything is written to stderr. |
/// | `ignoreLASTEXITCODE` | bool string | `"false"` | Do not fail when `$LASTEXITCODE` is non-zero. |
/// | `pwsh` | bool string | `"false"` | Use PowerShell Core (`pwsh`) instead of Windows PowerShell. |
/// | `workingDirectory` | string | — | Working directory for the script. |
///
/// ADO task reference:
/// <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/powershell-v2>
pub fn powershell_file_step(file_path: impl Into<String>) -> TaskStep {
    TaskStep::new("PowerShell@2", "PowerShell Script")
        .with_input("targetType", "filePath")
        .with_input("filePath", file_path)
}

/// Returns a [`TaskStep`] for `PowerShell@2` in inline mode.
///
/// Runs `script` as an inline PowerShell block on Linux, macOS, or Windows.
///
/// Optional inputs (applied via `.with_input(…)` on the returned value):
///
/// | Input key | Type | Default | Description |
/// |---|---|---|---|
/// | `errorActionPreference` | string | `"stop"` | Non-terminating error behaviour: `"stop"`, `"continue"`, `"silentlyContinue"`. |
/// | `failOnStderr` | bool string | `"false"` | Fail the step if anything is written to stderr. |
/// | `ignoreLASTEXITCODE` | bool string | `"false"` | Do not fail when `$LASTEXITCODE` is non-zero. |
/// | `pwsh` | bool string | `"false"` | Use PowerShell Core (`pwsh`) instead of Windows PowerShell. |
/// | `workingDirectory` | string | — | Working directory for the script. |
///
/// ADO task reference:
/// <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/powershell-v2>
pub fn powershell_inline_step(script: impl Into<String>) -> TaskStep {
    TaskStep::new("PowerShell@2", "PowerShell Script")
        .with_input("targetType", "inline")
        .with_input("script", script)
}

/// Returns a [`TaskStep`] for `PublishPipelineArtifact@1`.
///
/// Publishes (uploads) a file or directory as a named artifact for the
/// current pipeline run. The artifact is stored in Azure Pipelines and
/// can be downloaded by subsequent jobs or pipelines via
/// `DownloadPipelineArtifact@2`.
///
/// - `target_path` — path of the file or directory to publish. Can be
///   absolute or relative to the default working directory. Supports
///   ADO macro variables (e.g. `$(Build.ArtifactStagingDirectory)`),
///   but wildcards are **not** supported.
///
/// Optional inputs (applied with `.with_input(…)` on the returned
/// value):
///
/// | Input key | Alias | Type | Default | Description |
/// |---|---|---|---|---|
/// | `artifact` | `artifactName` | string | *(unique job-scoped ID)* | Name of the published artifact (e.g. `"drop"`). May not contain `\`, `/`, `"`, `:`, `<`, `>`, `\|`, `*`, or `?`. |
/// | `publishLocation` | `artifactType` | string | `"pipeline"` | Where to store the artifact: `"pipeline"` (Azure Pipelines) or `"filepath"` (a UNC file share). |
/// | `fileSharePath` | — | string | — | Required when `publishLocation = filepath`. UNC path of the file share. |
/// | `parallel` | — | bool string | `"false"` | Enable multi-threaded copy when `publishLocation = filepath`. |
/// | `parallelCount` | — | string | `"8"` | Thread count for parallel copy (1–128). Applies when `parallel = true`. |
/// | `properties` | — | string | — | JSON string of custom properties to associate with the artifact (keys must start with `user-`). |
///
/// ADO task reference:
/// <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/publish-pipeline-artifact-v1>
pub fn publish_pipeline_artifact_step(target_path: impl Into<String>) -> TaskStep {
    TaskStep::new("PublishPipelineArtifact@1", "Publish Pipeline Artifact")
        .with_input("targetPath", target_path)
}

/// Returns a [`TaskStep`] for `DownloadPipelineArtifact@2`.
///
/// Downloads artifacts produced by a pipeline run into `target_path`.
/// By default the step downloads from the **current** run; set
/// `source = "specific"` via `.with_input(…)` to pull from a
/// different run or pipeline.
///
/// - `target_path` — local filesystem path where the artifact will be
///   downloaded. Maps to the `targetPath` ADO task input, which is
///   **required** by the task.
///
/// Optional inputs (applied with `.with_input(…)` on the returned value):
///
/// | Input key | Type | Default | Description |
/// |---|---|---|
/// | `artifact` | string | — | Name of the artifact to download. Omit to download all artifacts. |
/// | `patterns` | string | `"**"` | Newline-separated glob patterns that filter which files inside the artifact are downloaded. |
/// | `source` | string | `"current"` | `"current"` (this run) or `"specific"` (another run). |
/// | `project` | string | — | ADO project name or ID (`source = "specific"` only). |
/// | `pipeline` | string | — | Pipeline definition ID or name (`source = "specific"` only). |
/// | `runVersion` | string | `"latest"` | Which run to download from: `"latest"`, `"latestFromBranch"`, or `"specific"` (`source = "specific"` only). |
/// | `branchName` | string | — | Branch filter, e.g. `"refs/heads/main"` (`runVersion = "latestFromBranch"` only). |
/// | `runId` | string | — | The build ID to download from (`runVersion = "specific"` only). |
/// | `tags` | string | — | Comma-separated build tags used to filter candidate runs. |
/// | `allowPartiallySucceededBuilds` | bool string | `"false"` | Also consider partially-succeeded runs as download candidates. |
/// | `allowFailedBuilds` | bool string | `"false"` | Also consider failed runs as download candidates. |
/// | `preferTriggeringPipeline` | bool string | `"false"` | Prefer the run that triggered the current pipeline. |
/// | `itemPattern` | string | `"**"` | Minimatch pattern applied after download to select a subset of files. |
///
/// ADO task reference:
/// <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/download-pipeline-artifact-v2>
pub fn download_pipeline_artifact_step(target_path: impl Into<String>) -> TaskStep {
    TaskStep::new("DownloadPipelineArtifact@2", "Download Pipeline Artifact")
        .with_input("targetPath", target_path)
}

/// Returns a [`TaskStep`] for `DeleteFiles@1`.
///
/// Deletes files or folders matching one or more patterns from a source folder.
///
/// - `contents` — newline-separated glob patterns identifying the files or
///   folders to remove (e.g. `"**/*.tmp"` or `"dist\n*.log"`). This is the
///   only required input.
///
/// Optional inputs (applied with `.with_input(…)` on the returned value):
///
/// | Input key | Type | Default | Description |
/// |---|---|---|---|
/// | `SourceFolder` | string | working directory | Root folder to delete from. Use `$(Build.ArtifactStagingDirectory)` to clean staging. |
/// | `RemoveSourceFolder` | bool string | `"false"` | Remove the `SourceFolder` itself after deleting its contents. Set to `"true"` and `contents` to `"*"` to wipe the whole folder. |
/// | `RemoveDotFiles` | bool string | `"false"` | Also delete files whose name starts with a dot. Defaults to `"false"` (dot files are preserved). |
///
/// ADO task reference:
/// <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/delete-files-v1>
pub fn delete_files_step(contents: impl Into<String>) -> TaskStep {
    TaskStep::new("DeleteFiles@1", "Delete Files")
        .with_input("Contents", contents)
}

/// Returns a [`TaskStep`] for `Npm@1`.
///
/// Runs an npm command against the package in the working directory.
/// Supports `npmjs.com` and authenticated registries such as Azure Artifacts.
///
/// - `command` — the npm operation: `"install"`, `"ci"`, `"publish"`, or
///   `"custom"`. The ADO task default is `"install"`.
///
/// Optional inputs (applied with `.with_input(…)` on the returned value):
///
/// | Input key | Type | Default | Description |
/// |---|---|---|---|
/// | `workingDir` | string | — | Working folder containing `package.json`. |
/// | `verbose` | bool string | — | Enable verbose logging (for `install`, `ci`, `publish`). |
/// | `customCommand` | string | — | Required when `command = "custom"`. The npm arguments to forward. |
/// | `customRegistry` | string | `"useNpmrc"` | Registry for `install`/`ci`/`custom`: `"useNpmrc"` or `"useFeed"`. |
/// | `customFeed` | string | — | Required when `customRegistry = "useFeed"`. Azure Artifacts feed ID or URL. |
/// | `customEndpoint` | string | — | Service connection for registries outside the organisation. |
/// | `publishRegistry` | string | `"useExternalRegistry"` | Registry for `publish`: `"useExternalRegistry"` or `"useFeed"`. |
/// | `publishFeed` | string | — | Required when `publishRegistry = "useFeed"`. Target Azure Artifacts feed. |
/// | `publishEndpoint` | string | — | Required when `publishRegistry = "useExternalRegistry"`. External registry service connection. |
/// | `publishPackageMetadata` | bool string | `"true"` | Attach pipeline metadata to packages published via `useFeed`. |
///
/// ADO task reference:
/// <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/npm-v1>
pub fn npm_step(command: impl Into<String>) -> TaskStep {
    let cmd: String = command.into();
    TaskStep::new("Npm@1", format!("npm {cmd}")).with_input("command", cmd)
}

/// Returns a [`TaskStep`] for `CmdLine@2`.
///
/// Runs an inline command-line script. On Linux and macOS the script
/// is executed with Bash; on Windows it runs in `cmd.exe`. This makes
/// `CmdLine@2` the cross-platform sibling of the `bash:` step shorthand.
///
/// - `script` — the inline script text to execute (required). Maps to
///   the `script` ADO task input.
///
/// Optional inputs (applied via `.with_input(…)` on the returned value):
///
/// | Input key | Type | Default | Description |
/// |---|---|---|---|
/// | `workingDirectory` | string | — | Working directory in which to run the script. |
/// | `failOnStderr` | bool string | `"false"` | Fail the step if the script writes anything to stderr. |
///
/// ADO task reference:
/// <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/cmd-line-v2>
pub fn cmd_line_step(script: impl Into<String>) -> TaskStep {
    TaskStep::new("CmdLine@2", "Command Line Script").with_input("script", script)
}

/// Returns a [`TaskStep`] for `Docker@2` in `buildAndPush` mode.
///
/// Builds a Docker image and pushes it to a container registry in one step.
/// This is the most common Docker@2 use case; it combines `docker build`
/// and `docker push` into a single pipeline step and ensures the pushed image
/// digest matches what was built.
///
/// All inputs are optional at the Rust API level because the ADO task ships
/// sensible defaults (`Dockerfile = **/Dockerfile`, `tags = $(Build.BuildId)`).
/// Apply them with `.with_input(…)`:
///
/// | Input key | Type | Default | Description |
/// |---|---|---|---|
/// | `containerRegistry` | string | — | Docker registry service connection name. Required in practice to push to a private registry. |
/// | `repository` | string | — | Container repository name (e.g. `"myapp"` or `"username/myapp"` for Docker Hub). |
/// | `Dockerfile` | string | `**/Dockerfile` | Path or glob to the Dockerfile. |
/// | `buildContext` | string | `**` | Build context path relative to the repo root. |
/// | `tags` | string | `$(Build.BuildId)` | Newline-separated list of image tags. |
///
/// ADO task reference:
/// <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/docker-v2>
pub fn docker_build_and_push_step() -> TaskStep {
    TaskStep::new("Docker@2", "Build and Push Docker Image").with_input("command", "buildAndPush")
}

/// Returns a [`TaskStep`] for `Docker@2` in `build` mode.
///
/// Builds a Docker image without pushing it to a registry. Use
/// `docker_build_and_push_step()` when you want to build and push in one
/// step; use this when you need to run a scan or test between build and push.
///
/// Optional inputs (applied via `.with_input(…)` on the returned value):
///
/// | Input key | Type | Default | Description |
/// |---|---|---|---|
/// | `containerRegistry` | string | — | Docker registry service connection for authentication. |
/// | `repository` | string | — | Image name to tag the build as. |
/// | `Dockerfile` | string | `**/Dockerfile` | Path or glob to the Dockerfile. |
/// | `buildContext` | string | `**` | Build context path relative to the repo root. |
/// | `tags` | string | `$(Build.BuildId)` | Newline-separated list of image tags. |
/// | `arguments` | string | — | Extra arguments appended to the `docker build` command. |
///
/// ADO task reference:
/// <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/docker-v2>
pub fn docker_build_step() -> TaskStep {
    TaskStep::new("Docker@2", "Build Docker Image").with_input("command", "build")
}

/// Returns a [`TaskStep`] for `Docker@2` in `push` mode.
///
/// Pushes a previously-built Docker image to a container registry. Use after
/// `docker_build_step()` when the build and push need to be separate steps
/// (e.g. to run a security scan in between).
///
/// Optional inputs (applied via `.with_input(…)` on the returned value):
///
/// | Input key | Type | Default | Description |
/// |---|---|---|---|
/// | `containerRegistry` | string | — | Docker registry service connection name. |
/// | `repository` | string | — | Container repository name to push to. |
/// | `tags` | string | `$(Build.BuildId)` | Newline-separated list of tags to push. |
/// | `arguments` | string | — | Extra arguments appended to the `docker push` command. |
///
/// ADO task reference:
/// <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/docker-v2>
pub fn docker_push_step() -> TaskStep {
    TaskStep::new("Docker@2", "Push Docker Image").with_input("command", "push")
}

/// Returns a [`TaskStep`] for `Docker@2` in `login` mode.
///
/// Logs in to a Docker container registry. Pair this with
/// `docker_logout_step()` at the end of the job. The service connection is
/// specified via `.with_input("containerRegistry", "<service-connection>")`.
///
/// Optional inputs (applied via `.with_input(…)` on the returned value):
///
/// | Input key | Type | Default | Description |
/// |---|---|---|---|
/// | `containerRegistry` | string | — | Docker registry service connection name. When omitted the task logs in to Docker Hub. |
///
/// ADO task reference:
/// <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/docker-v2>
pub fn docker_login_step() -> TaskStep {
    TaskStep::new("Docker@2", "Docker Login").with_input("command", "login")
}

/// Returns a [`TaskStep`] for `Docker@2` in `logout` mode.
///
/// Logs out from a Docker container registry. Use after a series of Docker
/// steps to ensure credentials are not left on the agent.
///
/// Optional inputs (applied via `.with_input(…)` on the returned value):
///
/// | Input key | Type | Default | Description |
/// |---|---|---|---|
/// | `containerRegistry` | string | — | Docker registry service connection name. |
///
/// ADO task reference:
/// <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/docker-v2>
pub fn docker_logout_step() -> TaskStep {
    TaskStep::new("Docker@2", "Docker Logout").with_input("command", "logout")
}

/// Returns a [`TaskStep`] for `Cache@2`.
///
/// Caches `path` between pipeline runs using `key` as the cache key.
/// On a cache hit the folder is restored from a prior run, avoiding
/// expensive downloads or builds. On a miss the cache is saved at the
/// end of the job for use by future runs.
///
/// - `key` — a `|`-delimited string used to identify the cache entry.
///   Typically includes a platform token, a lockfile hash, and a version
///   discriminator, e.g.
///   `"npm | \"$(Agent.OS)\" | package-lock.json"`.
/// - `path` — the folder to cache. May be absolute or relative to
///   `$(System.DefaultWorkingDirectory)`.  Wildcards are **not** supported.
///
/// Optional inputs (applied via `.with_input(…)` on the returned value):
///
/// | Input key | Type | Default | Description |
/// |---|---|---|---|
/// | `cacheHitVar` | string | — | Variable set to `"true"` (exact hit), `"inexact"` (restore-key hit), or `"false"` (miss). |
/// | `restoreKeys` | string | — | Newline-delimited list of fallback key prefixes searched when the primary key misses. |
///
/// ADO task reference:
/// <https://learn.microsoft.com/en-us/azure/devops/pipelines/release/caching>
pub fn cache_step(key: impl Into<String>, path: impl Into<String>) -> TaskStep {
    TaskStep::new("Cache@2", "Cache")
        .with_input("key", key)
        .with_input("path", path)
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

    // ── NuGetCommand@2 ───────────────────────────────────────────────────

    #[test]
    fn nuget_command_step_restore_sets_task_and_command() {
        let t = nuget_command_step("restore");
        assert_eq!(t.task, "NuGetCommand@2");
        assert_eq!(t.display_name, "NuGet restore");
        assert_eq!(t.inputs.get("command").map(|s| s.as_str()), Some("restore"));
        // only the required input is set by default
        assert_eq!(t.inputs.len(), 1);
    }

    #[test]
    fn nuget_command_step_custom_with_arguments() {
        let t = nuget_command_step("custom").with_input(
            "arguments",
            "install My.Package -Version 1.0.0 -Source https://example.com/nuget -NonInteractive",
        );
        assert_eq!(t.task, "NuGetCommand@2");
        assert_eq!(t.display_name, "NuGet custom");
        assert_eq!(t.inputs.get("command").map(|s| s.as_str()), Some("custom"));
        assert_eq!(
            t.inputs.get("arguments").map(|s| s.as_str()),
            Some("install My.Package -Version 1.0.0 -Source https://example.com/nuget -NonInteractive")
        );
        assert_eq!(t.inputs.len(), 2);
    }

    #[test]
    fn nuget_command_step_push_with_feed() {
        let t = nuget_command_step("push")
            .with_input("nuGetFeedType", "internal")
            .with_input(
                "packagesToPush",
                "$(Build.ArtifactStagingDirectory)/**/*.nupkg",
            )
            .with_input("publishVstsFeed", "myorg/myfeed")
            .with_input("allowPackageConflicts", "true");
        assert_eq!(t.task, "NuGetCommand@2");
        assert_eq!(t.inputs.get("command").map(|s| s.as_str()), Some("push"));
        assert_eq!(
            t.inputs.get("nuGetFeedType").map(|s| s.as_str()),
            Some("internal")
        );
        assert_eq!(
            t.inputs.get("publishVstsFeed").map(|s| s.as_str()),
            Some("myorg/myfeed")
        );
        assert_eq!(
            t.inputs.get("allowPackageConflicts").map(|s| s.as_str()),
            Some("true")
        );
        assert_eq!(t.inputs.len(), 5);
    }

    #[test]
    fn nuget_command_step_restore_with_vsts_feed() {
        let t = nuget_command_step("restore")
            .with_input("solution", "src/MyApp.sln")
            .with_input("feedsToUse", "select")
            .with_input("vstsFeed", "myorg/myproject/myfeed")
            .with_input("includeNuGetOrg", "false");
        assert_eq!(t.task, "NuGetCommand@2");
        assert_eq!(
            t.inputs.get("solution").map(|s| s.as_str()),
            Some("src/MyApp.sln")
        );
        assert_eq!(
            t.inputs.get("vstsFeed").map(|s| s.as_str()),
            Some("myorg/myproject/myfeed")
        );
        assert_eq!(
            t.inputs.get("includeNuGetOrg").map(|s| s.as_str()),
            Some("false")
        );
        assert_eq!(t.inputs.len(), 5);
    }

    #[test]
    fn nuget_command_step_accepts_all_supported_commands() {
        for cmd in &["restore", "push", "pack", "custom"] {
            let t = nuget_command_step(*cmd);
            assert_eq!(t.task, "NuGetCommand@2");
            assert_eq!(t.display_name, format!("NuGet {cmd}"));
            assert_eq!(t.inputs.get("command").map(|s| s.as_str()), Some(*cmd));
        }
    }

    // ── PowerShell@2 ─────────────────────────────────────────────────────

    #[test]
    fn powershell_file_step_sets_task_and_required_inputs() {
        let t = powershell_file_step("scripts/deploy.ps1");
        assert_eq!(t.task, "PowerShell@2");
        assert_eq!(t.display_name, "PowerShell Script");
        assert_eq!(
            t.inputs.get("targetType").map(|s| s.as_str()),
            Some("filePath")
        );
        assert_eq!(
            t.inputs.get("filePath").map(|s| s.as_str()),
            Some("scripts/deploy.ps1")
        );
        // only the required inputs are set by default
        assert_eq!(t.inputs.len(), 2);
    }

    #[test]
    fn powershell_file_step_accepts_optional_arguments() {
        let t = powershell_file_step("$(System.DefaultWorkingDirectory)/scripts/build.ps1")
            .with_input("arguments", "-Configuration Release -OutputDir $(Build.ArtifactStagingDirectory)")
            .with_input("workingDirectory", "$(Build.SourcesDirectory)");
        assert_eq!(t.task, "PowerShell@2");
        assert_eq!(
            t.inputs.get("filePath").map(|s| s.as_str()),
            Some("$(System.DefaultWorkingDirectory)/scripts/build.ps1")
        );
        assert_eq!(
            t.inputs.get("arguments").map(|s| s.as_str()),
            Some("-Configuration Release -OutputDir $(Build.ArtifactStagingDirectory)")
        );
        assert_eq!(
            t.inputs.get("workingDirectory").map(|s| s.as_str()),
            Some("$(Build.SourcesDirectory)")
        );
        assert_eq!(t.inputs.len(), 4);
    }

    #[test]
    fn powershell_file_step_accepts_error_and_exit_flags() {
        let t = powershell_file_step("scripts/test.ps1")
            .with_input("errorActionPreference", "continue")
            .with_input("failOnStderr", "true")
            .with_input("ignoreLASTEXITCODE", "true")
            .with_input("pwsh", "true");
        assert_eq!(t.task, "PowerShell@2");
        assert_eq!(
            t.inputs.get("errorActionPreference").map(|s| s.as_str()),
            Some("continue")
        );
        assert_eq!(
            t.inputs.get("failOnStderr").map(|s| s.as_str()),
            Some("true")
        );
        assert_eq!(
            t.inputs.get("ignoreLASTEXITCODE").map(|s| s.as_str()),
            Some("true")
        );
        assert_eq!(t.inputs.get("pwsh").map(|s| s.as_str()), Some("true"));
        assert_eq!(t.inputs.len(), 6);
    }

    #[test]
    fn powershell_inline_step_sets_task_and_required_inputs() {
        let script = "Write-Host 'Hello, World!'";
        let t = powershell_inline_step(script);
        assert_eq!(t.task, "PowerShell@2");
        assert_eq!(t.display_name, "PowerShell Script");
        assert_eq!(
            t.inputs.get("targetType").map(|s| s.as_str()),
            Some("inline")
        );
        assert_eq!(
            t.inputs.get("script").map(|s| s.as_str()),
            Some("Write-Host 'Hello, World!'")
        );
        // only the required inputs are set by default
        assert_eq!(t.inputs.len(), 2);
    }

    #[test]
    fn powershell_inline_step_accepts_optional_flags() {
        let t = powershell_inline_step("Get-Date")
            .with_input("pwsh", "true")
            .with_input("errorActionPreference", "silentlyContinue")
            .with_input("workingDirectory", "$(Build.SourcesDirectory)");
        assert_eq!(t.task, "PowerShell@2");
        assert_eq!(t.inputs.get("pwsh").map(|s| s.as_str()), Some("true"));
        assert_eq!(
            t.inputs.get("errorActionPreference").map(|s| s.as_str()),
            Some("silentlyContinue")
        );
        assert_eq!(
            t.inputs.get("workingDirectory").map(|s| s.as_str()),
            Some("$(Build.SourcesDirectory)")
        );
        assert_eq!(t.inputs.len(), 5);
    }

    #[test]
    fn powershell_inline_step_multiline_script() {
        let script = "$version = Get-Content VERSION\nWrite-Host \"Building version $version\"";
        let t = powershell_inline_step(script);
        assert_eq!(t.task, "PowerShell@2");
        assert_eq!(
            t.inputs.get("script").map(|s| s.as_str()),
            Some("$version = Get-Content VERSION\nWrite-Host \"Building version $version\"")
        );
    }

    // ── DeleteFiles@1 ────────────────────────────────────────────────────

    #[test]
    fn delete_files_step_sets_task_and_required_input() {
        let t = delete_files_step("**/*.tmp");
        assert_eq!(t.task, "DeleteFiles@1");
        assert_eq!(t.display_name, "Delete Files");
        assert_eq!(
            t.inputs.get("Contents").map(|s| s.as_str()),
            Some("**/*.tmp")
        );
        // only the required input is set by default
        assert_eq!(t.inputs.len(), 1);
    }

    #[test]
    fn delete_files_step_accepts_source_folder() {
        let t =
            delete_files_step("**/*.log").with_input("SourceFolder", "$(Build.ArtifactStagingDirectory)");
        assert_eq!(t.task, "DeleteFiles@1");
        assert_eq!(
            t.inputs.get("SourceFolder").map(|s| s.as_str()),
            Some("$(Build.ArtifactStagingDirectory)")
        );
        assert_eq!(t.inputs.len(), 2);
    }

    #[test]
    fn delete_files_step_accepts_remove_source_folder_flag() {
        let t = delete_files_step("*")
            .with_input("SourceFolder", "$(Build.ArtifactStagingDirectory)")
            .with_input("RemoveSourceFolder", "true");
        assert_eq!(t.task, "DeleteFiles@1");
        assert_eq!(
            t.inputs.get("RemoveSourceFolder").map(|s| s.as_str()),
            Some("true")
        );
        assert_eq!(t.inputs.len(), 3);
    }

    #[test]
    fn delete_files_step_accepts_remove_dot_files_flag() {
        let t = delete_files_step("**").with_input("RemoveDotFiles", "true");
        assert_eq!(t.task, "DeleteFiles@1");
        assert_eq!(
            t.inputs.get("RemoveDotFiles").map(|s| s.as_str()),
            Some("true")
        );
        assert_eq!(t.inputs.len(), 2);
    }

    #[test]
    fn delete_files_step_multiline_contents() {
        let t = delete_files_step("**/*.tmp\n**/*.log\ndist/");
        assert_eq!(t.task, "DeleteFiles@1");
        assert_eq!(
            t.inputs.get("Contents").map(|s| s.as_str()),
            Some("**/*.tmp\n**/*.log\ndist/")
        );
    }

    // ── CmdLine@2 ────────────────────────────────────────────────────────

    #[test]
    fn cmd_line_step_sets_task_and_required_input() {
        let t = cmd_line_step("echo hello");
        assert_eq!(t.task, "CmdLine@2");
        assert_eq!(t.display_name, "Command Line Script");
        assert_eq!(
            t.inputs.get("script").map(|s| s.as_str()),
            Some("echo hello")
        );
        // only the required input is set by default
        assert_eq!(t.inputs.len(), 1);
    }

    #[test]
    fn cmd_line_step_accepts_working_directory() {
        let t = cmd_line_step("dir /b")
            .with_input("workingDirectory", "$(Build.SourcesDirectory)");
        assert_eq!(t.task, "CmdLine@2");
        assert_eq!(
            t.inputs.get("workingDirectory").map(|s| s.as_str()),
            Some("$(Build.SourcesDirectory)")
        );
        assert_eq!(t.inputs.len(), 2);
    }

    #[test]
    fn cmd_line_step_accepts_fail_on_stderr() {
        let t = cmd_line_step("my-tool --verbose").with_input("failOnStderr", "true");
        assert_eq!(t.task, "CmdLine@2");
        assert_eq!(
            t.inputs.get("failOnStderr").map(|s| s.as_str()),
            Some("true")
        );
        assert_eq!(t.inputs.len(), 2);
    }

    #[test]
    fn cmd_line_step_accepts_multiline_script() {
        let script = "echo step 1\necho step 2\necho step 3";
        let t = cmd_line_step(script);
        assert_eq!(t.task, "CmdLine@2");
        assert_eq!(t.inputs.get("script").map(|s| s.as_str()), Some(script));
        assert_eq!(t.inputs.len(), 1);
    }

    // ── PublishPipelineArtifact@1 ─────────────────────────────────────────

    #[test]
    fn publish_pipeline_artifact_step_sets_task_and_target_path() {
        let t = publish_pipeline_artifact_step("$(Build.ArtifactStagingDirectory)");
        assert_eq!(t.task, "PublishPipelineArtifact@1");
        assert_eq!(t.display_name, "Publish Pipeline Artifact");
        assert_eq!(
            t.inputs.get("targetPath").map(|s| s.as_str()),
            Some("$(Build.ArtifactStagingDirectory)")
        );
        // only the required input is set by default
        assert_eq!(t.inputs.len(), 1);
    }

    // ── DownloadPipelineArtifact@2 ───────────────────────────────────────

    #[test]
    fn download_pipeline_artifact_step_sets_task_and_required_input() {
        let t = download_pipeline_artifact_step("$(Pipeline.Workspace)/drop");
        assert_eq!(t.task, "DownloadPipelineArtifact@2");
        assert_eq!(t.display_name, "Download Pipeline Artifact");
        assert_eq!(
            t.inputs.get("targetPath").map(|s| s.as_str()),
            Some("$(Pipeline.Workspace)/drop")
        );
        // only the required input is set by default
        assert_eq!(t.inputs.len(), 1);
    }

    #[test]
    fn publish_pipeline_artifact_step_accepts_artifact_name() {
        let t = publish_pipeline_artifact_step("$(Build.ArtifactStagingDirectory)/output")
            .with_input("artifact", "drop");
        assert_eq!(t.task, "PublishPipelineArtifact@1");
        assert_eq!(
            t.inputs.get("artifact").map(|s| s.as_str()),
            Some("drop")
        );
        assert_eq!(t.inputs.len(), 2);
    }

    #[test]
    fn download_pipeline_artifact_step_filters_by_artifact_name() {
        let t = download_pipeline_artifact_step("$(Agent.TempDirectory)/out")
            .with_input("artifact", "drop");
        assert_eq!(t.task, "DownloadPipelineArtifact@2");
        assert_eq!(
            t.inputs.get("artifact").map(|s| s.as_str()),
            Some("drop")
        );
        assert_eq!(
            t.inputs.get("targetPath").map(|s| s.as_str()),
            Some("$(Agent.TempDirectory)/out")
        );
        assert_eq!(t.inputs.len(), 2);
    }

    #[test]
    fn publish_pipeline_artifact_step_accepts_publish_location() {
        let t = publish_pipeline_artifact_step("$(Build.ArtifactStagingDirectory)")
            .with_input("artifact", "binaries")
            .with_input("publishLocation", "pipeline");
        assert_eq!(t.task, "PublishPipelineArtifact@1");
        assert_eq!(
            t.inputs.get("publishLocation").map(|s| s.as_str()),
            Some("pipeline")
        );
        assert_eq!(t.inputs.len(), 3);
    }

    #[test]
    fn publish_pipeline_artifact_step_accepts_file_share_path() {
        let t = publish_pipeline_artifact_step("$(Build.ArtifactStagingDirectory)")
            .with_input("publishLocation", "filepath")
            .with_input("fileSharePath", "\\\\myserver\\share\\$(Build.DefinitionName)");
        assert_eq!(t.task, "PublishPipelineArtifact@1");
        assert_eq!(
            t.inputs.get("publishLocation").map(|s| s.as_str()),
            Some("filepath")
        );
        assert_eq!(
            t.inputs.get("fileSharePath").map(|s| s.as_str()),
            Some("\\\\myserver\\share\\$(Build.DefinitionName)")
        );
        assert_eq!(t.inputs.len(), 3);
    }

    #[test]
    fn download_pipeline_artifact_step_specific_source_with_branch() {
        let t = download_pipeline_artifact_step("$(Agent.TempDirectory)/prev")
            .with_input("source", "specific")
            .with_input("project", "$(System.TeamProject)")
            .with_input("pipeline", "$(System.DefinitionId)")
            .with_input("runVersion", "latestFromBranch")
            .with_input("branchName", "$(Build.SourceBranch)")
            .with_input("artifact", "safe_outputs")
            .with_input("allowPartiallySucceededBuilds", "true");
        assert_eq!(t.task, "DownloadPipelineArtifact@2");
        assert_eq!(
            t.inputs.get("source").map(|s| s.as_str()),
            Some("specific")
        );
        assert_eq!(
            t.inputs.get("runVersion").map(|s| s.as_str()),
            Some("latestFromBranch")
        );
        assert_eq!(
            t.inputs.get("branchName").map(|s| s.as_str()),
            Some("$(Build.SourceBranch)")
        );
        assert_eq!(
            t.inputs.get("allowPartiallySucceededBuilds").map(|s| s.as_str()),
            Some("true")
        );
        assert_eq!(t.inputs.len(), 8);
    }

    #[test]
    fn download_pipeline_artifact_step_accepts_glob_patterns() {
        let t = download_pipeline_artifact_step("$(Build.ArtifactStagingDirectory)")
            .with_input("patterns", "**/*.zip\n**/*.tar.gz");
        assert_eq!(t.task, "DownloadPipelineArtifact@2");
        assert_eq!(
            t.inputs.get("patterns").map(|s| s.as_str()),
            Some("**/*.zip\n**/*.tar.gz")
        );
        assert_eq!(t.inputs.len(), 2);
    }

    // ── Npm@1 ─────────────────────────────────────────────────────────────

    #[test]
    fn npm_step_install_sets_task_and_command() {
        let t = npm_step("install");
        assert_eq!(t.task, "Npm@1");
        assert_eq!(t.display_name, "npm install");
        assert_eq!(t.inputs.get("command").map(|s| s.as_str()), Some("install"));
        // only the required input is set by default
        assert_eq!(t.inputs.len(), 1);
    }

    #[test]
    fn npm_step_ci_command() {
        let t = npm_step("ci");
        assert_eq!(t.task, "Npm@1");
        assert_eq!(t.display_name, "npm ci");
        assert_eq!(t.inputs.get("command").map(|s| s.as_str()), Some("ci"));
        assert_eq!(t.inputs.len(), 1);
    }

    #[test]
    fn npm_step_publish_command() {
        let t = npm_step("publish");
        assert_eq!(t.task, "Npm@1");
        assert_eq!(t.display_name, "npm publish");
        assert_eq!(
            t.inputs.get("command").map(|s| s.as_str()),
            Some("publish")
        );
        assert_eq!(t.inputs.len(), 1);
    }

    #[test]
    fn npm_step_custom_with_working_dir_and_command() {
        let t = npm_step("custom")
            .with_input("customCommand", "run build -- --production")
            .with_input("workingDir", "$(Build.SourcesDirectory)/frontend");
        assert_eq!(t.task, "Npm@1");
        assert_eq!(t.display_name, "npm custom");
        assert_eq!(t.inputs.get("command").map(|s| s.as_str()), Some("custom"));
        assert_eq!(
            t.inputs.get("customCommand").map(|s| s.as_str()),
            Some("run build -- --production")
        );
        assert_eq!(
            t.inputs.get("workingDir").map(|s| s.as_str()),
            Some("$(Build.SourcesDirectory)/frontend")
        );
        assert_eq!(t.inputs.len(), 3);
    }

    #[test]
    fn npm_step_publish_with_azure_artifacts_feed() {
        let t = npm_step("publish")
            .with_input("publishRegistry", "useFeed")
            .with_input("publishFeed", "my-org/my-feed");
        assert_eq!(t.task, "Npm@1");
        assert_eq!(
            t.inputs.get("command").map(|s| s.as_str()),
            Some("publish")
        );
        assert_eq!(
            t.inputs.get("publishRegistry").map(|s| s.as_str()),
            Some("useFeed")
        );
        assert_eq!(
            t.inputs.get("publishFeed").map(|s| s.as_str()),
            Some("my-org/my-feed")
        );
        assert_eq!(t.inputs.len(), 3);
    }

    #[test]
    fn npm_step_install_with_custom_feed() {
        let t = npm_step("install")
            .with_input("customRegistry", "useFeed")
            .with_input("customFeed", "my-org/npm-feed");
        assert_eq!(t.task, "Npm@1");
        assert_eq!(
            t.inputs.get("customRegistry").map(|s| s.as_str()),
            Some("useFeed")
        );
        assert_eq!(
            t.inputs.get("customFeed").map(|s| s.as_str()),
            Some("my-org/npm-feed")
        );
        assert_eq!(t.inputs.len(), 3);
    }

    // ── Docker@2 ─────────────────────────────────────────────────────────

    #[test]
    fn docker_build_and_push_step_sets_task_and_command() {
        let t = docker_build_and_push_step();
        assert_eq!(t.task, "Docker@2");
        assert_eq!(t.display_name, "Build and Push Docker Image");
        assert_eq!(
            t.inputs.get("command").map(|s| s.as_str()),
            Some("buildAndPush")
        );
        // only the command input is set by default
        assert_eq!(t.inputs.len(), 1);
    }

    #[test]
    fn docker_build_and_push_step_accepts_registry_and_repository() {
        let t = docker_build_and_push_step()
            .with_input("containerRegistry", "myRegistryServiceConnection")
            .with_input("repository", "myapp");
        assert_eq!(t.task, "Docker@2");
        assert_eq!(
            t.inputs.get("containerRegistry").map(|s| s.as_str()),
            Some("myRegistryServiceConnection")
        );
        assert_eq!(
            t.inputs.get("repository").map(|s| s.as_str()),
            Some("myapp")
        );
        assert_eq!(t.inputs.len(), 3);
    }

    #[test]
    fn docker_build_and_push_step_accepts_dockerfile_and_tags() {
        let t = docker_build_and_push_step()
            .with_input("Dockerfile", "src/Dockerfile")
            .with_input("buildContext", "src/")
            .with_input("tags", "latest\n$(Build.BuildId)");
        assert_eq!(t.task, "Docker@2");
        assert_eq!(
            t.inputs.get("Dockerfile").map(|s| s.as_str()),
            Some("src/Dockerfile")
        );
        assert_eq!(
            t.inputs.get("buildContext").map(|s| s.as_str()),
            Some("src/")
        );
        assert_eq!(
            t.inputs.get("tags").map(|s| s.as_str()),
            Some("latest\n$(Build.BuildId)")
        );
        assert_eq!(t.inputs.len(), 4);
    }

    #[test]
    fn docker_build_step_sets_task_and_command() {
let t = docker_build_step();
assert_eq!(t.task, "Docker@2");
assert_eq!(t.display_name, "Build Docker Image");
assert_eq!(
    t.inputs.get("command").map(|s| s.as_str()),
    Some("build")
);
assert_eq!(t.inputs.len(), 1);
    }

    #[test]
    fn docker_build_step_accepts_optional_inputs() {
        let t = docker_build_step()
            .with_input("repository", "myapp")
            .with_input("Dockerfile", "Dockerfile.prod")
            .with_input("arguments", "--no-cache --build-arg ENV=prod");
        assert_eq!(t.task, "Docker@2");
        assert_eq!(
            t.inputs.get("repository").map(|s| s.as_str()),
            Some("myapp")
        );
        assert_eq!(
            t.inputs.get("Dockerfile").map(|s| s.as_str()),
            Some("Dockerfile.prod")
        );
        assert_eq!(
            t.inputs.get("arguments").map(|s| s.as_str()),
            Some("--no-cache --build-arg ENV=prod")
        );
        assert_eq!(t.inputs.len(), 4);
    }

    #[test]
    fn docker_push_step_sets_task_and_command() {
        let t = docker_push_step();
        assert_eq!(t.task, "Docker@2");
        assert_eq!(t.display_name, "Push Docker Image");
        assert_eq!(
            t.inputs.get("command").map(|s| s.as_str()),
            Some("push")
        );
        assert_eq!(t.inputs.len(), 1);
    }

    #[test]
    fn docker_push_step_accepts_registry_repository_and_tags() {
        let t = docker_push_step()
            .with_input("containerRegistry", "myRegistry")
            .with_input("repository", "myapp")
            .with_input("tags", "$(Build.BuildId)");
        assert_eq!(t.task, "Docker@2");
        assert_eq!(
            t.inputs.get("containerRegistry").map(|s| s.as_str()),
            Some("myRegistry")
        );
        assert_eq!(t.inputs.len(), 4);
    }

    #[test]
    fn docker_login_step_sets_task_and_command() {
        let t = docker_login_step();
        assert_eq!(t.task, "Docker@2");
        assert_eq!(t.display_name, "Docker Login");
        assert_eq!(
            t.inputs.get("command").map(|s| s.as_str()),
            Some("login")
        );
        assert_eq!(t.inputs.len(), 1);
    }

    #[test]
    fn docker_login_step_accepts_container_registry() {
        let t = docker_login_step().with_input("containerRegistry", "myPrivateRegistry");
        assert_eq!(t.task, "Docker@2");
        assert_eq!(
            t.inputs.get("containerRegistry").map(|s| s.as_str()),
            Some("myPrivateRegistry")
        );
        assert_eq!(t.inputs.len(), 2);
    }

    #[test]
    fn docker_logout_step_sets_task_and_command() {
        let t = docker_logout_step();
        assert_eq!(t.task, "Docker@2");
        assert_eq!(t.display_name, "Docker Logout");
        assert_eq!(
            t.inputs.get("command").map(|s| s.as_str()),
            Some("logout")
        );
        assert_eq!(t.inputs.len(), 1);
    }

    #[test]
    fn docker_logout_step_accepts_container_registry() {
        let t = docker_logout_step().with_input("containerRegistry", "myPrivateRegistry");
        assert_eq!(t.task, "Docker@2");
        assert_eq!(
            t.inputs.get("containerRegistry").map(|s| s.as_str()),
            Some("myPrivateRegistry")
        );
        assert_eq!(t.inputs.len(), 2);
    }

    #[test]
    fn docker_login_and_logout_use_same_task_name() {
        let login = docker_login_step();
        let logout = docker_logout_step();
        assert_eq!(login.task, logout.task);
        assert_eq!(login.task, "Docker@2");
        assert_ne!(
            login.inputs.get("command"),
            logout.inputs.get("command"),
            "login and logout must use different command values"
        );
    }

    // ── Cache@2 ──────────────────────────────────────────────────────────

    #[test]
    fn cache_step_sets_task_and_required_inputs() {
        let t = cache_step(
            "npm | \"$(Agent.OS)\" | package-lock.json",
            "$(Pipeline.Workspace)/.npm",
        );
        assert_eq!(t.task, "Cache@2");
        assert_eq!(t.display_name, "Cache");
        assert_eq!(
            t.inputs.get("key").map(|s| s.as_str()),
            Some("npm | \"$(Agent.OS)\" | package-lock.json")
        );
        assert_eq!(
            t.inputs.get("path").map(|s| s.as_str()),
            Some("$(Pipeline.Workspace)/.npm")
        );
        assert_eq!(t.inputs.len(), 2, "no optional inputs emitted by default");
    }

    #[test]
    fn cache_step_accepts_cache_hit_var() {
        let t = cache_step("nuget | packages.lock.json", "$(UserProfile)/.nuget/packages")
            .with_input("cacheHitVar", "CACHE_RESTORED");
        assert_eq!(t.task, "Cache@2");
        assert_eq!(
            t.inputs.get("cacheHitVar").map(|s| s.as_str()),
            Some("CACHE_RESTORED")
        );
        assert_eq!(t.inputs.len(), 3);
    }

    #[test]
    fn cache_step_accepts_restore_keys() {
        let t = cache_step("pip | \"$(Agent.OS)\" | requirements.txt", ".venv")
            .with_input("restoreKeys", "pip | \"$(Agent.OS)\"");
        assert_eq!(t.task, "Cache@2");
        assert_eq!(
            t.inputs.get("restoreKeys").map(|s| s.as_str()),
            Some("pip | \"$(Agent.OS)\"")
        );
        assert_eq!(t.inputs.len(), 3);
    }
}
