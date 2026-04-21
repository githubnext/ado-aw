// ─── Azure DevOps MCP ────────────────────────────────────────────────

use super::{
    CompileContext, CompilerExtension, ExtensionPhase, McpgServerConfig, PipelineEnvMapping,
};
use crate::allowed_hosts::mcp_required_hosts;
use crate::compile::common::{
    ADO_MCP_ENTRYPOINT, ADO_MCP_IMAGE, ADO_MCP_PACKAGE, ADO_MCP_SERVER_NAME,
};
use crate::compile::types::AzureDevOpsToolConfig;
use anyhow::Result;
use std::collections::HashMap;

/// Azure DevOps first-party tool extension.
///
/// Injects: network hosts (ADO domains), MCPG server entry (containerized
/// ADO MCP), and compile-time validation (org inference, duplicate MCP).
pub struct AzureDevOpsExtension {
    config: AzureDevOpsToolConfig,
    auth_mode: AdoAuthMode,
}

/// Authentication mode for the ADO MCP server.
///
/// Pipelines use bearer tokens (JWT from ARM service connections).
/// Local development uses PATs (Personal Access Tokens).
#[derive(Debug, Clone, Copy, Default)]
pub enum AdoAuthMode {
    /// `-a envvar` + `ADO_MCP_AUTH_TOKEN` — bearer JWT from ARM (pipeline default)
    #[default]
    Bearer,
    /// `-a pat` + `AZURE_DEVOPS_EXT_PAT` — Personal Access Token (local dev)
    Pat,
}

impl AzureDevOpsExtension {
    pub fn new(config: AzureDevOpsToolConfig) -> Self {
        Self {
            config,
            auth_mode: AdoAuthMode::default(),
        }
    }

    /// Set the authentication mode (e.g., `AdoAuthMode::Pat` for local runs).
    pub fn with_auth_mode(mut self, mode: AdoAuthMode) -> Self {
        self.auth_mode = mode;
        self
    }
}

impl CompilerExtension for AzureDevOpsExtension {
    fn name(&self) -> &str {
        "Azure DevOps MCP"
    }

    fn phase(&self) -> ExtensionPhase {
        ExtensionPhase::Tool
    }

    fn required_hosts(&self) -> Vec<String> {
        let mut hosts: Vec<String> = mcp_required_hosts("ado")
            .iter()
            .map(|h| (*h).to_string())
            .collect();
        // The ADO MCP runs in a container via `npx -y @azure-devops/mcp`.
        // npx needs npm registry access to resolve and install the package.
        hosts.push("node".to_string());
        hosts
    }

    fn allowed_copilot_tools(&self) -> Vec<String> {
        vec![ADO_MCP_SERVER_NAME.to_string()]
    }

    fn mcpg_servers(&self, ctx: &CompileContext) -> Result<Vec<(String, McpgServerConfig)>> {
        // Build entrypoint args: npx -y @azure-devops/mcp <org> [-d toolset1 toolset2 ...]
        let mut entrypoint_args = vec!["-y".to_string(), ADO_MCP_PACKAGE.to_string()];

        // Org: use explicit override, then inferred from git remote, then fail
        let org = self
            .config
            .org()
            .map(|s| s.to_string())
            .or_else(|| ctx.ado_org().map(|s| s.to_string()))
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Agent '{}' has tools.azure-devops enabled but no ADO organization could be \
                     determined. Either set tools.azure-devops.org explicitly, or compile from \
                     within a git repository with an Azure DevOps remote URL.",
                    ctx.agent_name
                )
            })?;
        if !org.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            anyhow::bail!(
                "Invalid ADO org name '{}': must contain only alphanumerics and hyphens",
                org
            );
        }
        entrypoint_args.push(org);

        // Toolsets: passed as -d flag followed by space-separated toolset names
        if !self.config.toolsets().is_empty() {
            entrypoint_args.push("-d".to_string());
            for toolset in self.config.toolsets() {
                if !toolset
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-')
                {
                    anyhow::bail!(
                        "Invalid ADO toolset name '{}': must contain only alphanumerics and hyphens",
                        toolset
                    );
                }
                entrypoint_args.push(toolset.clone());
            }
        }

        // Tool allow-list for MCPG filtering
        let tools = if self.config.allowed().is_empty() {
            None
        } else {
            Some(self.config.allowed().to_vec())
        };

        // ADO MCP authentication: the @azure-devops/mcp npm package accepts
        // auth type via CLI arg (-a) and token via env var.
        //   Bearer: `-a envvar` reads ADO_MCP_AUTH_TOKEN (pipeline JWT from ARM)
        //   Pat:    `-a pat`    reads PERSONAL_ACCESS_TOKEN (base64-encoded PAT)
        let (auth_flag, token_var) = match self.auth_mode {
            AdoAuthMode::Bearer => ("envvar", "ADO_MCP_AUTH_TOKEN"),
            AdoAuthMode::Pat => ("pat", "PERSONAL_ACCESS_TOKEN"),
        };
        entrypoint_args.extend(["-a".to_string(), auth_flag.to_string()]);

        let env = Some(HashMap::from([(
            token_var.to_string(),
            String::new(), // Passthrough from MCPG process env
        )]));

        // --network host: AWF's DOCKER-USER iptables rules block outbound from
        // containers on Docker's default bridge. Host networking bypasses FORWARD
        // chain rules so the ADO MCP can reach dev.azure.com.
        // This matches gh-aw's approach for its built-in agentic-workflows MCP.
        let args = Some(vec!["--network".to_string(), "host".to_string()]);

        Ok(vec![(
            ADO_MCP_SERVER_NAME.to_string(),
            McpgServerConfig {
                server_type: "stdio".to_string(),
                container: Some(ADO_MCP_IMAGE.to_string()),
                entrypoint: Some(ADO_MCP_ENTRYPOINT.to_string()),
                entrypoint_args: Some(entrypoint_args),
                mounts: None,
                args,
                url: None,
                headers: None,
                env,
                tools,
            },
        )])
    }

    fn validate(&self, ctx: &CompileContext) -> Result<Vec<String>> {
        let mut warnings = Vec::new();

        // Warn if user also has a manual mcp-servers entry for azure-devops
        if ctx
            .front_matter
            .mcp_servers
            .contains_key(ADO_MCP_SERVER_NAME)
        {
            warnings.push(format!(
                "Agent '{}' has both tools.azure-devops and mcp-servers.azure-devops configured. \
                 The tools.azure-devops auto-configuration takes precedence. \
                 Remove the mcp-servers entry to silence this warning.",
                ctx.agent_name
            ));
        }

        Ok(warnings)
    }
    fn required_pipeline_vars(&self) -> Vec<PipelineEnvMapping> {
        match self.auth_mode {
            AdoAuthMode::Bearer => vec![PipelineEnvMapping {
                container_var: "ADO_MCP_AUTH_TOKEN".to_string(),
                pipeline_var: "SC_READ_TOKEN".to_string(),
            }],
            // PAT mode: no pipeline var mapping needed — the PAT is passed
            // directly via AZURE_DEVOPS_EXT_PAT in the MCPG env file.
            AdoAuthMode::Pat => vec![],
        }
    }
}
