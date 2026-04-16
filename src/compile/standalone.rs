//! Standalone pipeline compiler.
//!
//! This compiler generates a self-contained Azure DevOps pipeline with:
//! - Full 3-job pipeline: PerformAgenticTask → AnalyzeSafeOutputs → ProcessSafeOutputs
//! - AWF (Agentic Workflow Firewall) L7 domain whitelisting via Squid proxy + Docker
//! - MCP firewall with tool-level filtering and custom MCP server support
//! - Setup/teardown job support

use anyhow::{Context, Result};
use async_trait::async_trait;
use log::info;
use std::path::Path;

use super::Compiler;
use super::common::{
    AWF_VERSION, MCPG_VERSION, MCPG_IMAGE,
    CompileConfig, compile_shared,
    generate_cancel_previous_builds,
    generate_enabled_tools_args,
    generate_mcpg_config, generate_mcpg_docker_env,
};
use super::extensions::CompilerExtension;
use super::types::{FrontMatter, McpConfig};
use crate::allowed_hosts::{CORE_ALLOWED_HOSTS, mcp_required_hosts};
use crate::ecosystem_domains::{get_ecosystem_domains, is_ecosystem_identifier, is_known_ecosystem};
use std::collections::HashSet;

/// Standalone pipeline compiler.
pub struct StandaloneCompiler;

#[async_trait]
impl Compiler for StandaloneCompiler {
    fn target_name(&self) -> &'static str {
        "standalone"
    }

    async fn compile(
        &self,
        input_path: &Path,
        output_path: &Path,
        front_matter: &FrontMatter,
        markdown_body: &str,
    ) -> Result<String> {
        info!("Compiling for standalone target");

        // Collect extensions (needed before compile_shared for MCPG config)
        let extensions = super::extensions::collect_extensions(front_matter);

        // Build compile context for MCPG config generation
        let input_dir = input_path.parent().unwrap_or(std::path::Path::new("."));
        let ctx = super::extensions::CompileContext::new(front_matter, input_dir).await;

        // Standalone-specific values
        let allowed_domains = generate_allowed_domains(front_matter, &extensions)?;
        let enabled_tools_args = generate_enabled_tools_args(front_matter);
        let cancel_previous_builds = generate_cancel_previous_builds(&front_matter.triggers);

        let config_obj = generate_mcpg_config(front_matter, &ctx, &extensions)?;
        let mcpg_config_json =
            serde_json::to_string_pretty(&config_obj).context("Failed to serialize MCPG config")?;
        let mcpg_docker_env = generate_mcpg_docker_env(front_matter);

        let config = CompileConfig {
            template: include_str!("../../templates/base.yml").to_string(),
            extra_replacements: vec![
                ("{{ firewall_version }}".into(), AWF_VERSION.into()),
                ("{{ mcpg_version }}".into(), MCPG_VERSION.into()),
                ("{{ mcpg_image }}".into(), MCPG_IMAGE.into()),
                ("{{ allowed_domains }}".into(), allowed_domains),
                ("{{ enabled_tools_args }}".into(), enabled_tools_args),
                ("{{ cancel_previous_builds }}".into(), cancel_previous_builds),
                ("{{ mcpg_config }}".into(), mcpg_config_json),
                ("{{ mcpg_docker_env }}".into(), mcpg_docker_env),
            ],
        };

        compile_shared(input_path, output_path, front_matter, markdown_body, &extensions, config).await
    }
}

// ==================== Standalone-specific helpers ====================

/// Generate the allowed domains list for AWF network isolation.
///
/// This generates a comma-separated list of domain patterns for AWF's
/// `--allow-domains` flag. The list includes:
/// 1. Core Azure DevOps/GitHub endpoints
/// 2. MCP-specific endpoints for each enabled MCP
/// 3. User-specified additional hosts from network.allowed
fn generate_allowed_domains(
    front_matter: &FrontMatter,
    extensions: &[super::extensions::Extension],
) -> Result<String> {
    // Collect enabled MCP names (user-defined MCPs, not first-party tools)
    let enabled_mcps: Vec<String> = front_matter
        .mcp_servers
        .iter()
        .filter_map(|(name, config)| {
            let is_enabled = match config {
                McpConfig::Enabled(enabled) => *enabled,
                McpConfig::WithOptions(_) => true,
            };
            if is_enabled { Some(name.clone()) } else { None }
        })
        .collect();

    // Get user-specified hosts
    let user_hosts: Vec<String> = front_matter
        .network
        .as_ref()
        .map(|n| n.allowed.clone())
        .unwrap_or_default();

    // Generate the allowlist by combining core + MCP + extension + user hosts
    let mut hosts: HashSet<String> = HashSet::new();

    // Add core hosts
    for host in CORE_ALLOWED_HOSTS {
        hosts.insert((*host).to_string());
    }

    // Add host.docker.internal — required for the AWF container to reach
    // MCPG and SafeOutputs on the host. Only added for standalone pipelines
    // that always use MCPG.
    hosts.insert("host.docker.internal".to_string());

    // Add MCP-specific hosts (user-defined MCPs via mcp_required_hosts lookup)
    for mcp in &enabled_mcps {
        for host in mcp_required_hosts(mcp) {
            hosts.insert((*host).to_string());
        }
    }

    // Add extension-declared hosts (runtimes + first-party tools).
    // Extensions may return ecosystem identifiers (e.g., "lean") which are
    // expanded to their domain lists, or raw domain names.
    for ext in extensions {
        for host in ext.required_hosts() {
            if is_ecosystem_identifier(&host) {
                let domains = get_ecosystem_domains(&host);
                if domains.is_empty() {
                    eprintln!(
                        "warning: extension '{}' requires unknown ecosystem '{}'; \
                         no domains added",
                        ext.name(),
                        host
                    );
                }
                for domain in domains {
                    hosts.insert(domain);
                }
            } else {
                hosts.insert(host);
            }
        }
    }

    // Add user-specified hosts (validated against DNS-safe characters)
    // Entries may be ecosystem identifiers (e.g., "python", "rust") which
    // expand to their domain lists, or raw domain names.
    for host in &user_hosts {
        if is_ecosystem_identifier(host) {
            let domains = get_ecosystem_domains(host);
            if domains.is_empty() && !is_known_ecosystem(host) {
                eprintln!(
                    "warning: network.allowed contains unknown ecosystem identifier '{}'. \
                     Known ecosystems: python, rust, node, go, java, etc. \
                     If this is a domain name, it should contain a dot.",
                    host
                );
            }
            for domain in domains {
                hosts.insert(domain);
            }
        } else {
            let valid_chars = !host.is_empty()
                && host
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '*'));
            if !valid_chars {
                anyhow::bail!(
                    "network.allowed domain '{}' contains characters invalid in DNS names. \
                     Only ASCII alphanumerics, '.', '-', and '*' are allowed.",
                    host
                );
            }
            if host.contains('*') && !(host.starts_with("*.") && !host[2..].contains('*')) {
                anyhow::bail!(
                    "network.allowed domain '{}' uses '*' in an unsupported position. \
                     Wildcards must appear only as a leading prefix (e.g. '*.example.com').",
                    host
                );
            }
            hosts.insert(host.clone());
        }
    }

    // Remove blocked hosts (supports both ecosystem identifiers and raw domains)
    let blocked_hosts: Vec<String> = front_matter
        .network
        .as_ref()
        .map(|n| n.blocked.clone())
        .unwrap_or_default();
    for blocked in &blocked_hosts {
        if is_ecosystem_identifier(blocked) {
            for domain in get_ecosystem_domains(blocked) {
                hosts.remove(&domain);
            }
        } else {
            hosts.remove(blocked);
        }
    }

    // Sort for deterministic output
    let mut allowlist: Vec<String> = hosts.into_iter().collect();
    allowlist.sort();

    // Format as comma-separated list for AWF --allow-domains
    Ok(allowlist.join(","))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::common::parse_markdown;

    fn minimal_front_matter() -> FrontMatter {
        let (fm, _) = parse_markdown("---\nname: test-agent\ndescription: test\n---\n").unwrap();
        fm
    }

    // ─── generate_allowed_domains ────────────────────────────────────────────

    #[test]
    fn test_generate_allowed_domains_blocked_takes_precedence_over_allow() {
        let mut fm = minimal_front_matter();
        fm.network = Some(crate::compile::types::NetworkConfig {
            allowed: vec!["evil.example.com".to_string()],
            blocked: vec!["evil.example.com".to_string()],
        });
        let exts = super::super::extensions::collect_extensions(&fm);
        let domains = generate_allowed_domains(&fm, &exts).unwrap();
        assert!(
            !domains.contains("evil.example.com"),
            "blocked host must be excluded even if also in allow"
        );
    }

    #[test]
    fn test_generate_allowed_domains_host_docker_internal_always_present() {
        let fm = minimal_front_matter();
        let exts = super::super::extensions::collect_extensions(&fm);
        let domains = generate_allowed_domains(&fm, &exts).unwrap();
        assert!(
            domains.contains("host.docker.internal"),
            "host.docker.internal must always be in the allowlist"
        );
    }

    #[test]
    fn test_generate_allowed_domains_user_allow_host_included() {
        let mut fm = minimal_front_matter();
        fm.network = Some(crate::compile::types::NetworkConfig {
            allowed: vec!["api.mycompany.com".to_string()],
            blocked: vec![],
        });
        let exts = super::super::extensions::collect_extensions(&fm);
        let domains = generate_allowed_domains(&fm, &exts).unwrap();
        assert!(
            domains.contains("api.mycompany.com"),
            "user-specified allow host must be present in the allowlist"
        );
    }

    #[test]
    fn test_generate_allowed_domains_blocked_core_host_removed() {
        // Blocking a core host (e.g. github.com) via network.blocked removes it.
        // Note: blocking uses exact-string removal — blocking "github.com" does NOT
        // also remove wildcard variants like "*.github.com". This is intentional.
        let mut fm = minimal_front_matter();
        fm.network = Some(crate::compile::types::NetworkConfig {
            allowed: vec![],
            blocked: vec!["github.com".to_string()],
        });
        let exts = super::super::extensions::collect_extensions(&fm);
        let domains = generate_allowed_domains(&fm, &exts).unwrap();
        let domain_list: Vec<&str> = domains.split(',').collect();
        assert!(
            !domain_list.contains(&"github.com"),
            "blocked host must be removed even if it is in the core allowlist"
        );
    }

    #[test]
    fn test_generate_allowed_domains_invalid_host_returns_error() {
        let mut fm = minimal_front_matter();
        fm.network = Some(crate::compile::types::NetworkConfig {
            allowed: vec!["bad host!".to_string()],
            blocked: vec![],
        });
        let exts = super::super::extensions::collect_extensions(&fm);
        let result = generate_allowed_domains(&fm, &exts);
        assert!(result.is_err(), "invalid DNS characters should return an error");
    }

    #[test]
    fn test_generate_allowed_domains_lean_adds_lean_hosts() {
        let mut fm = minimal_front_matter();
        fm.runtimes = Some(crate::compile::types::RuntimesConfig {
            lean: Some(crate::runtimes::lean::LeanRuntimeConfig::Enabled(true)),
        });
        let exts = super::super::extensions::collect_extensions(&fm);
        let domains = generate_allowed_domains(&fm, &exts).unwrap();
        assert!(domains.contains("elan.lean-lang.org"), "should include elan domain");
        assert!(domains.contains("leanprover.github.io"), "should include leanprover domain");
        assert!(domains.contains("lean-lang.org"), "should include lean-lang domain");
    }

    #[test]
    fn test_generate_allowed_domains_lean_disabled_no_lean_hosts() {
        let mut fm = minimal_front_matter();
        fm.runtimes = Some(crate::compile::types::RuntimesConfig {
            lean: Some(crate::runtimes::lean::LeanRuntimeConfig::Enabled(false)),
        });
        let exts = super::super::extensions::collect_extensions(&fm);
        let domains = generate_allowed_domains(&fm, &exts).unwrap();
        assert!(!domains.contains("elan.lean-lang.org"), "lean disabled should not add lean hosts");
    }

    // ─── ecosystem identifier tests ──────────────────────────────────────────

    #[test]
    fn test_generate_allowed_domains_ecosystem_python_expands() {
        let mut fm = minimal_front_matter();
        fm.network = Some(crate::compile::types::NetworkConfig {
            allowed: vec!["python".to_string()],
            blocked: vec![],
        });
        let exts = super::super::extensions::collect_extensions(&fm);
        let domains = generate_allowed_domains(&fm, &exts).unwrap();
        assert!(domains.contains("pypi.org"), "python ecosystem should include pypi.org");
        assert!(domains.contains("pip.pypa.io"), "python ecosystem should include pip.pypa.io");
    }

    #[test]
    fn test_generate_allowed_domains_ecosystem_rust_expands() {
        let mut fm = minimal_front_matter();
        fm.network = Some(crate::compile::types::NetworkConfig {
            allowed: vec!["rust".to_string()],
            blocked: vec![],
        });
        let exts = super::super::extensions::collect_extensions(&fm);
        let domains = generate_allowed_domains(&fm, &exts).unwrap();
        assert!(domains.contains("crates.io"), "rust ecosystem should include crates.io");
        assert!(domains.contains("static.rust-lang.org"), "rust ecosystem should include static.rust-lang.org");
    }

    #[test]
    fn test_generate_allowed_domains_ecosystem_mixed_with_raw_domains() {
        let mut fm = minimal_front_matter();
        fm.network = Some(crate::compile::types::NetworkConfig {
            allowed: vec!["python".to_string(), "api.custom.com".to_string()],
            blocked: vec![],
        });
        let exts = super::super::extensions::collect_extensions(&fm);
        let domains = generate_allowed_domains(&fm, &exts).unwrap();
        assert!(domains.contains("pypi.org"), "ecosystem domains should be present");
        assert!(domains.contains("api.custom.com"), "raw domains should be present");
    }

    #[test]
    fn test_generate_allowed_domains_ecosystem_blocked_removes_all_ecosystem_domains() {
        let mut fm = minimal_front_matter();
        fm.network = Some(crate::compile::types::NetworkConfig {
            allowed: vec!["python".to_string()],
            blocked: vec!["python".to_string()],
        });
        let exts = super::super::extensions::collect_extensions(&fm);
        let domains = generate_allowed_domains(&fm, &exts).unwrap();
        assert!(!domains.contains("pypi.org"), "blocked ecosystem should remove its domains");
        assert!(!domains.contains("pip.pypa.io"), "blocked ecosystem should remove all its domains");
    }

    #[test]
    fn test_generate_allowed_domains_multiple_ecosystems() {
        let mut fm = minimal_front_matter();
        fm.network = Some(crate::compile::types::NetworkConfig {
            allowed: vec!["python".to_string(), "node".to_string(), "rust".to_string()],
            blocked: vec![],
        });
        let exts = super::super::extensions::collect_extensions(&fm);
        let domains = generate_allowed_domains(&fm, &exts).unwrap();
        assert!(domains.contains("pypi.org"), "python domains present");
        assert!(domains.contains("registry.npmjs.org"), "node domains present");
        assert!(domains.contains("crates.io"), "rust domains present");
    }

}
