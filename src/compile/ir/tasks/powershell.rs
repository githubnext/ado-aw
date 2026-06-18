//! Typed builder for `PowerShell@2`.
//!
//! Collapses the former `powershell_file_step` / `powershell_inline_step` pair
//! into a single builder whose [`PowerShellTarget`] selects file-path vs inline
//! mode. The `arguments` input only exists on the `File` variant, so an
//! `arguments` + inline-script combination is structurally unrepresentable in
//! the emitted YAML.

use super::common::bool_input;
use crate::compile::ir::step::TaskStep;

/// Non-terminating error behaviour for [`PowerShell`] (`errorActionPreference`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorActionPreference {
    Stop,
    Continue,
    SilentlyContinue,
}

impl ErrorActionPreference {
    /// The exact token the ADO task expects.
    pub fn as_ado_str(self) -> &'static str {
        match self {
            ErrorActionPreference::Stop => "stop",
            ErrorActionPreference::Continue => "continue",
            ErrorActionPreference::SilentlyContinue => "silentlyContinue",
        }
    }
}

/// Execution target for [`PowerShell`] — `targetType: filePath` vs `inline`.
#[derive(Debug, Clone)]
pub enum PowerShellTarget {
    /// `targetType: filePath` — run the script at `file_path`. `arguments`
    /// applies only to this variant.
    File {
        file_path: String,
        arguments: Option<String>,
    },
    /// `targetType: inline` — run `script` as an inline block.
    Inline { script: String },
}

/// Builder for a [`TaskStep`] invoking `PowerShell@2`.
///
/// ADO task reference:
/// <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/powershell-v2>
#[derive(Debug, Clone)]
pub struct PowerShell {
    target: PowerShellTarget,
    error_action_preference: Option<ErrorActionPreference>,
    fail_on_stderr: Option<bool>,
    ignore_last_exit_code: Option<bool>,
    pwsh: Option<bool>,
    working_directory: Option<String>,
    display_name: Option<String>,
}

impl PowerShell {
    /// Construct from an explicit [`PowerShellTarget`].
    pub fn new(target: PowerShellTarget) -> Self {
        Self {
            target,
            error_action_preference: None,
            fail_on_stderr: None,
            ignore_last_exit_code: None,
            pwsh: None,
            working_directory: None,
            display_name: None,
        }
    }

    /// File-path mode: run the script at `file_path`.
    pub fn file(file_path: impl Into<String>) -> Self {
        Self::new(PowerShellTarget::File {
            file_path: file_path.into(),
            arguments: None,
        })
    }

    /// Inline mode: run `script` as an inline block.
    pub fn inline(script: impl Into<String>) -> Self {
        Self::new(PowerShellTarget::Inline {
            script: script.into(),
        })
    }

    /// `arguments` — script arguments. Applies to the `File` target only; a
    /// no-op on an inline target.
    pub fn arguments(mut self, value: impl Into<String>) -> Self {
        if let PowerShellTarget::File { arguments, .. } = &mut self.target {
            *arguments = Some(value.into());
        }
        self
    }

    /// `errorActionPreference` — non-terminating error behaviour (default `stop`).
    pub fn error_action_preference(mut self, value: ErrorActionPreference) -> Self {
        self.error_action_preference = Some(value);
        self
    }

    /// `failOnStderr` — fail the step if anything is written to stderr.
    pub fn fail_on_stderr(mut self, value: bool) -> Self {
        self.fail_on_stderr = Some(value);
        self
    }

    /// `ignoreLASTEXITCODE` — do not fail when `$LASTEXITCODE` is non-zero.
    pub fn ignore_last_exit_code(mut self, value: bool) -> Self {
        self.ignore_last_exit_code = Some(value);
        self
    }

    /// `pwsh` — use PowerShell Core (`pwsh`) instead of Windows PowerShell.
    pub fn pwsh(mut self, value: bool) -> Self {
        self.pwsh = Some(value);
        self
    }

    /// `workingDirectory` — working directory for the script.
    pub fn working_directory(mut self, value: impl Into<String>) -> Self {
        self.working_directory = Some(value.into());
        self
    }

    /// Override the default `displayName` (`"PowerShell Script"`).
    pub fn with_display_name(mut self, value: impl Into<String>) -> Self {
        self.display_name = Some(value.into());
        self
    }

    /// Lower into a [`TaskStep`].
    pub fn into_step(self) -> TaskStep {
        let mut t = TaskStep::new(
            "PowerShell@2",
            self.display_name.unwrap_or_else(|| "PowerShell Script".into()),
        );
        match self.target {
            PowerShellTarget::File {
                file_path,
                arguments,
            } => {
                t = t
                    .with_input("targetType", "filePath")
                    .with_input("filePath", file_path);
                if let Some(v) = arguments {
                    t = t.with_input("arguments", v);
                }
            }
            PowerShellTarget::Inline { script } => {
                t = t
                    .with_input("targetType", "inline")
                    .with_input("script", script);
            }
        }
        if let Some(v) = self.error_action_preference {
            t = t.with_input("errorActionPreference", v.as_ado_str());
        }
        if let Some(v) = self.fail_on_stderr {
            t = t.with_input("failOnStderr", bool_input(v));
        }
        if let Some(v) = self.ignore_last_exit_code {
            t = t.with_input("ignoreLASTEXITCODE", bool_input(v));
        }
        if let Some(v) = self.pwsh {
            t = t.with_input("pwsh", bool_input(v));
        }
        if let Some(v) = self.working_directory {
            t = t.with_input("workingDirectory", v);
        }
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_mode_sets_target_and_path() {
        let t = PowerShell::file("scripts/build.ps1").into_step();
        assert_eq!(t.task, "PowerShell@2");
        assert_eq!(t.inputs.get("targetType").map(String::as_str), Some("filePath"));
        assert_eq!(t.inputs.get("filePath").map(String::as_str), Some("scripts/build.ps1"));
    }

    #[test]
    fn file_mode_arguments_and_options() {
        let t = PowerShell::file("scripts/build.ps1")
            .arguments("-Configuration Release")
            .working_directory("$(Build.SourcesDirectory)")
            .pwsh(true)
            .error_action_preference(ErrorActionPreference::Continue)
            .into_step();
        assert_eq!(t.inputs.get("arguments").map(String::as_str), Some("-Configuration Release"));
        assert_eq!(t.inputs.get("pwsh").map(String::as_str), Some("true"));
        assert_eq!(t.inputs.get("errorActionPreference").map(String::as_str), Some("continue"));
    }

    #[test]
    fn inline_mode_sets_script() {
        let t = PowerShell::inline("Write-Host hi")
            .ignore_last_exit_code(true)
            .into_step();
        assert_eq!(t.inputs.get("targetType").map(String::as_str), Some("inline"));
        assert_eq!(t.inputs.get("script").map(String::as_str), Some("Write-Host hi"));
        assert_eq!(t.inputs.get("ignoreLASTEXITCODE").map(String::as_str), Some("true"));
    }

    #[test]
    fn arguments_is_noop_on_inline_target() {
        let t = PowerShell::inline("Write-Host hi").arguments("-X").into_step();
        assert!(t.inputs.get("arguments").is_none());
    }
}
