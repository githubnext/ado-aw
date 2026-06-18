//! Front-matter task-step **validation** (prototype).
//!
//! The builder structs in this module are normally used one-way
//! (`Builder::new(...).into_step()`), but the ones that derive
//! [`serde::Deserialize`] keyed on ADO input names can *also* parse and validate
//! an authored task step. [`parse_task_step`] is the inverse direction of
//! `into_step()` and is designed to sit in front of the front-matter `steps:`
//! passthrough with **partial coverage**, so it has three outcomes:
//!
//! - `Ok(Some(step))` — the `task:` is one we model; its inputs were valid, and
//!   a normalized [`TaskStep`] is returned.
//! - `Ok(None)` — this step is **not** something we validate (a task with no
//!   typed builder yet, or a non-task step like `bash:`/`script:`). The caller
//!   should keep the original YAML untouched (today: `Step::RawYaml`). Coverage
//!   is therefore additive: mapping a new task only ever *adds* validation and
//!   never rejects a workflow that compiled before.
//! - `Err(e)` — the `task:` is one we model but its inputs are wrong (missing a
//!   required input, an unknown input key, a bad constrained value, or an input
//!   supplied for the wrong command of a command-dispatch task). This is a real
//!   authoring error worth surfacing.
//!
//! Only `CopyFiles@2` and `Docker@2` are wired up here as a proof of concept;
//! extending coverage is a matter of deriving `Deserialize` on the remaining
//! builders and adding a match arm.

use anyhow::{Context, Result, bail};
use serde_yaml::Value;

use super::copy_files::CopyFiles;
use super::docker::{Docker, DockerCommand};
use crate::compile::ir::step::TaskStep;

/// Validate a single front-matter step. Returns `Ok(Some(_))` for a recognized
/// and valid task step, `Ok(None)` for anything we don't model (pass it through
/// unchanged), and `Err(_)` for a recognized task with invalid inputs.
pub fn parse_task_step(step: &Value) -> Result<Option<TaskStep>> {
    // Not a mapping, or not a `task:` step (e.g. `bash:` / `script:` / a
    // checkout) → nothing for us to validate; leave it to the existing
    // passthrough.
    let Some(map) = step.as_mapping() else {
        return Ok(None);
    };
    let Some(task) = map.get("task").and_then(Value::as_str) else {
        return Ok(None);
    };

    let display_name = map
        .get("displayName")
        .and_then(Value::as_str)
        .map(str::to_string);
    let inputs = map
        .get("inputs")
        .cloned()
        .unwrap_or_else(|| Value::Mapping(Default::default()));

    let validated = match task {
        "CopyFiles@2" => {
            let mut builder: CopyFiles = serde_yaml::from_value(inputs)
                .with_context(|| format!("invalid inputs for `{task}`"))?;
            if let Some(dn) = display_name {
                builder = builder.with_display_name(dn);
            }
            builder.into_step()
        }
        "Docker@2" => parse_docker(inputs, display_name)?,
        // No typed builder for this task (yet) → not an error; the caller keeps
        // the original YAML as an opaque passthrough step.
        _ => return Ok(None),
    };
    Ok(Some(validated))
}

/// `Docker@2` selects the command via the `command` input; dispatch on it and
/// validate the remaining inputs against that command's allowed set.
fn parse_docker(inputs: Value, display_name: Option<String>) -> Result<TaskStep> {
    let mut map = match inputs {
        Value::Mapping(m) => m,
        Value::Null => Default::default(),
        _ => bail!("`inputs` must be a mapping"),
    };
    let command = map
        .remove("command")
        .and_then(|v| v.as_str().map(str::to_string))
        .context("Docker@2 requires a `command` input")?;
    let rest = Value::Mapping(map);

    let cmd = match command.as_str() {
        "buildAndPush" => DockerCommand::BuildAndPush(
            serde_yaml::from_value(rest).context("invalid inputs for command `buildAndPush`")?,
        ),
        "build" => DockerCommand::Build(
            serde_yaml::from_value(rest).context("invalid inputs for command `build`")?,
        ),
        "push" => DockerCommand::Push(
            serde_yaml::from_value(rest).context("invalid inputs for command `push`")?,
        ),
        "login" => DockerCommand::Login(
            serde_yaml::from_value(rest).context("invalid inputs for command `login`")?,
        ),
        "logout" => DockerCommand::Logout(
            serde_yaml::from_value(rest).context("invalid inputs for command `logout`")?,
        ),
        other => bail!("Docker@2: unknown command `{other}`"),
    };

    let mut docker = Docker::new(cmd);
    if let Some(dn) = display_name {
        docker = docker.with_display_name(dn);
    }
    Ok(docker.into_step())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn yaml(input: &str) -> Value {
        serde_yaml::from_str(input).expect("test YAML should parse")
    }

    // ── CopyFiles@2 ──────────────────────────────────────────────────────

    #[test]
    fn copy_files_valid_roundtrips_to_task_step() {
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
        let t = parse_task_step(&step).expect("valid CopyFiles step").expect("recognized task");
        assert_eq!(t.task, "CopyFiles@2");
        assert_eq!(t.display_name, "Stage build output");
        assert_eq!(t.inputs.get("Contents").map(String::as_str), Some("**/*.dll"));
        assert_eq!(
            t.inputs.get("TargetFolder").map(String::as_str),
            Some("$(Build.ArtifactStagingDirectory)")
        );
        assert_eq!(
            t.inputs.get("SourceFolder").map(String::as_str),
            Some("$(Build.SourcesDirectory)/bin")
        );
        // Native YAML bool and ADO-style string bool both normalize to "true".
        assert_eq!(t.inputs.get("CleanTargetFolder").map(String::as_str), Some("true"));
        assert_eq!(t.inputs.get("OverWrite").map(String::as_str), Some("true"));
        // Untouched optionals are absent.
        assert!(t.inputs.get("flattenFolders").is_none());
    }

    #[test]
    fn copy_files_missing_required_input_is_rejected() {
        let step = yaml(
            r#"
            task: CopyFiles@2
            inputs:
              Contents: "**"
            "#,
        );
        let err = parse_task_step(&step).unwrap_err().to_string();
        assert!(err.contains("invalid inputs for `CopyFiles@2`"), "got: {err}");
    }

    #[test]
    fn copy_files_unknown_input_is_rejected() {
        let step = yaml(
            r#"
            task: CopyFiles@2
            inputs:
              Contents: "**"
              TargetFolder: out
              Bogus: nope
            "#,
        );
        let err = parse_task_step(&step).unwrap_err().to_string();
        assert!(err.contains("invalid inputs for `CopyFiles@2`"), "got: {err}");
    }

    #[test]
    fn copy_files_bad_bool_value_is_rejected() {
        let step = yaml(
            r#"
            task: CopyFiles@2
            inputs:
              Contents: "**"
              TargetFolder: out
              CleanTargetFolder: yesplease
            "#,
        );
        assert!(parse_task_step(&step).is_err());
    }

    // ── Docker@2 (command-dispatch) ──────────────────────────────────────

    #[test]
    fn docker_build_and_push_valid() {
        let step = yaml(
            r#"
            task: Docker@2
            inputs:
              command: buildAndPush
              repository: myapp
              tags: latest
            "#,
        );
        let t = parse_task_step(&step).expect("valid Docker buildAndPush").expect("recognized");
        assert_eq!(t.task, "Docker@2");
        assert_eq!(t.inputs.get("command").map(String::as_str), Some("buildAndPush"));
        assert_eq!(t.inputs.get("repository").map(String::as_str), Some("myapp"));
        assert_eq!(t.inputs.get("tags").map(String::as_str), Some("latest"));
    }

    #[test]
    fn docker_login_valid() {
        let step = yaml(
            r#"
            task: Docker@2
            inputs:
              command: login
              containerRegistry: myRegistry
            "#,
        );
        let t = parse_task_step(&step).expect("valid Docker login").expect("recognized");
        assert_eq!(t.inputs.get("command").map(String::as_str), Some("login"));
        assert_eq!(t.inputs.get("containerRegistry").map(String::as_str), Some("myRegistry"));
    }

    #[test]
    fn docker_input_for_wrong_command_is_rejected() {
        // `repository` is not valid for `command: login`.
        let step = yaml(
            r#"
            task: Docker@2
            inputs:
              command: login
              repository: myapp
            "#,
        );
        let err = parse_task_step(&step).unwrap_err().to_string();
        assert!(err.contains("command `login`"), "got: {err}");
    }

    #[test]
    fn docker_missing_command_is_rejected() {
        let step = yaml(
            r#"
            task: Docker@2
            inputs:
              repository: myapp
            "#,
        );
        let err = parse_task_step(&step).unwrap_err().to_string();
        assert!(err.contains("requires a `command`"), "got: {err}");
    }

    #[test]
    fn docker_unknown_command_is_rejected() {
        let step = yaml(
            r#"
            task: Docker@2
            inputs:
              command: teleport
            "#,
        );
        let err = parse_task_step(&step).unwrap_err().to_string();
        assert!(err.contains("unknown command `teleport`"), "got: {err}");
    }

    // ── Dispatch / partial coverage ──────────────────────────────────────

    #[test]
    fn unmapped_task_passes_through_unvalidated() {
        // A task we don't model is NOT an error — it returns None so the caller
        // keeps the original YAML (today: Step::RawYaml). Note the bogus input:
        // we deliberately don't validate it.
        let step = yaml(
            r#"
            task: SomeRandomTask@9
            inputs:
              whatever: 123
            "#,
        );
        let result = parse_task_step(&step).expect("unmapped task is not an error");
        assert!(result.is_none(), "unmapped task should pass through (None)");
    }

    #[test]
    fn non_task_step_passes_through() {
        // A `bash:`/`script:`/checkout step has no `task:` key → None.
        let step = yaml("bash: echo hi\ndisplayName: greet");
        assert!(parse_task_step(&step).expect("not an error").is_none());

        // A scalar (malformed step) also just passes through rather than erroring.
        let scalar = yaml("\"- some weird string\"");
        assert!(parse_task_step(&scalar).expect("not an error").is_none());
    }
}
