use super::*;
use crate::compile::{ADO_MCP_SERVER_NAME, parse_markdown};
use crate::compile::types::{AzureDevOpsToolConfig, CacheMemoryToolConfig};
use crate::runtimes::lean::LeanRuntimeConfig;

fn minimal_front_matter() -> FrontMatter {
    let (fm, _) = parse_markdown("---\nname: test-agent\ndescription: test\n---\n").unwrap();
    fm
}

fn ctx_from(fm: &FrontMatter) -> CompileContext<'_> {
    CompileContext::for_test(fm)
}

// ── AwfMount ────────────────────────────────────────────────────

#[test]
fn test_awf_mount_mode_display() {
    assert_eq!(AwfMountMode::ReadOnly.to_string(), "ro");
    assert_eq!(AwfMountMode::ReadWrite.to_string(), "rw");
}

#[test]
fn test_awf_mount_mode_parse() {
    assert_eq!("ro".parse::<AwfMountMode>().unwrap(), AwfMountMode::ReadOnly);
    assert_eq!("rw".parse::<AwfMountMode>().unwrap(), AwfMountMode::ReadWrite);
    assert!("invalid".parse::<AwfMountMode>().is_err());
}

#[test]
fn test_awf_mount_display_with_mode() {
    let m = AwfMount::new("$HOME/.elan", "$HOME/.elan", AwfMountMode::ReadOnly);
    assert_eq!(m.to_string(), "$HOME/.elan:$HOME/.elan:ro");
}

#[test]
fn test_awf_mount_display_no_mode() {
    let m = AwfMount::new("/tmp/foo", "/tmp/foo", AwfMountMode::ReadOnly);
    assert_eq!(m.to_string(), "/tmp/foo:/tmp/foo:ro");
}

#[test]
fn test_awf_mount_parse_with_mode() {
    let m: AwfMount = "$HOME/.elan:$HOME/.elan:ro".parse().unwrap();
    assert_eq!(m.host_path, "$HOME/.elan");
    assert_eq!(m.container_path, "$HOME/.elan");
    assert_eq!(m.mode, AwfMountMode::ReadOnly);
}

#[test]
fn test_awf_mount_parse_rw_mode() {
    let m: AwfMount = "/tmp/work:/tmp/work:rw".parse().unwrap();
    assert_eq!(m.mode, AwfMountMode::ReadWrite);
}

#[test]
fn test_awf_mount_parse_no_mode() {
    let m: AwfMount = "/tmp/foo:/tmp/foo".parse().unwrap();
    assert_eq!(m.host_path, "/tmp/foo");
    assert_eq!(m.container_path, "/tmp/foo");
    assert_eq!(m.mode, AwfMountMode::ReadOnly);
}

#[test]
fn test_awf_mount_parse_invalid_mode_errors() {
    let result = "/tmp/foo:/tmp/foo:invalid".parse::<AwfMount>();
    assert!(result.is_err());
}

#[test]
fn test_awf_mount_parse_single_segment_errors() {
    let result = "elan".parse::<AwfMount>();
    assert!(result.is_err());
}

#[test]
fn test_awf_mount_serde_roundtrip() {
    let m = AwfMount::new("$HOME/.elan", "$HOME/.elan", AwfMountMode::ReadOnly);
    let json = serde_json::to_string(&m).unwrap();
    assert_eq!(json, r#""$HOME/.elan:$HOME/.elan:ro""#);
    let parsed: AwfMount = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, m);
}

// ── collect_extensions ──────────────────────────────────────────

#[test]
fn test_collect_extensions_empty_front_matter() {
    let fm = minimal_front_matter();
    let exts = collect_extensions(&fm);
    // Always-on: ado-aw-marker + GitHub + SafeOutputs
    assert_eq!(exts.len(), 3);
    assert!(exts.iter().any(|e| e.name() == "ado-aw-marker"));
    assert!(exts.iter().any(|e| e.name() == "GitHub"));
    assert!(exts.iter().any(|e| e.name() == "SafeOutputs"));
}

#[test]
fn test_collect_extensions_lean_enabled() {
    let (fm, _) =
        parse_markdown("---\nname: test\ndescription: test\nruntimes:\n  lean: true\n---\n")
            .unwrap();
    let exts = collect_extensions(&fm);
    assert_eq!(exts.len(), 4); // ado-aw-marker + GitHub + SafeOutputs + Lean
    assert_eq!(exts[0].name(), "Lean 4"); // Runtime phase sorts first
}

#[test]
fn test_collect_extensions_lean_disabled() {
    let (fm, _) =
        parse_markdown("---\nname: test\ndescription: test\nruntimes:\n  lean: false\n---\n")
            .unwrap();
    let exts = collect_extensions(&fm);
    assert_eq!(exts.len(), 3); // Just always-on
}

#[test]
fn test_collect_extensions_azure_devops_enabled() {
    let (fm, _) =
        parse_markdown("---\nname: test\ndescription: test\ntools:\n  azure-devops: true\n---\n")
            .unwrap();
    let exts = collect_extensions(&fm);
    assert_eq!(exts.len(), 4); // ado-aw-marker + GitHub + SafeOutputs + AzureDevOps
    assert!(exts.iter().any(|e| e.name() == "Azure DevOps MCP"));
}

#[test]
fn test_collect_extensions_cache_memory_enabled() {
    let (fm, _) =
        parse_markdown("---\nname: test\ndescription: test\ntools:\n  cache-memory: true\n---\n")
            .unwrap();
    let exts = collect_extensions(&fm);
    assert_eq!(exts.len(), 4); // ado-aw-marker + GitHub + SafeOutputs + CacheMemory
    assert!(exts.iter().any(|e| e.name() == "Cache Memory"));
}

#[test]
fn test_collect_extensions_all_enabled() {
    let (fm, _) = parse_markdown(
        "---\nname: test\ndescription: test\nruntimes:\n  lean: true\ntools:\n  azure-devops: true\n  cache-memory: true\n---\n",
    )
    .unwrap();
    let exts = collect_extensions(&fm);
    assert_eq!(exts.len(), 6); // ado-aw-marker + GitHub + SafeOutputs + Lean + AzureDevOps + CacheMemory
    assert_eq!(exts[0].name(), "Lean 4"); // Runtime phase first
    // All tool-phase extensions follow
    assert!(exts[1..].iter().all(|e| e.phase() == ExtensionPhase::Tool));
}

#[test]
fn test_collect_extensions_runtimes_always_before_tools() {
    // Verify the phase ordering policy: all Runtime-phase extensions
    // must appear before any Tool-phase extensions, regardless of
    // front matter field order.
    let (fm, _) = parse_markdown(
        "---\nname: test\ndescription: test\ntools:\n  azure-devops: true\n  cache-memory: true\nruntimes:\n  lean: true\n---\n",
    )
    .unwrap();
    let exts = collect_extensions(&fm);
    assert_eq!(exts.len(), 6); // ado-aw-marker + GitHub + SafeOutputs + Lean + AzureDevOps + CacheMemory

    // Find the boundary: last Runtime and first Tool
    let last_runtime_idx = exts
        .iter()
        .rposition(|e| e.phase() == ExtensionPhase::Runtime)
        .expect("expected at least one Runtime extension");
    let first_tool_idx = exts
        .iter()
        .position(|e| e.phase() == ExtensionPhase::Tool)
        .expect("expected at least one Tool extension");

    assert!(
        last_runtime_idx < first_tool_idx,
        "Runtime extensions must come before Tool extensions. \
         Last runtime at index {last_runtime_idx}, first tool at index {first_tool_idx}"
    );
}

// ── LeanExtension ──────────────────────────────────────────────

#[test]
fn test_lean_required_hosts() {
    let ext = LeanExtension::new(LeanRuntimeConfig::Enabled(true));
    let hosts = ext.required_hosts();
    // Lean extension returns the ecosystem identifier; domain expansion
    // happens in generate_allowed_domains().
    assert_eq!(hosts, vec!["lean".to_string()]);
}

#[test]
fn test_lean_required_bash_commands() {
    let ext = LeanExtension::new(LeanRuntimeConfig::Enabled(true));
    let cmds = ext.required_bash_commands();
    assert!(cmds.contains(&"lean".to_string()));
    assert!(cmds.contains(&"lake".to_string()));
    assert!(cmds.contains(&"elan".to_string()));
}

#[test]
fn test_lean_prompt_supplement() {
    let ext = LeanExtension::new(LeanRuntimeConfig::Enabled(true));
    let prompt = ext.prompt_supplement().unwrap();
    assert!(prompt.contains("Lean 4"));
    assert!(prompt.contains("lake build"));
}

#[test]
fn test_lean_prepare_steps() {
    let ext = LeanExtension::new(LeanRuntimeConfig::Enabled(true));
    let fm = minimal_front_matter();
    let ctx = ctx_from(&fm);
    let steps = ext.prepare_steps(&ctx);
    assert_eq!(steps.len(), 1);
    assert!(steps[0].contains("elan-init.sh"));
}

#[test]
fn test_lean_required_awf_mounts() {
    let ext = LeanExtension::new(LeanRuntimeConfig::Enabled(true));
    let mounts = ext.required_awf_mounts();
    assert_eq!(mounts.len(), 1);
    assert_eq!(mounts[0].host_path, "$HOME/.elan");
    assert_eq!(mounts[0].container_path, "$HOME/.elan");
    assert_eq!(mounts[0].mode, AwfMountMode::ReadOnly);
    // Round-trips to Docker format string
    assert_eq!(mounts[0].to_string(), "$HOME/.elan:$HOME/.elan:ro");
}

#[test]
fn test_default_required_awf_mounts_empty() {
    let ext = GitHubExtension;
    assert!(ext.required_awf_mounts().is_empty());
}

#[test]
fn test_lean_awf_path_prepends() {
    let ext = LeanExtension::new(LeanRuntimeConfig::Enabled(true));
    let paths = ext.awf_path_prepends();
    assert_eq!(paths.len(), 1);
    assert_eq!(paths[0], "$HOME/.elan/bin");
}

#[test]
fn test_default_awf_path_prepends_empty() {
    let ext = GitHubExtension;
    assert!(ext.awf_path_prepends().is_empty());
}

#[test]
fn test_lean_validate_bash_disabled_warning() {
    let (fm, _) =
        parse_markdown("---\nname: test\ndescription: test\ntools:\n  bash: []\n---\n").unwrap();
    let ext = LeanExtension::new(LeanRuntimeConfig::Enabled(true));
    let ctx = ctx_from(&fm);
    let warnings = ext.validate(&ctx).unwrap();
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("tools.bash is empty"));
}

#[test]
fn test_lean_validate_bash_not_disabled_no_warning() {
    let fm = minimal_front_matter();
    let ext = LeanExtension::new(LeanRuntimeConfig::Enabled(true));
    let ctx = ctx_from(&fm);
    let warnings = ext.validate(&ctx).unwrap();
    assert!(warnings.is_empty());
}

// ── AzureDevOpsExtension ───────────────────────────────────────

#[test]
fn test_ado_required_hosts() {
    let ext = AzureDevOpsExtension::new(AzureDevOpsToolConfig::Enabled(true));
    let hosts = ext.required_hosts();
    assert!(hosts.contains(&"dev.azure.com".to_string()));
    // Node ecosystem is required for npx to resolve @azure-devops/mcp
    assert!(hosts.contains(&"node".to_string()));
}

#[test]
fn test_ado_mcpg_servers_with_inferred_org() {
    let fm = minimal_front_matter();
    let ctx = CompileContext::for_test_with_org(&fm, "myorg");
    let ext = AzureDevOpsExtension::new(AzureDevOpsToolConfig::Enabled(true));
    let servers = ext.mcpg_servers(&ctx).unwrap();
    assert_eq!(servers.len(), 1);
    assert_eq!(servers[0].0, ADO_MCP_SERVER_NAME);
    assert_eq!(servers[0].1.server_type, "stdio");
    assert!(
        servers[0]
            .1
            .entrypoint_args
            .as_ref()
            .unwrap()
            .contains(&"myorg".to_string())
    );
    // Must use --network host so AWF iptables don't block outbound
    let args = servers[0].1.args.as_ref().expect("args should be set");
    assert_eq!(args, &vec!["--network".to_string(), "host".to_string()]);
}

#[test]
fn test_ado_mcpg_servers_no_org_fails() {
    let fm = minimal_front_matter();
    let ctx = CompileContext::for_test(&fm);
    let ext = AzureDevOpsExtension::new(AzureDevOpsToolConfig::Enabled(true));
    assert!(ext.mcpg_servers(&ctx).is_err());
}

#[test]
fn test_ado_validate_duplicate_mcp_warning() {
    let (mut fm, _) = parse_markdown("---\nname: test\ndescription: test\n---\n").unwrap();
    fm.mcp_servers.insert(
        ADO_MCP_SERVER_NAME.to_string(),
        crate::compile::types::McpConfig::Enabled(true),
    );
    let ctx = ctx_from(&fm);
    let ext = AzureDevOpsExtension::new(AzureDevOpsToolConfig::Enabled(true));
    let warnings = ext.validate(&ctx).unwrap();
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("both tools.azure-devops and mcp-servers"));
}

// ── CacheMemoryExtension ───────────────────────────────────────

#[test]
fn test_cache_memory_prepare_steps() {
    let ext = CacheMemoryExtension::new(CacheMemoryToolConfig::Enabled(true));
    let fm = minimal_front_matter();
    let ctx = ctx_from(&fm);
    let steps = ext.prepare_steps(&ctx);
    assert_eq!(steps.len(), 1);
    assert!(steps[0].contains("DownloadPipelineArtifact"));
}

#[test]
fn test_cache_memory_prompt_supplement() {
    let ext = CacheMemoryExtension::new(CacheMemoryToolConfig::Enabled(true));
    let prompt = ext.prompt_supplement().unwrap();
    assert!(prompt.contains("Agent Memory"));
    assert!(prompt.contains("/tmp/awf-tools/staging/agent_memory/"));
}

// ── wrap_prompt_append ─────────────────────────────────────────

#[test]
fn test_wrap_prompt_append_generates_valid_yaml_step() {
    let content = "## Test\n\nSome instructions.";
    let step = wrap_prompt_append(content, "Test Feature").unwrap();
    assert!(step.contains("cat >>"));
    assert!(step.contains("agent-prompt.md"));
    assert!(step.contains("TEST_FEATURE_EOF"));
    assert!(step.contains("Test Feature"));
}

#[test]
fn test_wrap_prompt_append_rejects_unsafe_display_name() {
    let result = wrap_prompt_append("content", "My \"Ext\"");
    assert!(result.is_err());

    let result = wrap_prompt_append("content", "ext$(rm -rf)");
    assert!(result.is_err());
}

// ── PythonExtension ────────────────────────────────────────────

#[test]
fn test_collect_extensions_python_enabled() {
    let (fm, _) =
        parse_markdown("---\nname: test\ndescription: test\nruntimes:\n  python: true\n---\n")
            .unwrap();
    let exts = collect_extensions(&fm);
    assert!(exts.iter().any(|e| e.name() == "Python"));
}

#[test]
fn test_collect_extensions_python_disabled() {
    let (fm, _) =
        parse_markdown("---\nname: test\ndescription: test\nruntimes:\n  python: false\n---\n")
            .unwrap();
    let exts = collect_extensions(&fm);
    assert!(!exts.iter().any(|e| e.name() == "Python"));
}

#[test]
fn test_collect_extensions_python_with_version() {
    let (fm, _) =
        parse_markdown("---\nname: test\ndescription: test\nruntimes:\n  python:\n    version: '3.12'\n---\n")
            .unwrap();
    let exts = collect_extensions(&fm);
    assert!(exts.iter().any(|e| e.name() == "Python"));
}

#[test]
fn test_python_required_hosts() {
    let ext = crate::runtimes::python::PythonExtension::new(
        crate::runtimes::python::PythonRuntimeConfig::Enabled(true),
    );
    let hosts = ext.required_hosts();
    assert_eq!(hosts, vec!["python".to_string()]);
}

#[test]
fn test_python_prepare_steps() {
    let ext = crate::runtimes::python::PythonExtension::new(
        crate::runtimes::python::PythonRuntimeConfig::Enabled(true),
    );
    let fm = minimal_front_matter();
    let ctx = ctx_from(&fm);
    let steps = ext.prepare_steps(&ctx);
    assert_eq!(steps.len(), 1, "no auth step without feed-url/config");
    assert!(steps[0].contains("UsePythonVersion@0"));
}

#[test]
fn test_python_prepare_steps_with_feed_url() {
    let (fm, _) = parse_markdown(
        "---\nname: test\ndescription: test\nruntimes:\n  python:\n    feed-url: 'https://pkgs.dev.azure.com/org/_packaging/feed/pypi/simple/'\n---\n",
    ).unwrap();
    let python = fm.runtimes.as_ref().unwrap().python.as_ref().unwrap();
    let ext = crate::runtimes::python::PythonExtension::new(python.clone());
    let fm = minimal_front_matter();
    let ctx = ctx_from(&fm);
    let steps = ext.prepare_steps(&ctx);
    assert_eq!(steps.len(), 2);
    assert!(steps[0].contains("UsePythonVersion@0"));
    assert!(steps[1].contains("PipAuthenticate@1"));
}

#[test]
fn test_python_agent_env_vars_no_feed() {
    let ext = crate::runtimes::python::PythonExtension::new(
        crate::runtimes::python::PythonRuntimeConfig::Enabled(true),
    );
    assert!(ext.agent_env_vars().is_empty());
}

#[test]
fn test_python_agent_env_vars_with_feed() {
    let (fm, _) = parse_markdown(
        "---\nname: test\ndescription: test\nruntimes:\n  python:\n    version: '3.12'\n    feed-url: 'https://pkgs.dev.azure.com/org/_packaging/feed/pypi/simple/'\n---\n",
    ).unwrap();
    let python = fm.runtimes.as_ref().unwrap().python.as_ref().unwrap();
    let ext = crate::runtimes::python::PythonExtension::new(python.clone());
    let vars = ext.agent_env_vars();
    assert_eq!(vars.len(), 2);
    assert_eq!(vars[0].0, "PIP_INDEX_URL");
    assert_eq!(vars[1].0, "UV_DEFAULT_INDEX");
}

#[test]
fn test_python_config_warns_not_functional() {
    let (fm, _) = parse_markdown(
        "---\nname: test\ndescription: test\nruntimes:\n  python:\n    version: '3.12'\n    config: '/path/to/pip.conf'\n---\n",
    ).unwrap();
    let python = fm.runtimes.as_ref().unwrap().python.as_ref().unwrap();
    let ext = crate::runtimes::python::PythonExtension::new(python.clone());
    let ctx = ctx_from(&fm);
    let result = ext.validate(&ctx);
    assert!(result.is_ok(), "config: should be accepted (warning, not error)");
    let warnings = result.unwrap();
    assert!(warnings.iter().any(|w| w.contains("will not be available")));
}

#[test]
fn test_python_validate_bash_disabled_warning() {
    let (fm, _) =
        parse_markdown("---\nname: test\ndescription: test\ntools:\n  bash: []\n---\n").unwrap();
    let ext = crate::runtimes::python::PythonExtension::new(
        crate::runtimes::python::PythonRuntimeConfig::Enabled(true),
    );
    let ctx = ctx_from(&fm);
    let warnings = ext.validate(&ctx).unwrap();
    assert!(!warnings.is_empty());
    assert!(warnings[0].contains("tools.bash is empty"));
}

#[test]
fn test_python_validate_bash_not_disabled_no_warning() {
    let fm = minimal_front_matter();
    let ext = crate::runtimes::python::PythonExtension::new(
        crate::runtimes::python::PythonRuntimeConfig::Enabled(true),
    );
    let ctx = ctx_from(&fm);
    let warnings = ext.validate(&ctx).unwrap();
    assert!(warnings.is_empty());
}

#[test]
fn test_python_invalid_feed_url_rejected() {
    let (fm, _) = parse_markdown(
        "---\nname: test\ndescription: test\nruntimes:\n  python:\n    feed-url: 'pkgs.dev.azure.com/no-scheme'\n---\n",
    ).unwrap();
    let python = fm.runtimes.as_ref().unwrap().python.as_ref().unwrap();
    let ext = crate::runtimes::python::PythonExtension::new(python.clone());
    let ctx = ctx_from(&fm);
    assert!(ext.validate(&ctx).is_err());
}

#[test]
fn test_python_validate_version_injection_rejected() {
    let (fm, _) = parse_markdown(
        "---\nname: test\ndescription: test\nruntimes:\n  python:\n    version: '$(SECRET)'\n---\n",
    ).unwrap();
    let python = fm.runtimes.as_ref().unwrap().python.as_ref().unwrap();
    let ext = crate::runtimes::python::PythonExtension::new(python.clone());
    let ctx = ctx_from(&fm);
    assert!(ext.validate(&ctx).is_err());
}

// ── NodeExtension ──────────────────────────────────────────────

#[test]
fn test_collect_extensions_node_enabled() {
    let (fm, _) =
        parse_markdown("---\nname: test\ndescription: test\nruntimes:\n  node: true\n---\n")
            .unwrap();
    let exts = collect_extensions(&fm);
    assert!(exts.iter().any(|e| e.name() == "Node"));
}

#[test]
fn test_collect_extensions_node_disabled() {
    let (fm, _) =
        parse_markdown("---\nname: test\ndescription: test\nruntimes:\n  node: false\n---\n")
            .unwrap();
    let exts = collect_extensions(&fm);
    assert!(!exts.iter().any(|e| e.name() == "Node"));
}

#[test]
fn test_collect_extensions_node_with_version() {
    let (fm, _) =
        parse_markdown("---\nname: test\ndescription: test\nruntimes:\n  node:\n    version: '22.x'\n---\n")
            .unwrap();
    let exts = collect_extensions(&fm);
    assert!(exts.iter().any(|e| e.name() == "Node"));
}

#[test]
fn test_node_required_hosts() {
    let ext = crate::runtimes::node::NodeExtension::new(
        crate::runtimes::node::NodeRuntimeConfig::Enabled(true),
    );
    let hosts = ext.required_hosts();
    assert_eq!(hosts, vec!["node".to_string()]);
}

#[test]
fn test_node_prepare_steps() {
    let ext = crate::runtimes::node::NodeExtension::new(
        crate::runtimes::node::NodeRuntimeConfig::Enabled(true),
    );
    let fm = minimal_front_matter();
    let ctx = ctx_from(&fm);
    let steps = ext.prepare_steps(&ctx);
    assert_eq!(steps.len(), 1, "no auth steps without feed-url/config");
    assert!(steps[0].contains("NodeTool@0"));
}

#[test]
fn test_node_prepare_steps_with_feed_url() {
    let (fm, _) = parse_markdown(
        "---\nname: test\ndescription: test\nruntimes:\n  node:\n    feed-url: 'https://pkgs.dev.azure.com/ORG/PROJECT/_packaging/FEED/npm/registry/'\n---\n",
    ).unwrap();
    let node = fm.runtimes.as_ref().unwrap().node.as_ref().unwrap();
    let ext = crate::runtimes::node::NodeExtension::new(node.clone());
    let fm = minimal_front_matter();
    let ctx = ctx_from(&fm);
    let steps = ext.prepare_steps(&ctx);
    assert_eq!(steps.len(), 3);
    assert!(steps[0].contains("NodeTool@0"));
    assert!(steps[1].contains("Ensure .npmrc"));
    assert!(steps[2].contains("npmAuthenticate@0"));
}

#[test]
fn test_node_agent_env_vars_no_feed() {
    let ext = crate::runtimes::node::NodeExtension::new(
        crate::runtimes::node::NodeRuntimeConfig::Enabled(true),
    );
    assert!(ext.agent_env_vars().is_empty());
}

#[test]
fn test_node_agent_env_vars_with_feed() {
    let (fm, _) = parse_markdown(
        "---\nname: test\ndescription: test\nruntimes:\n  node:\n    version: '22.x'\n    feed-url: 'https://pkgs.dev.azure.com/ORG/PROJECT/_packaging/FEED/npm/registry/'\n---\n",
    ).unwrap();
    let node = fm.runtimes.as_ref().unwrap().node.as_ref().unwrap();
    let ext = crate::runtimes::node::NodeExtension::new(node.clone());
    let vars = ext.agent_env_vars();
    assert_eq!(vars.len(), 1);
    assert_eq!(vars[0].0, "NPM_CONFIG_REGISTRY");
}

#[test]
fn test_node_config_warns_not_functional() {
    let (fm, _) = parse_markdown(
        "---\nname: test\ndescription: test\nruntimes:\n  node:\n    version: '22.x'\n    config: '/path/to/.npmrc'\n---\n",
    ).unwrap();
    let node = fm.runtimes.as_ref().unwrap().node.as_ref().unwrap();
    let ext = crate::runtimes::node::NodeExtension::new(node.clone());
    let ctx = ctx_from(&fm);
    let result = ext.validate(&ctx);
    assert!(result.is_ok(), "config: should be accepted (warning, not error)");
    let warnings = result.unwrap();
    assert!(warnings.iter().any(|w| w.contains("will not be available")));
}

#[test]
fn test_node_config_and_feed_url_mutually_exclusive() {
    let (fm, _) = parse_markdown(
        "---\nname: test\ndescription: test\nruntimes:\n  node:\n    config: '/path/to/.npmrc'\n    feed-url: 'https://example.com/npm/'\n---\n",
    ).unwrap();
    let node = fm.runtimes.as_ref().unwrap().node.as_ref().unwrap();
    let ext = crate::runtimes::node::NodeExtension::new(node.clone());
    let ctx = ctx_from(&fm);
    let result = ext.validate(&ctx);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("mutually exclusive"));
}

#[test]
fn test_node_validate_bash_disabled_warning() {
    let (fm, _) =
        parse_markdown("---\nname: test\ndescription: test\ntools:\n  bash: []\n---\n").unwrap();
    let ext = crate::runtimes::node::NodeExtension::new(
        crate::runtimes::node::NodeRuntimeConfig::Enabled(true),
    );
    let ctx = ctx_from(&fm);
    let warnings = ext.validate(&ctx).unwrap();
    assert!(!warnings.is_empty());
    assert!(warnings[0].contains("tools.bash is empty"));
}

#[test]
fn test_node_invalid_feed_url_rejected() {
    let (fm, _) = parse_markdown(
        "---\nname: test\ndescription: test\nruntimes:\n  node:\n    feed-url: 'pkgs.dev.azure.com/no-scheme'\n---\n",
    ).unwrap();
    let node = fm.runtimes.as_ref().unwrap().node.as_ref().unwrap();
    let ext = crate::runtimes::node::NodeExtension::new(node.clone());
    let ctx = ctx_from(&fm);
    assert!(ext.validate(&ctx).is_err());
}

#[test]
fn test_node_validate_version_injection_rejected() {
    let (fm, _) = parse_markdown(
        "---\nname: test\ndescription: test\nruntimes:\n  node:\n    version: '$(SECRET)'\n---\n",
    ).unwrap();
    let node = fm.runtimes.as_ref().unwrap().node.as_ref().unwrap();
    let ext = crate::runtimes::node::NodeExtension::new(node.clone());
    let ctx = ctx_from(&fm);
    assert!(ext.validate(&ctx).is_err());
}

#[test]
fn test_python_config_and_feed_url_mutually_exclusive() {
    let (fm, _) = parse_markdown(
        "---\nname: test\ndescription: test\nruntimes:\n  python:\n    config: '/path/to/pip.conf'\n    feed-url: 'https://example.com/pypi/'\n---\n",
    ).unwrap();
    let python = fm.runtimes.as_ref().unwrap().python.as_ref().unwrap();
    let ext = crate::runtimes::python::PythonExtension::new(python.clone());
    let ctx = ctx_from(&fm);
    let result = ext.validate(&ctx);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("mutually exclusive"));
}

// ── DotnetExtension ────────────────────────────────────────────

#[test]
fn test_collect_extensions_dotnet_enabled() {
    let (fm, _) =
        parse_markdown("---\nname: test\ndescription: test\nruntimes:\n  dotnet: true\n---\n")
            .unwrap();
    let exts = collect_extensions(&fm);
    assert!(exts.iter().any(|e| e.name() == "dotnet"));
}

#[test]
fn test_collect_extensions_dotnet_disabled() {
    let (fm, _) =
        parse_markdown("---\nname: test\ndescription: test\nruntimes:\n  dotnet: false\n---\n")
            .unwrap();
    let exts = collect_extensions(&fm);
    assert!(!exts.iter().any(|e| e.name() == "dotnet"));
}

#[test]
fn test_collect_extensions_dotnet_with_version() {
    let (fm, _) =
        parse_markdown("---\nname: test\ndescription: test\nruntimes:\n  dotnet:\n    version: '8.0.x'\n---\n")
            .unwrap();
    let exts = collect_extensions(&fm);
    assert!(exts.iter().any(|e| e.name() == "dotnet"));
}

#[test]
fn test_dotnet_required_hosts() {
    let ext = crate::runtimes::dotnet::DotnetExtension::new(
        crate::runtimes::dotnet::DotnetRuntimeConfig::Enabled(true),
    );
    let hosts = ext.required_hosts();
    assert_eq!(hosts, vec!["dotnet".to_string()]);
}

#[test]
fn test_dotnet_required_bash_commands() {
    let ext = crate::runtimes::dotnet::DotnetExtension::new(
        crate::runtimes::dotnet::DotnetRuntimeConfig::Enabled(true),
    );
    assert_eq!(ext.required_bash_commands(), vec!["dotnet".to_string()]);
}

#[test]
fn test_dotnet_prepare_steps() {
    let ext = crate::runtimes::dotnet::DotnetExtension::new(
        crate::runtimes::dotnet::DotnetRuntimeConfig::Enabled(true),
    );
    let fm = minimal_front_matter();
    let ctx = ctx_from(&fm);
    let steps = ext.prepare_steps(&ctx);
    assert_eq!(steps.len(), 1, "no auth steps without feed-url/config");
    assert!(steps[0].contains("UseDotNet@2"));
    assert!(steps[0].contains("packageType: 'sdk'"));
}

#[test]
fn test_dotnet_prepare_steps_with_feed_url() {
    let (fm, _) = parse_markdown(
        "---\nname: test\ndescription: test\nruntimes:\n  dotnet:\n    feed-url: 'https://pkgs.dev.azure.com/myorg/_packaging/myfeed/nuget/v3/index.json'\n---\n",
    ).unwrap();
    let dotnet = fm.runtimes.as_ref().unwrap().dotnet.as_ref().unwrap();
    let ext = crate::runtimes::dotnet::DotnetExtension::new(dotnet.clone());
    let fm = minimal_front_matter();
    let ctx = ctx_from(&fm);
    let steps = ext.prepare_steps(&ctx);
    assert_eq!(steps.len(), 3);
    assert!(steps[0].contains("UseDotNet@2"));
    assert!(steps[1].contains("Ensure nuget.config"));
    assert!(steps[2].contains("NuGetAuthenticate@1"));
}

#[test]
fn test_dotnet_prepare_steps_with_config_only() {
    let (fm, _) = parse_markdown(
        "---\nname: test\ndescription: test\nruntimes:\n  dotnet:\n    config: 'nuget.config'\n---\n",
    ).unwrap();
    let dotnet = fm.runtimes.as_ref().unwrap().dotnet.as_ref().unwrap();
    let ext = crate::runtimes::dotnet::DotnetExtension::new(dotnet.clone());
    let fm = minimal_front_matter();
    let ctx = ctx_from(&fm);
    let steps = ext.prepare_steps(&ctx);
    // config: alone trusts the user-checked-in nuget.config — no shim,
    // just the auth step.
    assert_eq!(steps.len(), 2);
    assert!(steps[0].contains("UseDotNet@2"));
    assert!(steps[1].contains("NuGetAuthenticate@1"));
}

#[test]
fn test_dotnet_agent_env_vars_no_feed() {
    let ext = crate::runtimes::dotnet::DotnetExtension::new(
        crate::runtimes::dotnet::DotnetRuntimeConfig::Enabled(true),
    );
    assert!(ext.agent_env_vars().is_empty());
}

#[test]
fn test_dotnet_agent_env_vars_with_feed() {
    // Unlike Python (PIP_INDEX_URL) and Node (NPM_CONFIG_REGISTRY), .NET
    // does NOT inject any env var for feed configuration — it relies on
    // nuget.config files. This test pins that contract.
    let (fm, _) = parse_markdown(
        "---\nname: test\ndescription: test\nruntimes:\n  dotnet:\n    version: '8.0.x'\n    feed-url: 'https://pkgs.dev.azure.com/myorg/_packaging/myfeed/nuget/v3/index.json'\n---\n",
    ).unwrap();
    let dotnet = fm.runtimes.as_ref().unwrap().dotnet.as_ref().unwrap();
    let ext = crate::runtimes::dotnet::DotnetExtension::new(dotnet.clone());
    assert!(ext.agent_env_vars().is_empty());
}

#[test]
fn test_dotnet_config_and_feed_url_mutually_exclusive() {
    let (fm, _) = parse_markdown(
        "---\nname: test\ndescription: test\nruntimes:\n  dotnet:\n    config: 'nuget.config'\n    feed-url: 'https://example.com/nuget/v3/index.json'\n---\n",
    ).unwrap();
    let dotnet = fm.runtimes.as_ref().unwrap().dotnet.as_ref().unwrap();
    let ext = crate::runtimes::dotnet::DotnetExtension::new(dotnet.clone());
    let ctx = ctx_from(&fm);
    let result = ext.validate(&ctx);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("mutually exclusive"));
}

#[test]
fn test_dotnet_invalid_feed_url_rejected() {
    let (fm, _) = parse_markdown(
        "---\nname: test\ndescription: test\nruntimes:\n  dotnet:\n    feed-url: 'https://example.com/$(SECRET)/nuget'\n---\n",
    ).unwrap();
    let dotnet = fm.runtimes.as_ref().unwrap().dotnet.as_ref().unwrap();
    let ext = crate::runtimes::dotnet::DotnetExtension::new(dotnet.clone());
    let ctx = ctx_from(&fm);
    assert!(ext.validate(&ctx).is_err());
}

#[test]
fn test_dotnet_global_json_sentinel_emits_use_global_json() {
    let (fm, _) = parse_markdown(
        "---\nname: test\ndescription: test\nruntimes:\n  dotnet:\n    version: 'global.json'\n---\n",
    ).unwrap();
    let dotnet = fm.runtimes.as_ref().unwrap().dotnet.as_ref().unwrap();
    assert!(dotnet.use_global_json());
    let ext = crate::runtimes::dotnet::DotnetExtension::new(dotnet.clone());
    let fm = minimal_front_matter();
    let ctx = ctx_from(&fm);
    let steps = ext.prepare_steps(&ctx);
    assert!(steps[0].contains("useGlobalJson: true"));
    assert!(!steps[0].contains("version:"), "explicit version must be omitted in global.json mode");
    assert!(steps[0].contains("from global.json"));
}

#[test]
fn test_dotnet_global_json_sentinel_case_insensitive() {
    let (fm, _) = parse_markdown(
        "---\nname: test\ndescription: test\nruntimes:\n  dotnet:\n    version: 'Global.JSON'\n---\n",
    ).unwrap();
    let dotnet = fm.runtimes.as_ref().unwrap().dotnet.as_ref().unwrap();
    assert!(dotnet.use_global_json());
}

#[test]
fn test_dotnet_global_json_sentinel_skips_injection_check() {
    // The sentinel is a literal keyword, not a version — it must not be
    // rejected by reject_pipeline_injection.
    let (fm, _) = parse_markdown(
        "---\nname: test\ndescription: test\nruntimes:\n  dotnet:\n    version: 'global.json'\n---\n",
    ).unwrap();
    let dotnet = fm.runtimes.as_ref().unwrap().dotnet.as_ref().unwrap();
    let ext = crate::runtimes::dotnet::DotnetExtension::new(dotnet.clone());
    let ctx = ctx_from(&fm);
    assert!(ext.validate(&ctx).is_ok());
}

#[test]
fn test_dotnet_version_with_global_json_present_errors() {
    use std::io::Write;
    let tmp = tempfile::tempdir().unwrap();
    let mut f = std::fs::File::create(tmp.path().join("global.json")).unwrap();
    writeln!(f, r#"{{ "sdk": {{ "version": "8.0.100" }} }}"#).unwrap();

    let (fm, _) = parse_markdown(
        "---\nname: test\ndescription: test\nruntimes:\n  dotnet:\n    version: '9.0.x'\n---\n",
    ).unwrap();
    let dotnet = fm.runtimes.as_ref().unwrap().dotnet.as_ref().unwrap();
    let ext = crate::runtimes::dotnet::DotnetExtension::new(dotnet.clone());
    let ctx = CompileContext::for_test_with_compile_dir(&fm, tmp.path());
    let result = ext.validate(&ctx);
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(msg.contains("global.json"), "error must mention global.json: {msg}");
    assert!(msg.contains("useGlobalJson") || msg.contains("'global.json'"), "error must hint at the sentinel: {msg}");
}

#[test]
fn test_dotnet_global_json_sentinel_with_global_json_present_ok() {
    // Using the sentinel alongside an on-disk global.json is the intended
    // happy path — no error.
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("global.json"), r#"{"sdk":{"version":"8.0.100"}}"#).unwrap();

    let (fm, _) = parse_markdown(
        "---\nname: test\ndescription: test\nruntimes:\n  dotnet:\n    version: 'global.json'\n---\n",
    ).unwrap();
    let dotnet = fm.runtimes.as_ref().unwrap().dotnet.as_ref().unwrap();
    let ext = crate::runtimes::dotnet::DotnetExtension::new(dotnet.clone());
    let ctx = CompileContext::for_test_with_compile_dir(&fm, tmp.path());
    assert!(ext.validate(&ctx).is_ok());
}

#[test]
fn test_dotnet_no_version_with_global_json_present_ok() {
    // Without an explicit version, no conflict — the user simply gets the
    // compiler default. This intentionally does not auto-promote to
    // useGlobalJson; users opt in with the sentinel.
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("global.json"), r#"{"sdk":{"version":"8.0.100"}}"#).unwrap();

    let (fm, _) =
        parse_markdown("---\nname: test\ndescription: test\nruntimes:\n  dotnet: true\n---\n")
            .unwrap();
    let dotnet = fm.runtimes.as_ref().unwrap().dotnet.as_ref().unwrap();
    let ext = crate::runtimes::dotnet::DotnetExtension::new(dotnet.clone());
    let ctx = CompileContext::for_test_with_compile_dir(&fm, tmp.path());
    assert!(ext.validate(&ctx).is_ok());
}

#[test]
fn test_dotnet_validate_bash_disabled_warning() {
    let (fm, _) =
        parse_markdown("---\nname: test\ndescription: test\ntools:\n  bash: []\n---\n").unwrap();
    let ext = crate::runtimes::dotnet::DotnetExtension::new(
        crate::runtimes::dotnet::DotnetRuntimeConfig::Enabled(true),
    );
    let ctx = ctx_from(&fm);
    let warnings = ext.validate(&ctx).unwrap();
    assert!(!warnings.is_empty());
    assert!(warnings[0].contains("tools.bash is empty"));
}

#[test]
fn test_dotnet_validate_version_injection_rejected() {
    let (fm, _) = parse_markdown(
        "---\nname: test\ndescription: test\nruntimes:\n  dotnet:\n    version: '$(SECRET)'\n---\n",
    ).unwrap();
    let dotnet = fm.runtimes.as_ref().unwrap().dotnet.as_ref().unwrap();
    let ext = crate::runtimes::dotnet::DotnetExtension::new(dotnet.clone());
    let ctx = ctx_from(&fm);
    assert!(ext.validate(&ctx).is_err());
}

#[test]
fn test_dotnet_validate_config_injection_rejected() {
    let (fm, _) = parse_markdown(
        "---\nname: test\ndescription: test\nruntimes:\n  dotnet:\n    config: '$(SECRET)/nuget.config'\n---\n",
    ).unwrap();
    let dotnet = fm.runtimes.as_ref().unwrap().dotnet.as_ref().unwrap();
    let ext = crate::runtimes::dotnet::DotnetExtension::new(dotnet.clone());
    let ctx = ctx_from(&fm);
    assert!(ext.validate(&ctx).is_err());
}

// ── Multiple runtimes ──────────────────────────────────────────

#[test]
fn test_collect_extensions_all_runtimes_enabled() {
    let (fm, _) = parse_markdown(
        "---\nname: test\ndescription: test\nruntimes:\n  lean: true\n  python: true\n  node: true\n  dotnet: true\n---\n",
    ).unwrap();
    let exts = collect_extensions(&fm);
    assert!(exts.iter().any(|e| e.name() == "Lean 4"));
    assert!(exts.iter().any(|e| e.name() == "Python"));
    assert!(exts.iter().any(|e| e.name() == "Node"));
    assert!(exts.iter().any(|e| e.name() == "dotnet"));
    // All are Runtime phase
    let runtime_exts: Vec<_> = exts.iter().filter(|e| e.phase() == ExtensionPhase::Runtime).collect();
    assert_eq!(runtime_exts.len(), 4);
}

#[test]
fn test_collect_extensions_runtimes_before_tools_with_python_and_node() {
    let (fm, _) = parse_markdown(
        "---\nname: test\ndescription: test\ntools:\n  azure-devops: true\nruntimes:\n  python: true\n  node: true\n---\n",
    ).unwrap();
    let exts = collect_extensions(&fm);
    let last_runtime_idx = exts
        .iter()
        .rposition(|e| e.phase() == ExtensionPhase::Runtime)
        .expect("expected Runtime extension");
    let first_tool_idx = exts
        .iter()
        .position(|e| e.phase() == ExtensionPhase::Tool)
        .expect("expected Tool extension");
    assert!(
        last_runtime_idx < first_tool_idx,
        "Runtime extensions must come before Tool extensions"
    );
}
