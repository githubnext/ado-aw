//! Typed builder for `NodeTool@0`.
//!
//! ADO task reference:
//! <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/node-tool-installer-v0>

use super::common::bool_input;
use crate::compile::ir::step::TaskStep;

/// Version source for [`NodeTool`].
///
/// Controls whether the Node.js version is taken from an explicit version
/// spec string or read from an `.nvmrc` / `.node-version` file.
#[derive(Debug, Clone)]
enum VersionSource {
    /// Use an explicit version spec (e.g. `"20.x"`).  This is the default.
    Spec(String),
    /// Read the version from the file at the given path (e.g. `".nvmrc"`).
    File(String),
}

/// Builder for a [`TaskStep`] invoking `NodeTool@0`.
///
/// Installs a specific version of Node.js and adds it to the PATH.
/// `NodeTool@0` is the legacy Node.js tool installer; new pipelines should
/// prefer [`super::use_node::UseNode`] (`UseNode@1`).
///
/// ADO task reference:
/// <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/node-tool-installer-v0>
#[derive(Debug, Clone)]
pub struct NodeTool {
    source: VersionSource,
    check_latest: Option<bool>,
    force32bit: Option<bool>,
    nodejs_mirror: Option<String>,
    retry_count_on_download_fails: Option<String>,
    delay_between_retries: Option<String>,
    display_name: Option<String>,
}

impl NodeTool {
    /// Construct with an explicit version spec (e.g. `"20.x"`, `"18.x"`).
    ///
    /// The `versionSpec` input is set to `spec`; `versionSource` defaults to
    /// `spec` and is omitted from the emitted YAML (no unnecessary noise).
    pub fn new(version_spec: impl Into<String>) -> Self {
        Self {
            source: VersionSource::Spec(version_spec.into()),
            check_latest: None,
            force32bit: None,
            nodejs_mirror: None,
            retry_count_on_download_fails: None,
            delay_between_retries: None,
            display_name: None,
        }
    }

    /// Construct from a version file (e.g. `".nvmrc"` or `".node-version"`).
    ///
    /// Sets `versionSource` to `fromFile` and `versionFilePath` to `path`.
    pub fn from_file(path: impl Into<String>) -> Self {
        Self {
            source: VersionSource::File(path.into()),
            check_latest: None,
            force32bit: None,
            nodejs_mirror: None,
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

    /// `nodejsMirror` — base URL for Node.js binaries.
    /// Default: `"https://nodejs.org/dist"`.
    pub fn nodejs_mirror(mut self, value: impl Into<String>) -> Self {
        self.nodejs_mirror = Some(value.into());
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

    /// Override the default `displayName`.
    pub fn with_display_name(mut self, value: impl Into<String>) -> Self {
        self.display_name = Some(value.into());
        self
    }

    /// Lower into a [`TaskStep`].
    pub fn into_step(self) -> TaskStep {
        let default_display = match &self.source {
            VersionSource::Spec(spec) => format!("Install Node.js {spec}"),
            VersionSource::File(path) => format!("Install Node.js (from {path})"),
        };
        let mut t = TaskStep::new("NodeTool@0", self.display_name.unwrap_or(default_display));
        match self.source {
            VersionSource::Spec(spec) => {
                t = t.with_input("versionSpec", spec);
            }
            VersionSource::File(path) => {
                t = t
                    .with_input("versionSource", "fromFile")
                    .with_input("versionFilePath", path);
            }
        }
        if let Some(v) = self.check_latest {
            t = t.with_input("checkLatest", bool_input(v));
        }
        if let Some(v) = self.force32bit {
            t = t.with_input("force32bit", bool_input(v));
        }
        if let Some(v) = self.nodejs_mirror {
            t = t.with_input("nodejsMirror", v);
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
    fn sets_task_and_version_spec() {
        let t = NodeTool::new("20.x").into_step();
        assert_eq!(t.task, "NodeTool@0");
        assert_eq!(t.display_name, "Install Node.js 20.x");
        assert_eq!(
            t.inputs.get("versionSpec").map(String::as_str),
            Some("20.x")
        );
        assert!(t.inputs.get("versionSource").is_none());
        assert!(t.inputs.get("checkLatest").is_none());
        assert!(t.inputs.get("force32bit").is_none());
        assert!(t.inputs.get("nodejsMirror").is_none());
    }

    #[test]
    fn from_file_sets_version_source_and_path() {
        let t = NodeTool::from_file(".nvmrc").into_step();
        assert_eq!(t.task, "NodeTool@0");
        assert_eq!(t.display_name, "Install Node.js (from .nvmrc)");
        assert_eq!(
            t.inputs.get("versionSource").map(String::as_str),
            Some("fromFile")
        );
        assert_eq!(
            t.inputs.get("versionFilePath").map(String::as_str),
            Some(".nvmrc")
        );
        assert!(t.inputs.get("versionSpec").is_none());
    }

    #[test]
    fn optional_inputs_are_emitted_when_set() {
        let t = NodeTool::new("18.x")
            .check_latest(true)
            .force32bit(false)
            .nodejs_mirror("https://my.mirror/nodejs/dist")
            .retry_count_on_download_fails("3")
            .delay_between_retries("500")
            .into_step();
        assert_eq!(
            t.inputs.get("versionSpec").map(String::as_str),
            Some("18.x")
        );
        assert_eq!(
            t.inputs.get("checkLatest").map(String::as_str),
            Some("true")
        );
        assert_eq!(
            t.inputs.get("force32bit").map(String::as_str),
            Some("false")
        );
        assert_eq!(
            t.inputs.get("nodejsMirror").map(String::as_str),
            Some("https://my.mirror/nodejs/dist")
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
        let t = NodeTool::new("20.x")
            .with_display_name("Install Node.js for ado-script")
            .into_step();
        assert_eq!(t.display_name, "Install Node.js for ado-script");
        assert_eq!(
            t.inputs.get("versionSpec").map(String::as_str),
            Some("20.x")
        );
    }

    #[test]
    fn different_versions() {
        for version in &["16.x", "18.x", "20.x", "22.x"] {
            let t = NodeTool::new(*version).into_step();
            assert_eq!(t.task, "NodeTool@0");
            assert_eq!(
                t.inputs.get("versionSpec").map(String::as_str),
                Some(*version)
            );
            assert_eq!(t.display_name, format!("Install Node.js {version}"));
        }
    }
}
