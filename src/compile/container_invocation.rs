//! Typed compiler-owned container invocations.
//!
//! `Docker@2` models Azure Pipelines build/push/login tasks; it cannot start a
//! long-lived runtime container. This module models `docker run` independently
//! of the pipeline transport, then lowers it to a shell fragment at the final
//! boundary. Compiler-owned credential containers deliberately get no raw
//! argument escape hatch.

use std::collections::BTreeMap;

use anyhow::{Result, bail};

use super::shell::bindings::{contains_secret_name, is_shell_var_name, single_quote};

#[derive(Debug, Clone, PartialEq, Eq)]
enum ShellPart {
    Literal(String),
    Variable(String),
    CurrentUid,
    CurrentGid,
}

/// One shell argument assembled from literals and validated variable
/// references.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellWord {
    parts: Vec<ShellPart>,
}

impl ShellWord {
    pub fn literal(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        validate_literal(&value)?;
        Ok(Self {
            parts: vec![ShellPart::Literal(value)],
        })
    }

    pub fn variable(name: impl Into<String>) -> Result<Self> {
        let name = name.into();
        if !is_shell_var_name(&name) {
            bail!(
                "container invocation variable '{name}' is invalid; expected SCREAMING_SNAKE_CASE"
            );
        }
        if contains_secret_name(&name) {
            bail!("credential '{name}' must not be passed through a container command argument");
        }
        Ok(Self {
            parts: vec![ShellPart::Variable(name)],
        })
    }

    pub fn current_user() -> Self {
        Self {
            parts: vec![
                ShellPart::CurrentUid,
                ShellPart::Literal(":".to_string()),
                ShellPart::CurrentGid,
            ],
        }
    }

    pub fn with_literal(mut self, value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        validate_literal(&value)?;
        self.parts.push(ShellPart::Literal(value));
        Ok(self)
    }

    fn is_empty(&self) -> bool {
        self.parts
            .iter()
            .all(|part| matches!(part, ShellPart::Literal(value) if value.is_empty()))
    }

    fn render(&self) -> String {
        if self
            .parts
            .iter()
            .all(|part| matches!(part, ShellPart::Literal(_)))
        {
            let literal = self
                .parts
                .iter()
                .filter_map(|part| match part {
                    ShellPart::Literal(value) => Some(value.as_str()),
                    _ => None,
                })
                .collect::<String>();
            return single_quote(&literal);
        }

        let mut rendered = String::from("\"");
        for part in &self.parts {
            match part {
                ShellPart::Literal(value) => {
                    for character in value.chars() {
                        if matches!(character, '\\' | '"' | '$' | '`') {
                            rendered.push('\\');
                        }
                        rendered.push(character);
                    }
                }
                ShellPart::Variable(name) => {
                    rendered.push_str("${");
                    rendered.push_str(name);
                    rendered.push('}');
                }
                ShellPart::CurrentUid => rendered.push_str("$(id -u)"),
                ShellPart::CurrentGid => rendered.push_str("$(id -g)"),
            }
        }
        rendered.push('"');
        rendered
    }
}

fn validate_literal(value: &str) -> Result<()> {
    if value.contains(['\0', '\n', '\r']) {
        bail!("container invocation arguments must be single-line and contain no NUL bytes");
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DockerMountMode {
    ReadOnly,
    ReadWrite,
}

impl DockerMountMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::ReadOnly => "ro",
            Self::ReadWrite => "rw",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockerMount {
    source: ShellWord,
    destination: String,
    mode: DockerMountMode,
}

impl DockerMount {
    pub fn read_only(source: ShellWord, destination: impl Into<String>) -> Result<Self> {
        Self::new(source, destination, DockerMountMode::ReadOnly)
    }

    pub fn read_write(source: ShellWord, destination: impl Into<String>) -> Result<Self> {
        Self::new(source, destination, DockerMountMode::ReadWrite)
    }

    fn new(
        source: ShellWord,
        destination: impl Into<String>,
        mode: DockerMountMode,
    ) -> Result<Self> {
        let destination = destination.into();
        if source.is_empty() || !destination.starts_with('/') {
            bail!("container mounts require a non-empty source and absolute destination");
        }
        validate_literal(&destination)?;
        Ok(Self {
            source,
            destination,
            mode,
        })
    }

    fn render(&self) -> Result<String> {
        self.source
            .clone()
            .with_literal(format!(":{}:{}", self.destination, self.mode.as_str()))
            .map(|word| word.render())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockerTmpfs {
    destination: String,
    options: String,
}

impl DockerTmpfs {
    pub fn new(destination: impl Into<String>, options: impl Into<String>) -> Result<Self> {
        let destination = destination.into();
        let options = options.into();
        if !destination.starts_with('/') || options.is_empty() {
            bail!("container tmpfs requires an absolute destination and non-empty options");
        }
        validate_literal(&destination)?;
        validate_literal(&options)?;
        Ok(Self {
            destination,
            options,
        })
    }

    fn render(&self) -> String {
        ShellWord {
            parts: vec![ShellPart::Literal(format!(
                "{}:{}",
                self.destination, self.options
            ))],
        }
        .render()
    }
}

/// A validated `docker run` invocation for a compiler-owned container.
#[derive(Debug, Clone)]
pub struct DockerRun {
    image: ShellWord,
    names: Vec<ShellWord>,
    networks: Vec<ShellWord>,
    users: Vec<ShellWord>,
    detached: bool,
    remove_on_exit: bool,
    cap_drop_all: bool,
    no_new_privileges: bool,
    read_only: bool,
    tmpfs: Vec<DockerTmpfs>,
    pids_limits: Vec<u32>,
    entrypoints: Vec<ShellWord>,
    mounts: Vec<DockerMount>,
    command: Vec<ShellWord>,
    discard_stdout: bool,
}

impl DockerRun {
    pub fn new(image: ShellWord) -> Self {
        Self {
            image,
            names: Vec::new(),
            networks: Vec::new(),
            users: Vec::new(),
            detached: false,
            remove_on_exit: false,
            cap_drop_all: false,
            no_new_privileges: false,
            read_only: false,
            tmpfs: Vec::new(),
            pids_limits: Vec::new(),
            entrypoints: Vec::new(),
            mounts: Vec::new(),
            command: Vec::new(),
            discard_stdout: false,
        }
    }

    pub fn name(mut self, value: ShellWord) -> Self {
        self.names.push(value);
        self
    }

    pub fn network(mut self, value: ShellWord) -> Self {
        self.networks.push(value);
        self
    }

    pub fn user(mut self, value: ShellWord) -> Self {
        self.users.push(value);
        self
    }

    pub fn detached(mut self) -> Self {
        self.detached = true;
        self
    }

    #[allow(dead_code)]
    pub fn remove_on_exit(mut self) -> Self {
        self.remove_on_exit = true;
        self
    }

    pub fn cap_drop_all(mut self) -> Self {
        self.cap_drop_all = true;
        self
    }

    pub fn no_new_privileges(mut self) -> Self {
        self.no_new_privileges = true;
        self
    }

    pub fn read_only(mut self) -> Self {
        self.read_only = true;
        self
    }

    pub fn tmpfs(mut self, value: DockerTmpfs) -> Self {
        self.tmpfs.push(value);
        self
    }

    pub fn pids_limit(mut self, value: u32) -> Self {
        self.pids_limits.push(value);
        self
    }

    pub fn entrypoint(mut self, value: ShellWord) -> Self {
        self.entrypoints.push(value);
        self
    }

    pub fn mount(mut self, value: DockerMount) -> Self {
        self.mounts.push(value);
        self
    }

    pub fn command_arg(mut self, value: ShellWord) -> Self {
        self.command.push(value);
        self
    }

    pub fn discard_stdout(mut self) -> Self {
        self.discard_stdout = true;
        self
    }

    pub fn render_bash(&self) -> Result<String> {
        if self.image.is_empty() {
            bail!("container image must not be empty");
        }
        validate_singleton(&self.names, "name")?;
        validate_singleton(&self.networks, "network")?;
        validate_singleton(&self.users, "user")?;
        validate_singleton(&self.pids_limits, "PID limit")?;
        validate_singleton(&self.entrypoints, "entrypoint")?;
        if self.pids_limits.first() == Some(&0) {
            bail!("container PID limit must be greater than zero");
        }
        validate_mount_destinations(&self.mounts)?;

        let mut segments = vec!["docker run".to_string()];
        if self.detached {
            segments.push("-d".to_string());
        }
        if self.remove_on_exit {
            segments.push("--rm".to_string());
        }
        if let Some(name) = self.names.first() {
            segments.push(format!("--name {}", name.render()));
        }
        if let Some(network) = self.networks.first() {
            segments.push(format!("--network {}", network.render()));
        }
        if let Some(user) = self.users.first() {
            segments.push(format!("--user {}", user.render()));
        }
        if self.cap_drop_all {
            segments.push("--cap-drop ALL".to_string());
        }
        if self.no_new_privileges {
            segments.push("--security-opt no-new-privileges".to_string());
        }
        if self.read_only {
            segments.push("--read-only".to_string());
        }
        for tmpfs in &self.tmpfs {
            segments.push(format!("--tmpfs {}", tmpfs.render()));
        }
        if let Some(limit) = self.pids_limits.first() {
            segments.push(format!("--pids-limit {limit}"));
        }
        if let Some(entrypoint) = self.entrypoints.first() {
            segments.push(format!("--entrypoint {}", entrypoint.render()));
        }
        for mount in &self.mounts {
            segments.push(format!("-v {}", mount.render()?));
        }
        segments.push(self.image.render());
        segments.extend(self.command.iter().map(ShellWord::render));

        let mut rendered = String::new();
        for (index, segment) in segments.iter().enumerate() {
            if index == 0 {
                rendered.push_str(segment);
            } else {
                rendered.push_str("  ");
                rendered.push_str(segment);
            }
            if index + 1 != segments.len() || self.discard_stdout {
                rendered.push_str(" \\\n");
            } else {
                rendered.push('\n');
            }
        }
        if self.discard_stdout {
            rendered.push_str("  >/dev/null\n");
        }
        Ok(rendered)
    }
}

fn validate_singleton<T>(values: &[T], setting: &str) -> Result<()> {
    if values.len() > 1 {
        bail!("container invocation must not configure {setting} more than once");
    }
    Ok(())
}

fn validate_mount_destinations(mounts: &[DockerMount]) -> Result<()> {
    let mut destinations = BTreeMap::new();
    for mount in mounts {
        if destinations
            .insert(mount.destination.as_str(), mount.source.render())
            .is_some()
        {
            bail!(
                "container mount destination '{}' is configured more than once",
                mount.destination
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_hardened_detached_container_without_raw_arguments() {
        let invocation = DockerRun::new(ShellWord::variable("IMAGE").unwrap())
            .detached()
            .name(ShellWord::variable("CONTAINER").unwrap())
            .network(ShellWord::literal("bridge").unwrap())
            .user(ShellWord::current_user())
            .cap_drop_all()
            .no_new_privileges()
            .read_only()
            .tmpfs(DockerTmpfs::new("/tmp", "rw,nosuid,nodev,noexec").unwrap())
            .pids_limit(64)
            .entrypoint(ShellWord::literal("sh").unwrap())
            .mount(
                DockerMount::read_only(ShellWord::variable("BUNDLE").unwrap(), "/app/bundle.js")
                    .unwrap(),
            )
            .command_arg(ShellWord::literal("-c").unwrap())
            .command_arg(ShellWord::literal("exec node /app/bundle.js").unwrap())
            .discard_stdout();

        assert_eq!(
            invocation.render_bash().unwrap(),
            concat!(
                "docker run \\\n",
                "  -d \\\n",
                "  --name \"${CONTAINER}\" \\\n",
                "  --network 'bridge' \\\n",
                "  --user \"$(id -u):$(id -g)\" \\\n",
                "  --cap-drop ALL \\\n",
                "  --security-opt no-new-privileges \\\n",
                "  --read-only \\\n",
                "  --tmpfs '/tmp:rw,nosuid,nodev,noexec' \\\n",
                "  --pids-limit 64 \\\n",
                "  --entrypoint 'sh' \\\n",
                "  -v \"${BUNDLE}:/app/bundle.js:ro\" \\\n",
                "  \"${IMAGE}\" \\\n",
                "  '-c' \\\n",
                "  'exec node /app/bundle.js' \\\n",
                "  >/dev/null\n",
            )
        );
    }

    #[test]
    fn rejects_duplicate_singletons_and_mount_destinations() {
        let duplicate_name = DockerRun::new(ShellWord::literal("image").unwrap())
            .name(ShellWord::literal("one").unwrap())
            .name(ShellWord::literal("two").unwrap());
        assert!(duplicate_name.render_bash().is_err());

        let duplicate_mount = DockerRun::new(ShellWord::literal("image").unwrap())
            .mount(DockerMount::read_only(ShellWord::literal("/one").unwrap(), "/target").unwrap())
            .mount(
                DockerMount::read_write(ShellWord::literal("/two").unwrap(), "/target").unwrap(),
            );
        assert!(duplicate_mount.render_bash().is_err());
    }

    #[test]
    fn shell_words_quote_literals_and_validate_variables() {
        assert_eq!(ShellWord::literal("a'b").unwrap().render(), "'a'\\''b'");
        assert_eq!(
            ShellWord::variable("ROOT")
                .unwrap()
                .with_literal("/child")
                .unwrap()
                .render(),
            "\"${ROOT}/child\""
        );
        assert!(ShellWord::variable("bad-name").is_err());
        assert!(ShellWord::variable("ADO_PROXY_BEARER").is_err());
    }
}
