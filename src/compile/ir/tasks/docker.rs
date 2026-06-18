//! Typed builder for `Docker@2`.
//!
//! This is the **canonical command-dispatch task template**: a single
//! [`Docker`] builder wraps a [`DockerCommand`] enum whose variants carry the
//! per-command optional inputs. Because each command's optionals live inside its
//! own variant, applying an input to the wrong command (e.g. a `build`-only
//! `arguments` to a `login`) is unrepresentable. Model new command/mode-dispatch
//! tasks (e.g. `DotNetCoreCLI@2`, `NuGetCommand@2`) after this file.
//!
//! ADO task reference:
//! <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/docker-v2>

use crate::compile::ir::step::TaskStep;

/// `Docker@2` `command` selector, carrying the per-command optional inputs.
#[derive(Debug, Clone)]
pub enum DockerCommand {
    BuildAndPush(DockerBuildAndPush),
    Build(DockerBuild),
    Push(DockerPush),
    Login(DockerLogin),
    Logout(DockerLogout),
}

/// Optionals for `Docker@2` `command: buildAndPush`.
#[derive(Debug, Clone, Default)]
pub struct DockerBuildAndPush {
    container_registry: Option<String>,
    repository: Option<String>,
    dockerfile: Option<String>,
    build_context: Option<String>,
    tags: Option<String>,
}

impl DockerBuildAndPush {
    pub fn new() -> Self {
        Self::default()
    }
    /// `containerRegistry` — Docker registry service connection.
    pub fn container_registry(mut self, value: impl Into<String>) -> Self {
        self.container_registry = Some(value.into());
        self
    }
    /// `repository` — container repository name.
    pub fn repository(mut self, value: impl Into<String>) -> Self {
        self.repository = Some(value.into());
        self
    }
    /// `Dockerfile` — path or glob to the Dockerfile.
    pub fn dockerfile(mut self, value: impl Into<String>) -> Self {
        self.dockerfile = Some(value.into());
        self
    }
    /// `buildContext` — build context path.
    pub fn build_context(mut self, value: impl Into<String>) -> Self {
        self.build_context = Some(value.into());
        self
    }
    /// `tags` — newline-separated image tags.
    pub fn tags(mut self, value: impl Into<String>) -> Self {
        self.tags = Some(value.into());
        self
    }
}

/// Optionals for `Docker@2` `command: build`.
#[derive(Debug, Clone, Default)]
pub struct DockerBuild {
    container_registry: Option<String>,
    repository: Option<String>,
    dockerfile: Option<String>,
    build_context: Option<String>,
    tags: Option<String>,
    arguments: Option<String>,
}

impl DockerBuild {
    pub fn new() -> Self {
        Self::default()
    }
    /// `containerRegistry` — Docker registry service connection.
    pub fn container_registry(mut self, value: impl Into<String>) -> Self {
        self.container_registry = Some(value.into());
        self
    }
    /// `repository` — image name to tag the build as.
    pub fn repository(mut self, value: impl Into<String>) -> Self {
        self.repository = Some(value.into());
        self
    }
    /// `Dockerfile` — path or glob to the Dockerfile.
    pub fn dockerfile(mut self, value: impl Into<String>) -> Self {
        self.dockerfile = Some(value.into());
        self
    }
    /// `buildContext` — build context path.
    pub fn build_context(mut self, value: impl Into<String>) -> Self {
        self.build_context = Some(value.into());
        self
    }
    /// `tags` — newline-separated image tags.
    pub fn tags(mut self, value: impl Into<String>) -> Self {
        self.tags = Some(value.into());
        self
    }
    /// `arguments` — extra arguments for `docker build`.
    pub fn arguments(mut self, value: impl Into<String>) -> Self {
        self.arguments = Some(value.into());
        self
    }
}

/// Optionals for `Docker@2` `command: push`.
#[derive(Debug, Clone, Default)]
pub struct DockerPush {
    container_registry: Option<String>,
    repository: Option<String>,
    tags: Option<String>,
    arguments: Option<String>,
}

impl DockerPush {
    pub fn new() -> Self {
        Self::default()
    }
    /// `containerRegistry` — Docker registry service connection.
    pub fn container_registry(mut self, value: impl Into<String>) -> Self {
        self.container_registry = Some(value.into());
        self
    }
    /// `repository` — container repository to push to.
    pub fn repository(mut self, value: impl Into<String>) -> Self {
        self.repository = Some(value.into());
        self
    }
    /// `tags` — newline-separated tags to push.
    pub fn tags(mut self, value: impl Into<String>) -> Self {
        self.tags = Some(value.into());
        self
    }
    /// `arguments` — extra arguments for `docker push`.
    pub fn arguments(mut self, value: impl Into<String>) -> Self {
        self.arguments = Some(value.into());
        self
    }
}

/// Optionals for `Docker@2` `command: login`.
#[derive(Debug, Clone, Default)]
pub struct DockerLogin {
    container_registry: Option<String>,
}

impl DockerLogin {
    pub fn new() -> Self {
        Self::default()
    }
    /// `containerRegistry` — Docker registry service connection.
    pub fn container_registry(mut self, value: impl Into<String>) -> Self {
        self.container_registry = Some(value.into());
        self
    }
}

/// Optionals for `Docker@2` `command: logout`.
#[derive(Debug, Clone, Default)]
pub struct DockerLogout {
    container_registry: Option<String>,
}

impl DockerLogout {
    pub fn new() -> Self {
        Self::default()
    }
    /// `containerRegistry` — Docker registry service connection.
    pub fn container_registry(mut self, value: impl Into<String>) -> Self {
        self.container_registry = Some(value.into());
        self
    }
}

/// Builder for a [`TaskStep`] invoking `Docker@2`.
#[derive(Debug, Clone)]
pub struct Docker {
    command: DockerCommand,
    display_name: Option<String>,
}

impl Docker {
    /// Construct from an explicit [`DockerCommand`].
    pub fn new(command: DockerCommand) -> Self {
        Self {
            command,
            display_name: None,
        }
    }

    /// `command: buildAndPush`.
    pub fn build_and_push(spec: DockerBuildAndPush) -> Self {
        Self::new(DockerCommand::BuildAndPush(spec))
    }

    /// `command: build`.
    pub fn build(spec: DockerBuild) -> Self {
        Self::new(DockerCommand::Build(spec))
    }

    /// `command: push`.
    pub fn push(spec: DockerPush) -> Self {
        Self::new(DockerCommand::Push(spec))
    }

    /// `command: login`.
    pub fn login(spec: DockerLogin) -> Self {
        Self::new(DockerCommand::Login(spec))
    }

    /// `command: logout`.
    pub fn logout(spec: DockerLogout) -> Self {
        Self::new(DockerCommand::Logout(spec))
    }

    /// Override the default per-command `displayName`.
    pub fn with_display_name(mut self, value: impl Into<String>) -> Self {
        self.display_name = Some(value.into());
        self
    }

    /// Lower into a [`TaskStep`].
    pub fn into_step(self) -> TaskStep {
        let (command, default_display): (&str, &str) = match &self.command {
            DockerCommand::BuildAndPush(_) => ("buildAndPush", "Build and Push Docker Image"),
            DockerCommand::Build(_) => ("build", "Build Docker Image"),
            DockerCommand::Push(_) => ("push", "Push Docker Image"),
            DockerCommand::Login(_) => ("login", "Docker Login"),
            DockerCommand::Logout(_) => ("logout", "Docker Logout"),
        };
        let mut t = TaskStep::new(
            "Docker@2",
            self.display_name.unwrap_or_else(|| default_display.into()),
        )
        .with_input("command", command);
        match self.command {
            DockerCommand::BuildAndPush(s) => {
                if let Some(v) = s.container_registry {
                    t = t.with_input("containerRegistry", v);
                }
                if let Some(v) = s.repository {
                    t = t.with_input("repository", v);
                }
                if let Some(v) = s.dockerfile {
                    t = t.with_input("Dockerfile", v);
                }
                if let Some(v) = s.build_context {
                    t = t.with_input("buildContext", v);
                }
                if let Some(v) = s.tags {
                    t = t.with_input("tags", v);
                }
            }
            DockerCommand::Build(s) => {
                if let Some(v) = s.container_registry {
                    t = t.with_input("containerRegistry", v);
                }
                if let Some(v) = s.repository {
                    t = t.with_input("repository", v);
                }
                if let Some(v) = s.dockerfile {
                    t = t.with_input("Dockerfile", v);
                }
                if let Some(v) = s.build_context {
                    t = t.with_input("buildContext", v);
                }
                if let Some(v) = s.tags {
                    t = t.with_input("tags", v);
                }
                if let Some(v) = s.arguments {
                    t = t.with_input("arguments", v);
                }
            }
            DockerCommand::Push(s) => {
                if let Some(v) = s.container_registry {
                    t = t.with_input("containerRegistry", v);
                }
                if let Some(v) = s.repository {
                    t = t.with_input("repository", v);
                }
                if let Some(v) = s.tags {
                    t = t.with_input("tags", v);
                }
                if let Some(v) = s.arguments {
                    t = t.with_input("arguments", v);
                }
            }
            DockerCommand::Login(s) => {
                if let Some(v) = s.container_registry {
                    t = t.with_input("containerRegistry", v);
                }
            }
            DockerCommand::Logout(s) => {
                if let Some(v) = s.container_registry {
                    t = t.with_input("containerRegistry", v);
                }
            }
        }
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_and_push_defaults() {
        let t = Docker::build_and_push(DockerBuildAndPush::new()).into_step();
        assert_eq!(t.task, "Docker@2");
        assert_eq!(t.display_name, "Build and Push Docker Image");
        assert_eq!(t.inputs.get("command").map(String::as_str), Some("buildAndPush"));
    }

    #[test]
    fn build_and_push_inputs() {
        let t = Docker::build_and_push(
            DockerBuildAndPush::new()
                .container_registry("myRegistryServiceConnection")
                .repository("myapp")
                .dockerfile("src/Dockerfile")
                .build_context("src/")
                .tags("latest\n$(Build.BuildId)"),
        )
        .into_step();
        assert_eq!(
            t.inputs.get("containerRegistry").map(String::as_str),
            Some("myRegistryServiceConnection")
        );
        assert_eq!(t.inputs.get("repository").map(String::as_str), Some("myapp"));
        assert_eq!(t.inputs.get("Dockerfile").map(String::as_str), Some("src/Dockerfile"));
        assert_eq!(t.inputs.get("buildContext").map(String::as_str), Some("src/"));
        assert_eq!(t.inputs.get("tags").map(String::as_str), Some("latest\n$(Build.BuildId)"));
    }

    #[test]
    fn build_command_has_arguments() {
        let t = Docker::build(DockerBuild::new().arguments("--no-cache")).into_step();
        assert_eq!(t.inputs.get("command").map(String::as_str), Some("build"));
        assert_eq!(t.inputs.get("arguments").map(String::as_str), Some("--no-cache"));
    }

    #[test]
    fn login_logout_only_registry() {
        let login =
            Docker::login(DockerLogin::new().container_registry("myPrivateRegistry")).into_step();
        assert_eq!(login.inputs.get("command").map(String::as_str), Some("login"));
        assert_eq!(
            login.inputs.get("containerRegistry").map(String::as_str),
            Some("myPrivateRegistry")
        );
        assert!(login.inputs.get("repository").is_none());

        let logout = Docker::logout(DockerLogout::new()).into_step();
        assert_eq!(logout.inputs.get("command").map(String::as_str), Some("logout"));
        assert_eq!(logout.display_name, "Docker Logout");
    }
}
