//! Typed builder for `GoTool@0`.
//!
//! ADO task reference:
//! <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/go-tool-v0>

use crate::compile::ir::step::TaskStep;

/// Builder for a [`TaskStep`] invoking `GoTool@0`.
///
/// Finds in the tools cache or downloads a specific version of Go and adds
/// it to the PATH. Equivalent to the `GoTool@0` Azure DevOps task.
///
/// ADO task reference:
/// <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/go-tool-v0>
#[derive(Debug, Clone)]
pub struct GoTool {
    version: String,
    go_path: Option<String>,
    go_bin: Option<String>,
    go_download_url: Option<String>,
    display_name: Option<String>,
}

impl GoTool {
    /// Required input: `version` — the Go version to install (e.g. `"1.21"`).
    pub fn new(version: impl Into<String>) -> Self {
        Self {
            version: version.into(),
            go_path: None,
            go_bin: None,
            go_download_url: None,
            display_name: None,
        }
    }

    /// `goPath` — sets the GOPATH environment variable.
    pub fn go_path(mut self, value: impl Into<String>) -> Self {
        self.go_path = Some(value.into());
        self
    }

    /// `goBin` — sets the GOBIN environment variable.
    pub fn go_bin(mut self, value: impl Into<String>) -> Self {
        self.go_bin = Some(value.into());
        self
    }

    /// `goDownloadUrl` — base URL for Go downloads (default: `https://go.dev/dl`).
    pub fn go_download_url(mut self, value: impl Into<String>) -> Self {
        self.go_download_url = Some(value.into());
        self
    }

    /// Override the default `displayName` (`"Install Go"`).
    pub fn with_display_name(mut self, value: impl Into<String>) -> Self {
        self.display_name = Some(value.into());
        self
    }

    /// Lower into a [`TaskStep`].
    pub fn into_step(self) -> TaskStep {
        let mut t = TaskStep::new(
            "GoTool@0",
            self.display_name
                .unwrap_or_else(|| format!("Install Go {}", self.version)),
        )
        .with_input("version", self.version);
        if let Some(v) = self.go_path {
            t = t.with_input("goPath", v);
        }
        if let Some(v) = self.go_bin {
            t = t.with_input("goBin", v);
        }
        if let Some(v) = self.go_download_url {
            t = t.with_input("goDownloadUrl", v);
        }
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sets_task_and_required_version() {
        let t = GoTool::new("1.21").into_step();
        assert_eq!(t.task, "GoTool@0");
        assert_eq!(t.display_name, "Install Go 1.21");
        assert_eq!(t.inputs.get("version").map(String::as_str), Some("1.21"));
        assert!(t.inputs.get("goPath").is_none());
        assert!(t.inputs.get("goBin").is_none());
        assert!(t.inputs.get("goDownloadUrl").is_none());
    }

    #[test]
    fn optional_inputs_are_emitted_when_set() {
        let t = GoTool::new("1.22")
            .go_path("/home/runner/go")
            .go_bin("/home/runner/go/bin")
            .go_download_url("https://go.dev/dl")
            .into_step();
        assert_eq!(t.inputs.get("version").map(String::as_str), Some("1.22"));
        assert_eq!(
            t.inputs.get("goPath").map(String::as_str),
            Some("/home/runner/go")
        );
        assert_eq!(
            t.inputs.get("goBin").map(String::as_str),
            Some("/home/runner/go/bin")
        );
        assert_eq!(
            t.inputs.get("goDownloadUrl").map(String::as_str),
            Some("https://go.dev/dl")
        );
    }

    #[test]
    fn display_name_override() {
        let t = GoTool::new("1.21")
            .with_display_name("Install Go for agent")
            .into_step();
        assert_eq!(t.display_name, "Install Go for agent");
    }
}
