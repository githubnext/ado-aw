//! Typed builder for `PythonScript@0`.
//!
//! [`PythonScript::file`] and [`PythonScript::inline`] return **distinct typestate
//! builders** ([`PythonScriptFile`] / [`PythonScriptInline`]). The `arguments`
//! input is available only on the file builder (arguments passed to a Python
//! script file via `sys.argv`; inline scripts have no meaningful argv). Shared
//! optionals (`pythonInterpreter`, `workingDirectory`, `failOnStderr`) are
//! available on both builders.
//!
//! ADO task reference:
//! <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/python-script-v0>

use super::common::{de_opt_bool_flex, push_bool, push_opt};
use crate::compile::ir::step::TaskStep;
use serde::Deserialize;
use serde_yaml::Value;

/// Validate an authored `PythonScript@0` `inputs:` mapping (advisory
/// front-matter validation, see [`super::parse`]).
pub(crate) fn validate_inputs(inputs: Value) -> Result<(), String> {
    let mut map = match inputs {
        Value::Mapping(m) => m,
        Value::Null => Default::default(),
        other => return Err(format!("`inputs` must be a mapping, got {other:?}")),
    };
    let script_source = match map.remove("scriptSource") {
        Some(v) => Some(
            v.as_str()
                .ok_or_else(|| "PythonScript@0: `scriptSource` must be a string".to_string())?
                .to_string(),
        ),
        None => None,
    };
    let mode = script_source.as_deref().unwrap_or("filePath");
    let rest = Value::Mapping(map);

    let result = match mode {
        "filePath" => serde_yaml::from_value::<PythonScriptFile>(rest).map(drop),
        "inline" => serde_yaml::from_value::<PythonScriptInline>(rest).map(drop),
        other => return Err(format!("PythonScript@0: unknown scriptSource `{other}`")),
    };
    result.map_err(|e| format!("scriptSource `{mode}`: {e}"))
}

/// Generate the optional setters shared by both PythonScript builders.
macro_rules! shared_python_script_setters {
    () => {
        /// `pythonInterpreter` — path to the Python interpreter. If not set,
        /// uses the interpreter from PATH (or the one configured by
        /// `UsePythonVersion@0` / `UsePythonVersion@0`).
        pub fn python_interpreter(mut self, value: impl Into<String>) -> Self {
            self.python_interpreter = Some(value.into());
            self
        }

        /// `workingDirectory` — working directory for the script. Defaults to
        /// the repository root (`$(System.DefaultWorkingDirectory)`).
        pub fn working_directory(mut self, value: impl Into<String>) -> Self {
            self.working_directory = Some(value.into());
            self
        }

        /// `failOnStderr` — fail the step if the script writes to stderr.
        pub fn fail_on_stderr(mut self, value: bool) -> Self {
            self.fail_on_stderr = Some(value);
            self
        }

        /// Override the default `displayName` (`"Run a Python script"`).
        pub fn with_display_name(mut self, value: impl Into<String>) -> Self {
            self.display_name = Some(value.into());
            self
        }
    };
}

/// Builder for `PythonScript@0` in file-path mode (`scriptSource: filePath`).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PythonScriptFile {
    #[serde(rename = "scriptPath")]
    script_path: String,
    #[serde(rename = "arguments", default)]
    arguments: Option<String>,
    #[serde(rename = "pythonInterpreter", default)]
    python_interpreter: Option<String>,
    #[serde(rename = "workingDirectory", default)]
    working_directory: Option<String>,
    #[serde(
        rename = "failOnStderr",
        default,
        deserialize_with = "de_opt_bool_flex"
    )]
    fail_on_stderr: Option<bool>,
    #[serde(skip)]
    display_name: Option<String>,
}

impl PythonScriptFile {
    /// `arguments` — arguments passed to the script, available through
    /// `sys.argv`. Only meaningful in file mode.
    pub fn arguments(mut self, value: impl Into<String>) -> Self {
        self.arguments = Some(value.into());
        self
    }

    shared_python_script_setters!();

    /// Lower into a [`TaskStep`].
    pub fn into_step(self) -> TaskStep {
        let mut t = TaskStep::new(
            "PythonScript@0",
            self.display_name
                .unwrap_or_else(|| "Run a Python script".into()),
        )
        .with_input("scriptSource", "filePath")
        .with_input("scriptPath", self.script_path);
        push_opt(&mut t, "arguments", self.arguments);
        push_opt(&mut t, "pythonInterpreter", self.python_interpreter);
        push_opt(&mut t, "workingDirectory", self.working_directory);
        push_bool(&mut t, "failOnStderr", self.fail_on_stderr);
        t
    }
}

/// Builder for `PythonScript@0` in inline mode (`scriptSource: inline`).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PythonScriptInline {
    #[serde(rename = "script")]
    script: String,
    #[serde(rename = "pythonInterpreter", default)]
    python_interpreter: Option<String>,
    #[serde(rename = "workingDirectory", default)]
    working_directory: Option<String>,
    #[serde(
        rename = "failOnStderr",
        default,
        deserialize_with = "de_opt_bool_flex"
    )]
    fail_on_stderr: Option<bool>,
    #[serde(skip)]
    display_name: Option<String>,
}

impl PythonScriptInline {
    shared_python_script_setters!();

    /// Lower into a [`TaskStep`].
    pub fn into_step(self) -> TaskStep {
        let mut t = TaskStep::new(
            "PythonScript@0",
            self.display_name
                .unwrap_or_else(|| "Run a Python script".into()),
        )
        .with_input("scriptSource", "inline")
        .with_input("script", self.script);
        push_opt(&mut t, "pythonInterpreter", self.python_interpreter);
        push_opt(&mut t, "workingDirectory", self.working_directory);
        push_bool(&mut t, "failOnStderr", self.fail_on_stderr);
        t
    }
}

/// Entry point for the `PythonScript@0` builders. [`PythonScript::file`] and
/// [`PythonScript::inline`] return distinct typestate builders so each mode
/// only exposes its valid inputs.
pub struct PythonScript;

impl PythonScript {
    /// File-path mode: run the Python script at `script_path`.
    pub fn file(script_path: impl Into<String>) -> PythonScriptFile {
        PythonScriptFile {
            script_path: script_path.into(),
            arguments: None,
            python_interpreter: None,
            working_directory: None,
            fail_on_stderr: None,
            display_name: None,
        }
    }

    /// Inline mode: run `script` as an inline Python block.
    pub fn inline(script: impl Into<String>) -> PythonScriptInline {
        PythonScriptInline {
            script: script.into(),
            python_interpreter: None,
            working_directory: None,
            fail_on_stderr: None,
            display_name: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_mode_sets_source_and_path() {
        let t = PythonScript::file("scripts/analyze.py").into_step();
        assert_eq!(t.task, "PythonScript@0");
        assert_eq!(
            t.inputs.get("scriptSource").map(String::as_str),
            Some("filePath")
        );
        assert_eq!(
            t.inputs.get("scriptPath").map(String::as_str),
            Some("scripts/analyze.py")
        );
        assert!(!t.inputs.contains_key("script"));
    }

    #[test]
    fn file_mode_with_arguments_and_options() {
        let t = PythonScript::file("scripts/build.py")
            .arguments("--config release")
            .python_interpreter("/usr/bin/python3")
            .working_directory("$(Build.SourcesDirectory)")
            .fail_on_stderr(true)
            .with_display_name("Build Python project")
            .into_step();
        assert_eq!(
            t.inputs.get("arguments").map(String::as_str),
            Some("--config release")
        );
        assert_eq!(
            t.inputs.get("pythonInterpreter").map(String::as_str),
            Some("/usr/bin/python3")
        );
        assert_eq!(
            t.inputs.get("workingDirectory").map(String::as_str),
            Some("$(Build.SourcesDirectory)")
        );
        assert_eq!(
            t.inputs.get("failOnStderr").map(String::as_str),
            Some("true")
        );
        assert_eq!(t.display_name, "Build Python project");
    }

    #[test]
    fn inline_mode_sets_source_and_script() {
        let t = PythonScript::inline("print('hello')").into_step();
        assert_eq!(t.task, "PythonScript@0");
        assert_eq!(
            t.inputs.get("scriptSource").map(String::as_str),
            Some("inline")
        );
        assert_eq!(
            t.inputs.get("script").map(String::as_str),
            Some("print('hello')")
        );
        assert!(!t.inputs.contains_key("scriptPath"));
        assert!(!t.inputs.contains_key("arguments"));
    }

    #[test]
    fn inline_mode_with_interpreter_and_working_dir() {
        let t = PythonScript::inline("import sys; print(sys.version)")
            .python_interpreter("python3.11")
            .working_directory("/workspace")
            .into_step();
        assert_eq!(
            t.inputs.get("pythonInterpreter").map(String::as_str),
            Some("python3.11")
        );
        assert_eq!(
            t.inputs.get("workingDirectory").map(String::as_str),
            Some("/workspace")
        );
    }

    #[test]
    fn default_display_name() {
        let t = PythonScript::file("foo.py").into_step();
        assert_eq!(t.display_name, "Run a Python script");
    }

    #[test]
    fn omits_unset_optionals() {
        let t = PythonScript::inline("pass").into_step();
        assert!(!t.inputs.contains_key("pythonInterpreter"));
        assert!(!t.inputs.contains_key("workingDirectory"));
        assert!(!t.inputs.contains_key("failOnStderr"));
        assert!(!t.inputs.contains_key("arguments"));
    }

    // Note: `PythonScriptInline` intentionally has no `arguments` setter,
    // so `PythonScript::inline(...).arguments(...)` does not compile —
    // the arguments/inline mismatch is unrepresentable rather than silently dropped.
}
