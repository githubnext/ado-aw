//! Front-matter task-step **validation** (advisory).
//!
//! The builder structs in this module are normally used one-way
//! (`Builder::new(...).into_step()`), but the ones that derive
//! [`serde::Deserialize`] keyed on ADO input names can *also* parse and validate
//! an authored task step. This follows the idiomatic Rust *parse, don't
//! validate* pattern: the typed builder **is** the schema, a successful
//! deserialization **is** the validation, and a serde error **is** the
//! diagnostic.
//!
//! [`validate_task_step`] sits in front of the front-matter `steps:` passthrough
//! with **partial coverage** and three outcomes:
//!
//! - `None` — the step is **not** something we validate (a task with no typed
//!   builder yet, or a non-task step like `bash:`/`script:`). The caller keeps
//!   the original YAML untouched. Coverage is additive: mapping a new task only
//!   ever *adds* validation and never rejects a workflow that compiled before.
//! - `Some(Ok(()))` — the `task:` is one we model and its inputs are valid.
//! - `Some(Err(msg))` — the `task:` is one we model but its inputs are wrong
//!   (missing a required input, an unknown input key, a bad constrained value,
//!   or an input supplied for the wrong command of a command-dispatch task).
//!
//! The function **never** returns `anyhow::Err` / panics, so the compiler can
//! treat every finding as an advisory **warning** rather than a hard failure.
//!
//! New tasks are wired up by deriving `Deserialize` on the builder (keyed on ADO
//! input names) and adding one line to [`VALIDATORS`].

use serde::de::DeserializeOwned;
use serde_yaml::Value;

use super::{
    archive_files, azure_cli, azure_container_apps, azure_file_copy, azure_function_app,
    azure_key_vault, azure_powershell, azure_web_app, bicep_deploy, cargo_authenticate, cmd_line,
    copy_files, delete_files, docker, docker_installer, dotnet_core_cli, download_build_artifacts,
    download_package, download_pipeline_artifact, download_secure_file, extract_files,
    github_release, go_tool, gradle, helm_installer, java_tool_installer, manual_validation, maven,
    maven_authenticate, node_tool, npm, npm_authenticate, nuget_authenticate, nuget_command,
    pip_authenticate, powershell, publish_build_artifacts, publish_code_coverage_results,
    publish_pipeline_artifact, publish_test_results, python_script, sonar_qube_analyze,
    sonar_qube_prepare, sonar_qube_publish, twine_authenticate, universal_packages, use_dotnet,
    use_node, use_python_version, use_ruby_version, vs_build, vstest,
};

/// Registry mapping an ADO task id (`"CopyFiles@2"`) to a validator that checks
/// an authored `inputs:` mapping. Flat builders use
/// [`validate_by_deserialize`]; command/mode-dispatch tasks supply a bespoke
/// `validate_inputs` that dispatches on their discriminator input (e.g.
/// `command`, `targetType`, `Destination`). One line per task — adding a new
/// task is a `#[derive(Deserialize)]` on the builder plus one entry here.
///
/// Lookup is a linear scan ([`validate_task_step`]), run once per authored task
/// step. At ~46 entries and a handful of steps per workflow this is negligible;
/// switch to a sorted array + `partition_point` binary search (or a
/// `OnceLock<HashMap<&str, _>>`) if coverage grows past a few hundred entries.
#[allow(clippy::type_complexity)]
const VALIDATORS: &[(&str, fn(Value) -> Result<(), String>)] = &[
    // ── Flat single-struct builders (validity == clean deserialization) ──
    ("ArchiveFiles@2", validate_by_deserialize::<archive_files::ArchiveFiles>),
    ("AzureContainerApps@1", validate_by_deserialize::<azure_container_apps::AzureContainerApps>),
    ("AzureFunctionApp@2", validate_by_deserialize::<azure_function_app::AzureFunctionApp>),
    ("AzureKeyVault@2", validate_by_deserialize::<azure_key_vault::AzureKeyVault>),
    ("AzureWebApp@1", validate_by_deserialize::<azure_web_app::AzureWebApp>),
    ("CargoAuthenticate@0", validate_by_deserialize::<cargo_authenticate::CargoAuthenticate>),
    ("CmdLine@2", validate_by_deserialize::<cmd_line::CmdLine>),
    ("CopyFiles@2", validate_by_deserialize::<copy_files::CopyFiles>),
    ("DeleteFiles@1", validate_by_deserialize::<delete_files::DeleteFiles>),
    ("DockerInstaller@0", validate_by_deserialize::<docker_installer::DockerInstaller>),
    (
        "DownloadBuildArtifacts@1",
        validate_by_deserialize::<download_build_artifacts::DownloadBuildArtifacts>,
    ),
    ("DownloadPackage@1", validate_by_deserialize::<download_package::DownloadPackage>),
    (
        "DownloadPipelineArtifact@2",
        validate_by_deserialize::<download_pipeline_artifact::DownloadPipelineArtifact>,
    ),
    ("DownloadSecureFile@1", validate_by_deserialize::<download_secure_file::DownloadSecureFile>),
    ("ExtractFiles@1", validate_by_deserialize::<extract_files::ExtractFiles>),
    ("GoTool@0", validate_by_deserialize::<go_tool::GoTool>),
    ("Gradle@3", validate_by_deserialize::<gradle::Gradle>),
    ("HelmInstaller@1", validate_by_deserialize::<helm_installer::HelmInstaller>),
    ("ManualValidation@1", validate_by_deserialize::<manual_validation::ManualValidation>),
    ("Maven@3", validate_by_deserialize::<maven::Maven>),
    ("MavenAuthenticate@0", validate_by_deserialize::<maven_authenticate::MavenAuthenticate>),
    ("NuGetAuthenticate@1", validate_by_deserialize::<nuget_authenticate::NuGetAuthenticate>),
    ("NodeTool@0", validate_by_deserialize::<node_tool::NodeTool>),
    ("PipAuthenticate@1", validate_by_deserialize::<pip_authenticate::PipAuthenticate>),
    (
        "PublishCodeCoverageResults@2",
        validate_by_deserialize::<publish_code_coverage_results::PublishCodeCoverageResults>,
    ),
    (
        "PublishPipelineArtifact@1",
        validate_by_deserialize::<publish_pipeline_artifact::PublishPipelineArtifact>,
    ),
    ("PublishTestResults@2", validate_by_deserialize::<publish_test_results::PublishTestResults>),
    ("SonarQubeAnalyze@8", validate_by_deserialize::<sonar_qube_analyze::SonarQubeAnalyze>),
    ("SonarQubePublish@8", validate_by_deserialize::<sonar_qube_publish::SonarQubePublish>),
    ("TwineAuthenticate@1", validate_by_deserialize::<twine_authenticate::TwineAuthenticate>),
    ("UseDotNet@2", validate_by_deserialize::<use_dotnet::UseDotNet>),
    ("UseNode@1", validate_by_deserialize::<use_node::UseNode>),
    ("UsePythonVersion@0", validate_by_deserialize::<use_python_version::UsePythonVersion>),
    ("UseRubyVersion@0", validate_by_deserialize::<use_ruby_version::UseRubyVersion>),
    ("VSBuild@1", validate_by_deserialize::<vs_build::VsBuild>),
    ("npmAuthenticate@0", validate_by_deserialize::<npm_authenticate::NpmAuthenticate>),
    // ── Command / mode-dispatch builders (custom discriminator dispatch) ──
    ("AzureCLI@2", azure_cli::validate_inputs),
    ("AzureFileCopy@6", azure_file_copy::validate_inputs),
    ("AzurePowerShell@5", azure_powershell::validate_inputs),
    ("BicepDeploy@0", bicep_deploy::validate_inputs),
    ("Docker@2", docker::validate_inputs),
    ("DotNetCoreCLI@2", dotnet_core_cli::validate_inputs),
    ("GitHubRelease@1", github_release::validate_inputs),
    ("JavaToolInstaller@0", java_tool_installer::validate_inputs),
    ("Npm@1", npm::validate_inputs),
    ("NuGetCommand@2", nuget_command::validate_inputs),
    ("PowerShell@2", powershell::validate_inputs),
    ("PublishBuildArtifacts@1", publish_build_artifacts::validate_inputs),
    ("PythonScript@0", python_script::validate_inputs),
    ("SonarQubePrepare@8", sonar_qube_prepare::validate_inputs),
    ("UniversalPackages@1", universal_packages::validate_inputs),
    ("VSTest@2", vstest::validate_inputs),
];

/// Validate one authored front-matter step.
///
/// Returns `None` for anything we don't model (pass it through unchanged),
/// `Some(Ok(()))` for a recognized + valid task, and `Some(Err(msg))` for a
/// recognized task with invalid inputs. Never returns an unrecoverable error.
pub fn validate_task_step(step: &Value) -> Option<Result<(), String>> {
    // Not a mapping, or not a `task:` step (e.g. `bash:` / `script:` / a
    // checkout) → nothing for us to validate.
    let map = step.as_mapping()?;
    let task = map.get("task").and_then(Value::as_str)?;

    // No typed builder for this task (yet) → not validated.
    let (_, validate) = VALIDATORS.iter().find(|(id, _)| *id == task)?;

    let inputs = map
        .get("inputs")
        .cloned()
        .unwrap_or_else(|| Value::Mapping(Default::default()));

    Some(validate(inputs).map_err(|e| format!("task `{task}`: {e}")))
}

/// A structured finding for one invalid authored task step, suitable for
/// surfacing through the agent-facing lint channel (`ado-aw lint` /
/// `lint_workflow` MCP tool). Carries enough location to let an agent that
/// synthesised the step attribute and fix the finding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskStepFinding {
    /// Which front-matter step list the step came from: `"setup"`, `"steps"`,
    /// `"post-steps"`, or `"teardown"`.
    pub list: &'static str,
    /// Zero-based index of the step within that list.
    pub index: usize,
    /// The ADO task id (e.g. `"CopyFiles@2"`).
    pub task: String,
    /// Human-readable description of why the inputs are invalid.
    pub message: String,
}

/// Validate every authored task step across the four front-matter step lists,
/// returning a structured finding per recognized-but-invalid step. Valid steps,
/// unmodeled tasks, and non-task steps produce no finding. Never errors.
///
/// The lists are passed individually (rather than the `FrontMatter` type) to
/// keep this module free of the front-matter grammar; callers pass
/// `front_matter.setup`, `.steps`, `.post_steps`, `.teardown`.
pub fn validate_front_matter_task_steps(
    setup: &[Value],
    steps: &[Value],
    post_steps: &[Value],
    teardown: &[Value],
) -> Vec<TaskStepFinding> {
    let mut findings = Vec::new();
    for (list, values) in [
        ("setup", setup),
        ("steps", steps),
        ("post-steps", post_steps),
        ("teardown", teardown),
    ] {
        for (index, step) in values.iter().enumerate() {
            if let Some(Err(message)) = validate_task_step(step) {
                // A `Some(_)` result guarantees `validate_task_step` already
                // matched a string `task:` key (it early-returns `None`
                // otherwise), so this re-extraction cannot actually fail; the
                // fallback is only to avoid another `Option` in the finding.
                let task = step
                    .as_mapping()
                    .and_then(|m| m.get("task"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                findings.push(TaskStepFinding {
                    list,
                    index,
                    task,
                    message,
                });
            }
        }
    }
    findings
}

/// Validate an `inputs:` mapping by attempting to deserialize it into the typed
/// builder `T`. `Ok(())` iff it deserializes cleanly (required inputs present,
/// no unknown keys, constrained values valid).
pub(crate) fn validate_by_deserialize<T: DeserializeOwned>(inputs: Value) -> Result<(), String> {
    serde_yaml::from_value::<T>(inputs)
        .map(drop)
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn yaml(input: &str) -> Value {
        serde_yaml::from_str(input).expect("test YAML should parse")
    }

    // ── CopyFiles@2 ──────────────────────────────────────────────────────

    #[test]
    fn copy_files_valid_is_ok() {
        let step = yaml(
            r#"
            task: CopyFiles@2
            displayName: Stage build output
            inputs:
              Contents: "**/*.dll"
              TargetFolder: $(Build.ArtifactStagingDirectory)
              SourceFolder: $(Build.SourcesDirectory)/bin
              CleanTargetFolder: true
              OverWrite: "true"
            "#,
        );
        let r = validate_task_step(&step).expect("recognized task");
        assert!(r.is_ok(), "expected valid, got: {r:?}");
    }

    #[test]
    fn copy_files_missing_required_input_warns() {
        let step = yaml(
            r#"
            task: CopyFiles@2
            inputs:
              Contents: "**"
            "#,
        );
        let err = validate_task_step(&step).expect("recognized").unwrap_err();
        assert!(err.contains("CopyFiles@2"), "got: {err}");
    }

    #[test]
    fn copy_files_unknown_input_warns() {
        let step = yaml(
            r#"
            task: CopyFiles@2
            inputs:
              Contents: "**"
              TargetFolder: out
              Bogus: nope
            "#,
        );
        let err = validate_task_step(&step).expect("recognized").unwrap_err();
        assert!(err.contains("CopyFiles@2"), "got: {err}");
    }

    #[test]
    fn copy_files_bad_bool_value_warns() {
        let step = yaml(
            r#"
            task: CopyFiles@2
            inputs:
              Contents: "**"
              TargetFolder: out
              CleanTargetFolder: yesplease
            "#,
        );
        assert!(validate_task_step(&step).expect("recognized").is_err());
    }

    #[test]
    fn copy_files_capitalized_bool_is_ok() {
        // serde_yaml (YAML 1.2) parses `True`/`False` as strings, not bools;
        // ADO accepts them, so the flexible bool deserializer must too.
        let step = yaml(
            r#"
            task: CopyFiles@2
            inputs:
              Contents: "**"
              TargetFolder: out
              CleanTargetFolder: "True"
              OverWrite: "FALSE"
            "#,
        );
        assert!(
            validate_task_step(&step).expect("recognized").is_ok(),
            "capitalized string booleans must not be rejected"
        );
    }

    #[test]
    fn copy_files_integer_authored_retry_count_is_ok() {
        // `retryCount`/`delayBetweenRetries` are ADO string inputs, but authors
        // naturally write them as bare integers. These must validate (no
        // false-positive), matching how ADO coerces them.
        let step = yaml(
            r#"
            task: CopyFiles@2
            inputs:
              Contents: "**"
              TargetFolder: out
              retryCount: 3
              delayBetweenRetries: 1000
            "#,
        );
        assert!(
            validate_task_step(&step).expect("recognized").is_ok(),
            "integer-authored string inputs must not be rejected"
        );
    }

    // ── Docker@2 (command-dispatch) ──────────────────────────────────────

    #[test]
    fn docker_build_and_push_valid_is_ok() {
        let step = yaml(
            r#"
            task: Docker@2
            inputs:
              command: buildAndPush
              repository: myapp
              tags: latest
            "#,
        );
        assert!(validate_task_step(&step).expect("recognized").is_ok());
    }

    #[test]
    fn docker_login_valid_is_ok() {
        let step = yaml(
            r#"
            task: Docker@2
            inputs:
              command: login
              containerRegistry: myRegistry
            "#,
        );
        assert!(validate_task_step(&step).expect("recognized").is_ok());
    }

    #[test]
    fn docker_input_for_wrong_command_warns() {
        // `repository` is not valid for `command: login`.
        let step = yaml(
            r#"
            task: Docker@2
            inputs:
              command: login
              repository: myapp
            "#,
        );
        let err = validate_task_step(&step).expect("recognized").unwrap_err();
        assert!(err.contains("login"), "got: {err}");
    }

    #[test]
    fn docker_missing_command_defaults_to_build_and_push() {
        // ADO defaults `command` to `buildAndPush`; `repository` is valid there,
        // so an omitted command is NOT flagged (matches what ADO would run).
        let step = yaml(
            r#"
            task: Docker@2
            inputs:
              repository: myapp
            "#,
        );
        assert!(validate_task_step(&step).expect("recognized").is_ok());
    }

    #[test]
    fn docker_null_inputs_defaults_and_is_ok() {
        // `inputs: ~` deserialises to a null inputs value; the dispatcher must
        // treat it as an empty mapping (default command), not panic or error.
        let step = yaml("task: Docker@2\ninputs: ~");
        assert!(validate_task_step(&step).expect("recognized").is_ok());
    }

    #[test]
    fn docker_unknown_command_warns() {
        let step = yaml(
            r#"
            task: Docker@2
            inputs:
              command: teleport
            "#,
        );
        let err = validate_task_step(&step).expect("recognized").unwrap_err();
        assert!(err.contains("teleport"), "got: {err}");
    }

    // ── Dispatch / partial coverage ──────────────────────────────────────

    #[test]
    fn unmapped_task_is_not_validated() {
        // A task we don't model returns None so the caller keeps the original
        // YAML. Note the bogus input: we deliberately don't validate it.
        let step = yaml(
            r#"
            task: SomeRandomTask@9
            inputs:
              whatever: 123
            "#,
        );
        assert!(validate_task_step(&step).is_none());
    }

    #[test]
    fn non_task_step_is_not_validated() {
        // A `bash:`/`script:`/checkout step has no `task:` key → None.
        let step = yaml("bash: echo hi\ndisplayName: greet");
        assert!(validate_task_step(&step).is_none());

        // A scalar (malformed step) also just passes through rather than erroring.
        let scalar = yaml("\"- some weird string\"");
        assert!(validate_task_step(&scalar).is_none());
    }

    // ── Round-trip coverage ──────────────────────────────────────────────
    // `into_step()` output must re-validate. This proves the Deserialize side
    // (field renames + enum tokens + discriminator dispatch) accepts exactly
    // what the construction side emits, across a representative spread of
    // flat-enum and command/mode-dispatch builders.

    use crate::compile::ir::step::TaskStep;

    /// Turn an `into_step()` result into a `{ task, inputs }` step value and
    /// assert it re-validates cleanly through the registry.
    fn assert_roundtrips(t: TaskStep) {
        let mut inputs = serde_yaml::Mapping::new();
        for (k, v) in &t.inputs {
            inputs.insert(Value::String(k.clone()), Value::String(v.clone()));
        }
        let mut step = serde_yaml::Mapping::new();
        step.insert(Value::String("task".into()), Value::String(t.task.clone()));
        step.insert(Value::String("inputs".into()), Value::Mapping(inputs));

        let result = validate_task_step(&Value::Mapping(step))
            .unwrap_or_else(|| panic!("task `{}` is not registered", t.task));
        assert!(
            result.is_ok(),
            "into_step() output for `{}` failed re-validation: {:?}",
            t.task,
            result
        );
    }

    #[test]
    fn roundtrip_archive_files_flat_enum() {
        assert_roundtrips(archive_files::ArchiveFiles::new("src", "out.zip").into_step());
    }

    #[test]
    fn roundtrip_gradle_flat_enum() {
        assert_roundtrips(gradle::Gradle::new("gradlew", "build").into_step());
    }

    #[test]
    fn roundtrip_powershell_file_dispatch() {
        assert_roundtrips(powershell::PowerShell::file("scripts/build.ps1").into_step());
    }

    #[test]
    fn roundtrip_powershell_inline_with_enum() {
        assert_roundtrips(
            powershell::PowerShell::inline("Write-Host hi")
                .error_action_preference(powershell::ErrorActionPreference::Continue)
                .into_step(),
        );
    }

    #[test]
    fn roundtrip_dotnet_command_dispatch() {
        assert_roundtrips(
            dotnet_core_cli::DotNetCoreCli::build(dotnet_core_cli::DotNetBuild::new()).into_step(),
        );
    }

    #[test]
    fn roundtrip_vstest_selector_dispatch() {
        assert_roundtrips(
            vstest::VsTest::assemblies(vstest::VsTestAssemblies::new("**/*tests.dll")).into_step(),
        );
    }

    #[test]
    fn roundtrip_universal_packages_command_dispatch() {
        assert_roundtrips(
            universal_packages::UniversalPackages::download(
                "my-feed",
                "my-package",
                universal_packages::UniversalPackagesDownload::new(),
            )
            .into_step(),
        );
    }

    #[test]
    fn roundtrip_azure_cli_flat_script_location() {
        // Guards the flat `scriptLocation: inlineScript` + sibling `inlineScript:`
        // authoring shape (the builder models it as a typed enum for construction,
        // but validation must accept the flat form ADO authors / into_step emits).
        assert_roundtrips(
            azure_cli::AzureCli::new(
                "conn",
                azure_cli::ScriptType::Bash,
                azure_cli::ScriptLocation::Inline("echo hi\n".into()),
            )
            .into_step(),
        );
        assert_roundtrips(
            azure_cli::AzureCli::new(
                "conn",
                azure_cli::ScriptType::PsCore,
                azure_cli::ScriptLocation::ScriptPath("scripts/deploy.ps1".into()),
            )
            .into_step(),
        );
    }

    #[test]
    fn azure_cli_unknown_input_warns() {
        let step = yaml(
            r#"
            task: AzureCLI@2
            inputs:
              azureSubscription: conn
              scriptType: bash
              scriptLocation: inlineScript
              inlineScript: echo hi
              Bogus: nope
            "#,
        );
        assert!(validate_task_step(&step).expect("recognized").is_err());
    }

    #[test]
    fn roundtrip_nuget_command_dispatch() {
        assert_roundtrips(
            nuget_command::NuGetCommand::restore(nuget_command::NuGetRestore::new()).into_step(),
        );
    }

    #[test]
    fn roundtrip_npm_command_dispatch() {
        assert_roundtrips(npm::Npm::install(npm::NpmInstall::new()).into_step());
    }

    #[test]
    fn roundtrip_github_release_action_dispatch() {
        assert_roundtrips(
            github_release::GitHubRelease::create(
                "gh-conn",
                "owner/repo",
                github_release::GitHubReleaseCreate::new(),
            )
            .into_step(),
        );
    }

    #[test]
    fn roundtrip_azure_file_copy_destination_dispatch() {
        assert_roundtrips(
            azure_file_copy::AzureFileCopy::new(
                "$(Build.ArtifactStagingDirectory)",
                "arm-conn",
                "mystorage",
                azure_file_copy::AzureFileCopyDestination::AzureBlob(
                    azure_file_copy::AzureFileCopyToBlob::new("my-container"),
                ),
            )
            .into_step(),
        );
    }

    #[test]
    fn roundtrip_publish_build_artifacts_location_dispatch() {
        assert_roundtrips(
            publish_build_artifacts::PublishBuildArtifacts::container(
                "$(Build.ArtifactStagingDirectory)",
                "drop",
            )
            .into_step(),
        );
    }

    #[test]
    fn roundtrip_java_tool_installer_source_dispatch() {
        assert_roundtrips(
            java_tool_installer::JavaToolInstaller::pre_installed(
                "11",
                java_tool_installer::JdkArchitecture::X64,
            )
            .into_step(),
        );
    }

    #[test]
    fn roundtrip_azure_powershell_inline_dispatch() {
        assert_roundtrips(
            azure_powershell::AzurePowerShell::inline("arm-conn", "Write-Host hi").into_step(),
        );
    }

    #[test]
    fn roundtrip_azure_powershell_file_dispatch() {
        assert_roundtrips(
            azure_powershell::AzurePowerShell::file("arm-conn", "scripts/deploy.ps1").into_step(),
        );
    }

    #[test]
    fn roundtrip_python_script_file_dispatch() {
        assert_roundtrips(python_script::PythonScript::file("scripts/run.py").into_step());
    }

    // Flat builders whose enum input is a *required* constructor arg — these
    // exercise the enum's `as_ado_str()` token through the validation path,
    // guarding against a wrong `#[serde(rename = "...")]` on a variant.

    #[test]
    fn roundtrip_azure_web_app_required_enum() {
        assert_roundtrips(
            azure_web_app::AzureWebApp::new(
                "arm-conn",
                azure_web_app::AppType::WebApp,
                "my-app",
                "$(Build.ArtifactStagingDirectory)/app.zip",
            )
            .into_step(),
        );
    }

    #[test]
    fn roundtrip_download_package_required_enum() {
        assert_roundtrips(
            download_package::DownloadPackage::new(
                download_package::PackageType::NuGet,
                "my-feed",
                "my-def",
                "1.0.0",
                "$(System.ArtifactsDirectory)",
            )
            .into_step(),
        );
    }

    #[test]
    fn roundtrip_publish_test_results_required_enum() {
        assert_roundtrips(
            publish_test_results::PublishTestResults::new(
                publish_test_results::TestResultsFormat::JUnit,
                "**/TEST-*.xml",
            )
            .into_step(),
        );
    }

    #[test]
    fn roundtrip_node_tool_spec() {
        assert_roundtrips(node_tool::NodeTool::new("20.x").into_step());
    }

    #[test]
    fn roundtrip_node_tool_from_file() {
        assert_roundtrips(node_tool::NodeTool::from_file(".nvmrc").into_step());
    }

    #[test]
    fn node_tool_unknown_input_warns() {
        let step = yaml(
            r#"
            task: NodeTool@0
            inputs:
              versionSpec: "20.x"
              bogusInput: nope
            "#,
        );
        let err = validate_task_step(&step).expect("recognized").unwrap_err();
        assert!(err.contains("NodeTool@0"), "got: {err}");
    }

    #[test]
    fn roundtrip_sonar_qube_publish() {
        assert_roundtrips(sonar_qube_publish::SonarQubePublish::new().into_step());
        assert_roundtrips(
            sonar_qube_publish::SonarQubePublish::new()
                .polling_timeout_sec(600)
                .into_step(),
        );
    }

    #[test]
    fn sonar_qube_publish_unknown_input_warns() {
        let step = yaml(
            r#"
            task: SonarQubePublish@8
            inputs:
              bogusInput: nope
            "#,
        );
        let err = validate_task_step(&step).expect("recognized").unwrap_err();
        assert!(err.contains("SonarQubePublish@8"), "got: {err}");
    }

    #[test]
    fn roundtrip_sonar_qube_analyze() {
        assert_roundtrips(sonar_qube_analyze::SonarQubeAnalyze::new().into_step());
        assert_roundtrips(
            sonar_qube_analyze::SonarQubeAnalyze::new()
                .jdk_version(sonar_qube_analyze::JdkVersion::JavaHome21X64)
                .into_step(),
        );
    }

    #[test]
    fn sonar_qube_analyze_unknown_input_warns() {
        let step = yaml(
            r#"
            task: SonarQubeAnalyze@8
            inputs:
              bogusInput: nope
            "#,
        );
        let err = validate_task_step(&step).expect("recognized").unwrap_err();
        assert!(err.contains("SonarQubeAnalyze@8"), "got: {err}");
    }

    #[test]
    fn roundtrip_sonar_qube_prepare_dotnet() {
        assert_roundtrips(
            sonar_qube_prepare::SonarQubePrepare::dotnet(
                "sq-conn",
                sonar_qube_prepare::DotNetMode::new("my-key"),
            )
            .into_step(),
        );
    }

    #[test]
    fn roundtrip_sonar_qube_prepare_cli_file() {
        assert_roundtrips(
            sonar_qube_prepare::SonarQubePrepare::cli(
                "sq-conn",
                sonar_qube_prepare::CliMode::File(sonar_qube_prepare::CliFileMode::new()),
            )
            .into_step(),
        );
    }

    #[test]
    fn roundtrip_sonar_qube_prepare_cli_manual() {
        assert_roundtrips(
            sonar_qube_prepare::SonarQubePrepare::cli(
                "sq-conn",
                sonar_qube_prepare::CliMode::Manual(sonar_qube_prepare::CliManualMode::new(
                    "my-key",
                )),
            )
            .into_step(),
        );
    }

    #[test]
    fn roundtrip_sonar_qube_prepare_other() {
        assert_roundtrips(sonar_qube_prepare::SonarQubePrepare::other("sq-conn").into_step());
    }

    #[test]
    fn sonar_qube_prepare_input_for_wrong_mode_warns() {
        // `configFile` is a cli-only input; it must not be accepted in dotnet mode.
        let step = yaml(
            r#"
            task: SonarQubePrepare@8
            inputs:
              SonarQube: sq-conn
              scannerMode: dotnet
              projectKey: my-key
              configFile: sonar-project.properties
            "#,
        );
        let err = validate_task_step(&step).expect("recognized").unwrap_err();
        assert!(err.contains("SonarQubePrepare@8"), "got: {err}");
    }

    #[test]
    fn sonar_qube_prepare_missing_service_connection_warns() {
        let step = yaml(
            r#"
            task: SonarQubePrepare@8
            inputs:
              scannerMode: other
            "#,
        );
        let err = validate_task_step(&step).expect("recognized").unwrap_err();
        assert!(err.contains("SonarQube"), "got: {err}");
    }

    #[test]
    fn roundtrip_azure_function_app() {
        assert_roundtrips(
            azure_function_app::AzureFunctionApp::new(
                "arm-conn",
                azure_function_app::FunctionAppType::Linux,
                "my-func",
                "$(Build.ArtifactStagingDirectory)/app.zip",
            )
            .deploy_to_slot_or_ase(true)
            .resource_group_name("rg")
            .slot_name("staging")
            .deployment_method(azure_function_app::FunctionDeploymentMethod::ZipDeploy)
            .into_step(),
        );
    }

    #[test]
    fn azure_function_app_unknown_input_warns() {
        let step = yaml(
            r#"
            task: AzureFunctionApp@2
            inputs:
              azureSubscription: conn
              appType: functionApp
              appName: my-func
              package: app.zip
              bogusInput: nope
            "#,
        );
        let err = validate_task_step(&step).expect("recognized").unwrap_err();
        assert!(err.contains("AzureFunctionApp@2"), "got: {err}");
    }

    #[test]
    fn roundtrip_bicep_deploy_resource_group() {
        assert_roundtrips(
            bicep_deploy::deploy_to_resource_group("arm-conn", "$(SubId)", "my-rg").into_step(),
        );
    }

    #[test]
    fn roundtrip_bicep_deploy_stack_subscription() {
        assert_roundtrips(
            bicep_deploy::BicepDeploy::new(
                "arm-conn",
                bicep_deploy::BicepScope::Subscription {
                    subscription_id: "$(SubId)".into(),
                    location: "eastus".into(),
                },
                bicep_deploy::BicepDeploymentType::DeploymentStack(
                    bicep_deploy::BicepDeploymentStack::new()
                        .deny_settings_mode(bicep_deploy::BicepDenySettingsMode::DenyDelete)
                        .bypass_stack_out_of_sync_error(true),
                ),
            )
            .template_file("infra/main.bicep")
            .into_step(),
        );
    }

    #[test]
    fn bicep_deploy_stack_input_in_deployment_type_warns() {
        // `denySettingsMode` is a deploymentStack-only input.
        let step = yaml(
            r#"
            task: BicepDeploy@0
            inputs:
              azureResourceManagerConnection: arm-conn
              type: deployment
              scope: resourceGroup
              subscriptionId: $(SubId)
              resourceGroupName: my-rg
              denySettingsMode: denyDelete
            "#,
        );
        let err = validate_task_step(&step).expect("recognized").unwrap_err();
        assert!(err.contains("BicepDeploy@0"), "got: {err}");
    }

    #[test]
    fn bicep_deploy_missing_scope_required_input_warns() {
        let step = yaml(
            r#"
            task: BicepDeploy@0
            inputs:
              azureResourceManagerConnection: arm-conn
              scope: resourceGroup
              subscriptionId: $(SubId)
            "#,
        );
        let err = validate_task_step(&step).expect("recognized").unwrap_err();
        assert!(err.contains("resourceGroupName"), "got: {err}");
    }

    #[test]
    fn registry_has_no_duplicate_task_ids() {
        let mut ids: Vec<&str> = VALIDATORS.iter().map(|(id, _)| *id).collect();
        ids.sort_unstable();
        let mut deduped = ids.clone();
        deduped.dedup();
        assert_eq!(ids, deduped, "VALIDATORS must not contain duplicate task ids");
    }
}
