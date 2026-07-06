//! Typed builder for `HelmInstaller@1`.
//!
//! ADO task reference:
//! <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/helm-installer-v1>

use crate::compile::ir::step::TaskStep;
use serde::Deserialize;

/// Builder for a [`TaskStep`] invoking `HelmInstaller@1`.
///
/// Installs a specific version of Helm on the agent machine and adds it to
/// the PATH. When no version is specified, the ADO task installs the latest
/// available release. Equivalent to the `HelmInstaller@1` Azure DevOps task.
///
/// ADO task reference:
/// <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/helm-installer-v1>
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HelmInstaller {
    #[serde(rename = "helmVersionToInstall", default)]
    helm_version: Option<String>,
    #[serde(skip)]
    display_name: Option<String>,
}

impl HelmInstaller {
    /// Creates a new [`HelmInstaller`] that installs the latest Helm release
    /// (ADO default). Call [`helm_version`][Self::helm_version] to pin a
    /// specific version.
    pub fn new() -> Self {
        Self {
            helm_version: None,
            display_name: None,
        }
    }

    /// `helmVersionToInstall` — the Helm version to install. Accepts any
    /// semantic version string (e.g. `"3.14.0"`) or `"latest"` for the most
    /// recent release. When not set, the ADO task defaults to `"latest"`.
    pub fn helm_version(mut self, value: impl Into<String>) -> Self {
        self.helm_version = Some(value.into());
        self
    }

    /// Override the default `displayName`
    /// (`"Install Helm"` or `"Install Helm <version>"`).
    pub fn with_display_name(mut self, value: impl Into<String>) -> Self {
        self.display_name = Some(value.into());
        self
    }

    /// Lower into a [`TaskStep`].
    pub fn into_step(self) -> TaskStep {
        let display = self.display_name.unwrap_or_else(|| {
            match &self.helm_version {
                Some(v) => format!("Install Helm {v}"),
                None => "Install Helm".into(),
            }
        });
        let mut t = TaskStep::new("HelmInstaller@1", display);
        if let Some(v) = self.helm_version {
            t = t.with_input("helmVersionToInstall", v);
        }
        t
    }
}

impl Default for HelmInstaller {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_installs_latest_with_no_inputs() {
        let t = HelmInstaller::new().into_step();
        assert_eq!(t.task, "HelmInstaller@1");
        assert_eq!(t.display_name, "Install Helm");
        assert!(
            t.inputs.get("helmVersionToInstall").is_none(),
            "no input emitted when using ADO default"
        );
    }

    #[test]
    fn pinned_version_sets_input_and_display_name() {
        let t = HelmInstaller::new().helm_version("3.14.0").into_step();
        assert_eq!(t.task, "HelmInstaller@1");
        assert_eq!(t.display_name, "Install Helm 3.14.0");
        assert_eq!(
            t.inputs.get("helmVersionToInstall").map(String::as_str),
            Some("3.14.0")
        );
    }

    #[test]
    fn latest_string_is_emitted_when_explicit() {
        let t = HelmInstaller::new().helm_version("latest").into_step();
        assert_eq!(t.display_name, "Install Helm latest");
        assert_eq!(
            t.inputs.get("helmVersionToInstall").map(String::as_str),
            Some("latest")
        );
    }

    #[test]
    fn display_name_override_respected() {
        let t = HelmInstaller::new()
            .helm_version("3.12.0")
            .with_display_name("Set up Helm 3.12.0 for deployment")
            .into_step();
        assert_eq!(t.display_name, "Set up Helm 3.12.0 for deployment");
        assert_eq!(
            t.inputs.get("helmVersionToInstall").map(String::as_str),
            Some("3.12.0")
        );
    }

    #[test]
    fn default_trait_matches_new() {
        let t = HelmInstaller::default().into_step();
        assert_eq!(t.task, "HelmInstaller@1");
        assert!(t.inputs.get("helmVersionToInstall").is_none());
    }
}
