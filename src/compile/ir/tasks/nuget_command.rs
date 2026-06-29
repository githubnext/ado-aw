//! Typed builder for `NuGetCommand@2`.
//!
//! Command-dispatch task modeled after [`super::docker`]: a [`NuGetCommand`]
//! builder wraps a [`NuGetOp`] enum whose variants carry each command's optional
//! inputs, so applying an input to the wrong command is unrepresentable.
//!
//! ADO task reference:
//! <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/nuget-command-v2>

use super::common::{push_bool, push_opt};
use crate::compile::ir::step::TaskStep;

/// NuGet task verbosity (`verbosityRestore` / `verbosityPush` / `verbosityPack`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verbosity {
    Quiet,
    Normal,
    Detailed,
}

impl Verbosity {
    /// The exact token the ADO task expects.
    pub fn as_ado_str(self) -> &'static str {
        match self {
            Verbosity::Quiet => "Quiet",
            Verbosity::Normal => "Normal",
            Verbosity::Detailed => "Detailed",
        }
    }
}

/// `feedsToUse` selector for `nuget restore`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeedsToUse {
    Select,
    Config,
}

impl FeedsToUse {
    /// The exact token the ADO task expects.
    pub fn as_ado_str(self) -> &'static str {
        match self {
            FeedsToUse::Select => "select",
            FeedsToUse::Config => "config",
        }
    }
}

/// `nuGetFeedType` selector for `nuget push`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NuGetFeedType {
    Internal,
    External,
}

impl NuGetFeedType {
    /// The exact token the ADO task expects.
    pub fn as_ado_str(self) -> &'static str {
        match self {
            NuGetFeedType::Internal => "internal",
            NuGetFeedType::External => "external",
        }
    }
}

/// `versioningScheme` selector for `nuget pack`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersioningScheme {
    Off,
    ByPrereleaseNumber,
    ByEnvVar,
    ByBuildNumber,
}

impl VersioningScheme {
    /// The exact token the ADO task expects.
    pub fn as_ado_str(self) -> &'static str {
        match self {
            VersioningScheme::Off => "off",
            VersioningScheme::ByPrereleaseNumber => "byPrereleaseNumber",
            VersioningScheme::ByEnvVar => "byEnvVar",
            VersioningScheme::ByBuildNumber => "byBuildNumber",
        }
    }
}

/// `NuGetCommand@2` `command` selector, carrying per-command optional inputs.
#[derive(Debug, Clone)]
pub enum NuGetOp {
    Restore(NuGetRestore),
    Push(NuGetPush),
    Pack(NuGetPack),
    Custom(NuGetCustom),
}

/// Optionals for `nuget restore`.
#[derive(Debug, Clone, Default)]
pub struct NuGetRestore {
    solution: Option<String>,
    feeds_to_use: Option<FeedsToUse>,
    vsts_feed: Option<String>,
    include_nuget_org: Option<bool>,
    nuget_config_path: Option<String>,
    external_feed_credentials: Option<String>,
    no_cache: Option<bool>,
    disable_parallel_processing: Option<bool>,
    restore_directory: Option<String>,
    verbosity_restore: Option<Verbosity>,
}

impl NuGetRestore {
    pub fn new() -> Self {
        Self::default()
    }
    /// `solution` — path to solution, `packages.config`, or `project.json`.
    pub fn solution(mut self, value: impl Into<String>) -> Self {
        self.solution = Some(value.into());
        self
    }
    /// `feedsToUse` — dropdown vs NuGet.config.
    pub fn feeds_to_use(mut self, value: FeedsToUse) -> Self {
        self.feeds_to_use = Some(value);
        self
    }
    /// `vstsFeed` — Azure Artifacts feed (when `feedsToUse = select`).
    pub fn vsts_feed(mut self, value: impl Into<String>) -> Self {
        self.vsts_feed = Some(value.into());
        self
    }
    /// `includeNuGetOrg` — include NuGet.org as a package source.
    pub fn include_nuget_org(mut self, value: bool) -> Self {
        self.include_nuget_org = Some(value);
        self
    }
    /// `nugetConfigPath` — path to `NuGet.config` (when `feedsToUse = config`).
    pub fn nuget_config_path(mut self, value: impl Into<String>) -> Self {
        self.nuget_config_path = Some(value.into());
        self
    }
    /// `externalFeedCredentials` — credentials for external feeds.
    pub fn external_feed_credentials(mut self, value: impl Into<String>) -> Self {
        self.external_feed_credentials = Some(value.into());
        self
    }
    /// `noCache` — disable the local NuGet cache.
    pub fn no_cache(mut self, value: bool) -> Self {
        self.no_cache = Some(value);
        self
    }
    /// `disableParallelProcessing` — disable parallel package restore.
    pub fn disable_parallel_processing(mut self, value: bool) -> Self {
        self.disable_parallel_processing = Some(value);
        self
    }
    /// `restoreDirectory` — destination directory for restored packages.
    pub fn restore_directory(mut self, value: impl Into<String>) -> Self {
        self.restore_directory = Some(value.into());
        self
    }
    /// `verbosityRestore` — restore verbosity.
    pub fn verbosity_restore(mut self, value: Verbosity) -> Self {
        self.verbosity_restore = Some(value);
        self
    }
}

/// Optionals for `nuget push`.
#[derive(Debug, Clone, Default)]
pub struct NuGetPush {
    packages_to_push: Option<String>,
    nuget_feed_type: Option<NuGetFeedType>,
    publish_vsts_feed: Option<String>,
    allow_package_conflicts: Option<bool>,
    publish_feed_credentials: Option<String>,
    publish_package_metadata: Option<bool>,
    verbosity_push: Option<Verbosity>,
}

impl NuGetPush {
    pub fn new() -> Self {
        Self::default()
    }
    /// `packagesToPush` — glob for `.nupkg` files to publish.
    pub fn packages_to_push(mut self, value: impl Into<String>) -> Self {
        self.packages_to_push = Some(value.into());
        self
    }
    /// `nuGetFeedType` — internal (Azure Artifacts) or external.
    pub fn nuget_feed_type(mut self, value: NuGetFeedType) -> Self {
        self.nuget_feed_type = Some(value);
        self
    }
    /// `publishVstsFeed` — target Azure Artifacts feed (when internal).
    pub fn publish_vsts_feed(mut self, value: impl Into<String>) -> Self {
        self.publish_vsts_feed = Some(value.into());
        self
    }
    /// `allowPackageConflicts` — skip duplicate packages instead of failing.
    pub fn allow_package_conflicts(mut self, value: bool) -> Self {
        self.allow_package_conflicts = Some(value);
        self
    }
    /// `publishFeedCredentials` — external NuGet server endpoint (when external).
    pub fn publish_feed_credentials(mut self, value: impl Into<String>) -> Self {
        self.publish_feed_credentials = Some(value.into());
        self
    }
    /// `publishPackageMetadata` — publish pipeline metadata with the package.
    pub fn publish_package_metadata(mut self, value: bool) -> Self {
        self.publish_package_metadata = Some(value);
        self
    }
    /// `verbosityPush` — push verbosity.
    pub fn verbosity_push(mut self, value: Verbosity) -> Self {
        self.verbosity_push = Some(value);
        self
    }
}

/// Optionals for `nuget pack`.
#[derive(Debug, Clone, Default)]
pub struct NuGetPack {
    packages_to_pack: Option<String>,
    configuration: Option<String>,
    versioning_scheme: Option<VersioningScheme>,
    verbosity_pack: Option<Verbosity>,
}

impl NuGetPack {
    pub fn new() -> Self {
        Self::default()
    }
    /// `packagesToPack` — glob for `.csproj`/`.nuspec` files to pack.
    pub fn packages_to_pack(mut self, value: impl Into<String>) -> Self {
        self.packages_to_pack = Some(value.into());
        self
    }
    /// `configuration` — build configuration (e.g. `"Release"`).
    pub fn configuration(mut self, value: impl Into<String>) -> Self {
        self.configuration = Some(value.into());
        self
    }
    /// `versioningScheme` — version strategy.
    pub fn versioning_scheme(mut self, value: VersioningScheme) -> Self {
        self.versioning_scheme = Some(value);
        self
    }
    /// `verbosityPack` — pack verbosity.
    pub fn verbosity_pack(mut self, value: Verbosity) -> Self {
        self.verbosity_pack = Some(value);
        self
    }
}

/// Inputs for `nuget custom`. `arguments` is required.
#[derive(Debug, Clone)]
pub struct NuGetCustom {
    arguments: String,
}

impl NuGetCustom {
    /// Required: the full NuGet command-line arguments.
    pub fn new(arguments: impl Into<String>) -> Self {
        Self {
            arguments: arguments.into(),
        }
    }
}

/// Builder for a [`TaskStep`] invoking `NuGetCommand@2`.
#[derive(Debug, Clone)]
pub struct NuGetCommand {
    command: NuGetOp,
    display_name: Option<String>,
}

impl NuGetCommand {
    /// Construct from an explicit [`NuGetOp`].
    pub fn new(command: NuGetOp) -> Self {
        Self {
            command,
            display_name: None,
        }
    }

    /// `command: restore`.
    pub fn restore(spec: NuGetRestore) -> Self {
        Self::new(NuGetOp::Restore(spec))
    }
    /// `command: push`.
    pub fn push(spec: NuGetPush) -> Self {
        Self::new(NuGetOp::Push(spec))
    }
    /// `command: pack`.
    pub fn pack(spec: NuGetPack) -> Self {
        Self::new(NuGetOp::Pack(spec))
    }
    /// `command: custom`.
    pub fn custom(spec: NuGetCustom) -> Self {
        Self::new(NuGetOp::Custom(spec))
    }

    /// Override the default `displayName` (`"NuGet <command>"`).
    pub fn with_display_name(mut self, value: impl Into<String>) -> Self {
        self.display_name = Some(value.into());
        self
    }

    /// Lower into a [`TaskStep`].
    pub fn into_step(self) -> TaskStep {
        let command = match &self.command {
            NuGetOp::Restore(_) => "restore",
            NuGetOp::Push(_) => "push",
            NuGetOp::Pack(_) => "pack",
            NuGetOp::Custom(_) => "custom",
        };
        let mut t = TaskStep::new(
            "NuGetCommand@2",
            self.display_name.unwrap_or_else(|| format!("NuGet {command}")),
        )
        .with_input("command", command);
        match self.command {
            NuGetOp::Restore(s) => {
                push_opt(&mut t, "solution", s.solution);
                push_opt(&mut t, "feedsToUse", s.feeds_to_use.map(|v| v.as_ado_str().to_string()));
                push_opt(&mut t, "vstsFeed", s.vsts_feed);
                push_bool(&mut t, "includeNuGetOrg", s.include_nuget_org);
                push_opt(&mut t, "nugetConfigPath", s.nuget_config_path);
                push_opt(&mut t, "externalFeedCredentials", s.external_feed_credentials);
                push_bool(&mut t, "noCache", s.no_cache);
                push_bool(&mut t, "disableParallelProcessing", s.disable_parallel_processing);
                push_opt(&mut t, "restoreDirectory", s.restore_directory);
                push_opt(
                    &mut t,
                    "verbosityRestore",
                    s.verbosity_restore.map(|v| v.as_ado_str().to_string()),
                );
            }
            NuGetOp::Push(s) => {
                push_opt(&mut t, "packagesToPush", s.packages_to_push);
                push_opt(
                    &mut t,
                    "nuGetFeedType",
                    s.nuget_feed_type.map(|v| v.as_ado_str().to_string()),
                );
                push_opt(&mut t, "publishVstsFeed", s.publish_vsts_feed);
                push_bool(&mut t, "allowPackageConflicts", s.allow_package_conflicts);
                push_opt(&mut t, "publishFeedCredentials", s.publish_feed_credentials);
                push_bool(&mut t, "publishPackageMetadata", s.publish_package_metadata);
                push_opt(
                    &mut t,
                    "verbosityPush",
                    s.verbosity_push.map(|v| v.as_ado_str().to_string()),
                );
            }
            NuGetOp::Pack(s) => {
                push_opt(&mut t, "packagesToPack", s.packages_to_pack);
                push_opt(&mut t, "configuration", s.configuration);
                push_opt(
                    &mut t,
                    "versioningScheme",
                    s.versioning_scheme.map(|v| v.as_ado_str().to_string()),
                );
                push_opt(
                    &mut t,
                    "verbosityPack",
                    s.verbosity_pack.map(|v| v.as_ado_str().to_string()),
                );
            }
            NuGetOp::Custom(s) => {
                t = t.with_input("arguments", s.arguments);
            }
        }
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restore_default_display_and_command() {
        let t = NuGetCommand::restore(NuGetRestore::new()).into_step();
        assert_eq!(t.task, "NuGetCommand@2");
        assert_eq!(t.display_name, "NuGet restore");
        assert_eq!(t.inputs.get("command").map(String::as_str), Some("restore"));
    }

    #[test]
    fn restore_typed_inputs() {
        let t = NuGetCommand::restore(
            NuGetRestore::new()
                .solution("src/MyApp.sln")
                .feeds_to_use(FeedsToUse::Select)
                .vsts_feed("myorg/myproject/myfeed")
                .include_nuget_org(false)
                .verbosity_restore(Verbosity::Detailed),
        )
        .into_step();
        assert_eq!(t.inputs.get("solution").map(String::as_str), Some("src/MyApp.sln"));
        assert_eq!(t.inputs.get("feedsToUse").map(String::as_str), Some("select"));
        assert_eq!(t.inputs.get("includeNuGetOrg").map(String::as_str), Some("false"));
        assert_eq!(t.inputs.get("verbosityRestore").map(String::as_str), Some("Detailed"));
    }

    #[test]
    fn push_internal_feed() {
        let t = NuGetCommand::push(
            NuGetPush::new()
                .nuget_feed_type(NuGetFeedType::Internal)
                .publish_vsts_feed("myorg/myfeed")
                .allow_package_conflicts(true),
        )
        .into_step();
        assert_eq!(t.inputs.get("command").map(String::as_str), Some("push"));
        assert_eq!(t.inputs.get("nuGetFeedType").map(String::as_str), Some("internal"));
        assert_eq!(t.inputs.get("allowPackageConflicts").map(String::as_str), Some("true"));
    }

    #[test]
    fn custom_requires_arguments() {
        let t = NuGetCommand::custom(NuGetCustom::new("install Foo -Version 1.0")).into_step();
        assert_eq!(t.inputs.get("command").map(String::as_str), Some("custom"));
        assert_eq!(
            t.inputs.get("arguments").map(String::as_str),
            Some("install Foo -Version 1.0")
        );
    }
}
