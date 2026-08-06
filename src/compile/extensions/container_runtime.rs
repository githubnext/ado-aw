//! Typed Docker runtime configuration for MCPG stdio servers.

use anyhow::{Result, bail};
use serde::ser::{Serialize, SerializeMap, Serializer};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MountMode {
    ReadOnly,
    ReadWrite,
}

impl MountMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::ReadOnly => "ro",
            Self::ReadWrite => "rw",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mount {
    source: String,
    destination: String,
    mode: MountMode,
}

impl Mount {
    pub fn new(
        source: impl Into<String>,
        destination: impl Into<String>,
        mode: MountMode,
    ) -> Result<Self> {
        let source = source.into();
        let destination = destination.into();
        if source.is_empty() || destination.is_empty() {
            bail!("container mount source and destination must not be empty");
        }
        Ok(Self {
            source,
            destination,
            mode,
        })
    }

    pub fn read_only(source: impl Into<String>, destination: impl Into<String>) -> Result<Self> {
        Self::new(source, destination, MountMode::ReadOnly)
    }

    pub fn read_write(source: impl Into<String>, destination: impl Into<String>) -> Result<Self> {
        Self::new(source, destination, MountMode::ReadWrite)
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn destination(&self) -> &str {
        &self.destination
    }

    pub fn mode(&self) -> MountMode {
        self.mode
    }

    fn render(&self) -> String {
        format!(
            "{}:{}:{}",
            self.source,
            self.destination,
            self.mode.as_str()
        )
    }
}

impl TryFrom<&str> for Mount {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self> {
        let (paths, mode) = value
            .rsplit_once(':')
            .ok_or_else(|| anyhow::anyhow!("container mount must use source:destination:mode"))?;
        let (source, destination) = paths
            .split_once(':')
            .ok_or_else(|| anyhow::anyhow!("container mount must use source:destination:mode"))?;
        let mode = match mode {
            "ro" => MountMode::ReadOnly,
            "rw" => MountMode::ReadWrite,
            other => bail!("container mount mode must be `ro` or `rw`, got `{other}`"),
        };
        Self::new(source, destination, mode)
    }
}

impl Serialize for Mount {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.render())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Network {
    None,
    Named(String),
}

impl Network {
    pub fn named(name: impl Into<String>) -> Result<Self> {
        let name = name.into();
        if name.is_empty() {
            bail!("container network name must not be empty");
        }
        if name == "host" {
            bail!("host networking is not allowed for compiler-owned MCP containers");
        }
        if name == "none" {
            bail!("use Network::None for an isolated container network");
        }
        Ok(Self::Named(name))
    }

    fn as_str(&self) -> &str {
        match self {
            Self::None => "none",
            Self::Named(name) => name,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddHost {
    host: String,
    address: String,
}

impl AddHost {
    pub fn new(host: impl Into<String>, address: impl Into<String>) -> Result<Self> {
        let host = host.into();
        let address = address.into();
        if host.is_empty() || address.is_empty() {
            bail!("container host mapping host and address must not be empty");
        }
        Ok(Self { host, address })
    }

    fn render(&self) -> String {
        format!("{}:{}", self.host, self.address)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerUser(String);

impl ContainerUser {
    pub fn new(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        if value.is_empty() {
            bail!("container user must not be empty");
        }
        Ok(Self(value))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tmpfs {
    destination: String,
    options: String,
}

impl Tmpfs {
    pub fn new(destination: impl Into<String>, options: impl Into<String>) -> Result<Self> {
        let destination = destination.into();
        let options = options.into();
        if destination.is_empty() || options.is_empty() {
            bail!("tmpfs destination and options must not be empty");
        }
        Ok(Self {
            destination,
            options,
        })
    }

    fn render(&self) -> String {
        format!("{}:{}", self.destination, self.options)
    }
}

#[derive(Debug, Clone, Default)]
pub struct ContainerRuntimeConfig {
    mounts: Vec<Mount>,
    network: Option<Network>,
    add_hosts: Vec<AddHost>,
    user: Option<ContainerUser>,
    cap_drop_all: bool,
    no_new_privileges: bool,
    read_only: bool,
    tmpfs: Vec<Tmpfs>,
    pids_limit: Option<u32>,
    working_directory: Option<String>,
    extra_args: Vec<String>,
}

impl ContainerRuntimeConfig {
    pub fn builder() -> ContainerRuntimeBuilder {
        ContainerRuntimeBuilder::default()
    }

    pub fn mounts(&self) -> &[Mount] {
        &self.mounts
    }

    pub fn args(&self) -> Vec<String> {
        let mut args = Vec::new();
        if let Some(network) = &self.network {
            args.extend(["--network".to_string(), network.as_str().to_string()]);
        }
        for add_host in &self.add_hosts {
            args.extend(["--add-host".to_string(), add_host.render()]);
        }
        if let Some(user) = &self.user {
            args.extend(["--user".to_string(), user.0.clone()]);
        }
        if self.cap_drop_all {
            args.extend(["--cap-drop".to_string(), "ALL".to_string()]);
        }
        if self.no_new_privileges {
            args.extend([
                "--security-opt".to_string(),
                "no-new-privileges".to_string(),
            ]);
        }
        if self.read_only {
            args.push("--read-only".to_string());
        }
        for tmpfs in &self.tmpfs {
            args.extend(["--tmpfs".to_string(), tmpfs.render()]);
        }
        if let Some(limit) = self.pids_limit {
            args.extend(["--pids-limit".to_string(), limit.to_string()]);
        }
        if let Some(working_directory) = &self.working_directory {
            args.extend(["-w".to_string(), working_directory.clone()]);
        }
        args.extend(self.extra_args.iter().cloned());
        args
    }
}

impl Serialize for ContainerRuntimeConfig {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let args = self.args();
        let mut map = serializer.serialize_map(None)?;
        if !self.mounts.is_empty() {
            map.serialize_entry("mounts", &self.mounts)?;
        }
        if !args.is_empty() {
            map.serialize_entry("args", &args)?;
        }
        map.end()
    }
}

#[derive(Debug, Default)]
pub struct ContainerRuntimeBuilder {
    mounts: Vec<Mount>,
    networks: Vec<Network>,
    add_hosts: Vec<AddHost>,
    users: Vec<ContainerUser>,
    cap_drop_all: bool,
    no_new_privileges: bool,
    read_only: bool,
    tmpfs: Vec<Tmpfs>,
    pids_limits: Vec<u32>,
    working_directories: Vec<String>,
    extra_args: Vec<String>,
}

impl ContainerRuntimeBuilder {
    pub fn mount(mut self, mount: Mount) -> Self {
        self.mounts.push(mount);
        self
    }

    pub fn network(mut self, network: Network) -> Self {
        self.networks.push(network);
        self
    }

    pub fn add_host(mut self, add_host: AddHost) -> Self {
        self.add_hosts.push(add_host);
        self
    }

    pub fn user(mut self, user: ContainerUser) -> Self {
        self.users.push(user);
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

    pub fn tmpfs(mut self, tmpfs: Tmpfs) -> Self {
        self.tmpfs.push(tmpfs);
        self
    }

    pub fn pids_limit(mut self, limit: u32) -> Self {
        self.pids_limits.push(limit);
        self
    }

    pub fn working_directory(mut self, path: impl Into<String>) -> Self {
        self.working_directories.push(path.into());
        self
    }

    pub fn extra_args(mut self, args: &[String]) -> Self {
        self.extra_args.extend_from_slice(args);
        self
    }

    pub fn build(self) -> Result<ContainerRuntimeConfig> {
        if self.networks.len() > 1 {
            bail!("container runtime must not configure more than one network");
        }
        if self.users.len() > 1 {
            bail!("container runtime must not configure more than one user");
        }
        if self.pids_limits.len() > 1 {
            bail!("container runtime must not configure more than one PID limit");
        }
        if self.working_directories.len() > 1 {
            bail!("container runtime must not configure more than one working directory");
        }
        if self.working_directories.iter().any(String::is_empty) {
            bail!("container working directory must not be empty");
        }
        validate_mount_destinations(&self.mounts)?;
        validate_extra_args(&self.extra_args)?;
        reject_typed_raw_conflict(
            !self.networks.is_empty(),
            &self.extra_args,
            &["--network"],
            "network",
        )?;
        reject_typed_raw_conflict(
            !self.users.is_empty(),
            &self.extra_args,
            &["--user"],
            "user",
        )?;
        reject_typed_raw_conflict(
            !self.pids_limits.is_empty(),
            &self.extra_args,
            &["--pids-limit"],
            "PID limit",
        )?;
        reject_typed_raw_conflict(
            !self.working_directories.is_empty(),
            &self.extra_args,
            &["-w", "--workdir"],
            "working directory",
        )?;
        validate_add_hosts(&self.add_hosts)?;
        validate_tmpfs(&self.tmpfs)?;

        Ok(ContainerRuntimeConfig {
            mounts: self.mounts,
            network: self.networks.into_iter().next(),
            add_hosts: self.add_hosts,
            user: self.users.into_iter().next(),
            cap_drop_all: self.cap_drop_all,
            no_new_privileges: self.no_new_privileges,
            read_only: self.read_only,
            tmpfs: self.tmpfs,
            pids_limit: self.pids_limits.into_iter().next(),
            working_directory: self.working_directories.into_iter().next(),
            extra_args: self.extra_args,
        })
    }
}

fn validate_add_hosts(add_hosts: &[AddHost]) -> Result<()> {
    let mut hosts = BTreeMap::new();
    for add_host in add_hosts {
        if let Some(existing) = hosts.insert(&add_host.host, &add_host.address)
            && existing != &add_host.address
        {
            bail!(
                "container host `{}` maps to conflicting addresses `{existing}` and `{}`",
                add_host.host,
                add_host.address
            );
        }
    }
    Ok(())
}

fn validate_tmpfs(tmpfs: &[Tmpfs]) -> Result<()> {
    let mut destinations = BTreeMap::new();
    for entry in tmpfs {
        if destinations
            .insert(&entry.destination, &entry.options)
            .is_some()
        {
            bail!(
                "container tmpfs destination `{}` is configured more than once",
                entry.destination
            );
        }
    }
    Ok(())
}

fn validate_mount_destinations(mounts: &[Mount]) -> Result<()> {
    let mut destinations = BTreeMap::new();
    for mount in mounts {
        if let Some(existing) = destinations.insert(mount.destination(), mount) {
            bail!(
                "container mount destination `{}` is configured more than once (from `{}` and `{}`)",
                mount.destination(),
                existing.source(),
                mount.source()
            );
        }
    }
    Ok(())
}

fn validate_extra_args(args: &[String]) -> Result<()> {
    for flags in [
        &["--network"][..],
        &["--user"][..],
        &["--pids-limit"][..],
        &["-w", "--workdir"][..],
    ] {
        let count = count_args(args, flags)?;
        if count > 1 {
            bail!(
                "container runtime argument `{}` must not be configured more than once",
                flags.join("`/`")
            );
        }
    }
    Ok(())
}

fn reject_typed_raw_conflict(
    typed_is_set: bool,
    args: &[String],
    flags: &[&str],
    setting: &str,
) -> Result<()> {
    if typed_is_set && count_args(args, flags)? != 0 {
        bail!("container {setting} cannot be configured through both typed and raw arguments");
    }
    Ok(())
}

fn count_args(args: &[String], flags: &[&str]) -> Result<usize> {
    let mut count = 0;
    for (index, arg) in args.iter().enumerate() {
        for flag in flags {
            if arg == flag {
                if args.get(index + 1).is_none() {
                    bail!("container runtime argument `{flag}` requires a value");
                }
                count += 1;
            } else if arg.starts_with(&format!("{flag}=")) {
                count += 1;
            }
        }
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_mounts_and_runtime_args_in_mcpg_order() {
        let runtime = ContainerRuntimeConfig::builder()
            .mount(Mount::read_only("/host/a", "/a").unwrap())
            .mount(Mount::read_write("/host/b", "/b").unwrap())
            .network(Network::None)
            .user(ContainerUser::new("1000:1000").unwrap())
            .read_only()
            .build()
            .unwrap();

        assert_eq!(
            serde_json::to_value(&runtime).unwrap(),
            serde_json::json!({
                "mounts": ["/host/a:/a:ro", "/host/b:/b:rw"],
                "args": ["--network", "none", "--user", "1000:1000", "--read-only"]
            })
        );
    }

    #[test]
    fn rejects_host_networking_and_conflicting_singletons() {
        assert!(Network::named("host").is_err());
        assert!(
            ContainerRuntimeConfig::builder()
                .network(Network::None)
                .network(Network::named("internal").unwrap())
                .build()
                .is_err()
        );
        assert!(
            ContainerRuntimeConfig::builder()
                .user(ContainerUser::new("1000").unwrap())
                .user(ContainerUser::new("1001").unwrap())
                .build()
                .is_err()
        );
    }

    #[test]
    fn rejects_conflicting_mount_destinations_and_malformed_raw_args() {
        assert!(
            ContainerRuntimeConfig::builder()
                .mount(Mount::read_only("/one", "/target").unwrap())
                .mount(Mount::read_write("/two", "/target").unwrap())
                .build()
                .is_err()
        );
        assert!(
            ContainerRuntimeConfig::builder()
                .extra_args(&["--network".to_string()])
                .build()
                .is_err()
        );
        assert!(
            ContainerRuntimeConfig::builder()
                .extra_args(&[
                    "-w".to_string(),
                    "/one".to_string(),
                    "--workdir=/two".to_string(),
                ])
                .build()
                .is_err()
        );
    }
}
