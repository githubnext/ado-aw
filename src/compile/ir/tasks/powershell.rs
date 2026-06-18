//! Typed builder for `PowerShell@2`.
//!
//! [`PowerShell::file`] and [`PowerShell::inline`] return **distinct typestate
//! builders** ([`PowerShellFile`] / [`PowerShellInline`]). The `arguments`
//! input exists only on the file builder, so an `arguments` + inline-script
//! combination cannot even be written — there is no silent drop (the previous
//! single-struct design accepted `.arguments()` on an inline target and quietly
//! discarded it). Shared optionals (`errorActionPreference`, `failOnStderr`,
//! `ignoreLASTEXITCODE`, `pwsh`, `workingDirectory`) are available on both.
//!
//! ADO task reference:
//! <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/powershell-v2>

use super::common::{push_bool, push_opt};
use crate::compile::ir::step::TaskStep;

/// Non-terminating error behaviour for the PowerShell builders
/// (`errorActionPreference`).
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

/// Optionals shared by both `PowerShell@2` targets.
#[derive(Debug, Clone, Default)]
struct Shared {
    error_action_preference: Option<ErrorActionPreference>,
    fail_on_stderr: Option<bool>,
    ignore_last_exit_code: Option<bool>,
    pwsh: Option<bool>,
    working_directory: Option<String>,
}

impl Shared {
    fn apply(self, t: &mut TaskStep) {
        if let Some(v) = self.error_action_preference {
            t.inputs
                .insert("errorActionPreference".to_string(), v.as_ado_str().to_string());
        }
        push_bool(t, "failOnStderr", self.fail_on_stderr);
        push_bool(t, "ignoreLASTEXITCODE", self.ignore_last_exit_code);
        push_bool(t, "pwsh", self.pwsh);
        push_opt(t, "workingDirectory", self.working_directory);
    }
}

/// Generate the optional setters shared by both PowerShell builders. Each
/// builder has a `shared: Shared` field and a `display_name: Option<String>`.
macro_rules! shared_powershell_setters {
    () => {
        /// `errorActionPreference` — non-terminating error behaviour (default `stop`).
        pub fn error_action_preference(mut self, value: ErrorActionPreference) -> Self {
            self.shared.error_action_preference = Some(value);
            self
        }

        /// `failOnStderr` — fail the step if anything is written to stderr.
        pub fn fail_on_stderr(mut self, value: bool) -> Self {
            self.shared.fail_on_stderr = Some(value);
            self
        }

        /// `ignoreLASTEXITCODE` — do not fail when `$LASTEXITCODE` is non-zero.
        pub fn ignore_last_exit_code(mut self, value: bool) -> Self {
            self.shared.ignore_last_exit_code = Some(value);
            self
        }

        /// `pwsh` — use PowerShell Core (`pwsh`) instead of Windows PowerShell.
        pub fn pwsh(mut self, value: bool) -> Self {
            self.shared.pwsh = Some(value);
            self
        }

        /// `workingDirectory` — working directory for the script.
        pub fn working_directory(mut self, value: impl Into<String>) -> Self {
            self.shared.working_directory = Some(value.into());
            self
        }

        /// Override the default `displayName` (`"PowerShell Script"`).
        pub fn with_display_name(mut self, value: impl Into<String>) -> Self {
            self.display_name = Some(value.into());
            self
        }
    };
}

/// Builder for `PowerShell@2` in file-path mode (`targetType: filePath`).
#[derive(Debug, Clone)]
pub struct PowerShellFile {
    file_path: String,
    arguments: Option<String>,
    shared: Shared,
    display_name: Option<String>,
}

impl PowerShellFile {
    /// `arguments` — arguments passed to the script. Available only in file
    /// mode (ADO's `arguments` input is ignored for inline scripts).
    pub fn arguments(mut self, value: impl Into<String>) -> Self {
        self.arguments = Some(value.into());
        self
    }

    shared_powershell_setters!();

    /// Lower into a [`TaskStep`].
    pub fn into_step(self) -> TaskStep {
        let mut t = TaskStep::new(
            "PowerShell@2",
            self.display_name.unwrap_or_else(|| "PowerShell Script".into()),
        )
        .with_input("targetType", "filePath")
        .with_input("filePath", self.file_path);
        push_opt(&mut t, "arguments", self.arguments);
        self.shared.apply(&mut t);
        t
    }
}

/// Builder for `PowerShell@2` in inline mode (`targetType: inline`).
#[derive(Debug, Clone)]
pub struct PowerShellInline {
    script: String,
    shared: Shared,
    display_name: Option<String>,
}

impl PowerShellInline {
    shared_powershell_setters!();

    /// Lower into a [`TaskStep`].
    pub fn into_step(self) -> TaskStep {
        let mut t = TaskStep::new(
            "PowerShell@2",
            self.display_name.unwrap_or_else(|| "PowerShell Script".into()),
        )
        .with_input("targetType", "inline")
        .with_input("script", self.script);
        self.shared.apply(&mut t);
        t
    }
}

/// Entry point for the `PowerShell@2` builders. `file` and `inline` return
/// distinct typestate builders so each mode only exposes its valid inputs.
pub struct PowerShell;

impl PowerShell {
    /// File-path mode: run the script at `file_path`.
    pub fn file(file_path: impl Into<String>) -> PowerShellFile {
        PowerShellFile {
            file_path: file_path.into(),
            arguments: None,
            shared: Shared::default(),
            display_name: None,
        }
    }

    /// Inline mode: run `script` as an inline block.
    pub fn inline(script: impl Into<String>) -> PowerShellInline {
        PowerShellInline {
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

    // Note: there is intentionally no `arguments` setter on `PowerShellInline`,
    // so `PowerShell::inline(...).arguments(...)` does not compile — the
    // arguments/inline mismatch is unrepresentable rather than silently dropped.
}
