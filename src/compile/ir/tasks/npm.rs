//! Typed builder for `Npm@1`.
//!
//! Command-dispatch task modeled after [`super::docker`]: an [`Npm`] builder
//! wraps an [`NpmCommand`] enum whose variants carry each command's optional
//! inputs, so applying an input to the wrong command is unrepresentable.
//!
//! ADO task reference:
//! <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/npm-v1>

use super::common::{de_opt_bool_flex, push_bool, push_opt};
use crate::compile::ir::step::TaskStep;
use serde::Deserialize;
use serde_yaml::Value;

/// Validate an authored `Npm@1` `inputs:` mapping (advisory front-matter
/// validation, see [`super::parse`]).
pub(crate) fn validate_inputs(inputs: Value) -> Result<(), String> {
    let mut map = match inputs {
        Value::Mapping(m) => m,
        Value::Null => Default::default(),
        other => return Err(format!("`inputs` must be a mapping, got {other:?}")),
    };
    let command = map
        .remove("command")
        .and_then(|v| v.as_str().map(str::to_string))
        // ADO defaults `command` to `install` when omitted — treat a missing
        // command as the default variant rather than an error.
        .unwrap_or_else(|| "install".to_string());
    let rest = Value::Mapping(map);

    let result = match command.as_str() {
        "install" => serde_yaml::from_value::<NpmInstall>(rest).map(drop),
        "ci" => serde_yaml::from_value::<NpmCi>(rest).map(drop),
        "publish" => serde_yaml::from_value::<NpmPublish>(rest).map(drop),
        "custom" => serde_yaml::from_value::<NpmCustom>(rest).map(drop),
        other => return Err(format!("Npm@1: unknown command `{other}`")),
    };
    result.map_err(|e| format!("command `{command}`: {e}"))
}

/// `customRegistry` selector for `npm install` / `ci` / `custom`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum NpmCustomRegistry {
    #[serde(rename = "useNpmrc")]
    UseNpmrc,
    #[serde(rename = "useFeed")]
    UseFeed,
}

impl NpmCustomRegistry {
    /// The exact token the ADO task expects.
    pub fn as_ado_str(self) -> &'static str {
        match self {
            NpmCustomRegistry::UseNpmrc => "useNpmrc",
            NpmCustomRegistry::UseFeed => "useFeed",
        }
    }
}

/// `publishRegistry` selector for `npm publish`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum NpmPublishRegistry {
    #[serde(rename = "useExternalRegistry")]
    UseExternalRegistry,
    #[serde(rename = "useFeed")]
    UseFeed,
}

impl NpmPublishRegistry {
    /// The exact token the ADO task expects.
    pub fn as_ado_str(self) -> &'static str {
        match self {
            NpmPublishRegistry::UseExternalRegistry => "useExternalRegistry",
            NpmPublishRegistry::UseFeed => "useFeed",
        }
    }
}

/// `Npm@1` `command` selector, carrying per-command optional inputs.
#[derive(Debug, Clone)]
pub enum NpmCommand {
    Install(NpmInstall),
    Ci(NpmCi),
    Publish(NpmPublish),
    Custom(NpmCustom),
}

/// Optionals for `npm install`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NpmInstall {
    #[serde(rename = "workingDir", default)]
    working_dir: Option<String>,
    #[serde(rename = "verbose", default, deserialize_with = "de_opt_bool_flex")]
    verbose: Option<bool>,
    #[serde(rename = "customRegistry", default)]
    custom_registry: Option<NpmCustomRegistry>,
    #[serde(rename = "customFeed", default)]
    custom_feed: Option<String>,
    #[serde(rename = "customEndpoint", default)]
    custom_endpoint: Option<String>,
}

impl NpmInstall {
    pub fn new() -> Self {
        Self::default()
    }
    /// `workingDir` — folder containing `package.json`.
    pub fn working_dir(mut self, value: impl Into<String>) -> Self {
        self.working_dir = Some(value.into());
        self
    }
    /// `verbose` — enable verbose logging.
    pub fn verbose(mut self, value: bool) -> Self {
        self.verbose = Some(value);
        self
    }
    /// `customRegistry` — `useNpmrc` or `useFeed`.
    pub fn custom_registry(mut self, value: NpmCustomRegistry) -> Self {
        self.custom_registry = Some(value);
        self
    }
    /// `customFeed` — Azure Artifacts feed (when `customRegistry = useFeed`).
    pub fn custom_feed(mut self, value: impl Into<String>) -> Self {
        self.custom_feed = Some(value.into());
        self
    }
    /// `customEndpoint` — service connection for an external registry.
    pub fn custom_endpoint(mut self, value: impl Into<String>) -> Self {
        self.custom_endpoint = Some(value.into());
        self
    }
}

/// Optionals for `npm ci`.
///
/// `ci` accepts exactly the same inputs as `install`, so it reuses
/// [`NpmInstall`] rather than duplicating the field/setter surface.
pub type NpmCi = NpmInstall;

/// Optionals for `npm publish`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NpmPublish {
    #[serde(rename = "workingDir", default)]
    working_dir: Option<String>,
    #[serde(rename = "verbose", default, deserialize_with = "de_opt_bool_flex")]
    verbose: Option<bool>,
    #[serde(rename = "publishRegistry", default)]
    publish_registry: Option<NpmPublishRegistry>,
    #[serde(rename = "publishFeed", default)]
    publish_feed: Option<String>,
    #[serde(rename = "publishEndpoint", default)]
    publish_endpoint: Option<String>,
    #[serde(
        rename = "publishPackageMetadata",
        default,
        deserialize_with = "de_opt_bool_flex"
    )]
    publish_package_metadata: Option<bool>,
}

impl NpmPublish {
    pub fn new() -> Self {
        Self::default()
    }
    /// `workingDir` — folder containing `package.json`.
    pub fn working_dir(mut self, value: impl Into<String>) -> Self {
        self.working_dir = Some(value.into());
        self
    }
    /// `verbose` — enable verbose logging.
    pub fn verbose(mut self, value: bool) -> Self {
        self.verbose = Some(value);
        self
    }
    /// `publishRegistry` — `useExternalRegistry` or `useFeed`.
    pub fn publish_registry(mut self, value: NpmPublishRegistry) -> Self {
        self.publish_registry = Some(value);
        self
    }
    /// `publishFeed` — target Azure Artifacts feed (when `publishRegistry = useFeed`).
    pub fn publish_feed(mut self, value: impl Into<String>) -> Self {
        self.publish_feed = Some(value.into());
        self
    }
    /// `publishEndpoint` — external registry service connection
    /// (when `publishRegistry = useExternalRegistry`).
    pub fn publish_endpoint(mut self, value: impl Into<String>) -> Self {
        self.publish_endpoint = Some(value.into());
        self
    }
    /// `publishPackageMetadata` — attach pipeline metadata to packages.
    pub fn publish_package_metadata(mut self, value: bool) -> Self {
        self.publish_package_metadata = Some(value);
        self
    }
}

/// Inputs for `npm custom`. `customCommand` is required.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NpmCustom {
    #[serde(rename = "customCommand")]
    custom_command: String,
    #[serde(rename = "workingDir", default)]
    working_dir: Option<String>,
    #[serde(rename = "customRegistry", default)]
    custom_registry: Option<NpmCustomRegistry>,
    #[serde(rename = "customFeed", default)]
    custom_feed: Option<String>,
    #[serde(rename = "customEndpoint", default)]
    custom_endpoint: Option<String>,
}

impl NpmCustom {
    /// Required: the npm arguments to forward (e.g. `"run build -- --production"`).
    pub fn new(custom_command: impl Into<String>) -> Self {
        Self {
            custom_command: custom_command.into(),
            working_dir: None,
            custom_registry: None,
            custom_feed: None,
            custom_endpoint: None,
        }
    }
    /// `workingDir` — folder containing `package.json`.
    pub fn working_dir(mut self, value: impl Into<String>) -> Self {
        self.working_dir = Some(value.into());
        self
    }
    /// `customRegistry` — `useNpmrc` or `useFeed`.
    pub fn custom_registry(mut self, value: NpmCustomRegistry) -> Self {
        self.custom_registry = Some(value);
        self
    }
    /// `customFeed` — Azure Artifacts feed (when `customRegistry = useFeed`).
    pub fn custom_feed(mut self, value: impl Into<String>) -> Self {
        self.custom_feed = Some(value.into());
        self
    }
    /// `customEndpoint` — service connection for an external registry.
    pub fn custom_endpoint(mut self, value: impl Into<String>) -> Self {
        self.custom_endpoint = Some(value.into());
        self
    }
}

/// Builder for a [`TaskStep`] invoking `Npm@1`.
#[derive(Debug, Clone)]
pub struct Npm {
    command: NpmCommand,
    display_name: Option<String>,
}

impl Npm {
    /// Construct from an explicit [`NpmCommand`].
    pub fn new(command: NpmCommand) -> Self {
        Self {
            command,
            display_name: None,
        }
    }

    /// `command: install`.
    pub fn install(spec: NpmInstall) -> Self {
        Self::new(NpmCommand::Install(spec))
    }
    /// `command: ci`.
    pub fn ci(spec: NpmCi) -> Self {
        Self::new(NpmCommand::Ci(spec))
    }
    /// `command: publish`.
    pub fn publish(spec: NpmPublish) -> Self {
        Self::new(NpmCommand::Publish(spec))
    }
    /// `command: custom`.
    pub fn custom(spec: NpmCustom) -> Self {
        Self::new(NpmCommand::Custom(spec))
    }

    /// Override the default `displayName` (`"npm <command>"`).
    pub fn with_display_name(mut self, value: impl Into<String>) -> Self {
        self.display_name = Some(value.into());
        self
    }

    /// Lower into a [`TaskStep`].
    pub fn into_step(self) -> TaskStep {
        let command = match &self.command {
            NpmCommand::Install(_) => "install",
            NpmCommand::Ci(_) => "ci",
            NpmCommand::Publish(_) => "publish",
            NpmCommand::Custom(_) => "custom",
        };
        let mut t = TaskStep::new(
            "Npm@1",
            self.display_name
                .unwrap_or_else(|| format!("npm {command}")),
        )
        .with_input("command", command);
        match self.command {
            NpmCommand::Install(s) | NpmCommand::Ci(s) => {
                push_opt(&mut t, "workingDir", s.working_dir);
                push_bool(&mut t, "verbose", s.verbose);
                push_opt(
                    &mut t,
                    "customRegistry",
                    s.custom_registry.map(|v| v.as_ado_str().to_string()),
                );
                push_opt(&mut t, "customFeed", s.custom_feed);
                push_opt(&mut t, "customEndpoint", s.custom_endpoint);
            }
            NpmCommand::Publish(s) => {
                push_opt(&mut t, "workingDir", s.working_dir);
                push_bool(&mut t, "verbose", s.verbose);
                push_opt(
                    &mut t,
                    "publishRegistry",
                    s.publish_registry.map(|v| v.as_ado_str().to_string()),
                );
                push_opt(&mut t, "publishFeed", s.publish_feed);
                push_opt(&mut t, "publishEndpoint", s.publish_endpoint);
                push_bool(&mut t, "publishPackageMetadata", s.publish_package_metadata);
            }
            NpmCommand::Custom(s) => {
                t = t.with_input("customCommand", s.custom_command);
                push_opt(&mut t, "workingDir", s.working_dir);
                push_opt(
                    &mut t,
                    "customRegistry",
                    s.custom_registry.map(|v| v.as_ado_str().to_string()),
                );
                push_opt(&mut t, "customFeed", s.custom_feed);
                push_opt(&mut t, "customEndpoint", s.custom_endpoint);
            }
        }
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_sets_task_and_command() {
        let t = Npm::install(NpmInstall::new()).into_step();
        assert_eq!(t.task, "Npm@1");
        assert_eq!(t.display_name, "npm install");
        assert_eq!(t.inputs.get("command").map(String::as_str), Some("install"));
        // only the required input is set by default
        assert_eq!(t.inputs.len(), 1);
    }

    #[test]
    fn ci_command() {
        let t = Npm::ci(NpmCi::new()).into_step();
        assert_eq!(t.display_name, "npm ci");
        assert_eq!(t.inputs.get("command").map(String::as_str), Some("ci"));
        assert_eq!(t.inputs.len(), 1);
    }

    #[test]
    fn publish_command_with_feed() {
        let t = Npm::publish(
            NpmPublish::new()
                .publish_registry(NpmPublishRegistry::UseFeed)
                .publish_feed("myorg/myfeed")
                .publish_package_metadata(true),
        )
        .into_step();
        assert_eq!(t.inputs.get("command").map(String::as_str), Some("publish"));
        assert_eq!(
            t.inputs.get("publishRegistry").map(String::as_str),
            Some("useFeed")
        );
        assert_eq!(
            t.inputs.get("publishFeed").map(String::as_str),
            Some("myorg/myfeed")
        );
        assert_eq!(
            t.inputs.get("publishPackageMetadata").map(String::as_str),
            Some("true")
        );
    }

    #[test]
    fn custom_with_working_dir_and_command() {
        let t = Npm::custom(
            NpmCustom::new("run build -- --production")
                .working_dir("$(Build.SourcesDirectory)/frontend"),
        )
        .into_step();
        assert_eq!(t.inputs.get("command").map(String::as_str), Some("custom"));
        assert_eq!(
            t.inputs.get("customCommand").map(String::as_str),
            Some("run build -- --production")
        );
        assert_eq!(
            t.inputs.get("workingDir").map(String::as_str),
            Some("$(Build.SourcesDirectory)/frontend")
        );
    }

    #[test]
    fn install_use_feed_registry() {
        let t = Npm::install(
            NpmInstall::new()
                .custom_registry(NpmCustomRegistry::UseFeed)
                .custom_feed("myorg/myproject/myfeed")
                .verbose(true),
        )
        .into_step();
        assert_eq!(
            t.inputs.get("customRegistry").map(String::as_str),
            Some("useFeed")
        );
        assert_eq!(
            t.inputs.get("customFeed").map(String::as_str),
            Some("myorg/myproject/myfeed")
        );
        assert_eq!(t.inputs.get("verbose").map(String::as_str), Some("true"));
    }
}
