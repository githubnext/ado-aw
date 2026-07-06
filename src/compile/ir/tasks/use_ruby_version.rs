//! Typed builder for `UseRubyVersion@0`.
//!
//! ADO task reference:
//! <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/use-ruby-version-v0>

use super::common::{bool_input, de_opt_bool_flex};
use crate::compile::ir::step::TaskStep;
use serde::Deserialize;

/// Builder for a [`TaskStep`] invoking `UseRubyVersion@0`.
///
/// Selects the specified version of Ruby from the tool cache and optionally
/// adds it to the PATH. Equivalent to the `UseRubyVersion@0` Azure DevOps
/// task.
///
/// ADO task reference:
/// <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/use-ruby-version-v0>
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UseRubyVersion {
    #[serde(rename = "versionSpec")]
    version_spec: String,
    #[serde(rename = "addToPath", default, deserialize_with = "de_opt_bool_flex")]
    add_to_path: Option<bool>,
    #[serde(skip)]
    display_name: Option<String>,
}

impl UseRubyVersion {
    /// Required input: `versionSpec` — the version range or exact version of
    /// Ruby to use (e.g. `">= 2.4"`, `"3.x"`, `"3.2.0"`).
    /// ADO default: `">= 2.4"`.
    pub fn new(version_spec: impl Into<String>) -> Self {
        Self {
            version_spec: version_spec.into(),
            add_to_path: None,
            display_name: None,
        }
    }

    /// `addToPath` — whether to prepend the retrieved Ruby version to the
    /// PATH environment variable to make it available in subsequent tasks or
    /// scripts. ADO default: `true`.
    pub fn add_to_path(mut self, value: bool) -> Self {
        self.add_to_path = Some(value);
        self
    }

    /// Override the default `displayName`
    /// (`"Use Ruby <version_spec>"`).
    pub fn with_display_name(mut self, value: impl Into<String>) -> Self {
        self.display_name = Some(value.into());
        self
    }

    /// Lower into a [`TaskStep`].
    pub fn into_step(self) -> TaskStep {
        let mut t = TaskStep::new(
            "UseRubyVersion@0",
            self.display_name
                .unwrap_or_else(|| format!("Use Ruby {}", self.version_spec)),
        )
        .with_input("versionSpec", self.version_spec);
        if let Some(v) = self.add_to_path {
            t = t.with_input("addToPath", bool_input(v));
        }
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sets_task_and_version_spec() {
        let t = UseRubyVersion::new(">= 2.4").into_step();
        assert_eq!(t.task, "UseRubyVersion@0");
        assert_eq!(t.display_name, "Use Ruby >= 2.4");
        assert_eq!(
            t.inputs.get("versionSpec").map(String::as_str),
            Some(">= 2.4")
        );
    }

    #[test]
    fn optional_inputs_not_emitted_by_default() {
        let t = UseRubyVersion::new("3.x").into_step();
        assert!(t.inputs.get("addToPath").is_none());
    }

    #[test]
    fn add_to_path_false() {
        let t = UseRubyVersion::new("3.2").add_to_path(false).into_step();
        assert_eq!(t.inputs.get("addToPath").map(String::as_str), Some("false"));
    }

    #[test]
    fn add_to_path_true_is_emitted_explicitly() {
        let t = UseRubyVersion::new("3.x").add_to_path(true).into_step();
        assert_eq!(t.inputs.get("addToPath").map(String::as_str), Some("true"));
    }

    #[test]
    fn display_name_override() {
        let t = UseRubyVersion::new("3.2.0")
            .with_display_name("Install Ruby 3.2.0 for tests")
            .into_step();
        assert_eq!(t.display_name, "Install Ruby 3.2.0 for tests");
        assert_eq!(
            t.inputs.get("versionSpec").map(String::as_str),
            Some("3.2.0")
        );
    }

    #[test]
    fn different_version_specs() {
        for spec in &[">= 2.4", "3.x", "3.2.0", "~> 3.1"] {
            let t = UseRubyVersion::new(*spec).into_step();
            assert_eq!(t.task, "UseRubyVersion@0");
            assert_eq!(t.inputs.get("versionSpec").map(String::as_str), Some(*spec));
            assert_eq!(t.display_name, format!("Use Ruby {spec}"));
        }
    }

    #[test]
    fn all_inputs() {
        let t = UseRubyVersion::new("3.2")
            .add_to_path(true)
            .with_display_name("Set up Ruby 3.2")
            .into_step();
        assert_eq!(t.task, "UseRubyVersion@0");
        assert_eq!(t.display_name, "Set up Ruby 3.2");
        assert_eq!(t.inputs.get("versionSpec").map(String::as_str), Some("3.2"));
        assert_eq!(t.inputs.get("addToPath").map(String::as_str), Some("true"));
    }
}
