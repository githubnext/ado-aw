//! Standalone pipeline compiler.
//!
//! This compiler generates a self-contained Azure DevOps pipeline with:
//! - Full 3-job pipeline: Agent → Detection → Execution
//! - AWF (Agentic Workflow Firewall) L7 domain whitelisting via Squid proxy + Docker
//! - MCP firewall with tool-level filtering and custom MCP server support
//! - Setup/teardown job support

use anyhow::Result;
use async_trait::async_trait;
use log::info;
use std::path::Path;

use super::Compiler;
use super::common;
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
        debug_pipeline: bool,
    ) -> Result<String> {
        info!("Compiling for standalone target (typed IR)");

        let extensions = super::extensions::collect_extensions(front_matter);
        let ctx = super::extensions::CompileContext::new(front_matter, input_path).await?;

        let pipeline = super::standalone_ir::build_standalone_pipeline(
            front_matter,
            &extensions,
            &ctx,
            input_path,
            output_path,
            markdown_body,
            skip_integrity,
            debug_pipeline,
        )?;

        let yaml = super::ir::emit::emit(&pipeline)?;
        let yaml = common::normalize_yaml(&yaml)?;
        let header = common::generate_header_comment(input_path);
        // Legacy emitter inserts a blank line between the header
        // comment block and the first `name:` key — preserve it so
        // committed lock files stay byte-identical.
        let full = format!("{}\n{}", header, yaml);

        common::atomic_write(output_path, &full).await?;
        Ok(full)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::common::{generate_allowed_domains, parse_markdown};
    use crate::compile::extensions::{CompileContext, CompilerExtension, Declarations, Extension};

    fn minimal_front_matter() -> FrontMatter {
        let (fm, _) = parse_markdown("---\nname: test-agent\ndescription: test\n---\n").unwrap();
        fm
    }

    fn extension_declarations(extensions: &[Extension], fm: &FrontMatter) -> Vec<Declarations> {
        let ctx = CompileContext::for_test(fm);
        extensions
            .iter()
            .map(|ext| ext.declarations(&ctx).unwrap())
            .collect()
    }

    fn allowed_domains(fm: &FrontMatter, extensions: &[Extension]) -> anyhow::Result<String> {
        let declarations = extension_declarations(extensions, fm);
        generate_allowed_domains(fm, extensions, &declarations)
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
        let domains = allowed_domains(&fm, &exts).unwrap();
        assert!(
            !domains.contains("evil.example.com"),
            "blocked host must be excluded even if also in allow"
        );
    }

    #[test]
    fn test_generate_allowed_domains_host_docker_internal_always_present() {
        let fm = minimal_front_matter();
        let exts = super::super::extensions::collect_extensions(&fm);
        let domains = allowed_domains(&fm, &exts).unwrap();
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
        let domains = allowed_domains(&fm, &exts).unwrap();
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
        let domains = allowed_domains(&fm, &exts).unwrap();
        let domain_list: Vec<&str> = domains.split(',').collect();
        assert!(
            !domain_list.contains(&"github.com"),
            "blocked host must be removed even if it is in the core allowlist"
        );
        // Exact-string removal: *.github.com is a separate entry and must survive.
        assert!(
            domain_list.contains(&"*.github.com"),
            "exact-string block of 'github.com' must not remove the distinct '*.github.com' entry"
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
        let result = allowed_domains(&fm, &exts);
        assert!(
            result.is_err(),
            "invalid DNS characters should return an error"
        );
    }

    #[test]
    fn test_generate_allowed_domains_lean_adds_lean_hosts() {
        let mut fm = minimal_front_matter();
        fm.runtimes = Some(crate::compile::types::RuntimesConfig {
            lean: Some(crate::runtimes::lean::LeanRuntimeConfig::Enabled(true)),
            python: None,
            node: None,
            dotnet: None,
        });
        let exts = super::super::extensions::collect_extensions(&fm);
        let domains = allowed_domains(&fm, &exts).unwrap();
        assert!(
            domains.contains("elan.lean-lang.org"),
            "should include elan domain"
        );
        assert!(
            domains.contains("leanprover.github.io"),
            "should include leanprover domain"
        );
        assert!(
            domains.contains("lean-lang.org"),
            "should include lean-lang domain"
        );
    }

    #[test]
    fn test_generate_allowed_domains_lean_disabled_no_lean_hosts() {
        let mut fm = minimal_front_matter();
        fm.runtimes = Some(crate::compile::types::RuntimesConfig {
            lean: Some(crate::runtimes::lean::LeanRuntimeConfig::Enabled(false)),
            python: None,
            node: None,
            dotnet: None,
        });
        let exts = super::super::extensions::collect_extensions(&fm);
        let domains = allowed_domains(&fm, &exts).unwrap();
        assert!(
            !domains.contains("elan.lean-lang.org"),
            "lean disabled should not add lean hosts"
        );
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
        let domains = allowed_domains(&fm, &exts).unwrap();
        assert!(
            domains.contains("pypi.org"),
            "python ecosystem should include pypi.org"
        );
        assert!(
            domains.contains("pip.pypa.io"),
            "python ecosystem should include pip.pypa.io"
        );
    }

    #[test]
    fn test_generate_allowed_domains_ecosystem_rust_expands() {
        let mut fm = minimal_front_matter();
        fm.network = Some(crate::compile::types::NetworkConfig {
            allowed: vec!["rust".to_string()],
            blocked: vec![],
        });
        let exts = super::super::extensions::collect_extensions(&fm);
        let domains = allowed_domains(&fm, &exts).unwrap();
        assert!(
            domains.contains("crates.io"),
            "rust ecosystem should include crates.io"
        );
        assert!(
            domains.contains("static.rust-lang.org"),
            "rust ecosystem should include static.rust-lang.org"
        );
    }

    #[test]
    fn test_generate_allowed_domains_ecosystem_mixed_with_raw_domains() {
        let mut fm = minimal_front_matter();
        fm.network = Some(crate::compile::types::NetworkConfig {
            allowed: vec!["python".to_string(), "api.custom.com".to_string()],
            blocked: vec![],
        });
        let exts = super::super::extensions::collect_extensions(&fm);
        let domains = allowed_domains(&fm, &exts).unwrap();
        assert!(
            domains.contains("pypi.org"),
            "ecosystem domains should be present"
        );
        assert!(
            domains.contains("api.custom.com"),
            "raw domains should be present"
        );
    }

    #[test]
    fn test_generate_allowed_domains_ecosystem_blocked_removes_all_ecosystem_domains() {
        let mut fm = minimal_front_matter();
        fm.network = Some(crate::compile::types::NetworkConfig {
            allowed: vec!["python".to_string()],
            blocked: vec!["python".to_string()],
        });
        let exts = super::super::extensions::collect_extensions(&fm);
        let domains = allowed_domains(&fm, &exts).unwrap();
        assert!(
            !domains.contains("pypi.org"),
            "blocked ecosystem should remove its domains"
        );
        assert!(
            !domains.contains("pip.pypa.io"),
            "blocked ecosystem should remove all its domains"
        );
    }

    #[test]
    fn test_generate_allowed_domains_multiple_ecosystems() {
        let mut fm = minimal_front_matter();
        fm.network = Some(crate::compile::types::NetworkConfig {
            allowed: vec!["python".to_string(), "node".to_string(), "rust".to_string()],
            blocked: vec![],
        });
        let exts = super::super::extensions::collect_extensions(&fm);
        let domains = allowed_domains(&fm, &exts).unwrap();
        assert!(domains.contains("pypi.org"), "python domains present");
        assert!(
            domains.contains("registry.npmjs.org"),
            "node domains present"
        );
        assert!(domains.contains("crates.io"), "rust domains present");
    }

    #[test]
    fn test_generate_allowed_domains_api_target_included() {
        let (mut fm, _) = parse_markdown(
            "---\nname: test-agent\ndescription: test\nengine:\n  id: copilot\n  api-target: api.acme.ghe.com\n---\n",
        ).unwrap();
        fm.network = None;
        let exts = super::super::extensions::collect_extensions(&fm);
        let domains = allowed_domains(&fm, &exts).unwrap();
        assert!(
            domains.contains("api.acme.ghe.com"),
            "api-target hostname must be in the allowlist"
        );
    }
}
