//! Standalone pipeline compiler.
//!
//! This compiler generates a self-contained Azure DevOps pipeline with:
//! - Full 3-job pipeline: Agent → Detection → Execution
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
    generate_allowed_domains,
    generate_cancel_previous_builds,
    generate_enabled_tools_args,
    generate_mcpg_config, generate_mcpg_docker_env,
};
use super::types::FrontMatter;

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
        skip_integrity: bool,
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
            template: include_str!("../data/base.yml").to_string(),
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
            skip_integrity,
        };

        compile_shared(input_path, output_path, front_matter, markdown_body, &extensions, &ctx, config).await
    }
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
