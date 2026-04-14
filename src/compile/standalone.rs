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
use std::collections::HashMap;
use std::path::Path;

use super::Compiler;
use super::common::{
    self, AWF_VERSION, COPILOT_CLI_VERSION, DEFAULT_POOL, MCPG_PORT, MCPG_VERSION, MCPG_IMAGE,
    ADO_MCP_IMAGE, ADO_MCP_ENTRYPOINT, ADO_MCP_PACKAGE, ADO_MCP_SERVER_NAME,
    build_parameters, compute_effective_workspace, generate_acquire_ado_token,
    generate_cancel_previous_builds, generate_checkout_self, generate_checkout_steps,
    generate_ci_trigger, generate_copilot_ado_env, generate_copilot_params,
    generate_enabled_tools_args, generate_executor_ado_env, generate_header_comment,
    generate_job_timeout, generate_parameters, generate_pipeline_path, generate_pipeline_resources,
    generate_pr_trigger, generate_repositories, generate_schedule, generate_source_path,
    generate_working_directory, replace_with_indent, sanitize_filename, validate_comment_target,
    validate_front_matter_identity, validate_resolve_pr_thread_statuses,
    validate_submit_pr_review_events, validate_update_pr_votes, validate_update_work_item_target,
    validate_write_permissions,
};
use super::types::{FrontMatter, McpConfig};
use crate::allowed_hosts::{CORE_ALLOWED_HOSTS, mcp_required_hosts};
use serde::Serialize;
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

        // Validate inputs early, before any values are used in template substitution
        validate_front_matter_identity(front_matter)?;

        // Load base template
        let template = include_str!("../../templates/base.yml");

        // Generate schedule
        let schedule = match &front_matter.schedule {
            Some(s) => generate_schedule(&front_matter.name, s)
                .with_context(|| format!("Failed to parse schedule '{}'", s.expression()))?,
            None => String::new(),
        };

        let repositories = generate_repositories(&front_matter.repositories);
        let checkout_steps = generate_checkout_steps(&front_matter.checkout);
        let checkout_self = generate_checkout_self();
        let copilot_params = generate_copilot_params(front_matter)?;
        let agent_name = sanitize_filename(&front_matter.name);

        // Compute effective workspace
        let effective_workspace = compute_effective_workspace(
            &front_matter.workspace,
            &front_matter.checkout,
            &front_matter.name,
        );
        let working_directory = generate_working_directory(&effective_workspace);
        let pipeline_resources = generate_pipeline_resources(&front_matter.triggers)?;
        let has_schedule = front_matter.schedule.is_some();
        let pr_trigger = generate_pr_trigger(&front_matter.triggers, has_schedule);
        let ci_trigger = generate_ci_trigger(&front_matter.triggers, has_schedule);
        let cancel_previous_builds = generate_cancel_previous_builds(&front_matter.triggers);

        // Generate source path for Stage 2
        let source_path = generate_source_path(input_path);

        // Generate pipeline path for integrity checking
        let pipeline_path = generate_pipeline_path(output_path);

        // Generate comma-separated domain list for AWF
        let allowed_domains = generate_allowed_domains(front_matter)?;

        // Generate --enabled-tools args for SafeOutputs tool filtering
        let enabled_tools_args = generate_enabled_tools_args(front_matter);

        // Pool name
        let pool = front_matter
            .pool
            .as_ref()
            .map(|p| p.name().to_string())
            .unwrap_or_else(|| DEFAULT_POOL.to_string());

        // Generate hooks
        let setup_job = generate_setup_job(&front_matter.setup, &front_matter.name, &pool);
        let teardown_job = generate_teardown_job(&front_matter.teardown, &front_matter.name, &pool);
        let has_memory = front_matter
            .tools
            .as_ref()
            .and_then(|t| t.cache_memory.as_ref())
            .is_some_and(|cm| cm.is_enabled());

        // Build parameters list: user-defined + auto-injected clearMemory for memory
        let parameters = build_parameters(&front_matter.parameters, has_memory);
        let parameters_yaml = generate_parameters(&parameters)?;

        let prepare_steps = generate_prepare_steps(&front_matter.steps, has_memory);
        let finalize_steps = generate_finalize_steps(&front_matter.post_steps);
        let agentic_depends_on = generate_agentic_depends_on(&front_matter.setup);
        let job_timeout = generate_job_timeout(front_matter);

        // Generate service connection token acquisition steps and env vars
        let acquire_read_token = generate_acquire_ado_token(
            front_matter
                .permissions
                .as_ref()
                .and_then(|p| p.read.as_deref()),
            "SC_READ_TOKEN",
        );
        let copilot_ado_env = generate_copilot_ado_env(
            front_matter
                .permissions
                .as_ref()
                .and_then(|p| p.read.as_deref()),
        );
        let acquire_write_token = generate_acquire_ado_token(
            front_matter
                .permissions
                .as_ref()
                .and_then(|p| p.write.as_deref()),
            "SC_WRITE_TOKEN",
        );
        let executor_ado_env = generate_executor_ado_env(
            front_matter
                .permissions
                .as_ref()
                .and_then(|p| p.write.as_deref()),
        );

        // Validate that write-requiring safe-outputs have a write service connection
        validate_write_permissions(front_matter)?;
        // Validate comment-on-work-item has required target field
        validate_comment_target(front_matter)?;
        // Validate update-work-item has required target field
        validate_update_work_item_target(front_matter)?;
        // Validate submit-pr-review has required allowed-events field
        validate_submit_pr_review_events(front_matter)?;
        // Validate update-pr vote operation has required allowed-votes field
        validate_update_pr_votes(front_matter)?;
        // Validate resolve-pr-review-thread has required allowed-statuses field
        validate_resolve_pr_thread_statuses(front_matter)?;

        // Load threat analysis prompt template
        let threat_analysis_prompt = include_str!("../../templates/threat-analysis.md");

        // Insert threat analysis prompt first
        let template = replace_with_indent(
            template,
            "{{ threat_analysis_prompt }}",
            threat_analysis_prompt,
        );

        // Replace template markers
        let compiler_version = env!("CARGO_PKG_VERSION");
        let replacements: Vec<(&str, &str)> = vec![
            ("{{ parameters }}", &parameters_yaml),
            ("{{ compiler_version }}", compiler_version),
            ("{{ firewall_version }}", AWF_VERSION),
            ("{{ mcpg_version }}", MCPG_VERSION),
            ("{{ mcpg_image }}", MCPG_IMAGE),
            ("{{ copilot_version }}", COPILOT_CLI_VERSION),
            ("{{ pool }}", &pool),
            ("{{ setup_job }}", &setup_job),
            ("{{ teardown_job }}", &teardown_job),
            ("{{ prepare_steps }}", &prepare_steps),
            ("{{ finalize_steps }}", &finalize_steps),
            ("{{ agentic_depends_on }}", &agentic_depends_on),
            ("{{ job_timeout }}", &job_timeout),
            ("{{ repositories }}", &repositories),
            ("{{ schedule }}", &schedule),
            ("{{ pipeline_resources }}", &pipeline_resources),
            ("{{ pr_trigger }}", &pr_trigger),
            ("{{ ci_trigger }}", &ci_trigger),
            ("{{ checkout_self }}", &checkout_self),
            ("{{ checkout_repositories }}", &checkout_steps),
            ("{{ cancel_previous_builds }}", &cancel_previous_builds),
            ("{{ agent }}", &agent_name),
            ("{{ agent_name }}", &front_matter.name),
            ("{{ agent_description }}", &front_matter.description),
            ("{{ copilot_params }}", &copilot_params),
            ("{{ source_path }}", &source_path),
            ("{{ pipeline_path }}", &pipeline_path),
            ("{{ working_directory }}", &working_directory),
            ("{{ workspace }}", &working_directory),
            ("{{ allowed_domains }}", &allowed_domains),
            ("{{ enabled_tools_args }}", &enabled_tools_args),
            ("{{ agent_content }}", markdown_body),
            ("{{ acquire_ado_token }}", &acquire_read_token),
            ("{{ copilot_ado_env }}", &copilot_ado_env),
            ("{{ acquire_write_token }}", &acquire_write_token),
            ("{{ executor_ado_env }}", &executor_ado_env),
        ];

        let pipeline_yaml = replacements
            .into_iter()
            .fold(template, |yaml, (placeholder, replacement)| {
                replace_with_indent(&yaml, placeholder, replacement)
            });

        // Infer ADO org from git remote at compile time (for tools.azure-devops)
        let inferred_org = if front_matter
            .tools
            .as_ref()
            .and_then(|t| t.azure_devops.as_ref())
            .is_some_and(|ado| ado.is_enabled() && ado.org().is_none())
        {
            let input_dir = input_path.parent().unwrap_or(std::path::Path::new("."));
            match crate::configure::get_git_remote_url(input_dir).await {
                Ok(url) => match crate::configure::parse_ado_remote(&url) {
                    Ok(ctx) => {
                        let org = ctx
                            .org_url
                            .trim_end_matches('/')
                            .rsplit('/')
                            .next()
                            .unwrap_or("")
                            .to_string();
                        if org.is_empty() {
                            None
                        } else {
                            info!("Inferred ADO org '{}' from git remote", org);
                            Some(org)
                        }
                    }
                    Err(_) => {
                        log::debug!("Git remote is not an ADO URL — cannot infer org");
                        None
                    }
                },
                Err(_) => {
                    log::debug!("No git remote found — cannot infer org");
                    None
                }
            }
        } else {
            None
        };

        // Always generate MCPG config — safeoutputs is always required regardless
        // of whether additional mcp-servers are configured in front matter.
        let config = generate_mcpg_config(front_matter, inferred_org.as_deref())?;
        let mcpg_config_json =
            serde_json::to_string_pretty(&config).context("Failed to serialize MCPG config")?;

        let pipeline_yaml =
            replace_with_indent(&pipeline_yaml, "{{ mcpg_config }}", &mcpg_config_json);

        // Generate additional -e flags for MCPG Docker run (env passthrough for MCP containers)
        let mcpg_docker_env = generate_mcpg_docker_env(front_matter);
        let pipeline_yaml =
            replace_with_indent(&pipeline_yaml, "{{ mcpg_docker_env }}", &mcpg_docker_env);

        // Prepend header comment for pipeline detection
        let header = generate_header_comment(input_path);
        let pipeline_yaml = format!("{}{}", header, pipeline_yaml);

        Ok(pipeline_yaml)
    }
}

// ==================== Standalone-specific helpers ====================

/// Generate the allowed domains list for AWF network isolation.
///
/// This generates a comma-separated list of domain patterns for AWF's
/// `--allow-domains` flag. The list includes:
/// 1. Core Azure DevOps/GitHub endpoints
/// 2. MCP-specific endpoints for each enabled MCP
/// 3. User-specified additional hosts from network.allow
fn generate_allowed_domains(front_matter: &FrontMatter) -> Result<String> {
    // Collect enabled MCP names
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
        .map(|n| n.allow.clone())
        .unwrap_or_default();

    // Generate the allowlist by combining core + MCP + user hosts
    let mut hosts: HashSet<String> = HashSet::new();

    // Add core hosts
    for host in CORE_ALLOWED_HOSTS {
        hosts.insert((*host).to_string());
    }

    // Add host.docker.internal — required for the AWF container to reach
    // MCPG and SafeOutputs on the host. Only added for standalone pipelines
    // that always use MCPG.
    hosts.insert("host.docker.internal".to_string());

    // Add MCP-specific hosts
    for mcp in &enabled_mcps {
        for host in mcp_required_hosts(mcp) {
            hosts.insert((*host).to_string());
        }
    }

    // Add ADO-specific hosts when tools.azure-devops is enabled
    if front_matter
        .tools
        .as_ref()
        .and_then(|t| t.azure_devops.as_ref())
        .is_some_and(|ado| ado.is_enabled())
    {
        for host in mcp_required_hosts("ado") {
            hosts.insert((*host).to_string());
        }
    }

    // Add user-specified hosts (validated against DNS-safe characters)
    for host in &user_hosts {
        let valid = !host.is_empty()
            && host
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '*'));
        if !valid {
            anyhow::bail!(
                "network.allow domain '{}' contains characters invalid in DNS names. \
                 Only ASCII alphanumerics, '.', '-', and '*' are allowed.",
                host
            );
        }
        hosts.insert(host.clone());
    }

    // Remove blocked hosts
    let blocked_hosts: Vec<String> = front_matter
        .network
        .as_ref()
        .map(|n| n.blocked.clone())
        .unwrap_or_default();
    for blocked in &blocked_hosts {
        hosts.remove(blocked);
    }

    // Sort for deterministic output
    let mut allowlist: Vec<String> = hosts.into_iter().collect();
    allowlist.sort();

    // Format as comma-separated list for AWF --allow-domains
    Ok(allowlist.join(","))
}

/// Generate the setup job YAML
fn generate_setup_job(setup_steps: &[serde_yaml::Value], agent_name: &str, pool: &str) -> String {
    if setup_steps.is_empty() {
        return String::new();
    }

    let steps_yaml = common::format_steps_yaml_indented(setup_steps, 4);

    format!(
        r#"- job: SetupJob
  displayName: "{} - Setup"
  pool:
    name: {}
  steps:
    - checkout: self
{}
"#,
        agent_name, pool, steps_yaml
    )
}

/// Generate the teardown job YAML
fn generate_teardown_job(
    teardown_steps: &[serde_yaml::Value],
    agent_name: &str,
    pool: &str,
) -> String {
    if teardown_steps.is_empty() {
        return String::new();
    }

    let steps_yaml = common::format_steps_yaml(teardown_steps);

    format!(
        r#"  - job: TeardownJob
    displayName: "{} - Teardown"
    dependsOn: ProcessSafeOutputs
    pool:
      name: {}
    steps:
      - checkout: self
{}
"#,
        agent_name, pool, steps_yaml
    )
}

/// Generate prepare steps (inline), including memory download/restore/prompt if enabled
fn generate_prepare_steps(prepare_steps: &[serde_yaml::Value], has_memory: bool) -> String {
    let mut parts = Vec::new();

    // Memory steps run before user prepare steps
    if has_memory {
        parts.push(generate_memory_download());
        parts.push(generate_memory_prompt());
    }

    if !prepare_steps.is_empty() {
        parts.push(common::format_steps_yaml_indented(prepare_steps, 0));
    }

    parts.join("\n\n")
}

/// Generate finalize steps (inline)
fn generate_finalize_steps(finalize_steps: &[serde_yaml::Value]) -> String {
    if finalize_steps.is_empty() {
        return String::new();
    }

    common::format_steps_yaml_indented(finalize_steps, 0)
}

/// Generate dependsOn clause for setup job
fn generate_agentic_depends_on(setup_steps: &[serde_yaml::Value]) -> String {
    if !setup_steps.is_empty() {
        "dependsOn: SetupJob".to_string()
    } else {
        String::new()
    }
}

/// MCPG server configuration for a single MCP upstream.
#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct McpgServerConfig {
    /// Server type: "stdio" for container-based, "http" for HTTP backends
    #[serde(rename = "type")]
    pub server_type: String,
    /// Docker container image (for stdio type, per MCPG spec §4.1.2)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub container: Option<String>,
    /// Container entrypoint override (for stdio type)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entrypoint: Option<String>,
    /// Arguments passed to the container entrypoint (for stdio type)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entrypoint_args: Option<Vec<String>>,
    /// Volume mounts for containerized servers (format: "source:dest:mode")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mounts: Option<Vec<String>>,
    /// Additional Docker runtime arguments (inserted before image in `docker run`)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
    /// URL for HTTP backends
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// HTTP headers (e.g., Authorization)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers: Option<HashMap<String, String>>,
    /// Environment variables for the server process
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env: Option<HashMap<String, String>>,
    /// Tool allow-list (if empty or absent, all tools are allowed)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<String>>,
}

/// MCPG gateway configuration.
#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct McpgGatewayConfig {
    pub port: u16,
    pub domain: String,
    pub api_key: String,
    pub payload_dir: String,
}

/// Top-level MCPG configuration.
#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct McpgConfig {
    pub mcp_servers: HashMap<String, McpgServerConfig>,
    pub gateway: McpgGatewayConfig,
}

/// Generate MCPG configuration from front matter.
///
/// Converts the front matter `mcp-servers` definitions into MCPG-compatible JSON.
/// SafeOutputs is always included as an HTTP backend. Custom MCPs with explicit
/// `command:` are included as stdio servers.
///
/// `inferred_org` is the ADO organization name extracted from the git remote URL
/// at compile time. Used as the default org for `tools.azure-devops` when no
/// explicit `org:` override is provided.
///
/// Returns an error if `tools.azure-devops` is enabled but no org can be determined
/// (neither explicit override nor git remote inference).
pub fn generate_mcpg_config(front_matter: &FrontMatter, inferred_org: Option<&str>) -> Result<McpgConfig> {
    let mut mcp_servers = HashMap::new();

    // SafeOutputs is always included as an HTTP backend.
    // MCPG runs with --network host, so it reaches SafeOutputs via localhost
    // (not host.docker.internal, which requires Docker DNS and isn't available
    // in host network mode on Linux).
    mcp_servers.insert(
        "safeoutputs".to_string(),
        McpgServerConfig {
            server_type: "http".to_string(),
            container: None,
            entrypoint: None,
            entrypoint_args: None,
            mounts: None,
            args: None,
            url: Some("http://localhost:${SAFE_OUTPUTS_PORT}/mcp".to_string()),
            headers: Some(HashMap::from([(
                "Authorization".to_string(),
                "Bearer ${SAFE_OUTPUTS_API_KEY}".to_string(),
            )])),
            env: None,
            tools: None,
        },
    );

    // Auto-configure ADO MCP when tools.azure-devops is enabled.
    // This generates a containerized stdio MCP entry using the ADO MCP npm package.
    if let Some(ado_config) = front_matter
        .tools
        .as_ref()
        .and_then(|t| t.azure_devops.as_ref())
    {
        if ado_config.is_enabled() {
            // Warn if user also has a manual mcp-servers entry for azure-devops
            if front_matter.mcp_servers.contains_key(ADO_MCP_SERVER_NAME) {
                eprintln!(
                    "Warning: Agent '{}' has both tools.azure-devops and mcp-servers.azure-devops configured. \
                    The tools.azure-devops auto-configuration takes precedence. \
                    Remove the mcp-servers entry to silence this warning.",
                    front_matter.name
                );
            }

            // Build entrypoint args: npx -y @azure-devops/mcp <org> [-d toolset1 toolset2 ...]
            let mut entrypoint_args = vec!["-y".to_string(), ADO_MCP_PACKAGE.to_string()];

            // Org: use explicit override, then compile-time inferred, then fail
            let org = if let Some(explicit) = ado_config.org() {
                explicit.to_string()
            } else if let Some(inferred) = inferred_org {
                inferred.to_string()
            } else {
                anyhow::bail!(
                    "Agent '{}' has tools.azure-devops enabled but no ADO organization could be \
                    determined. Either set tools.azure-devops.org explicitly, or compile from \
                    within a git repository with an Azure DevOps remote URL.",
                    front_matter.name
                );
            };
            if !org.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
                anyhow::bail!(
                    "Invalid ADO org name '{}': must contain only alphanumerics and hyphens",
                    org
                );
            }
            entrypoint_args.push(org);

            // Toolsets: passed as -d flag followed by space-separated toolset names
            if !ado_config.toolsets().is_empty() {
                entrypoint_args.push("-d".to_string());
                for toolset in ado_config.toolsets() {
                    if !toolset.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
                        anyhow::bail!(
                            "Invalid ADO toolset name '{}': must contain only alphanumerics and hyphens",
                            toolset
                        );
                    }
                    entrypoint_args.push(toolset.clone());
                }
            }

            // Tool allow-list for MCPG filtering
            let tools = if ado_config.allowed().is_empty() {
                None
            } else {
                Some(ado_config.allowed().to_vec())
            };

            // ADO MCP needs the PAT token passed via environment
            let env = Some(HashMap::from([(
                "AZURE_DEVOPS_EXT_PAT".to_string(),
                String::new(), // Passthrough from pipeline
            )]));

            mcp_servers.insert(
                ADO_MCP_SERVER_NAME.to_string(),
                McpgServerConfig {
                    server_type: "stdio".to_string(),
                    container: Some(ADO_MCP_IMAGE.to_string()),
                    entrypoint: Some(ADO_MCP_ENTRYPOINT.to_string()),
                    entrypoint_args: Some(entrypoint_args),
                    mounts: None,
                    args: None,
                    url: None,
                    headers: None,
                    env,
                    tools,
                },
            );
        }
    }

    for (name, config) in &front_matter.mcp_servers {
        // Prevent user-defined MCPs from overwriting the reserved safeoutputs backend
        if name.eq_ignore_ascii_case("safeoutputs") {
            log::warn!(
                "MCP name 'safeoutputs' is reserved for the safe outputs HTTP backend — skipping"
            );
            continue;
        }

        // Skip if already auto-configured by tools.azure-devops
        if name == ADO_MCP_SERVER_NAME && mcp_servers.contains_key(ADO_MCP_SERVER_NAME) {
            continue;
        }

        let (is_enabled, options) = match config {
            McpConfig::Enabled(enabled) => (*enabled, None),
            McpConfig::WithOptions(opts) => (opts.enabled.unwrap_or(true), Some(opts)),
        };

        if !is_enabled {
            continue;
        }

        if let Some(opts) = options {
            if opts.container.is_some() && opts.url.is_some() {
                log::warn!(
                    "MCP '{}': both 'container' and 'url' are set — using 'container' (stdio). \
                    Remove 'url' to silence this warning.",
                    name
                );
            }

            if let Some(container) = &opts.container {
                // Container-based stdio MCP (MCPG-native, per spec §3.2.1)
                validate_container_image(container, name);
                // Validate mount paths for sensitive host directories
                for mount in &opts.mounts {
                    validate_mount_source(mount, name);
                }
                // Validate Docker runtime args for privilege escalation
                validate_docker_args(&opts.args, name);
                // Warn about potential inline secrets (check headers too in case user set both)
                warn_potential_secrets(name, &opts.env, &opts.headers);
                let entrypoint_args = if opts.entrypoint_args.is_empty() {
                    None
                } else {
                    Some(opts.entrypoint_args.clone())
                };
                let args = if opts.args.is_empty() {
                    None
                } else {
                    Some(opts.args.clone())
                };
                let mounts = if opts.mounts.is_empty() {
                    None
                } else {
                    Some(opts.mounts.clone())
                };
                let env = if opts.env.is_empty() {
                    None
                } else {
                    Some(opts.env.clone())
                };
                let tools = if opts.allowed.is_empty() {
                    None
                } else {
                    Some(opts.allowed.clone())
                };
                mcp_servers.insert(
                    name.clone(),
                    McpgServerConfig {
                        server_type: "stdio".to_string(),
                        container: Some(container.clone()),
                        entrypoint: opts.entrypoint.clone(),
                        entrypoint_args,
                        mounts,
                        args,
                        url: None,
                        headers: None,
                        env,
                        tools,
                    },
                );
            } else if let Some(url) = &opts.url {
                // HTTP-based MCP (remote server)
                validate_mcp_url(url, name);
                // Warn about potential inline secrets in headers
                warn_potential_secrets(name, &HashMap::new(), &opts.headers);
                if !opts.env.is_empty() {
                    eprintln!(
                        "Warning: MCP '{}': env vars are not supported for HTTP MCPs — they will be ignored. \
                        Use headers for authentication instead.",
                        name
                    );
                }
                let headers = if opts.headers.is_empty() {
                    None
                } else {
                    Some(opts.headers.clone())
                };
                let tools = if opts.allowed.is_empty() {
                    None
                } else {
                    Some(opts.allowed.clone())
                };
                mcp_servers.insert(
                    name.clone(),
                    McpgServerConfig {
                        server_type: "http".to_string(),
                        container: None,
                        entrypoint: None,
                        entrypoint_args: None,
                        mounts: None,
                        args: None,
                        url: Some(url.clone()),
                        headers,
                        env: None,
                        tools,
                    },
                );
            } else {
                log::warn!("MCP '{}' has no container or url — skipping", name);
                continue;
            }
        } else {
            log::warn!("MCP '{}' has no container or url — skipping", name);
        }
    }

    Ok(McpgConfig {
        mcp_servers,
        gateway: McpgGatewayConfig {
            port: MCPG_PORT,
            domain: "host.docker.internal".to_string(),
            api_key: "${MCP_GATEWAY_API_KEY}".to_string(),
            payload_dir: "/tmp/gh-aw/mcp-payloads".to_string(),
        },
    })
}

/// Sensitive host path prefixes that should not be bind-mounted into MCP containers.
const SENSITIVE_MOUNT_PREFIXES: &[&str] = &[
    "/etc",
    "/root",
    "/home",
    "/proc",
    "/sys",
];

/// Docker runtime flag names that grant dangerous host access.
/// Checked both as `--flag=value` and as `--flag value` (split across two args).
const DANGEROUS_DOCKER_FLAGS: &[&str] = &[
    "--privileged",
    "--cap-add",
    "--security-opt",
    "--pid",
    "--network",
    "--ipc",
    "--user",
    "-u",
    "--add-host",
    "--entrypoint",
];

/// Validate a container image name for injection attempts.
/// Allows `[a-zA-Z0-9./_:-]` which covers standard Docker image references.
fn validate_container_image(image: &str, mcp_name: &str) {
    if image.is_empty() {
        eprintln!("Warning: MCP '{}': container image name is empty.", mcp_name);
        return;
    }
    if !image.chars().all(|c| c.is_ascii_alphanumeric() || "._/:-@".contains(c)) {
        eprintln!(
            "Warning: MCP '{}': container image '{}' contains unexpected characters. \
            Image names should only contain [a-zA-Z0-9./_:-@].",
            mcp_name, image
        );
    }
}

/// Validate a volume mount source path, warning on sensitive host directories.
/// Docker socket mounts are escalated to stderr warnings since they grant container escape.
/// Note: paths are lowercased for comparison to catch cross-platform casing (e.g. `/ETC/shadow`).
fn validate_mount_source(mount: &str, mcp_name: &str) {
    // Format: "source:dest:mode"
    if let Some(source) = mount.split(':').next() {
        let source_lower = source.to_lowercase();
        if source_lower.contains("docker.sock") {
            eprintln!(
                "Warning: MCP '{}': mount '{}' exposes the Docker socket to the MCP container. \
                This grants full host Docker access and may allow container escape.",
                mcp_name, mount
            );
            return;
        }
        for prefix in SENSITIVE_MOUNT_PREFIXES {
            // Match exact path or path with trailing separator to avoid false positives
            // (e.g. /etc matches /etc and /etc/shadow, but not /etc-configs)
            if source_lower == *prefix || source_lower.starts_with(&format!("{}/", prefix)) {
                eprintln!(
                    "Warning: MCP '{}': mount source '{}' references a sensitive host path ({}). \
                    Ensure this is intentional.",
                    mcp_name, source, prefix
                );
                break;
            }
        }
    }
}

/// Validate Docker runtime args for dangerous flags that could escalate privileges.
/// Also detects volume mounts smuggled via `-v`/`--volume` that bypass `mounts` validation.
/// Handles both `--flag=value` and `--flag value` (split) forms.
fn validate_docker_args(args: &[String], mcp_name: &str) {
    for (i, arg) in args.iter().enumerate() {
        let arg_lower = arg.to_lowercase();
        // Check for dangerous Docker flags (both --flag=value and --flag value)
        for dangerous in DANGEROUS_DOCKER_FLAGS {
            if arg_lower == *dangerous
                || arg_lower.starts_with(&format!("{}=", dangerous))
            {
                let extra_hint = if *dangerous == "--entrypoint" {
                    " Use the 'entrypoint:' field instead of passing --entrypoint in args."
                } else {
                    ""
                };
                eprintln!(
                    "Warning: MCP '{}': Docker arg '{}' grants elevated privileges. \
                    Ensure this is intentional.{}",
                    mcp_name, arg, extra_hint
                );
            }
        }
        // Check for volume mounts smuggled via args (bypasses mounts validation)
        if arg == "-v" || arg == "--volume" {
            if let Some(mount_spec) = args.get(i + 1) {
                eprintln!(
                    "Warning: MCP '{}': volume mount '{}' in args bypasses mounts validation. \
                    Use the 'mounts:' field instead.",
                    mcp_name, mount_spec
                );
                validate_mount_source(mount_spec, mcp_name);
            }
        } else if arg_lower.starts_with("-v=") || arg_lower.starts_with("--volume=") {
            let mount_spec = arg.splitn(2, '=').nth(1).unwrap_or("");
            eprintln!(
                "Warning: MCP '{}': volume mount '{}' in args bypasses mounts validation. \
                Use the 'mounts:' field instead.",
                mcp_name, mount_spec
            );
            validate_mount_source(mount_spec, mcp_name);
        }
    }
}

/// Validate that an MCP HTTP URL uses an allowed scheme.
fn validate_mcp_url(url: &str, mcp_name: &str) {
    if !url.starts_with("https://") && !url.starts_with("http://") {
        eprintln!(
            "Warning: MCP '{}': URL '{}' does not use http:// or https:// scheme. \
            This may not work with MCPG.",
            mcp_name, url
        );
    }
}

/// Warn when env values or headers look like they contain inline secrets.
/// Secrets should use pipeline variables and passthrough ("") instead.
fn warn_potential_secrets(mcp_name: &str, env: &HashMap<String, String>, headers: &HashMap<String, String>) {
    for (key, value) in env {
        if !value.is_empty() && (key.to_lowercase().contains("token")
            || key.to_lowercase().contains("secret")
            || key.to_lowercase().contains("key")
            || key.to_lowercase().contains("password")
            || key.to_lowercase().contains("pat"))
        {
            eprintln!(
                "Warning: MCP '{}': env var '{}' has an inline value that may be a secret. \
                Use an empty string (\"\") for passthrough from pipeline variables instead.",
                mcp_name, key
            );
        }
    }
    for (key, value) in headers {
        if value.to_lowercase().contains("bearer ")
            || key.to_lowercase() == "authorization"
        {
            eprintln!(
                "Warning: MCP '{}': header '{}' may contain inline credentials. \
                These will appear in plaintext in the compiled pipeline YAML.",
                mcp_name, key
            );
        }
    }
}

/// Validate that a string is a legal environment variable name (`[A-Za-z_][A-Za-z0-9_]*`).
/// Prevents injection of arbitrary Docker flags via user-controlled front matter keys.
fn is_valid_env_var_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .map_or(false, |c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Generate additional `-e` flags for the MCPG Docker run command.
///
/// MCP containers spawned by MCPG may need environment variables that flow from
/// the pipeline through the MCPG container (passthrough). This function:
/// 1. Auto-maps `AZURE_DEVOPS_EXT_PAT` from `SC_READ_TOKEN` when `permissions.read` is configured
/// 2. Collects passthrough env vars (value is `""`) from container-based MCP configs
///
/// Only container-based MCPs are considered — HTTP MCPs don't have child containers
/// that need env passthrough.
///
/// Returns flags formatted for inline insertion in the `docker run` command.
/// The marker sits after the last hardcoded `-e` flag, so the output must
/// include leading `\\\n` for line continuation when non-empty.
pub fn generate_mcpg_docker_env(front_matter: &FrontMatter) -> String {
    let mut env_flags: Vec<String> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    // Check if any container MCP requests AZURE_DEVOPS_EXT_PAT passthrough
    let any_mcp_needs_ado_token = front_matter.mcp_servers.values().any(|config| {
        matches!(config, McpConfig::WithOptions(opts)
            if opts.enabled.unwrap_or(true)
                && opts.container.is_some()
                && opts.env.contains_key("AZURE_DEVOPS_EXT_PAT"))
    });

    // Also check if tools.azure-devops is enabled (auto-configured ADO MCP always needs token)
    let ado_tool_needs_token = front_matter
        .tools
        .as_ref()
        .and_then(|t| t.azure_devops.as_ref())
        .is_some_and(|ado| ado.is_enabled());

    // Auto-map AZURE_DEVOPS_EXT_PAT from SC_READ_TOKEN when permissions.read is configured
    // AND at least one container MCP requests it via env passthrough (or the ADO tool is enabled)
    if any_mcp_needs_ado_token || ado_tool_needs_token {
        if front_matter.permissions.as_ref().and_then(|p| p.read.as_ref()).is_some() {
            env_flags.push(
                "-e AZURE_DEVOPS_EXT_PAT=\"$(SC_READ_TOKEN)\"".to_string(),
            );
            seen.insert("AZURE_DEVOPS_EXT_PAT".to_string());
        } else {
            eprintln!(
                "Warning: one or more container MCPs request AZURE_DEVOPS_EXT_PAT passthrough \
                but permissions.read is not configured. The token will be empty at runtime. \
                Add `permissions: {{ read: <service-connection> }}` to enable auto-mapping."
            );
        }
    }

    // Collect passthrough env vars from container-based MCP configs only.
    // HTTP MCPs don't have child containers — env passthrough doesn't apply.
    for (mcp_name, config) in &front_matter.mcp_servers {
        let opts = match config {
            McpConfig::WithOptions(opts) if opts.enabled.unwrap_or(true) => opts,
            _ => continue,
        };

        // Only container-based MCPs need env passthrough on the MCPG Docker run
        if opts.container.is_none() {
            continue;
        }

        for (var_name, var_value) in &opts.env {
            // Validate env var name to prevent Docker flag injection (e.g. "X --privileged")
            if !is_valid_env_var_name(var_name) {
                log::warn!(
                    "MCP '{}': skipping invalid env var name '{}' — must match [A-Za-z_][A-Za-z0-9_]*",
                    mcp_name, var_name
                );
                continue;
            }
            if seen.contains(var_name) {
                continue;
            }
            // Passthrough: empty string means forward from host/pipeline environment
            if var_value.is_empty() {
                env_flags.push(format!("-e {}", var_name));
                seen.insert(var_name.clone());
            }
        }
    }

    env_flags.sort();
    if env_flags.is_empty() {
        // No extra flags — emit a lone `\` so the bash line continuation from the
        // preceding `-e MCP_GATEWAY_API_KEY=...` flag connects to the image name on
        // the next line. This is valid bash: a backslash at end-of-line continues
        // the command. replace_with_indent preserves this on its own indented line.
        "\\".to_string()
    } else {
        // Emit each flag on its own line with `\` continuation.
        // replace_with_indent handles indentation from the template (base.yml),
        // so we only emit the content without hardcoded spaces.
        let flags = env_flags.join(" \\\n");
        format!("{} \\", flags)
    }
}

/// Generate the steps to download agent memory from the previous successful run
/// and restore it to the staging directory.
///
/// When the `clearMemory` parameter is true, the download step is skipped
/// and only an empty memory directory is created.
fn generate_memory_download() -> String {
    r#"- task: DownloadPipelineArtifact@2
  displayName: "Download previous agent memory"
  condition: eq(${{ parameters.clearMemory }}, false)
  continueOnError: true
  inputs:
    source: "specific"
    project: "$(System.TeamProject)"
    pipeline: "$(System.DefinitionId)"
    runVersion: "latestFromBranch"
    branchName: "$(Build.SourceBranch)"
    artifact: "safe_outputs"
    targetPath: "$(Agent.TempDirectory)/previous_memory"
    allowPartiallySucceededBuilds: true

- bash: |
    mkdir -p /tmp/awf-tools/staging/agent_memory
    if [ -d "$(Agent.TempDirectory)/previous_memory/agent_memory" ]; then
      cp -a "$(Agent.TempDirectory)/previous_memory/agent_memory/." /tmp/awf-tools/staging/agent_memory/ 2>/dev/null || true
      echo "Previous agent memory restored to /tmp/awf-tools/staging/agent_memory"
      ls -laR /tmp/awf-tools/staging/agent_memory
    else
      echo "No previous agent memory found - empty memory directory created"
    fi
  displayName: "Restore previous agent memory"
  condition: eq(${{ parameters.clearMemory }}, false)
  continueOnError: true

- bash: |
    mkdir -p /tmp/awf-tools/staging/agent_memory
    echo "Memory cleared by pipeline parameter - starting fresh"
  displayName: "Initialize empty agent memory (clearMemory=true)"
  condition: eq(${{ parameters.clearMemory }}, true)"#
        .to_string()
}

/// Generate the prompt append step to inform the agent about its memory location.
fn generate_memory_prompt() -> String {
    r#"- bash: |
    cat >> "/tmp/awf-tools/agent-prompt.md" << 'MEMORY_PROMPT_EOF'

    ---

    ## Agent Memory

    You have persistent memory across runs. Your memory directory is located at `/tmp/awf-tools/staging/agent_memory/`.

    - **Read** previous memory files from this directory to recall context from prior runs.
    - **Write** new files or update existing ones in this directory to persist knowledge for future runs.
    - Use this memory to track patterns, accumulate findings, remember decisions, and improve over time.
    - The memory directory is yours to organize as you see fit (files, subdirectories, any structure).
    - Memory files are sanitized between runs for security; avoid including pipeline commands or secrets.
    MEMORY_PROMPT_EOF

    echo "Agent memory prompt appended"
  displayName: "Append memory prompt""#
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::common::parse_markdown;
    use crate::compile::types::{McpConfig, McpOptions};

    fn minimal_front_matter() -> FrontMatter {
        let (fm, _) = parse_markdown("---\nname: test-agent\ndescription: test\n---\n").unwrap();
        fm
    }

    #[test]
    fn test_generate_firewall_config_custom_mcp() {
        let mut fm = minimal_front_matter();
        fm.mcp_servers.insert(
            "my-tool".to_string(),
            McpConfig::WithOptions(McpOptions {
                container: Some("node:20-slim".to_string()),
                entrypoint: Some("node".to_string()),
                entrypoint_args: vec!["server.js".to_string()],
                allowed: vec!["do_thing".to_string()],
                ..Default::default()
            }),
        );
        let config = generate_mcpg_config(&fm, None).unwrap();
        let server = config.mcp_servers.get("my-tool").unwrap();
        assert_eq!(server.server_type, "stdio");
        assert_eq!(server.container.as_ref().unwrap(), "node:20-slim");
        assert_eq!(server.entrypoint.as_ref().unwrap(), "node");
        assert_eq!(
            server.entrypoint_args.as_ref().unwrap(),
            &vec!["server.js"]
        );
        assert_eq!(
            server.tools.as_ref().unwrap(),
            &vec!["do_thing".to_string()]
        );
    }

    #[test]
    fn test_generate_mcpg_config_mcp_without_transport_skipped() {
        let mut fm = minimal_front_matter();
        // An MCP with no container or url should be skipped
        fm.mcp_servers
            .insert("phantom".to_string(), McpConfig::Enabled(true));
        let config = generate_mcpg_config(&fm, None).unwrap();
        assert!(!config.mcp_servers.contains_key("phantom"));
        // safeoutputs is always present
        assert!(config.mcp_servers.contains_key("safeoutputs"));
    }

    #[test]
    fn test_generate_mcpg_config_disabled_mcp_skipped() {
        let mut fm = minimal_front_matter();
        fm.mcp_servers
            .insert("my-tool".to_string(), McpConfig::Enabled(false));
        let config = generate_mcpg_config(&fm, None).unwrap();
        assert!(!config.mcp_servers.contains_key("my-tool"));
    }

    #[test]
    fn test_generate_mcpg_config_empty_mcp_servers() {
        let fm = minimal_front_matter();
        let config = generate_mcpg_config(&fm, None).unwrap();
        // Only safeoutputs should be present
        assert_eq!(config.mcp_servers.len(), 1);
        assert!(config.mcp_servers.contains_key("safeoutputs"));
    }

    #[test]
    fn test_generate_mcpg_config_gateway_defaults() {
        let fm = minimal_front_matter();
        let config = generate_mcpg_config(&fm, None).unwrap();
        assert_eq!(config.gateway.port, 80);
        assert_eq!(config.gateway.domain, "host.docker.internal");
        assert_eq!(config.gateway.api_key, "${MCP_GATEWAY_API_KEY}");
        assert_eq!(config.gateway.payload_dir, "/tmp/gh-aw/mcp-payloads");
    }

    #[test]
    fn test_generate_mcpg_config_json_roundtrip() {
        let mut fm = minimal_front_matter();
        fm.mcp_servers.insert(
            "my-tool".to_string(),
            McpConfig::WithOptions(McpOptions {
                container: Some("python:3.12-slim".to_string()),
                entrypoint: Some("python".to_string()),
                entrypoint_args: vec!["-m".to_string(), "server".to_string()],
                allowed: vec!["query".to_string()],
                ..Default::default()
            }),
        );
        let config = generate_mcpg_config(&fm, None).unwrap();
        let json = serde_json::to_string_pretty(&config).expect("Config should serialize to JSON");
        let parsed: serde_json::Value =
            serde_json::from_str(&json).expect("Serialized JSON should parse back");

        // Verify top-level structure matches MCPG expectation
        assert!(
            parsed.get("mcpServers").is_some(),
            "Should have mcpServers key"
        );
        assert!(parsed.get("gateway").is_some(), "Should have gateway key");

        let gw = parsed.get("gateway").unwrap();
        assert!(gw.get("port").is_some(), "Gateway should have port");
        assert!(gw.get("domain").is_some(), "Gateway should have domain");
        assert!(gw.get("apiKey").is_some(), "Gateway should have apiKey");
        assert!(
            gw.get("payloadDir").is_some(),
            "Gateway should have payloadDir"
        );
    }

    #[test]
    fn test_generate_mcpg_config_safeoutputs_variable_placeholders() {
        let fm = minimal_front_matter();
        let config = generate_mcpg_config(&fm, None).unwrap();
        let so = config.mcp_servers.get("safeoutputs").unwrap();

        // URL should reference the runtime-substituted port
        let url = so.url.as_ref().unwrap();
        assert!(
            url.contains("${SAFE_OUTPUTS_PORT}"),
            "SafeOutputs URL should use ${{SAFE_OUTPUTS_PORT}} placeholder, got: {url}"
        );

        // Auth header should reference the runtime-substituted API key
        let headers = so.headers.as_ref().unwrap();
        let auth = headers.get("Authorization").unwrap();
        assert!(
            auth.contains("${SAFE_OUTPUTS_API_KEY}"),
            "SafeOutputs auth header should use ${{SAFE_OUTPUTS_API_KEY}} placeholder, got: {auth}"
        );
    }

    #[test]
    fn test_generate_mcpg_config_safeoutputs_is_http_type() {
        let fm = minimal_front_matter();
        let config = generate_mcpg_config(&fm, None).unwrap();
        let so = config.mcp_servers.get("safeoutputs").unwrap();
        assert_eq!(so.server_type, "http");
        assert!(
            so.container.is_none(),
            "HTTP backend should have no container"
        );
        assert!(so.args.is_none(), "HTTP backend should have no args");
        assert!(so.url.is_some(), "HTTP backend must have a URL");
    }

    #[test]
    fn test_generate_mcpg_config_container_mcp_is_stdio_type() {
        let mut fm = minimal_front_matter();
        fm.mcp_servers.insert(
            "runner".to_string(),
            McpConfig::WithOptions(McpOptions {
                container: Some("node:20-slim".to_string()),
                entrypoint: Some("node".to_string()),
                entrypoint_args: vec!["srv.js".to_string()],
                allowed: vec!["run".to_string()],
                ..Default::default()
            }),
        );
        let config = generate_mcpg_config(&fm, None).unwrap();
        let srv = config.mcp_servers.get("runner").unwrap();
        assert_eq!(srv.server_type, "stdio");
        assert!(
            srv.container.is_some(),
            "stdio server must have a container"
        );
        assert!(srv.url.is_none(), "stdio server should have no URL");
    }

    #[test]
    fn test_generate_mcpg_config_container_with_env() {
        let mut fm = minimal_front_matter();
        let mut env = std::collections::HashMap::new();
        env.insert("TOKEN".to_string(), "secret".to_string());
        fm.mcp_servers.insert(
            "with-env".to_string(),
            McpConfig::WithOptions(McpOptions {
                container: Some("node:20-slim".to_string()),
                env,
                ..Default::default()
            }),
        );
        let config = generate_mcpg_config(&fm, None).unwrap();
        let srv = config.mcp_servers.get("with-env").unwrap();
        let e = srv.env.as_ref().unwrap();
        assert_eq!(e.get("TOKEN").unwrap(), "secret");
    }

    #[test]
    fn test_generate_mcpg_config_reserved_safeoutputs_name_rejected() {
        let mut fm = minimal_front_matter();
        fm.mcp_servers.insert(
            "safeoutputs".to_string(),
            McpConfig::WithOptions(McpOptions {
                container: Some("evil:latest".to_string()),
                ..Default::default()
            }),
        );
        let config = generate_mcpg_config(&fm, None).unwrap();
        // The reserved entry should still be the HTTP backend, not the user's container
        let so = config.mcp_servers.get("safeoutputs").unwrap();
        assert_eq!(
            so.server_type, "http",
            "safeoutputs should remain HTTP backend"
        );
        assert!(
            so.container.is_none(),
            "User container should not overwrite safeoutputs"
        );
    }

    #[test]
    fn test_generate_mcpg_config_safeoutputs_reserved_name_skipped() {
        let mut fm = minimal_front_matter();
        fm.mcp_servers.insert(
            "SafeOutputs".to_string(),
            McpConfig::WithOptions(McpOptions {
                container: Some("node:20-slim".to_string()),
                entrypoint: Some("node".to_string()),
                entrypoint_args: vec!["evil.js".to_string()],
                allowed: vec!["hijack".to_string()],
                ..Default::default()
            }),
        );
        let config = generate_mcpg_config(&fm, None).unwrap();
        // The user-defined "SafeOutputs" must not overwrite the built-in entry
        let so = config.mcp_servers.get("safeoutputs").unwrap();
        assert_eq!(so.server_type, "http");
        assert!(so.url.as_ref().unwrap().contains("localhost"));
        // No stdio entry should have been added under any casing
        assert_eq!(config.mcp_servers.len(), 1);
    }

    #[test]
    fn test_generate_mcpg_config_http_mcp() {
        let mut fm = minimal_front_matter();
        fm.mcp_servers.insert(
            "remote".to_string(),
            McpConfig::WithOptions(McpOptions {
                url: Some("https://mcp.example.com/api".to_string()),
                headers: {
                    let mut h = HashMap::new();
                    h.insert("X-Custom".to_string(), "value".to_string());
                    h
                },
                allowed: vec!["query".to_string()],
                ..Default::default()
            }),
        );
        let config = generate_mcpg_config(&fm, None).unwrap();
        let srv = config.mcp_servers.get("remote").unwrap();
        assert_eq!(srv.server_type, "http");
        assert_eq!(
            srv.url.as_ref().unwrap(),
            "https://mcp.example.com/api"
        );
        assert_eq!(
            srv.headers.as_ref().unwrap().get("X-Custom").unwrap(),
            "value"
        );
        assert!(srv.container.is_none(), "HTTP server should have no container");
    }

    #[test]
    fn test_generate_mcpg_config_container_with_entrypoint() {
        let mut fm = minimal_front_matter();
        fm.mcp_servers.insert(
            "ado".to_string(),
            McpConfig::WithOptions(McpOptions {
                container: Some("node:20-slim".to_string()),
                entrypoint: Some("npx".to_string()),
                entrypoint_args: vec!["-y".to_string(), "@azure-devops/mcp".to_string()],
                ..Default::default()
            }),
        );
        let config = generate_mcpg_config(&fm, None).unwrap();
        let srv = config.mcp_servers.get("ado").unwrap();
        assert_eq!(srv.server_type, "stdio");
        assert_eq!(srv.container.as_ref().unwrap(), "node:20-slim");
        assert_eq!(srv.entrypoint.as_ref().unwrap(), "npx");
        assert_eq!(
            srv.entrypoint_args.as_ref().unwrap(),
            &vec!["-y", "@azure-devops/mcp"]
        );
    }

    #[test]
    fn test_generate_mcpg_config_container_with_mounts() {
        let mut fm = minimal_front_matter();
        fm.mcp_servers.insert(
            "data-tool".to_string(),
            McpConfig::WithOptions(McpOptions {
                container: Some("data-tool:latest".to_string()),
                mounts: vec!["/host/data:/app/data:ro".to_string()],
                ..Default::default()
            }),
        );
        let config = generate_mcpg_config(&fm, None).unwrap();
        let srv = config.mcp_servers.get("data-tool").unwrap();
        assert_eq!(
            srv.mounts.as_ref().unwrap(),
            &vec!["/host/data:/app/data:ro"]
        );
    }

    #[test]
    fn test_generate_mcpg_config_no_transport_skipped() {
        let mut fm = minimal_front_matter();
        // MCP with options but no container or url should be skipped
        fm.mcp_servers.insert(
            "no-transport".to_string(),
            McpConfig::WithOptions(McpOptions {
                allowed: vec!["tool".to_string()],
                ..Default::default()
            }),
        );
        let config = generate_mcpg_config(&fm, None).unwrap();
        assert!(!config.mcp_servers.contains_key("no-transport"));
    }

    #[test]
    fn test_generate_mcpg_docker_env_with_permissions_read() {
        let mut fm = minimal_front_matter();
        fm.permissions = Some(crate::compile::types::PermissionsConfig {
            read: Some("my-read-sc".to_string()),
            write: None,
        });
        // A container MCP must request AZURE_DEVOPS_EXT_PAT for the auto-map to trigger
        fm.mcp_servers.insert(
            "ado-tool".to_string(),
            McpConfig::WithOptions(McpOptions {
                container: Some("node:20-slim".to_string()),
                env: {
                    let mut e = HashMap::new();
                    e.insert("AZURE_DEVOPS_EXT_PAT".to_string(), "".to_string());
                    e
                },
                ..Default::default()
            }),
        );
        let env = generate_mcpg_docker_env(&fm);
        assert!(
            env.contains("-e AZURE_DEVOPS_EXT_PAT=\"$(SC_READ_TOKEN)\""),
            "Should auto-map ADO token when permissions.read is set and MCP requests it"
        );
    }

    #[test]
    fn test_generate_mcpg_docker_env_permissions_read_no_mcp_request() {
        let mut fm = minimal_front_matter();
        fm.permissions = Some(crate::compile::types::PermissionsConfig {
            read: Some("my-read-sc".to_string()),
            write: None,
        });
        // No MCP requests AZURE_DEVOPS_EXT_PAT — auto-map should NOT trigger
        fm.mcp_servers.insert(
            "unrelated-tool".to_string(),
            McpConfig::WithOptions(McpOptions {
                container: Some("node:20-slim".to_string()),
                ..Default::default()
            }),
        );
        let env = generate_mcpg_docker_env(&fm);
        assert!(
            !env.contains("AZURE_DEVOPS_EXT_PAT"),
            "Should NOT auto-map ADO token when no MCP requests it"
        );
    }

    #[test]
    fn test_generate_mcpg_docker_env_dedup_auto_map_and_passthrough() {
        // When permissions.read is set AND MCP has AZURE_DEVOPS_EXT_PAT: "",
        // the auto-mapped form (with SC_READ_TOKEN) should win — no duplicate
        let mut fm = minimal_front_matter();
        fm.permissions = Some(crate::compile::types::PermissionsConfig {
            read: Some("my-read-sc".to_string()),
            write: None,
        });
        fm.mcp_servers.insert(
            "ado-tool".to_string(),
            McpConfig::WithOptions(McpOptions {
                container: Some("node:20-slim".to_string()),
                env: {
                    let mut e = HashMap::new();
                    e.insert("AZURE_DEVOPS_EXT_PAT".to_string(), "".to_string());
                    e
                },
                ..Default::default()
            }),
        );
        let env = generate_mcpg_docker_env(&fm);
        // Should have the SC_READ_TOKEN form (auto-mapped), not bare passthrough
        assert!(
            env.contains("-e AZURE_DEVOPS_EXT_PAT=\"$(SC_READ_TOKEN)\""),
            "Auto-mapped form should be present"
        );
        // Should appear exactly once
        let count = env.matches("AZURE_DEVOPS_EXT_PAT").count();
        assert_eq!(count, 1, "AZURE_DEVOPS_EXT_PAT should appear exactly once, got {}", count);
    }

    #[test]
    fn test_generate_mcpg_docker_env_without_permissions() {
        let fm = minimal_front_matter();
        let env = generate_mcpg_docker_env(&fm);
        assert!(
            !env.contains("AZURE_DEVOPS_EXT_PAT"),
            "Should not map ADO token when permissions.read is not set"
        );
    }

    #[test]
    fn test_generate_mcpg_docker_env_passthrough_vars() {
        let mut fm = minimal_front_matter();
        fm.mcp_servers.insert(
            "tool".to_string(),
            McpConfig::WithOptions(McpOptions {
                container: Some("img:latest".to_string()),
                env: {
                    let mut e = HashMap::new();
                    e.insert("PASS_THROUGH".to_string(), "".to_string());
                    e.insert("STATIC".to_string(), "value".to_string());
                    e
                },
                ..Default::default()
            }),
        );
        let env = generate_mcpg_docker_env(&fm);
        assert!(env.contains("-e PASS_THROUGH"), "Should include passthrough var");
        assert!(!env.contains("-e STATIC"), "Should NOT include static var");
    }

    #[test]
    fn test_generate_mcpg_docker_env_rejects_invalid_names() {
        let mut fm = minimal_front_matter();
        fm.mcp_servers.insert(
            "evil".to_string(),
            McpConfig::WithOptions(McpOptions {
                container: Some("img:latest".to_string()),
                env: {
                    let mut e = HashMap::new();
                    // Injection attempt: env var name with Docker flag
                    e.insert("MY_VAR --privileged".to_string(), "".to_string());
                    // Valid env var for comparison
                    e.insert("GOOD_VAR".to_string(), "".to_string());
                    e
                },
                ..Default::default()
            }),
        );
        let env = generate_mcpg_docker_env(&fm);
        assert!(
            !env.contains("--privileged"),
            "Should reject invalid env var name with Docker flag injection"
        );
        assert!(
            env.contains("-e GOOD_VAR"),
            "Should include valid env var"
        );
    }

    #[test]
    fn test_is_valid_env_var_name() {
        assert!(is_valid_env_var_name("MY_VAR"));
        assert!(is_valid_env_var_name("_PRIVATE"));
        assert!(is_valid_env_var_name("A"));
        assert!(is_valid_env_var_name("VAR123"));
        assert!(!is_valid_env_var_name(""));
        assert!(!is_valid_env_var_name("123ABC"));
        assert!(!is_valid_env_var_name("MY-VAR"));
        assert!(!is_valid_env_var_name("MY VAR"));
        assert!(!is_valid_env_var_name("X --privileged"));
        assert!(!is_valid_env_var_name("X -v /etc:/etc:rw"));
    }

    // ─── tools.azure-devops MCPG integration ────────────────────────────────

    #[test]
    fn test_ado_tool_generates_mcpg_entry() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\ntools:\n  azure-devops: true\n---\n",
        )
        .unwrap();
        // Pass inferred org since no explicit org is set
        let config = generate_mcpg_config(&fm, Some("inferred-org")).unwrap();
        let ado = config.mcp_servers.get("azure-devops").unwrap();
        assert_eq!(ado.server_type, "stdio");
        assert_eq!(ado.container.as_deref(), Some(ADO_MCP_IMAGE));
        assert_eq!(ado.entrypoint.as_deref(), Some(ADO_MCP_ENTRYPOINT));
        let args = ado.entrypoint_args.as_ref().unwrap();
        assert!(args.contains(&"-y".to_string()));
        assert!(args.contains(&ADO_MCP_PACKAGE.to_string()));
        assert!(args.contains(&"inferred-org".to_string()));
        // Should have AZURE_DEVOPS_EXT_PAT in env
        let env = ado.env.as_ref().unwrap();
        assert!(env.contains_key("AZURE_DEVOPS_EXT_PAT"));
    }

    #[test]
    fn test_ado_tool_with_toolsets() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\ntools:\n  azure-devops:\n    toolsets: [repos, wit, core]\n---\n",
        )
        .unwrap();
        let config = generate_mcpg_config(&fm, Some("myorg")).unwrap();
        let ado = config.mcp_servers.get("azure-devops").unwrap();
        let args = ado.entrypoint_args.as_ref().unwrap();
        assert!(args.contains(&"-d".to_string()));
        assert!(args.contains(&"repos".to_string()));
        assert!(args.contains(&"wit".to_string()));
        assert!(args.contains(&"core".to_string()));
    }

    #[test]
    fn test_ado_tool_with_org_override() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\ntools:\n  azure-devops:\n    org: myorg\n---\n",
        )
        .unwrap();
        // Explicit org should be used even when inferred_org is None
        let config = generate_mcpg_config(&fm, None).unwrap();
        let ado = config.mcp_servers.get("azure-devops").unwrap();
        let args = ado.entrypoint_args.as_ref().unwrap();
        assert!(args.contains(&"myorg".to_string()));
    }

    #[test]
    fn test_ado_tool_explicit_org_overrides_inferred() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\ntools:\n  azure-devops:\n    org: explicit-org\n---\n",
        )
        .unwrap();
        let config = generate_mcpg_config(&fm, Some("inferred-org")).unwrap();
        let ado = config.mcp_servers.get("azure-devops").unwrap();
        let args = ado.entrypoint_args.as_ref().unwrap();
        assert!(args.contains(&"explicit-org".to_string()));
        assert!(!args.contains(&"inferred-org".to_string()));
    }

    #[test]
    fn test_ado_tool_no_org_fails() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\ntools:\n  azure-devops: true\n---\n",
        )
        .unwrap();
        // No explicit org and no inferred org — should fail
        let result = generate_mcpg_config(&fm, None);
        assert!(result.is_err());
        assert!(
            result.unwrap_err().to_string().contains("no ADO organization"),
            "Error should mention missing org"
        );
    }

    #[test]
    fn test_ado_tool_invalid_org_fails() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\ntools:\n  azure-devops:\n    org: \"my org/bad\"\n---\n",
        )
        .unwrap();
        let result = generate_mcpg_config(&fm, None);
        assert!(result.is_err());
        assert!(
            result.unwrap_err().to_string().contains("Invalid ADO org name"),
            "Error should mention invalid org"
        );
    }

    #[test]
    fn test_ado_tool_invalid_toolset_fails() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\ntools:\n  azure-devops:\n    org: myorg\n    toolsets: [\"repos\", \"bad toolset\"]\n---\n",
        )
        .unwrap();
        let result = generate_mcpg_config(&fm, None);
        assert!(result.is_err());
        assert!(
            result.unwrap_err().to_string().contains("Invalid ADO toolset name"),
            "Error should mention invalid toolset"
        );
    }

    #[test]
    fn test_ado_tool_with_allowed_tools() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\ntools:\n  azure-devops:\n    org: myorg\n    allowed:\n      - wit_get_work_item\n      - core_list_projects\n---\n",
        )
        .unwrap();
        let config = generate_mcpg_config(&fm, None).unwrap();
        let ado = config.mcp_servers.get("azure-devops").unwrap();
        let tools = ado.tools.as_ref().unwrap();
        assert_eq!(tools, &["wit_get_work_item", "core_list_projects"]);
    }

    #[test]
    fn test_ado_tool_disabled_not_generated() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\ntools:\n  azure-devops: false\n---\n",
        )
        .unwrap();
        let config = generate_mcpg_config(&fm, None).unwrap();
        assert!(!config.mcp_servers.contains_key("azure-devops"));
    }

    #[test]
    fn test_ado_tool_not_set_not_generated() {
        let fm = minimal_front_matter();
        let config = generate_mcpg_config(&fm, None).unwrap();
        assert!(!config.mcp_servers.contains_key("azure-devops"));
    }

    #[test]
    fn test_ado_tool_skips_manual_mcp_entry() {
        // When tools.azure-devops is enabled AND mcp-servers also has azure-devops,
        // the tools config takes precedence and the manual entry is skipped.
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\ntools:\n  azure-devops:\n    org: auto-org\nmcp-servers:\n  azure-devops:\n    container: \"node:20-slim\"\n    entrypoint: \"npx\"\n    entrypoint-args: [\"-y\", \"@azure-devops/mcp\", \"manual-org\"]\n---\n",
        )
        .unwrap();
        let config = generate_mcpg_config(&fm, None).unwrap();
        let ado = config.mcp_servers.get("azure-devops").unwrap();
        // Should use the auto-configured org, not the manual one
        let args = ado.entrypoint_args.as_ref().unwrap();
        assert!(args.contains(&"auto-org".to_string()));
        assert!(!args.contains(&"manual-org".to_string()));
    }

    #[test]
    fn test_ado_tool_docker_env_passthrough() {
        let (fm, _) = parse_markdown(
            "---\nname: test\ndescription: test\ntools:\n  azure-devops: true\npermissions:\n  read: my-read-sc\n---\n",
        )
        .unwrap();
        let env = generate_mcpg_docker_env(&fm);
        assert!(
            env.contains("AZURE_DEVOPS_EXT_PAT"),
            "Should include ADO token passthrough when permissions.read is set"
        );
    }
}
