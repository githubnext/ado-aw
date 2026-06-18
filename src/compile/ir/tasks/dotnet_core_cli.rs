//! Typed builder for `DotNetCoreCLI@2`.
//!
//! Command-dispatch task modeled after [`super::docker`]: a [`DotNetCoreCli`]
//! builder wraps a [`DotNetCommand`] enum whose variants carry each command's
//! optional inputs, so applying an input to the wrong command is
//! unrepresentable.
//!
//! ADO task reference:
//! <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/dotnet-core-cli-v2>

use super::common::{push_bool, push_opt};
use crate::compile::ir::step::TaskStep;

/// `DotNetCoreCLI@2` `command` selector, carrying per-command optional inputs.
#[derive(Debug, Clone)]
pub enum DotNetCommand {
    Build(DotNetBuild),
    Test(DotNetTest),
    Publish(DotNetPublish),
    Restore(DotNetRestore),
    Pack(DotNetPack),
    Run(DotNetRun),
    Push(DotNetPush),
    Custom(DotNetCustom),
}

/// Optionals for `dotnet build`.
#[derive(Debug, Clone, Default)]
pub struct DotNetBuild {
    projects: Option<String>,
    arguments: Option<String>,
    working_directory: Option<String>,
}

impl DotNetBuild {
    pub fn new() -> Self {
        Self::default()
    }
    /// `projects` — glob for `.csproj`/`.sln` files.
    pub fn projects(mut self, value: impl Into<String>) -> Self {
        self.projects = Some(value.into());
        self
    }
    /// `arguments` — extra CLI args.
    pub fn arguments(mut self, value: impl Into<String>) -> Self {
        self.arguments = Some(value.into());
        self
    }
    /// `workingDirectory` — working directory for the command.
    pub fn working_directory(mut self, value: impl Into<String>) -> Self {
        self.working_directory = Some(value.into());
        self
    }
}

/// Optionals for `dotnet test`.
#[derive(Debug, Clone, Default)]
pub struct DotNetTest {
    projects: Option<String>,
    arguments: Option<String>,
    working_directory: Option<String>,
    publish_test_results: Option<bool>,
    test_run_title: Option<String>,
}

impl DotNetTest {
    pub fn new() -> Self {
        Self::default()
    }
    /// `projects` — glob for `.csproj`/`.sln` files.
    pub fn projects(mut self, value: impl Into<String>) -> Self {
        self.projects = Some(value.into());
        self
    }
    /// `arguments` — extra CLI args.
    pub fn arguments(mut self, value: impl Into<String>) -> Self {
        self.arguments = Some(value.into());
        self
    }
    /// `workingDirectory` — working directory for the command.
    pub fn working_directory(mut self, value: impl Into<String>) -> Self {
        self.working_directory = Some(value.into());
        self
    }
    /// `publishTestResults` — publish test results to the pipeline.
    pub fn publish_test_results(mut self, value: bool) -> Self {
        self.publish_test_results = Some(value);
        self
    }
    /// `testRunTitle` — title shown in the build summary.
    pub fn test_run_title(mut self, value: impl Into<String>) -> Self {
        self.test_run_title = Some(value.into());
        self
    }
}

/// Optionals for `dotnet publish`.
#[derive(Debug, Clone, Default)]
pub struct DotNetPublish {
    projects: Option<String>,
    arguments: Option<String>,
    working_directory: Option<String>,
    zip_after_publish: Option<bool>,
    modify_output_path: Option<bool>,
    publish_web_projects: Option<bool>,
}

impl DotNetPublish {
    pub fn new() -> Self {
        Self::default()
    }
    /// `projects` — glob for `.csproj`/`.sln` files.
    pub fn projects(mut self, value: impl Into<String>) -> Self {
        self.projects = Some(value.into());
        self
    }
    /// `arguments` — extra CLI args.
    pub fn arguments(mut self, value: impl Into<String>) -> Self {
        self.arguments = Some(value.into());
        self
    }
    /// `workingDirectory` — working directory for the command.
    pub fn working_directory(mut self, value: impl Into<String>) -> Self {
        self.working_directory = Some(value.into());
        self
    }
    /// `zipAfterPublish` — zip output after publish.
    pub fn zip_after_publish(mut self, value: bool) -> Self {
        self.zip_after_publish = Some(value);
        self
    }
    /// `modifyOutputPath` — append project folder name to the publish path.
    pub fn modify_output_path(mut self, value: bool) -> Self {
        self.modify_output_path = Some(value);
        self
    }
    /// `publishWebProjects` — publish all web projects.
    pub fn publish_web_projects(mut self, value: bool) -> Self {
        self.publish_web_projects = Some(value);
        self
    }
}

/// Optionals for `dotnet restore`.
#[derive(Debug, Clone, Default)]
pub struct DotNetRestore {
    projects: Option<String>,
}

impl DotNetRestore {
    pub fn new() -> Self {
        Self::default()
    }
    /// `projects` — glob for `.csproj`/`.sln` files.
    pub fn projects(mut self, value: impl Into<String>) -> Self {
        self.projects = Some(value.into());
        self
    }
}

/// Optionals for `dotnet pack`.
#[derive(Debug, Clone, Default)]
pub struct DotNetPack {
    packages_to_pack: Option<String>,
}

impl DotNetPack {
    pub fn new() -> Self {
        Self::default()
    }
    /// `packagesToPack` — `.csproj`/`.nuspec` glob to pack.
    pub fn packages_to_pack(mut self, value: impl Into<String>) -> Self {
        self.packages_to_pack = Some(value.into());
        self
    }
}

/// Optionals for `dotnet run`.
#[derive(Debug, Clone, Default)]
pub struct DotNetRun {
    projects: Option<String>,
    arguments: Option<String>,
    working_directory: Option<String>,
}

impl DotNetRun {
    pub fn new() -> Self {
        Self::default()
    }
    /// `projects` — glob for `.csproj`/`.sln` files.
    pub fn projects(mut self, value: impl Into<String>) -> Self {
        self.projects = Some(value.into());
        self
    }
    /// `arguments` — extra CLI args.
    pub fn arguments(mut self, value: impl Into<String>) -> Self {
        self.arguments = Some(value.into());
        self
    }
    /// `workingDirectory` — working directory for the command.
    pub fn working_directory(mut self, value: impl Into<String>) -> Self {
        self.working_directory = Some(value.into());
        self
    }
}

/// Optionals for `dotnet push` (NuGet publish).
#[derive(Debug, Clone, Default)]
pub struct DotNetPush {
    packages_to_push: Option<String>,
}

impl DotNetPush {
    pub fn new() -> Self {
        Self::default()
    }
    /// `packagesToPush` — NuGet package glob to publish.
    pub fn packages_to_push(mut self, value: impl Into<String>) -> Self {
        self.packages_to_push = Some(value.into());
        self
    }
}

/// Inputs for `dotnet custom`. `custom` (the sub-command word) is required.
#[derive(Debug, Clone)]
pub struct DotNetCustom {
    custom: String,
    arguments: Option<String>,
}

impl DotNetCustom {
    /// Required: the custom dotnet sub-command word (e.g. `"tool"`).
    pub fn new(custom: impl Into<String>) -> Self {
        Self {
            custom: custom.into(),
            arguments: None,
        }
    }
    /// `arguments` — extra CLI args.
    pub fn arguments(mut self, value: impl Into<String>) -> Self {
        self.arguments = Some(value.into());
        self
    }
}

/// Builder for a [`TaskStep`] invoking `DotNetCoreCLI@2`.
#[derive(Debug, Clone)]
pub struct DotNetCoreCli {
    command: DotNetCommand,
    display_name: Option<String>,
}

impl DotNetCoreCli {
    /// Construct from an explicit [`DotNetCommand`].
    pub fn new(command: DotNetCommand) -> Self {
        Self {
            command,
            display_name: None,
        }
    }

    /// `command: build`.
    pub fn build(spec: DotNetBuild) -> Self {
        Self::new(DotNetCommand::Build(spec))
    }
    /// `command: test`.
    pub fn test(spec: DotNetTest) -> Self {
        Self::new(DotNetCommand::Test(spec))
    }
    /// `command: publish`.
    pub fn publish(spec: DotNetPublish) -> Self {
        Self::new(DotNetCommand::Publish(spec))
    }
    /// `command: restore`.
    pub fn restore(spec: DotNetRestore) -> Self {
        Self::new(DotNetCommand::Restore(spec))
    }
    /// `command: pack`.
    pub fn pack(spec: DotNetPack) -> Self {
        Self::new(DotNetCommand::Pack(spec))
    }
    /// `command: run`.
    pub fn run(spec: DotNetRun) -> Self {
        Self::new(DotNetCommand::Run(spec))
    }
    /// `command: push`.
    pub fn push(spec: DotNetPush) -> Self {
        Self::new(DotNetCommand::Push(spec))
    }
    /// `command: custom`.
    pub fn custom(spec: DotNetCustom) -> Self {
        Self::new(DotNetCommand::Custom(spec))
    }

    /// Override the default `displayName` (`"dotnet <command>"`).
    pub fn with_display_name(mut self, value: impl Into<String>) -> Self {
        self.display_name = Some(value.into());
        self
    }

    /// Lower into a [`TaskStep`].
    pub fn into_step(self) -> TaskStep {
        let command = match &self.command {
            DotNetCommand::Build(_) => "build",
            DotNetCommand::Test(_) => "test",
            DotNetCommand::Publish(_) => "publish",
            DotNetCommand::Restore(_) => "restore",
            DotNetCommand::Pack(_) => "pack",
            DotNetCommand::Run(_) => "run",
            DotNetCommand::Push(_) => "push",
            DotNetCommand::Custom(_) => "custom",
        };
        let mut t = TaskStep::new(
            "DotNetCoreCLI@2",
            self.display_name.unwrap_or_else(|| format!("dotnet {command}")),
        )
        .with_input("command", command);
        match self.command {
            DotNetCommand::Build(s) => {
                push_opt(&mut t, "projects", s.projects);
                push_opt(&mut t, "arguments", s.arguments);
                push_opt(&mut t, "workingDirectory", s.working_directory);
            }
            DotNetCommand::Test(s) => {
                push_opt(&mut t, "projects", s.projects);
                push_opt(&mut t, "arguments", s.arguments);
                push_opt(&mut t, "workingDirectory", s.working_directory);
                push_bool(&mut t, "publishTestResults", s.publish_test_results);
                push_opt(&mut t, "testRunTitle", s.test_run_title);
            }
            DotNetCommand::Publish(s) => {
                push_opt(&mut t, "projects", s.projects);
                push_opt(&mut t, "arguments", s.arguments);
                push_opt(&mut t, "workingDirectory", s.working_directory);
                push_bool(&mut t, "zipAfterPublish", s.zip_after_publish);
                push_bool(&mut t, "modifyOutputPath", s.modify_output_path);
                push_bool(&mut t, "publishWebProjects", s.publish_web_projects);
            }
            DotNetCommand::Restore(s) => {
                push_opt(&mut t, "projects", s.projects);
            }
            DotNetCommand::Pack(s) => {
                push_opt(&mut t, "packagesToPack", s.packages_to_pack);
            }
            DotNetCommand::Run(s) => {
                push_opt(&mut t, "projects", s.projects);
                push_opt(&mut t, "arguments", s.arguments);
                push_opt(&mut t, "workingDirectory", s.working_directory);
            }
            DotNetCommand::Push(s) => {
                push_opt(&mut t, "packagesToPush", s.packages_to_push);
            }
            DotNetCommand::Custom(s) => {
                t = t.with_input("custom", s.custom);
                push_opt(&mut t, "arguments", s.arguments);
            }
        }
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_default_display_and_command() {
        let t = DotNetCoreCli::build(DotNetBuild::new()).into_step();
        assert_eq!(t.task, "DotNetCoreCLI@2");
        assert_eq!(t.display_name, "dotnet build");
        assert_eq!(t.inputs.get("command").map(String::as_str), Some("build"));
    }

    #[test]
    fn test_command_inputs() {
        let t = DotNetCoreCli::test(
            DotNetTest::new()
                .projects("**/*Tests.csproj")
                .arguments("--configuration Release")
                .publish_test_results(true)
                .test_run_title("Unit Tests"),
        )
        .into_step();
        assert_eq!(t.inputs.get("command").map(String::as_str), Some("test"));
        assert_eq!(t.inputs.get("projects").map(String::as_str), Some("**/*Tests.csproj"));
        assert_eq!(t.inputs.get("publishTestResults").map(String::as_str), Some("true"));
        assert_eq!(t.inputs.get("testRunTitle").map(String::as_str), Some("Unit Tests"));
    }

    #[test]
    fn custom_requires_word() {
        let t = DotNetCoreCli::custom(DotNetCustom::new("tool").arguments("install -g foo"))
            .into_step();
        assert_eq!(t.inputs.get("command").map(String::as_str), Some("custom"));
        assert_eq!(t.inputs.get("custom").map(String::as_str), Some("tool"));
        assert_eq!(t.inputs.get("arguments").map(String::as_str), Some("install -g foo"));
    }
}
