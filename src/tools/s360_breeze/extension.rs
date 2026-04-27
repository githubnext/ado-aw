// ─── S360 Breeze MCP ──────────────────────────────────────────────────

use crate::compile::extensions::{
    CompileContext, CompilerExtension, ExtensionPhase, McpgConfigReplacement, McpgServerConfig,
    PipelineEnvMapping,
};
use crate::allowed_hosts::mcp_required_hosts;
use crate::compile::types::S360BreezeToolConfig;
use super::{
    S360_CONFIG_PLACEHOLDER, S360_ENDPOINT, S360_PIPELINE_VAR, S360_RESOURCE_ID,
    S360_SERVER_NAME,
};
use anyhow::Result;
use std::collections::BTreeMap;

/// S360 Breeze first-party tool extension.
///
/// Injects: network hosts (S360 + auth domains), MCPG HTTP server entry
/// (Bearer token via config placeholder), `AzureCLI@2` token acquisition
/// step, and compile-time validation (service-connection required).
pub struct S360BreezeExtension {
    config: S360BreezeToolConfig,
}

impl S360BreezeExtension {
    pub fn new(config: S360BreezeToolConfig) -> Self {
        Self { config }
    }
}

impl CompilerExtension for S360BreezeExtension {
    fn name(&self) -> &str {
        "S360 Breeze MCP"
    }

    fn phase(&self) -> ExtensionPhase {
        ExtensionPhase::Tool
    }

    fn required_hosts(&self) -> Vec<String> {
        mcp_required_hosts("s360")
            .iter()
            .map(|h| (*h).to_string())
            .collect()
    }

    fn allowed_copilot_tools(&self) -> Vec<String> {
        vec![S360_SERVER_NAME.to_string()]
    }

    fn prepare_steps(&self) -> Vec<String> {
        // Generate an AzureCLI@2 step to acquire an S360-scoped token
        let sc = match self.config.service_connection() {
            Some(sc) => sc,
            None => return vec![], // validate() will catch this
        };

        let mut lines = Vec::new();
        lines.push("- task: AzureCLI@2".to_string());
        lines.push(format!(
            r#"  displayName: "Acquire S360 token ({})""#,
            S360_PIPELINE_VAR
        ));
        lines.push("  inputs:".to_string());
        lines.push(format!(
            "    azureSubscription: '{}'",
            sc.replace('\'', "''")
        ));
        lines.push("    scriptType: 'bash'".to_string());
        lines.push("    scriptLocation: 'inlineScript'".to_string());
        lines.push("    addSpnToEnvironment: true".to_string());
        lines.push("    inlineScript: |".to_string());
        lines.push("      S360_TOKEN=$(az account get-access-token \\".to_string());
        lines.push(format!("        --resource {} \\", S360_RESOURCE_ID));
        lines.push("        --query accessToken -o tsv)".to_string());
        lines.push(format!(
            "      echo \"##vso[task.setvariable variable={};issecret=true]$S360_TOKEN\"",
            S360_PIPELINE_VAR
        ));

        vec![lines.join("\n")]
    }

    fn mcpg_servers(&self, _ctx: &CompileContext) -> Result<Vec<(String, McpgServerConfig)>> {
        let tools = if self.config.allowed().is_empty() {
            None
        } else {
            Some(self.config.allowed().to_vec())
        };

        Ok(vec![(
            S360_SERVER_NAME.to_string(),
            McpgServerConfig {
                server_type: "http".to_string(),
                container: None,
                entrypoint: None,
                entrypoint_args: None,
                mounts: None,
                args: None,
                url: Some(S360_ENDPOINT.to_string()),
                headers: Some(BTreeMap::from([(
                    "Authorization".to_string(),
                    format!("Bearer ${{{}}}", S360_CONFIG_PLACEHOLDER),
                )])),
                env: None,
                tools,
            },
        )])
    }

    fn validate(&self, ctx: &CompileContext) -> Result<Vec<String>> {
        let mut warnings = Vec::new();

        // Require service-connection
        if self.config.service_connection().is_none() {
            anyhow::bail!(
                "Agent '{}' has tools.s360-breeze enabled but no service-connection is configured. \
                 Set tools.s360-breeze.service-connection to an ARM service connection whose \
                 service principal is approved for S360 MCP access in the CORP tenant.",
                ctx.agent_name
            );
        }

        // Warn if user also has a manual mcp-servers entry for s360-breeze
        if ctx
            .front_matter
            .mcp_servers
            .contains_key(S360_SERVER_NAME)
        {
            warnings.push(format!(
                "Agent '{}' has both tools.s360-breeze and mcp-servers.s360-breeze configured. \
                 The tools.s360-breeze auto-configuration takes precedence. \
                 Remove the mcp-servers entry to silence this warning.",
                ctx.agent_name
            ));
        }

        Ok(warnings)
    }

    fn required_pipeline_vars(&self) -> Vec<PipelineEnvMapping> {
        vec![PipelineEnvMapping {
            container_var: S360_CONFIG_PLACEHOLDER.to_string(),
            pipeline_var: S360_PIPELINE_VAR.to_string(),
        }]
    }

    fn mcpg_config_replacements(&self) -> Vec<McpgConfigReplacement> {
        vec![McpgConfigReplacement {
            placeholder: S360_CONFIG_PLACEHOLDER.to_string(),
            pipeline_var: S360_PIPELINE_VAR.to_string(),
        }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::parse_markdown;

    fn minimal_fm() -> crate::compile::types::FrontMatter {
        let (fm, _) = parse_markdown("---\nname: test-agent\ndescription: test\n---\n").unwrap();
        fm
    }

    #[test]
    fn test_s360_extension_hosts() {
        let config = S360BreezeToolConfig::Enabled(true);
        let ext = S360BreezeExtension::new(config);
        let hosts = ext.required_hosts();
        assert!(
            hosts.iter().any(|h| h.contains("s360")),
            "Should include S360 domains"
        );
    }

    #[test]
    fn test_s360_extension_mcpg_config() {
        let config = S360BreezeToolConfig::Enabled(true);
        let ext = S360BreezeExtension::new(config);
        let fm = minimal_fm();
        let ctx = CompileContext::for_test(&fm);
        let servers = ext.mcpg_servers(&ctx).unwrap();
        assert_eq!(servers.len(), 1);
        let (name, cfg) = &servers[0];
        assert_eq!(name, S360_SERVER_NAME);
        assert_eq!(cfg.server_type, "http");
        assert_eq!(cfg.url.as_deref(), Some(S360_ENDPOINT));
        let headers = cfg.headers.as_ref().unwrap();
        assert_eq!(
            headers.get("Authorization").unwrap(),
            &format!("Bearer ${{{}}}", S360_CONFIG_PLACEHOLDER)
        );
    }

    #[test]
    fn test_s360_extension_validate_no_service_connection() {
        let config = S360BreezeToolConfig::Enabled(true);
        let ext = S360BreezeExtension::new(config);
        let fm = minimal_fm();
        let ctx = CompileContext::for_test(&fm);
        let result = ext.validate(&ctx);
        assert!(result.is_err(), "Should fail without service-connection");
        assert!(
            result.unwrap_err().to_string().contains("service-connection"),
            "Error should mention service-connection"
        );
    }

    #[test]
    fn test_s360_extension_validate_with_service_connection() {
        use crate::compile::types::S360BreezeOptions;
        let config = S360BreezeToolConfig::WithOptions(S360BreezeOptions {
            service_connection: Some("my-s360-sc".to_string()),
            allowed: vec![],
        });
        let ext = S360BreezeExtension::new(config);
        let fm = minimal_fm();
        let ctx = CompileContext::for_test(&fm);
        let result = ext.validate(&ctx).unwrap();
        assert!(result.is_empty(), "Should pass with service-connection");
    }

    #[test]
    fn test_s360_extension_prepare_steps() {
        use crate::compile::types::S360BreezeOptions;
        let config = S360BreezeToolConfig::WithOptions(S360BreezeOptions {
            service_connection: Some("my-s360-sc".to_string()),
            allowed: vec![],
        });
        let ext = S360BreezeExtension::new(config);
        let steps = ext.prepare_steps();
        assert_eq!(steps.len(), 1, "Should generate one AzureCLI step");
        assert!(steps[0].contains("AzureCLI@2"));
        assert!(steps[0].contains(S360_RESOURCE_ID));
        assert!(steps[0].contains(S360_PIPELINE_VAR));
        assert!(steps[0].contains("my-s360-sc"));
    }

    #[test]
    fn test_s360_extension_config_replacements() {
        let config = S360BreezeToolConfig::Enabled(true);
        let ext = S360BreezeExtension::new(config);
        let replacements = ext.mcpg_config_replacements();
        assert_eq!(replacements.len(), 1);
        assert_eq!(replacements[0].placeholder, S360_CONFIG_PLACEHOLDER);
        assert_eq!(replacements[0].pipeline_var, S360_PIPELINE_VAR);
    }

    #[test]
    fn test_s360_extension_with_tool_allowlist() {
        use crate::compile::types::S360BreezeOptions;
        let config = S360BreezeToolConfig::WithOptions(S360BreezeOptions {
            service_connection: Some("sc".to_string()),
            allowed: vec!["search_s360_kpi_metadata".to_string()],
        });
        let ext = S360BreezeExtension::new(config);
        let fm = minimal_fm();
        let ctx = CompileContext::for_test(&fm);
        let servers = ext.mcpg_servers(&ctx).unwrap();
        let tools = servers[0].1.tools.as_ref().unwrap();
        assert_eq!(tools, &["search_s360_kpi_metadata"]);
    }
}
