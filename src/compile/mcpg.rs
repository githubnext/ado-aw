//! Typed MCPG configuration and host-launch environment.

use anyhow::{Result, bail};
use std::collections::BTreeMap;

use super::extensions::McpgConfig;
use super::ir::env::EnvValue;
use crate::secure::AdoVariableName;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct McpgEnvName(String);

impl McpgEnvName {
    pub fn parse(value: impl Into<String>, origin: &str) -> Result<Self> {
        let value = value.into();
        if !crate::validate::is_valid_env_var_name(&value) {
            bail!(
                "{origin} environment variable name '{value}' is invalid; expected [A-Za-z_][A-Za-z0-9_]*"
            );
        }
        if value.starts_with("ADO_AW_MCPG_INTERNAL_")
            || matches!(
                value.as_str(),
                "MCP_GATEWAY_API_KEY"
                    | "MCP_GATEWAY_PORT"
                    | "MCP_GATEWAY_DOMAIN"
                    | "ADO_PROXY_IP"
                    | "MCPG_CONTAINER"
                    | "MCPG_IMAGE"
                    | "MCPG_PORT"
                    | "MCPG_DOMAIN"
                    | "MCP_RUNNER_UID"
                    | "MCP_RUNNER_GID"
                    | "MCPG_CONFIG"
                    | "GATEWAY_OUTPUT"
                    | "MCPG_ENV_NAMES"
                    | "MCPG_DOCKER_ENV_ARGS"
                    | "MCPG_ENV_NAME"
            )
        {
            bail!(
                "{origin} environment variable name '{value}' is reserved by the MCPG launch step"
            );
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone)]
struct McpgLaunchBinding {
    value: EnvValue,
    origin: String,
}

#[derive(Debug, Clone, Default)]
pub struct McpgLaunchEnvironment {
    bindings: BTreeMap<McpgEnvName, McpgLaunchBinding>,
}

impl McpgLaunchEnvironment {
    pub fn bind_pipeline_variable(
        &mut self,
        destination: impl Into<String>,
        source: &AdoVariableName,
        origin: impl Into<String>,
    ) -> Result<()> {
        self.bind(destination, EnvValue::pipeline_var(source.as_str()), origin)
    }

    pub fn bind_literal(
        &mut self,
        destination: impl Into<String>,
        value: impl Into<String>,
        origin: impl Into<String>,
    ) -> Result<()> {
        self.bind(destination, EnvValue::literal(value), origin)
    }

    fn bind(
        &mut self,
        destination: impl Into<String>,
        value: EnvValue,
        origin: impl Into<String>,
    ) -> Result<()> {
        let origin = origin.into();
        let destination = McpgEnvName::parse(destination, &origin)?;
        if let Some(existing) = self.bindings.get(&destination) {
            if existing.value == value {
                return Ok(());
            }
            bail!(
                "conflicting MCPG environment binding for '{}': {} requires {:?}, but {} requires {:?}",
                destination.as_str(),
                existing.origin,
                existing.value,
                origin,
                value
            );
        }
        self.bindings
            .insert(destination, McpgLaunchBinding { value, origin });
        Ok(())
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &EnvValue)> {
        self.bindings
            .iter()
            .map(|(name, binding)| (name.as_str(), &binding.value))
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.bindings.keys().map(McpgEnvName::as_str)
    }

    #[cfg(test)]
    pub fn get(&self, name: &str) -> Option<&EnvValue> {
        self.bindings
            .iter()
            .find_map(|(key, binding)| (key.as_str() == name).then_some(&binding.value))
    }
}

#[derive(Debug, Clone)]
pub struct McpgCompilation {
    pub config: McpgConfig,
    pub launch_env: McpgLaunchEnvironment,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bindings_are_sorted_and_identical_values_deduplicate() {
        let source = AdoVariableName::parse("PIPELINE_TOKEN").unwrap();
        let mut env = McpgLaunchEnvironment::default();
        env.bind_pipeline_variable("Z_TOKEN", &source, "z").unwrap();
        env.bind_pipeline_variable("A_TOKEN", &source, "a").unwrap();
        env.bind_pipeline_variable("A_TOKEN", &source, "same")
            .unwrap();
        assert_eq!(env.names().collect::<Vec<_>>(), vec!["A_TOKEN", "Z_TOKEN"]);
    }

    #[test]
    fn conflicting_sources_fail_with_origins() {
        let first = AdoVariableName::parse("FIRST").unwrap();
        let second = AdoVariableName::parse("SECOND").unwrap();
        let mut env = McpgLaunchEnvironment::default();
        env.bind_pipeline_variable("TOKEN", &first, "mcp-servers.one")
            .unwrap();
        let error = env
            .bind_pipeline_variable("TOKEN", &second, "mcp-servers.two")
            .unwrap_err()
            .to_string();
        assert!(error.contains("mcp-servers.one"));
        assert!(error.contains("mcp-servers.two"));
    }

    #[test]
    fn invalid_destination_fails() {
        let source = AdoVariableName::parse("TOKEN").unwrap();
        let error = McpgLaunchEnvironment::default()
            .bind_pipeline_variable("BAD-NAME", &source, "mcp-servers.one")
            .unwrap_err()
            .to_string();
        assert!(error.contains("[A-Za-z_][A-Za-z0-9_]*"));
    }

    #[test]
    fn internal_destination_names_are_reserved() {
        let source = AdoVariableName::parse("TOKEN").unwrap();
        for destination in [
            "ADO_AW_MCPG_INTERNAL_IMAGE",
            "MCP_GATEWAY_API_KEY",
            "MCP_GATEWAY_PORT",
            "MCP_GATEWAY_DOMAIN",
            "ADO_PROXY_IP",
            "MCPG_IMAGE",
            "MCPG_ENV_NAMES",
        ] {
            let error = McpgLaunchEnvironment::default()
                .bind_pipeline_variable(destination, &source, "mcp-servers.one")
                .unwrap_err()
                .to_string();
            assert!(error.contains("reserved"));
        }
    }
}
