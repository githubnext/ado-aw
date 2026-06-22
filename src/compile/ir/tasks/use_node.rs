//! Typed builder for `UseNode@1`.
//!
//! ADO task reference:
//! <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/use-node-v1-task>

use super::common::bool_input;
use crate::compile::ir::step::TaskStep;

/// Builder for a [`TaskStep`] invoking `UseNode@1`.
///
/// Sets up the Node.js environment and adds it to the PATH. Equivalent to
/// the `UseNode@1` Azure DevOps task.
///
/// ADO task reference:
/// <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/use-node-v1-task>
#[derive(Debug, Clone)]
pub struct UseNode {
    version: String,
    check_latest: Option<bool>,
    force32bit: Option<bool>,
    retry_count_on_download_fails: Option<String>,
    delay_between_retries: Option<String>,
    display_name: Option<String>,
}

impl UseNode {
    /// Required input: `version` — the Node.js version spec to install
    /// (e.g. `"22.x"`, `"20.x"`, `"18.x"`).
    pub fn new(version: impl Into<String>) -> Self {
        Self {
            version: version.into(),
            check_latest: None,
            force32bit: None,
            retry_count_on_download_fails: None,
            delay_between_retries: None,
            display_name: None,
        }
    }

    /// `checkLatest` — check online for the latest available version that
    /// satisfies the version spec. Default: `false`.
    pub fn check_latest(mut self, value: bool) -> Self {
        self.check_latest = Some(value);
        self
    }

    /// `force32bit` — install the x86 version of Node.js on a 64-bit Windows
    /// agent. Default: `false`.
    pub fn force32bit(mut self, value: bool) -> Self {
        self.force32bit = Some(value);
        self
    }

    /// `retryCountOnDownloadFails` — how many times to retry if the Node.js
    /// binary download fails. Default: `"5"`.
    pub fn retry_count_on_download_fails(mut self, value: impl Into<String>) -> Self {
        self.retry_count_on_download_fails = Some(value.into());
        self
    }

    /// `delayBetweenRetries` — delay in milliseconds between download retries.
    /// Default: `"1000"`.
    pub fn delay_between_retries(mut self, value: impl Into<String>) -> Self {
        self.delay_between_retries = Some(value.into());
        self
    }

    /// Override the default `displayName` (`"Install Node.js <version>"`).
    pub fn with_display_name(mut self, value: impl Into<String>) -> Self {
        self.display_name = Some(value.into());
        self
    }

    /// Lower into a [`TaskStep`].
    pub fn into_step(self) -> TaskStep {
        let mut t = TaskStep::new(
            "UseNode@1",
            self.display_name
                .unwrap_or_else(|| format!("Install Node.js {}", self.version)),
        )
        .with_input("version", self.version);
        if let Some(v) = self.check_latest {
            t = t.with_input("checkLatest", bool_input(v));
        }
        if let Some(v) = self.force32bit {
            t = t.with_input("force32bit", bool_input(v));
        }
        if let Some(v) = self.retry_count_on_download_fails {
            t = t.with_input("retryCountOnDownloadFails", v);
        }
        if let Some(v) = self.delay_between_retries {
            t = t.with_input("delayBetweenRetries", v);
        }
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sets_task_and_required_version() {
        let t = UseNode::new("22.x").into_step();
        assert_eq!(t.task, "UseNode@1");
        assert_eq!(t.display_name, "Install Node.js 22.x");
        assert_eq!(t.inputs.get("version").map(String::as_str), Some("22.x"));
        assert!(t.inputs.get("checkLatest").is_none());
        assert!(t.inputs.get("force32bit").is_none());
        assert!(t.inputs.get("retryCountOnDownloadFails").is_none());
        assert!(t.inputs.get("delayBetweenRetries").is_none());
    }

    #[test]
    fn optional_inputs_are_emitted_when_set() {
        let t = UseNode::new("20.x")
            .check_latest(true)
            .force32bit(false)
            .retry_count_on_download_fails("3")
            .delay_between_retries("500")
            .into_step();
        assert_eq!(t.inputs.get("version").map(String::as_str), Some("20.x"));
        assert_eq!(
            t.inputs.get("checkLatest").map(String::as_str),
            Some("true")
        );
        assert_eq!(
            t.inputs.get("force32bit").map(String::as_str),
            Some("false")
        );
        assert_eq!(
            t.inputs.get("retryCountOnDownloadFails").map(String::as_str),
            Some("3")
        );
        assert_eq!(
            t.inputs.get("delayBetweenRetries").map(String::as_str),
            Some("500")
        );
    }

    #[test]
    fn display_name_override() {
        let t = UseNode::new("22.x")
            .with_display_name("Install Node.js for ado-script")
            .into_step();
        assert_eq!(t.display_name, "Install Node.js for ado-script");
        assert_eq!(t.inputs.get("version").map(String::as_str), Some("22.x"));
    }

    #[test]
    fn different_versions() {
        for version in &["18.x", "20.x", "22.x"] {
            let t = UseNode::new(*version).into_step();
            assert_eq!(t.task, "UseNode@1");
            assert_eq!(
                t.inputs.get("version").map(String::as_str),
                Some(*version)
            );
            assert_eq!(t.display_name, format!("Install Node.js {version}"));
        }
    }
}
