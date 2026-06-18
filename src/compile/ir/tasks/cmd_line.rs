//! Typed builder for `CmdLine@2`.

use super::common::bool_input;
use crate::compile::ir::step::TaskStep;

/// Builder for a [`TaskStep`] invoking `CmdLine@2`.
///
/// Runs an inline command-line script (Bash on Linux/macOS, `cmd.exe` on
/// Windows).
///
/// ADO task reference:
/// <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/cmd-line-v2>
#[derive(Debug, Clone)]
pub struct CmdLine {
    script: String,
    working_directory: Option<String>,
    fail_on_stderr: Option<bool>,
    display_name: Option<String>,
}

impl CmdLine {
    /// Required input: the inline `script` text.
    pub fn new(script: impl Into<String>) -> Self {
        Self {
            script: script.into(),
            working_directory: None,
            fail_on_stderr: None,
            display_name: None,
        }
    }

    /// `workingDirectory` — working directory for the script.
    pub fn working_directory(mut self, value: impl Into<String>) -> Self {
        self.working_directory = Some(value.into());
        self
    }

    /// `failOnStderr` — fail the step if the script writes to stderr.
    pub fn fail_on_stderr(mut self, value: bool) -> Self {
        self.fail_on_stderr = Some(value);
        self
    }

    /// Override the default `displayName` (`"Command Line Script"`).
    pub fn with_display_name(mut self, value: impl Into<String>) -> Self {
        self.display_name = Some(value.into());
        self
    }

    /// Lower into a [`TaskStep`].
    pub fn into_step(self) -> TaskStep {
        let mut t = TaskStep::new(
            "CmdLine@2",
            self.display_name.unwrap_or_else(|| "Command Line Script".into()),
        )
        .with_input("script", self.script);
        if let Some(v) = self.working_directory {
            t = t.with_input("workingDirectory", v);
        }
        if let Some(v) = self.fail_on_stderr {
            t = t.with_input("failOnStderr", bool_input(v));
        }
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sets_task_and_required_script() {
        let t = CmdLine::new("echo hello").into_step();
        assert_eq!(t.task, "CmdLine@2");
        assert_eq!(t.inputs.get("script").map(String::as_str), Some("echo hello"));
    }

    #[test]
    fn optional_inputs() {
        let t = CmdLine::new("my-tool --verbose")
            .working_directory("$(Build.SourcesDirectory)")
            .fail_on_stderr(true)
            .into_step();
        assert_eq!(
            t.inputs.get("workingDirectory").map(String::as_str),
            Some("$(Build.SourcesDirectory)")
        );
        assert_eq!(t.inputs.get("failOnStderr").map(String::as_str), Some("true"));
    }
}
