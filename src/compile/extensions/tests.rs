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
    // Always-on: GitHub + SafeOutputs
    assert_eq!(exts.len(), 2);
    assert!(exts.iter().any(|e| e.name() == "GitHub"));
    assert!(exts.iter().any(|e| e.name() == "SafeOutputs"));
}

#[test]
fn test_collect_extensions_lean_enabled() {
    let (fm, _) =
        parse_markdown("---\nname: test\ndescription: test\nruntimes:\n  lean: true\n---\n")
            .unwrap();
    let exts = collect_extensions(&fm);
    assert_eq!(exts.len(), 3); // GitHub + SafeOutputs + Lean
    assert_eq!(exts[0].name(), "Lean 4"); // Runtime phase sorts first
}

#[test]
fn test_collect_extensions_lean_disabled() {
    let (fm, _) =
        parse_markdown("---\nname: test\ndescription: test\nruntimes:\n  lean: false\n---\n")
            .unwrap();
    let exts = collect_extensions(&fm);
    assert_eq!(exts.len(), 2); // Just always-on
}

#[test]
fn test_collect_extensions_azure_devops_enabled() {
    let (fm, _) =
        parse_markdown("---\nname: test\ndescription: test\ntools:\n  azure-devops: true\n---\n")
            .unwrap();
    let exts = collect_extensions(&fm);
    assert_eq!(exts.len(), 3); // GitHub + SafeOutputs + AzureDevOps
    assert!(exts.iter().any(|e| e.name() == "Azure DevOps MCP"));
}

#[test]
fn test_collect_extensions_cache_memory_enabled() {
    let (fm, _) =
        parse_markdown("---\nname: test\ndescription: test\ntools:\n  cache-memory: true\n---\n")
            .unwrap();
    let exts = collect_extensions(&fm);
    assert_eq!(exts.len(), 3); // GitHub + SafeOutputs + CacheMemory
    assert!(exts.iter().any(|e| e.name() == "Cache Memory"));
}

#[test]
fn test_collect_extensions_all_enabled() {
    let (fm, _) = parse_markdown(
        "---\nname: test\ndescription: test\nruntimes:\n  lean: true\ntools:\n  azure-devops: true\n  cache-memory: true\n---\n",
    )
    .unwrap();
    let exts = collect_extensions(&fm);
    assert_eq!(exts.len(), 5); // GitHub + SafeOutputs + Lean + AzureDevOps + CacheMemory
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
    assert_eq!(exts.len(), 5); // GitHub + SafeOutputs + Lean + AzureDevOps + CacheMemory

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
    let steps = ext.prepare_steps();
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
    let steps = ext.prepare_steps();
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
