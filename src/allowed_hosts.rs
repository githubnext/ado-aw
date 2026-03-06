//! Core allowed hosts for network isolation.
//!
//! This module provides the definitive list of hosts required for Azure DevOps
//! agents, GitHub Copilot, and related services. This list is shared between:
//! - AWF domain allowlists (standalone pipeline compiler)
//! - The network proxy (runtime HTTP filtering, used by AnalyzeSafeOutputs in legacy mode)

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
];

/// Hosts required by specific MCP servers.
/// Returns empty slice for unknown MCPs - they must specify their own hosts.
pub fn mcp_required_hosts(mcp_name: &str) -> &'static [&'static str] {
    match mcp_name {
        // Azure DevOps MCP
        "ado" | "ado-ext" => &[
            // Already covered by core, but explicit for clarity
            "dev.azure.com",
            "*.dev.azure.com",
            "*.visualstudio.com",
            "vssps.dev.azure.com",
        ],

        // Kusto (Azure Data Explorer) MCP
        "kusto" => &[
            "*.kusto.windows.net",
            "*.kusto.azure.com",
            "*.kustomfa.windows.net",
            "kusto.azure.com",
        ],

        // IcM (Incident Management) MCP
        "icm" => &[
            "icm.ad.msft.net",
            "prod.microsofticm.com",
            "*.microsofticm.com",
        ],

        // Bluebird MCP (internal Microsoft service)
        "bluebird" => &["bluebird.microsoft.com", "*.bluebird.microsoft.com"],

        // ES Chat MCP (internal Microsoft service)
        "es-chat" => &["es-chat.microsoft.com", "*.es-chat.microsoft.com"],

        // Microsoft Learn / Docs MCP
        "msft-learn" => &[
            "learn.microsoft.com",
            "docs.microsoft.com",
            "*.learn.microsoft.com",
        ],

        // ASA (Azure Stream Analytics / internal service) MCP
        "asa" => &["*.azure.com", "asa.azure.com"],

        // Stack MCP (internal)
        "stack" => &["stack.microsoft.com", "*.stack.microsoft.com"],

        // Calculator MCP - no network needed
        "calculator" => &[],

        // GitHub MCP (for non-Copilot GitHub access)
        "github" => &["api.github.com", "github.com", "*.githubusercontent.com"],

        // Unknown MCP - return empty, user must specify hosts
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
    fn test_mcp_hosts_kusto() {
        let hosts = mcp_required_hosts("kusto");
        assert!(hosts.contains(&"*.kusto.windows.net"));
    }

    #[test]
    fn test_mcp_hosts_unknown_returns_empty() {
        let hosts = mcp_required_hosts("unknown-mcp");
        assert!(hosts.is_empty());
    }
}
