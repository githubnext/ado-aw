//! Typed builder for `UniversalPackages@1`.
//!
//! `UniversalPackages@1` is a command-dispatch task with two commands:
//! - `download` — download a package from an ADO Artifacts feed
//! - `publish` — publish a package to an ADO Artifacts feed
//!
//! Each command has its own per-command optional inputs represented as variant
//! data on [`UniversalPackagesCommand`]. Required inputs (`feed`, `packageName`)
//! and cross-command optionals (`workloadIdentityServiceConnection`, `organization`)
//! live on the top-level [`UniversalPackages`] builder.
//!
//! ADO task reference:
//! <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/universal-packages-v1>

use super::common::{push_bool, push_opt};
use crate::compile::ir::step::TaskStep;

/// `versionIncrement` value for the `publish` command.
///
/// Automatically increments the specified component of the package version.
/// Cannot be used together with an explicit `packageVersion`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionIncrement {
    Major,
    Minor,
    Patch,
}

impl VersionIncrement {
    /// Returns the exact token ADO expects for this value.
    pub fn as_ado_str(self) -> &'static str {
        match self {
            Self::Major => "major",
            Self::Minor => "minor",
            Self::Patch => "patch",
        }
    }
}

/// Per-command optionals for `UniversalPackages@1` `command: download`.
#[derive(Debug, Clone, Default)]
pub struct UniversalPackagesDownload {
    package_version: Option<String>,
    directory: Option<String>,
    extract: Option<bool>,
}

impl UniversalPackagesDownload {
    pub fn new() -> Self {
        Self::default()
    }

    /// `packageVersion` — version of the package to download.
    pub fn package_version(mut self, value: impl Into<String>) -> Self {
        self.package_version = Some(value.into());
        self
    }

    /// `directory` — directory to download the package into.
    /// Defaults to `$(System.DefaultWorkingDirectory)` when omitted.
    pub fn directory(mut self, value: impl Into<String>) -> Self {
        self.directory = Some(value.into());
        self
    }

    /// `extract` — whether to extract the package after download.
    pub fn extract(mut self, value: bool) -> Self {
        self.extract = Some(value);
        self
    }
}

/// Per-command optionals for `UniversalPackages@1` `command: publish`.
#[derive(Debug, Clone, Default)]
pub struct UniversalPackagesPublish {
    package_version: Option<String>,
    version_increment: Option<VersionIncrement>,
    directory: Option<String>,
    package_description: Option<String>,
}

impl UniversalPackagesPublish {
    pub fn new() -> Self {
        Self::default()
    }

    /// `packageVersion` — explicit version to publish.
    /// Mutually exclusive with [`version_increment`](Self::version_increment).
    pub fn package_version(mut self, value: impl Into<String>) -> Self {
        self.package_version = Some(value.into());
        self
    }

    /// `versionIncrement` — auto-increment the specified version component.
    /// Mutually exclusive with [`package_version`](Self::package_version).
    pub fn version_increment(mut self, value: VersionIncrement) -> Self {
        self.version_increment = Some(value);
        self
    }

    /// `directory` — directory whose contents are published as the package.
    /// Defaults to `$(System.DefaultWorkingDirectory)` when omitted.
    pub fn directory(mut self, value: impl Into<String>) -> Self {
        self.directory = Some(value.into());
        self
    }

    /// `packageDescription` — description for the package.
    pub fn package_description(mut self, value: impl Into<String>) -> Self {
        self.package_description = Some(value.into());
        self
    }
}

/// `UniversalPackages@1` command selector carrying per-command optional inputs.
#[derive(Debug, Clone)]
pub enum UniversalPackagesCommand {
    Download(UniversalPackagesDownload),
    Publish(UniversalPackagesPublish),
}

/// Builder for a [`TaskStep`] invoking `UniversalPackages@1`.
///
/// Downloads or publishes a Universal Package to/from an Azure DevOps
/// Artifacts feed.
///
/// # Example
///
/// ```rust
/// use crate::compile::ir::tasks::universal_packages::{
///     UniversalPackages, UniversalPackagesDownload,
/// };
///
/// let step = UniversalPackages::download(
///     "my-feed",
///     "my-package",
///     UniversalPackagesDownload::new().package_version("1.2.3"),
/// )
/// .into_step();
/// assert_eq!(step.task, "UniversalPackages@1");
/// ```
#[derive(Debug, Clone)]
pub struct UniversalPackages {
    feed: String,
    package_name: String,
    command: UniversalPackagesCommand,
    workload_identity_service_connection: Option<String>,
    organization: Option<String>,
    display_name: Option<String>,
}

impl UniversalPackages {
    /// Construct from an explicit [`UniversalPackagesCommand`].
    pub fn new(
        feed: impl Into<String>,
        package_name: impl Into<String>,
        command: UniversalPackagesCommand,
    ) -> Self {
        Self {
            feed: feed.into(),
            package_name: package_name.into(),
            command,
            workload_identity_service_connection: None,
            organization: None,
            display_name: None,
        }
    }

    /// Convenience constructor: `command: download`.
    pub fn download(
        feed: impl Into<String>,
        package_name: impl Into<String>,
        spec: UniversalPackagesDownload,
    ) -> Self {
        Self::new(feed, package_name, UniversalPackagesCommand::Download(spec))
    }

    /// Convenience constructor: `command: publish`.
    pub fn publish(
        feed: impl Into<String>,
        package_name: impl Into<String>,
        spec: UniversalPackagesPublish,
    ) -> Self {
        Self::new(feed, package_name, UniversalPackagesCommand::Publish(spec))
    }

    /// `workloadIdentityServiceConnection` — Azure DevOps service connection
    /// used for cross-organization or workload-identity authentication.
    pub fn workload_identity_service_connection(mut self, value: impl Into<String>) -> Self {
        self.workload_identity_service_connection = Some(value.into());
        self
    }

    /// `organization` — the ADO organization hosting the target feed.
    /// Only relevant when a `workloadIdentityServiceConnection` is set.
    pub fn organization(mut self, value: impl Into<String>) -> Self {
        self.organization = Some(value.into());
        self
    }

    /// Override the default `displayName`.
    pub fn with_display_name(mut self, value: impl Into<String>) -> Self {
        self.display_name = Some(value.into());
        self
    }

    /// Lower into a [`TaskStep`].
    pub fn into_step(self) -> TaskStep {
        let (command_str, default_display): (&str, &str) = match &self.command {
            UniversalPackagesCommand::Download(_) => ("download", "Download Universal Package"),
            UniversalPackagesCommand::Publish(_) => ("publish", "Publish Universal Package"),
        };
        let mut t = TaskStep::new(
            "UniversalPackages@1",
            self.display_name
                .unwrap_or_else(|| default_display.into()),
        )
        .with_input("command", command_str)
        .with_input("feed", self.feed)
        .with_input("packageName", self.package_name);

        push_opt(
            &mut t,
            "workloadIdentityServiceConnection",
            self.workload_identity_service_connection,
        );
        push_opt(&mut t, "organization", self.organization);

        match self.command {
            UniversalPackagesCommand::Download(s) => {
                push_opt(&mut t, "packageVersion", s.package_version);
                push_opt(&mut t, "directory", s.directory);
                push_bool(&mut t, "extract", s.extract);
            }
            UniversalPackagesCommand::Publish(s) => {
                push_opt(&mut t, "packageVersion", s.package_version);
                if let Some(v) = s.version_increment {
                    t = t.with_input("versionIncrement", v.as_ado_str());
                }
                push_opt(&mut t, "directory", s.directory);
                push_opt(&mut t, "packageDescription", s.package_description);
            }
        }
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn download_minimal() {
        let t = UniversalPackages::download(
            "my-feed",
            "my-package",
            UniversalPackagesDownload::new(),
        )
        .into_step();
        assert_eq!(t.task, "UniversalPackages@1");
        assert_eq!(t.display_name, "Download Universal Package");
        assert_eq!(t.inputs.get("command").map(String::as_str), Some("download"));
        assert_eq!(t.inputs.get("feed").map(String::as_str), Some("my-feed"));
        assert_eq!(
            t.inputs.get("packageName").map(String::as_str),
            Some("my-package")
        );
        assert!(t.inputs.get("packageVersion").is_none());
        assert!(t.inputs.get("directory").is_none());
    }

    #[test]
    fn download_with_version_and_directory() {
        let t = UniversalPackages::download(
            "my-feed",
            "my-package",
            UniversalPackagesDownload::new()
                .package_version("1.2.3")
                .directory("$(Build.ArtifactStagingDirectory)"),
        )
        .into_step();
        assert_eq!(
            t.inputs.get("packageVersion").map(String::as_str),
            Some("1.2.3")
        );
        assert_eq!(
            t.inputs.get("directory").map(String::as_str),
            Some("$(Build.ArtifactStagingDirectory)")
        );
    }

    #[test]
    fn publish_minimal() {
        let t = UniversalPackages::publish(
            "my-feed",
            "my-package",
            UniversalPackagesPublish::new().package_version("2.0.0"),
        )
        .into_step();
        assert_eq!(t.task, "UniversalPackages@1");
        assert_eq!(t.display_name, "Publish Universal Package");
        assert_eq!(t.inputs.get("command").map(String::as_str), Some("publish"));
        assert_eq!(
            t.inputs.get("packageVersion").map(String::as_str),
            Some("2.0.0")
        );
        assert!(t.inputs.get("versionIncrement").is_none());
    }

    #[test]
    fn publish_with_version_increment() {
        let t = UniversalPackages::publish(
            "my-feed",
            "my-package",
            UniversalPackagesPublish::new()
                .version_increment(VersionIncrement::Minor)
                .directory("$(Build.SourcesDirectory)/dist")
                .package_description("My package"),
        )
        .into_step();
        assert_eq!(
            t.inputs.get("versionIncrement").map(String::as_str),
            Some("minor")
        );
        assert_eq!(
            t.inputs.get("directory").map(String::as_str),
            Some("$(Build.SourcesDirectory)/dist")
        );
        assert_eq!(
            t.inputs.get("packageDescription").map(String::as_str),
            Some("My package")
        );
        assert!(t.inputs.get("packageVersion").is_none());
    }

    #[test]
    fn version_increment_as_ado_str() {
        assert_eq!(VersionIncrement::Major.as_ado_str(), "major");
        assert_eq!(VersionIncrement::Minor.as_ado_str(), "minor");
        assert_eq!(VersionIncrement::Patch.as_ado_str(), "patch");
    }

    #[test]
    fn cross_org_service_connection() {
        let t = UniversalPackages::download(
            "my-feed",
            "my-package",
            UniversalPackagesDownload::new().package_version("1.0.0"),
        )
        .workload_identity_service_connection("my-ado-sc")
        .organization("my-org")
        .with_display_name("Download my-package")
        .into_step();
        assert_eq!(t.display_name, "Download my-package");
        assert_eq!(
            t.inputs
                .get("workloadIdentityServiceConnection")
                .map(String::as_str),
            Some("my-ado-sc")
        );
        assert_eq!(
            t.inputs.get("organization").map(String::as_str),
            Some("my-org")
        );
    }

    #[test]
    fn download_extract() {
        let t = UniversalPackages::download(
            "my-feed",
            "my-package",
            UniversalPackagesDownload::new().extract(true),
        )
        .into_step();
        assert_eq!(
            t.inputs.get("extract").map(String::as_str),
            Some("true")
        );
    }
}
