// ─── Azure DevOps MCP ────────────────────────────────────────────────

use crate::compile::extensions::{
    CompileContext, CompilerExtension, ExtensionPhase, McpgServerConfig, PipelineEnvMapping,
};
use crate::allowed_hosts::mcp_required_hosts;
use crate::compile::{
    ADO_MCP_ENTRYPOINT, ADO_MCP_IMAGE, ADO_MCP_PACKAGE, ADO_MCP_SERVER_NAME,
};
use crate::compile::types::AzureDevOpsToolConfig;
use super::{ADO_MCP_PIPELINE_VAR, ADO_MCP_TOKEN_VAR, ADO_RESOURCE_ID};
use anyhow::Result;
use std::collections::BTreeMap;

/// Azure DevOps first-party tool extension.
///
/// Injects: network hosts (ADO domains), MCPG server entry (containerized
/// ADO MCP), `AzureCLI@2` token acquisition step, and compile-time
/// validation (org inference, service-connection required, duplicate MCP).
pub struct AzureDevOpsExtension {
    config: AzureDevOpsToolConfig,
}

impl AzureDevOpsExtension {
    pub fn new(config: AzureDevOpsToolConfig) -> Self {
        Self {
            config,
        }
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

    fn prepare_steps(&self) -> Vec<String> {
        let sc = match self.config.service_connection() {
            Some(sc) => sc,
            None => return vec![], // validate() will catch this
        };

        // Step 1: Acquire the ADO-scoped token via AzureCLI@2
        let mut acq = Vec::new();
        acq.push("- task: AzureCLI@2".to_string());
        acq.push(format!(
            r#"  displayName: "Acquire ADO token ({})""#,
            ADO_MCP_PIPELINE_VAR
        ));
        acq.push("  inputs:".to_string());
        acq.push(format!(
            "    azureSubscription: '{}'",
            sc.replace('\'', "''")
        ));
        acq.push("    scriptType: 'bash'".to_string());
        acq.push("    scriptLocation: 'inlineScript'".to_string());
        acq.push("    addSpnToEnvironment: true".to_string());
        acq.push("    inlineScript: |".to_string());
        acq.push("      ADO_TOKEN=$(az account get-access-token \\".to_string());
        acq.push(format!("        --resource {} \\", ADO_RESOURCE_ID));
        acq.push("        --query accessToken -o tsv)".to_string());
        acq.push(format!(
            "      echo \"##vso[task.setvariable variable={};issecret=true]$ADO_TOKEN\"",
            ADO_MCP_PIPELINE_VAR
        ));

        // Step 2: Validate the token is read-only by probing a write endpoint
        // with an intentionally empty body. ADO returns:
        //   403 → token is read-only (expected)
        //   400/2xx → token has write access (fail the pipeline)
        let org = self.config.org().unwrap_or("$(ADO_ORG)");
        let mut val = Vec::new();
        val.push("- bash: |".to_string());
        val.push(format!(
            "    TOKEN=\"$({})\"", ADO_MCP_PIPELINE_VAR
        ));
        val.push(format!(
            "    ORG=\"{}\"", org
        ));
        val.push("    PROJECT=\"$(System.TeamProject)\"".to_string());
        val.push(String::new());
        val.push("    # Decode JWT and validate audience".to_string());
        val.push("    PAYLOAD=$(echo \"$TOKEN\" | cut -d '.' -f2 | tr '_-' '/+' | base64 -d 2>/dev/null || true)".to_string());
        val.push("    AUD=$(echo \"$PAYLOAD\" | python3 -c \"import sys,json; print(json.load(sys.stdin).get('aud',''))\" 2>/dev/null || true)".to_string());
        val.push(format!(
            "    if [ \"$AUD\" != \"{}\" ]; then",
            ADO_RESOURCE_ID
        ));
        val.push(format!(
            "      echo \"##vso[task.logissue type=warning]ADO token audience mismatch: expected '{}', got '$AUD'\"\n    fi",
            ADO_RESOURCE_ID
        ));
        val.push(String::new());
        val.push("    # Probe write endpoint with empty body to verify read-only access".to_string());
        val.push("    STATUS=$(curl -s -o /dev/null -w \"%{http_code}\" \\".to_string());
        val.push("      -X POST \"https://dev.azure.com/$ORG/$PROJECT/_apis/wit/workitems/\\$Task?api-version=7.1\" \\".to_string());
        val.push("      -H \"Authorization: Bearer $TOKEN\" \\".to_string());
        val.push("      -H \"Content-Type: application/json-patch+json\" \\".to_string());
        val.push("      -d '[]')".to_string());
        val.push(String::new());
        val.push("    if [ \"$STATUS\" = \"403\" ] || [ \"$STATUS\" = \"401\" ]; then".to_string());
        val.push("      echo \"ADO token verified: read-only access (write probe returned $STATUS)\"".to_string());
        val.push("    elif [ \"$STATUS\" = \"000\" ]; then".to_string());
        val.push("      echo \"##vso[task.logissue type=warning]Could not reach ADO API to verify token (network error)\"".to_string());
        val.push("    else".to_string());
        val.push("      echo \"##vso[task.logissue type=error]ADO token has write access (write probe returned $STATUS). Expected read-only (403).\"".to_string());
        val.push("      echo \"##vso[task.complete result=Failed]ADO service-connection token must be read-only\"".to_string());
        val.push("      exit 1".to_string());
        val.push("    fi".to_string());
        val.push("  displayName: \"Verify ADO token is read-only\"".to_string());
        val.push("  env:".to_string());
        val.push(format!(
            "    {}: $({})", ADO_MCP_PIPELINE_VAR, ADO_MCP_PIPELINE_VAR
        ));

        vec![acq.join("\n"), val.join("\n")]
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
        // Bearer: `-a envvar` reads ADO_MCP_AUTH_TOKEN (pipeline JWT from ARM)
        entrypoint_args.extend(["-a".to_string(), "envvar".to_string()]);

        let env = Some(BTreeMap::from([(
            ADO_MCP_TOKEN_VAR.to_string(),
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

        // Require service-connection
        if self.config.service_connection().is_none() {
            anyhow::bail!(
                "Agent '{}' has tools.azure-devops enabled but no service-connection is configured. \
                 Set tools.azure-devops.service-connection to an ARM service connection for \
                 ADO API access.",
                ctx.agent_name
            );
        }

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
        vec![PipelineEnvMapping {
            container_var: ADO_MCP_TOKEN_VAR.to_string(),
            pipeline_var: ADO_MCP_PIPELINE_VAR.to_string(),
        }]
    }
}
