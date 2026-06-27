//! Core allowed hosts for network isolation.
//!
//! This module provides the definitive list of hosts required for Azure DevOps
//! agents, GitHub Copilot, and related services. This list is shared between:
//! - AWF domain allowlists (standalone pipeline compiler)
//! - The network proxy (runtime HTTP filtering, used by Detection in legacy mode)

/// Core hosts required for Azure DevOps agent operation.
/// These are always included in any network allowlist.
pub static CORE_ALLOWED_HOSTS: &[&str] = &[
    // ===== Azure DevOps =====
    "dev.azure.com",
    "*.dev.azure.com",
    "vstoken.dev.azure.com",
    "*.visualstudio.com",
    "*.vsassets.io",
    "*.vsblob.visualstudio.com",
    "*.vssps.visualstudio.com",
    "vssps.dev.azure.com",
    // Azure DevOps Artifacts / NuGet
    "pkgs.dev.azure.com",
    "*.pkgs.dev.azure.com",
    // Azure DevOps CDN
    "aex.dev.azure.com",
    "aexus.dev.azure.com",
    "vsrm.dev.azure.com",
    "*.vsrm.dev.azure.com",
    // ===== GitHub (for Copilot / Agency) =====
    "github.com",
    "api.github.com",
    "*.githubusercontent.com",
    "*.github.com",
    "*.copilot.github.com",
    "*.githubcopilot.com",
    "copilot-proxy.githubusercontent.com",
    // ===== Microsoft Identity / Authentication =====
    "login.microsoftonline.com",
    "login.live.com",
    "login.windows.net",
    "*.msauth.net",
    "*.msauthimages.net",
    "*.msftauth.net",
    "graph.microsoft.com",
    "management.azure.com",
    // ===== Azure Storage (artifacts, logs) =====
    "*.blob.core.windows.net",
    "*.table.core.windows.net",
    "*.queue.core.windows.net",
    // ===== Telemetry / App Insights =====
    "*.applicationinsights.azure.com",
    "*.in.applicationinsights.azure.com",
    "dc.services.visualstudio.com",
    "rt.services.visualstudio.com",
    // ===== Agency / Copilot configuration =====
    "config.edge.skype.com",
    // Note: 168.63.129.16 (Azure DNS) is handled separately as it's an IP
    // Note: host.docker.internal is NOT in CORE — it's always added by the
    // standalone compiler in generate_allowed_domains (standalone always uses
    // MCPG, which needs host access from the AWF container).
];

/// Hosts required by specific MCP servers.
/// Returns empty slice for unknown MCPs - they must specify their own hosts.
pub fn mcp_required_hosts(mcp_name: &str) -> &'static [&'static str] {
    match mcp_name {
        // Azure DevOps MCP (consumed by the always-on azure-devops tool
        // extension, not a user-facing `mcp-servers:` key).
        "ado" | "ado-ext" => &[
            // Already covered by core, but explicit for clarity
            "dev.azure.com",
            "*.dev.azure.com",
            "*.visualstudio.com",
            "vssps.dev.azure.com",
        ],

        // Unknown MCP - return empty, user must specify hosts via
        // `network.allowed`.
        _ => &[],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_core_hosts_includes_azure_devops() {
        assert!(CORE_ALLOWED_HOSTS.contains(&"dev.azure.com"));
        assert!(CORE_ALLOWED_HOSTS.contains(&"*.dev.azure.com"));
        assert!(CORE_ALLOWED_HOSTS.contains(&"github.com"));
    }

    #[test]
    fn test_mcp_hosts_ado() {
        let hosts = mcp_required_hosts("ado");
        assert!(hosts.contains(&"dev.azure.com"));
    }

    #[test]
    fn test_mcp_hosts_internal_services_removed() {
        // Internal Microsoft service identifiers (kusto, icm, bluebird,
        // es-chat, msft-learn, asa, stack, calculator, github) are no longer
        // special-cased — they must declare hosts via `network.allowed`.
        for name in [
            "kusto",
            "icm",
            "bluebird",
            "es-chat",
            "msft-learn",
            "asa",
            "stack",
            "calculator",
            "github",
        ] {
            assert!(
                mcp_required_hosts(name).is_empty(),
                "{name} should no longer auto-add hosts"
            );
        }
    }

}
