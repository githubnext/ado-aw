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

use super::common::{push_bool, push_opt};
use crate::compile::ir::step::TaskStep;

/// Optionals shared by both `PythonScript@0` builders.
#[derive(Debug, Clone, Default)]
struct Shared {
    python_interpreter: Option<String>,
    working_directory: Option<String>,
    fail_on_stderr: Option<bool>,
}

impl Shared {
    fn apply(self, t: &mut TaskStep) {
        push_opt(t, "pythonInterpreter", self.python_interpreter);
        push_opt(t, "workingDirectory", self.working_directory);
        push_bool(t, "failOnStderr", self.fail_on_stderr);
    }
}

/// Generate the optional setters shared by both PythonScript builders.
macro_rules! shared_python_script_setters {
    () => {
        /// `pythonInterpreter` — path to the Python interpreter. If not set,
        /// uses the interpreter from PATH (or the one configured by
        /// `UsePythonVersion@0` / `UsePythonVersion@0`).
        pub fn python_interpreter(mut self, value: impl Into<String>) -> Self {
            self.shared.python_interpreter = Some(value.into());
            self
        }

        /// `workingDirectory` — working directory for the script. Defaults to
        /// the repository root (`$(System.DefaultWorkingDirectory)`).
        pub fn working_directory(mut self, value: impl Into<String>) -> Self {
            self.shared.working_directory = Some(value.into());
            self
        }

        /// `failOnStderr` — fail the step if the script writes to stderr.
        pub fn fail_on_stderr(mut self, value: bool) -> Self {
            self.shared.fail_on_stderr = Some(value);
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
#[derive(Debug, Clone)]
pub struct PythonScriptFile {
    script_path: String,
    arguments: Option<String>,
    shared: Shared,
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
        self.shared.apply(&mut t);
        t
    }
}

/// Builder for `PythonScript@0` in inline mode (`scriptSource: inline`).
#[derive(Debug, Clone)]
pub struct PythonScriptInline {
    script: String,
    shared: Shared,
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
        self.shared.apply(&mut t);
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
            shared: Shared::default(),
            display_name: None,
        }
    }

    /// Inline mode: run `script` as an inline Python block.
    pub fn inline(script: impl Into<String>) -> PythonScriptInline {
        PythonScriptInline {
            script: script.into(),
            shared: Shared::default(),
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
