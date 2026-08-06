// ─── Azure DevOps MCP ────────────────────────────────────────────────

use crate::ado_proxy::catalog::ORGANIZATION_HOST;
use crate::compile::extensions::{
    AddHost, CompileContext, CompilerExtension, ContainerRuntimeConfig, Declarations,
    ExtensionPhase, McpgServerConfig, Mount, Network,
};
use crate::compile::types::AzureDevOpsToolConfig;
use crate::compile::{
    ADO_MCP_CA_MOUNT, ADO_MCP_ENTRY_SCRIPT, ADO_MCP_ENTRYPOINT, ADO_MCP_HOST_NODE_MODULES,
    ADO_MCP_IMAGE, ADO_MCP_NODE_MODULES, ADO_MCP_SERVER_NAME, ADO_MCP_TOKEN_SENTINEL,
    ADO_PROXY_NETWORK_NAME, ADO_PROXY_PUBLIC_CA_HOST_PATH,
};
use anyhow::Result;
use std::collections::BTreeMap;

/// Azure DevOps first-party tool extension.
///
/// Injects: network hosts (ADO domains), MCPG server entry (containerized
/// ADO MCP), and compile-time validation (org inference, duplicate MCP).
pub struct AzureDevOpsExtension {
    config: AzureDevOpsToolConfig,
}

impl AzureDevOpsExtension {
    pub fn new(config: AzureDevOpsToolConfig) -> Self {
        Self { config }
    }
}

impl CompilerExtension for AzureDevOpsExtension {
    fn name(&self) -> &str {
        "Azure DevOps MCP"
    }

    fn phase(&self) -> ExtensionPhase {
        ExtensionPhase::Tool
    }

    /// Typed-IR view. Azure DevOps MCP contributes only static
    /// signals — no pipeline steps.
    fn declarations(&self, ctx: &CompileContext) -> Result<Declarations> {
        // The MCP no longer reaches Azure DevOps itself: it is redirected at
        // the policy engine, which holds the credential. The engine's own
        // egress goes through Squid, so the hosts the MCP would otherwise
        // need are not required here. `node` is likewise gone — the package
        // is installed on the runner and mounted in, so the container
        // resolves nothing at start time.
        let hosts: Vec<String> = Vec::new();

        // Launch the package directly. `npx` would need registry access from
        // inside a container that, by design, can reach nothing but the engine.
        let mut entrypoint_args = vec![ADO_MCP_ENTRY_SCRIPT.to_string()];

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
        // auth type via CLI arg (-a) and token via env var. Under interception
        // the value is a sentinel — the engine injects the real bearer only
        // after a complete allow decision.
        entrypoint_args.extend(["-a".to_string(), "envvar".to_string()]);

        let env = Some(BTreeMap::from([
            (
                "ADO_MCP_AUTH_TOKEN".to_string(),
                ADO_MCP_TOKEN_SENTINEL.to_string(),
            ),
            // Trust is scoped to this container rather than installed
            // system-wide: it is an availability control, not a security one.
            // Enforcement comes from routing — Squid denies the protected
            // hosts, so a client that declines this certificate fails closed
            // instead of escaping the policy.
            (
                "NODE_EXTRA_CA_CERTS".to_string(),
                ADO_MCP_CA_MOUNT.to_string(),
            ),
        ]));

        // Mount the pre-installed package and the *public* CA certificate.
        // The CA private key is never mounted anywhere; it is destroyed by the
        // step that starts the engine.
        // Join the engine's network and redirect the Azure DevOps host at it.
        // `--add-host` is what makes the redirection total: it catches both
        // `node:https` and global `fetch`, so the MCP's raw `fetch()` call
        // sites cannot slip past it the way proxy environment variables would.
        // `ADO_PROXY_IP` is resolved at pipeline time and substituted into the
        // MCPG config by the step that starts the engine.
        let runtime = ContainerRuntimeConfig::builder()
            .mount(Mount::read_only(
                ADO_MCP_HOST_NODE_MODULES,
                ADO_MCP_NODE_MODULES,
            )?)
            .mount(Mount::read_only(
                ADO_PROXY_PUBLIC_CA_HOST_PATH,
                ADO_MCP_CA_MOUNT,
            )?)
            .network(Network::named(ADO_PROXY_NETWORK_NAME)?)
            .add_host(AddHost::new(ORGANIZATION_HOST, "${ADO_PROXY_IP}")?)
            .build()?;

        let mcpg_servers = vec![(
            ADO_MCP_SERVER_NAME.to_string(),
            McpgServerConfig {
                server_type: "stdio".to_string(),
                container: Some(ADO_MCP_IMAGE.to_string()),
                entrypoint: Some(ADO_MCP_ENTRYPOINT.to_string()),
                entrypoint_args: Some(entrypoint_args),
                runtime,
                url: None,
                headers: None,
                env,
                tools,
            },
        )];

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

        Ok(Declarations {
            network_hosts: hosts,
            mcpg_servers,
            copilot_allow_tools: vec![ADO_MCP_SERVER_NAME.to_string()],
            // Deliberately empty. This previously mapped
            // ADO_MCP_AUTH_TOKEN -> SC_READ_TOKEN, handing the MCP container a
            // real Azure DevOps credential. Under interception the engine holds
            // the only copy; the MCP gets a sentinel.
            pipeline_env: Vec::new(),
            warnings,
            ..Declarations::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::extensions::MountMode;
    use crate::compile::parse_markdown;

    #[test]
    fn declarations_returns_static_signals_only_no_steps() {
        let (fm, _) = parse_markdown(
            "---\nname: t\ndescription: x\ntools:\n  azure-devops:\n    org: 'myorg'\n---\n",
        )
        .unwrap();
        let cfg = fm
            .tools
            .as_ref()
            .and_then(|t| t.azure_devops.as_ref())
            .cloned()
            .unwrap();
        let ext = AzureDevOpsExtension::new(cfg);
        let ctx = CompileContext::for_test(&fm);
        let decl = ext.declarations(&ctx).unwrap();

        // No steps - this extension only contributes MCPG + env wiring.
        assert!(decl.agent_prepare_steps.is_empty());
        assert!(decl.setup_steps.is_empty());

        // copilot_allow_tools contains the ADO MCP server name.
        assert_eq!(
            decl.copilot_allow_tools,
            vec![ADO_MCP_SERVER_NAME.to_string()]
        );

        // mcpg_servers has one stdio entry for the ADO MCP container.
        assert_eq!(decl.mcpg_servers.len(), 1);
        let (name, config) = &decl.mcpg_servers[0];
        assert_eq!(name, ADO_MCP_SERVER_NAME);
        assert_eq!(config.server_type, "stdio");
        assert_eq!(config.container.as_deref(), Some(ADO_MCP_IMAGE));

        // The MCP must never receive a real Azure DevOps credential: the
        // policy engine holds the only copy and injects it after an allow
        // decision. This is the whole point of routing it through the proxy.
        assert!(
            decl.pipeline_env.is_empty(),
            "no pipeline variable may be projected into the MCP container: {:?}",
            decl.pipeline_env
        );
        let env = config.env.as_ref().expect("env is set");
        assert_eq!(
            env.get("ADO_MCP_AUTH_TOKEN").map(String::as_str),
            Some(ADO_MCP_TOKEN_SENTINEL)
        );

        // Nothing is fetched at start time, so the container needs no hosts.
        assert!(
            decl.network_hosts.is_empty(),
            "the MCP reaches only the policy engine: {:?}",
            decl.network_hosts
        );
    }

    fn config_for(markdown: &str) -> (crate::compile::types::FrontMatter, AzureDevOpsToolConfig) {
        let (fm, _) = parse_markdown(markdown).unwrap();
        let cfg = fm
            .tools
            .as_ref()
            .and_then(|t| t.azure_devops.as_ref())
            .cloned()
            .unwrap();
        (fm, cfg)
    }

    const MINIMAL: &str =
        "---\nname: t\ndescription: x\ntools:\n  azure-devops:\n    org: 'myorg'\n---\n";

    #[test]
    fn mcp_is_launched_directly_rather_than_resolved_at_start_time() {
        let (fm, cfg) = config_for(MINIMAL);
        let ctx = CompileContext::for_test(&fm);
        let decl = AzureDevOpsExtension::new(cfg).declarations(&ctx).unwrap();
        let (_, config) = &decl.mcpg_servers[0];

        // `npx` would need registry access from a container that, by design,
        // can reach nothing but the policy engine.
        assert_eq!(config.entrypoint.as_deref(), Some("node"));
        let args = config.entrypoint_args.as_ref().unwrap();
        assert_eq!(args[0], ADO_MCP_ENTRY_SCRIPT);
        assert!(!args.iter().any(|a| a == "-y"));
    }

    #[test]
    fn mcp_is_redirected_at_the_policy_engine() {
        let (fm, cfg) = config_for(MINIMAL);
        let ctx = CompileContext::for_test(&fm);
        let decl = AzureDevOpsExtension::new(cfg).declarations(&ctx).unwrap();
        let (_, config) = &decl.mcpg_servers[0];
        let args = config.runtime.args();

        // Host networking would put the MCP on the runner's own stack, where
        // it could reach Azure DevOps directly and bypass the policy entirely.
        assert!(
            !args.iter().any(|a| a == "host"),
            "the MCP must not use host networking: {args:?}"
        );
        assert!(args.windows(2).any(|w| w == ["--network", ADO_PROXY_NETWORK_NAME]));
        assert!(
            args.iter()
                .any(|a| a == &format!("{ORGANIZATION_HOST}:${{ADO_PROXY_IP}}")),
            "the Azure DevOps host must resolve to the engine: {args:?}"
        );
        assert_eq!(
            args,
            [
                "--network",
                ADO_PROXY_NETWORK_NAME,
                "--add-host",
                &format!("{ORGANIZATION_HOST}:${{ADO_PROXY_IP}}"),
            ]
        );
    }

    #[test]
    fn mcp_mounts_the_package_and_only_the_public_certificate() {
        let (fm, cfg) = config_for(MINIMAL);
        let ctx = CompileContext::for_test(&fm);
        let decl = AzureDevOpsExtension::new(cfg).declarations(&ctx).unwrap();
        let (_, config) = &decl.mcpg_servers[0];
        let mounts = config.runtime.mounts();
        assert_eq!(mounts.len(), 2, "only package tree and public CA are allowed");

        // Node resolves dependencies by walking upward from the importing
        // file, so this path is load-bearing: mounted elsewhere, the MCP's own
        // imports fail with ERR_MODULE_NOT_FOUND.
        assert!(
            mounts.iter().any(|m| {
                m.source() == ADO_MCP_HOST_NODE_MODULES
                    && m.destination() == ADO_MCP_NODE_MODULES
                    && m.mode() == MountMode::ReadOnly
            }),
            "{mounts:?}"
        );
        assert!(
            mounts.iter().all(|m| m.mode() == MountMode::ReadOnly),
            "{mounts:?}"
        );
        assert!(
            mounts.iter().any(|m| m.destination() == ADO_MCP_CA_MOUNT),
            "the MCP must trust the interception certificate: {mounts:?}"
        );
        assert!(
            !mounts.iter().any(|m| {
                [m.source(), m.destination()].iter().any(|path| {
                    path.contains(".key")
                        || path.to_ascii_lowercase().contains("token")
                        || path.to_ascii_lowercase().contains("credential")
                })
            }),
            "keys and tokens must never be mounted: {mounts:?}"
        );
        let env = config.env.as_ref().unwrap();
        assert_eq!(
            env.get("NODE_EXTRA_CA_CERTS").map(String::as_str),
            Some(ADO_MCP_CA_MOUNT)
        );
        assert_eq!(
            serde_json::to_value(mounts).unwrap(),
            serde_json::json!([
                format!("{ADO_MCP_HOST_NODE_MODULES}:{ADO_MCP_NODE_MODULES}:ro"),
                format!("{ADO_PROXY_PUBLIC_CA_HOST_PATH}:{ADO_MCP_CA_MOUNT}:ro")
            ])
        );
    }

    #[test]
    fn the_sentinel_is_not_a_credential() {
        // It appears in logs and error messages, so it must read as
        // deliberate rather than as a leaked or malformed token.
        assert!(ADO_MCP_TOKEN_SENTINEL.contains("ado-proxy"));
        assert!(!ADO_MCP_TOKEN_SENTINEL.is_empty());
    }
}
