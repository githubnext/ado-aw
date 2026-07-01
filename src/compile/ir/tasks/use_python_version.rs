//! Typed builder for `UsePythonVersion@0`.
//!
//! ADO task reference:
//! <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/use-python-version-v0>

use super::common::{bool_input, de_opt_bool_flex};
use crate::compile::ir::step::TaskStep;
use serde::Deserialize;

/// Target architecture for the Python interpreter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum Architecture {
    /// 32-bit (x86) Python interpreter.
    #[serde(rename = "x86")]
    X86,
    /// 64-bit (x64) Python interpreter (default).
    #[serde(rename = "x64")]
    X64,
    /// ARM 64-bit Python interpreter.
    #[serde(rename = "arm64")]
    Arm64,
}

impl Architecture {
    /// Return the ADO input token for this architecture.
    pub fn as_ado_str(self) -> &'static str {
        match self {
            Self::X86 => "x86",
            Self::X64 => "x64",
            Self::Arm64 => "arm64",
        }
    }
}

/// Builder for a [`TaskStep`] invoking `UsePythonVersion@0`.
///
/// Selects and installs the requested Python version on the build agent and
/// adds it to the PATH. Equivalent to the `UsePythonVersion@0` Azure DevOps
/// task.
///
/// ADO task reference:
/// <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/use-python-version-v0>
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UsePythonVersion {
    #[serde(rename = "versionSpec")]
    version_spec: String,
    #[serde(rename = "architecture", default)]
    architecture: Option<Architecture>,
    #[serde(rename = "addToPath", default, deserialize_with = "de_opt_bool_flex")]
    add_to_path: Option<bool>,
    #[serde(
        rename = "disableDownloadFromRegistry",
        default,
        deserialize_with = "de_opt_bool_flex"
    )]
    disable_download_from_registry: Option<bool>,
    #[serde(
        rename = "allowUnstable",
        default,
        deserialize_with = "de_opt_bool_flex"
    )]
    allow_unstable: Option<bool>,
    #[serde(rename = "githubToken", default)]
    github_token: Option<String>,
    #[serde(skip)]
    display_name: Option<String>,
}

impl UsePythonVersion {
    /// Required input: `versionSpec` — the Python version spec to install
    /// (e.g. `"3.x"`, `"3.11"`, `"3.12"`). ADO default: `"3.x"`.
    pub fn new(version_spec: impl Into<String>) -> Self {
        Self {
            version_spec: version_spec.into(),
            architecture: None,
            add_to_path: None,
            disable_download_from_registry: None,
            allow_unstable: None,
            github_token: None,
            display_name: None,
        }
    }

    /// `architecture` — target architecture for the Python interpreter.
    /// Allowed values: `x86`, `x64`, `arm64`. ADO default: `"x64"`.
    pub fn architecture(mut self, value: Architecture) -> Self {
        self.architecture = Some(value);
        self
    }

    /// `addToPath` — whether to prepend the retrieved Python version to the
    /// PATH environment variable. ADO default: `true`.
    pub fn add_to_path(mut self, value: bool) -> Self {
        self.add_to_path = Some(value);
        self
    }

    /// `disableDownloadFromRegistry` — disable downloading Python releases
    /// from the GitHub Actions python registry. ADO default: `false`.
    pub fn disable_download_from_registry(mut self, value: bool) -> Self {
        self.disable_download_from_registry = Some(value);
        self
    }

    /// `allowUnstable` — allow downloading unstable (pre-release) Python
    /// versions. Only meaningful when `disableDownloadFromRegistry` is
    /// `false`. ADO default: `false`.
    pub fn allow_unstable(mut self, value: bool) -> Self {
        self.allow_unstable = Some(value);
        self
    }

    /// `githubToken` — GitHub token used to authenticate against the GitHub
    /// Actions python registry. Only meaningful when
    /// `disableDownloadFromRegistry` is `false`.
    pub fn github_token(mut self, value: impl Into<String>) -> Self {
        self.github_token = Some(value.into());
        self
    }

    /// Override the default `displayName`
    /// (`"Install Python <version_spec>"`).
    pub fn with_display_name(mut self, value: impl Into<String>) -> Self {
        self.display_name = Some(value.into());
        self
    }

    /// Lower into a [`TaskStep`].
    pub fn into_step(self) -> TaskStep {
        let mut t = TaskStep::new(
            "UsePythonVersion@0",
            self.display_name
                .unwrap_or_else(|| format!("Install Python {}", self.version_spec)),
        )
        .with_input("versionSpec", self.version_spec);
        if let Some(v) = self.architecture {
            t = t.with_input("architecture", v.as_ado_str());
        }
        if let Some(v) = self.add_to_path {
            t = t.with_input("addToPath", bool_input(v));
        }
        if let Some(v) = self.disable_download_from_registry {
            t = t.with_input("disableDownloadFromRegistry", bool_input(v));
        }
        if let Some(v) = self.allow_unstable {
            t = t.with_input("allowUnstable", bool_input(v));
        }
        if let Some(v) = self.github_token {
            t = t.with_input("githubToken", v);
        }
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sets_task_and_version_spec() {
        let t = UsePythonVersion::new("3.x").into_step();
        assert_eq!(t.task, "UsePythonVersion@0");
        assert_eq!(t.display_name, "Install Python 3.x");
        assert_eq!(t.inputs.get("versionSpec").map(String::as_str), Some("3.x"));
    }

    #[test]
    fn optional_inputs_not_emitted_by_default() {
        let t = UsePythonVersion::new("3.11").into_step();
        assert!(t.inputs.get("architecture").is_none());
        assert!(t.inputs.get("addToPath").is_none());
        assert!(t.inputs.get("disableDownloadFromRegistry").is_none());
        assert!(t.inputs.get("allowUnstable").is_none());
        assert!(t.inputs.get("githubToken").is_none());
    }

    #[test]
    fn architecture_variants() {
        for (arch, expected) in [
            (Architecture::X86, "x86"),
            (Architecture::X64, "x64"),
            (Architecture::Arm64, "arm64"),
        ] {
            let t = UsePythonVersion::new("3.x").architecture(arch).into_step();
            assert_eq!(
                t.inputs.get("architecture").map(String::as_str),
                Some(expected)
            );
        }
    }

    #[test]
    fn add_to_path_false() {
        let t = UsePythonVersion::new("3.x").add_to_path(false).into_step();
        assert_eq!(t.inputs.get("addToPath").map(String::as_str), Some("false"));
    }

    #[test]
    fn disable_download_from_registry() {
        let t = UsePythonVersion::new("3.x")
            .disable_download_from_registry(true)
            .into_step();
        assert_eq!(
            t.inputs
                .get("disableDownloadFromRegistry")
                .map(String::as_str),
            Some("true")
        );
    }

    #[test]
    fn allow_unstable_and_github_token() {
        let t = UsePythonVersion::new("3.13")
            .allow_unstable(true)
            .github_token("$(MY_GITHUB_TOKEN)")
            .into_step();
        assert_eq!(
            t.inputs.get("allowUnstable").map(String::as_str),
            Some("true")
        );
        assert_eq!(
            t.inputs.get("githubToken").map(String::as_str),
            Some("$(MY_GITHUB_TOKEN)")
        );
    }

    #[test]
    fn display_name_override() {
        let t = UsePythonVersion::new("3.11")
            .with_display_name("Install Python 3.11 for tests")
            .into_step();
        assert_eq!(t.display_name, "Install Python 3.11 for tests");
        assert_eq!(
            t.inputs.get("versionSpec").map(String::as_str),
            Some("3.11")
        );
    }

    #[test]
    fn all_optional_inputs() {
        let t = UsePythonVersion::new("3.12")
            .architecture(Architecture::X64)
            .add_to_path(true)
            .disable_download_from_registry(false)
            .allow_unstable(false)
            .github_token("ghp_token")
            .into_step();
        assert_eq!(t.task, "UsePythonVersion@0");
        assert_eq!(
            t.inputs.get("versionSpec").map(String::as_str),
            Some("3.12")
        );
        assert_eq!(
            t.inputs.get("architecture").map(String::as_str),
            Some("x64")
        );
        assert_eq!(t.inputs.get("addToPath").map(String::as_str), Some("true"));
        assert_eq!(
            t.inputs
                .get("disableDownloadFromRegistry")
                .map(String::as_str),
            Some("false")
        );
        assert_eq!(
            t.inputs.get("allowUnstable").map(String::as_str),
            Some("false")
        );
        assert_eq!(
            t.inputs.get("githubToken").map(String::as_str),
            Some("ghp_token")
        );
    }
}
