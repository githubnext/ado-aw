//! Typed-IR builder for the canonical agentic-pipeline shape.
//!
//! Owns the Setup → Agent → Detection → (ManualReview?) → SafeOutputs
//! (+ SafeOutputs_Reviewed?) → Teardown → Conclusion shape consumed by
//! **every** compile target (`standalone`, `1es`,
//! `job`, `stage`). Each target's wrapper module (`standalone_ir.rs`,
//! `onees_ir.rs`, `job_ir.rs`, `stage_ir.rs`) is a one-screen
//! envelope that calls [`build_pipeline_context`] and lifts the
//! resulting [`BuiltPipelineContext`] into its target-specific
//! [`Pipeline`] shape.
//!
//! Replaces `src/data/base.yml` for the canonical pipeline shape:
//! instead of interpolating values into a YAML string template,
//! [`build_pipeline_context`] composes a typed [`Pipeline`]
//! programmatically that the [`crate::compile::ir::lower`] pass
//! serialises.
//!
//! ## "No `Step::RawYaml`" rule
//!
//! Every step body **this module generates** is a typed
//! [`Step::Bash`] / [`Step::Task`] / [`Step::Checkout`] /
//! [`Step::Download`] / [`Step::Publish`]. The bash bodies are
//! identical to the strings that lived in `base.yml`; what changes
//! is that they're now `format!`-composed from typed inputs in Rust
//! rather than `{{ marker }}`-substituted in a YAML template.
//!
//! User-supplied front-matter blocks (`setup:`, `steps:`,
//! `post_steps:`, `teardown:`) arrive as arbitrary `serde_yaml::Value`
//! and **legitimately** use [`Step::RawYaml`] — the IR does not
//! model arbitrary user-authored ADO step shapes.
//!
//! Extension contributions arrive via
//! [`crate::compile::extensions::Declarations`] already as typed
//! [`Step`] values.
//!
//! ## Job graph
//!
//! The canonical pipeline always has:
//!
//! - `Setup` (optional): user `setup:` steps + extension setup steps.
//!   Emitted when filters / synthPr / user setup are present.
//! - `Agent`: extensions + the static AWF / MCPG / agent-run scaffold.
//! - `Detection`: threat-analysis pass that produces the
//!   `threatAnalysis.SafeToProcess` output. When manual review is
//!   configured it also produces `reviewedProposals.HasReviewedProposals`.
//! - `ManualReview` (optional): an agentless (`pool: server`)
//!   `ManualValidation@1` gate inserted when a safe output is configured
//!   with `require-approval`. Pauses for human approval only when the run
//!   is safe **and** the agent proposed a reviewed-type output. Fail-closed
//!   on rejection/timeout.
//! - `SafeOutputs`: gated on Detection's `SafeToProcess` output via
//!   typed [`Condition::Eq`] over a typed
//!   [`crate::compile::ir::output::OutputRef`]. The lowering pass
//!   picks `dependencies.Detection.outputs['threatAnalysis.SafeToProcess']`
//!   — first production use of typed cross-job OutputRef in a
//!   condition. With mixed `require-approval`, execution splits into this
//!   automatic job (excludes reviewed tools) plus a `SafeOutputs_Reviewed`
//!   job gated behind `ManualReview` (runs only the reviewed tools,
//!   publishes a distinct `safe_outputs_reviewed` artifact).
//! - `Teardown` (optional): user `teardown:` steps.
//! - `Conclusion` (optional): post-run reporting / work-item filing.

use anyhow::{Context, Result};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use std::path::Path;

use super::common::PerJobPools;
use super::common::{
    self, ADO_BUILD_ID_SUFFIX, ADO_MCP_HOST_NODE_MODULES, ADO_MCP_PACKAGE,
    ADO_PROXY_CONTAINER_NAME, ADO_PROXY_IMAGE, ADO_PROXY_LISTEN_PORT, ADO_PROXY_NETWORK_NAME,
    ADO_PROXY_PUBLIC_CA_HOST_PATH, ADO_PROXY_TLS_PORT, AWF_SQUID_URL, AWF_VERSION, AZ_WRAPPER_DIR,
    HEADER_MARKER, MCPG_CONTAINER_NAME, MCPG_DOMAIN, MCPG_IMAGE, MCPG_PORT, MCPG_VERSION,
    image_ref,
};
use super::custom_tools::{CustomToolDefinition, collect_custom_tool_definitions};
use super::extensions::ado_script as paths;
use super::extensions::{CompileContext, CompilerExtension, Declarations, Extension, McpgConfig};
use super::ir::condition::{Condition, Expr};
use super::ir::env::EnvValue;
use super::ir::ids::{JobId, StepId};
use super::ir::job::{Job, JobVariable, Pool};
use super::ir::output::{OutputDecl, OutputRef};
use super::ir::step::{
    BashStep, CheckoutRepo, CheckoutStep, DownloadStep, PublishStep, Step, SubmodulesOpt, TaskStep,
};
use super::ir::tasks::azure_cli::{AzureCli, ScriptLocation, ScriptType};
use super::ir::tasks::docker_installer::DockerInstaller;
use super::ir::tasks::download_package::DownloadPackage;
use super::ir::tasks::download_pipeline_artifact::{
    ArtifactSource, DownloadPipelineArtifact, RunVersion,
};
use super::ir::tasks::manual_validation::{ManualValidation, OnTimeout};
use super::ir::tasks::nuget_authenticate::NuGetAuthenticate;
use super::ir::{
    CiTrigger, Parameter, ParameterDefault, ParameterKind, PipelineResource, PipelineVar,
    PrTrigger, RepositoryResource, Resources, Schedule, Triggers,
};
use super::shell::{Binding, ShellScript};
use super::types::{
    ApprovalConfig, ApprovalOnTimeout, CheckoutFetchOpts, EngineConfig, FrontMatter, OnConfig,
    PipelineArtifactConfig, PrMode, ProviderToken, Repository as RepoCfg, SELF_CHECKOUT_ALIAS,
    SupplyChainConfig, ThreatDetectionConfig,
};
use crate::ado_proxy::catalog;
use crate::ado_proxy::policy::PolicyDocument;
use crate::shell_script;

/// The `safe-outputs:` key for the create-pull-request tool. Matches the kebab
/// name `FrontMatter::create_pr_config`/`partition_safe_outputs_by_approval` use.
const CREATE_PULL_REQUEST_TOOL: &str = "create-pull-request";
const CUSTOM_PROPOSALS_STEP_ID: &str = "customProposals";

/// Built pipeline context — the result of running every validation,
/// scalar computation, extension declaration fanout, and canonical-
/// job construction once. Callers wrap the contained data into the
/// per-target [`Pipeline`] shape (`Standalone`, `JobTemplate`, or
/// `StageTemplate`).
pub(crate) struct BuiltPipelineContext {
    pub(crate) pipeline_name: String,
    pub(crate) parameters: Vec<Parameter>,
    pub(crate) resources: super::ir::Resources,
    pub(crate) triggers: super::ir::Triggers,
    pub(crate) jobs: Vec<Job>,
}

/// Computes the AWF `--exclude-env` key list for BYOM/BYOK provider
/// credentials on a Copilot engine. Returns an empty list for
/// non-Copilot engines — gating on the engine type ensures a future
/// non-Copilot engine whose env happens to contain a
/// `COPILOT_PROVIDER_*` key is never treated as a Copilot provider
/// credential.
///
/// Defense-in-depth: when the compiler mints the provider bearer
/// token, also excludes the intermediate same-job secret var
/// (`AW_PROVIDER_BEARER_TOKEN`) from the AWF `--env-all` passthrough.
/// Today ADO never exposes an `issecret=true` variable as a process
/// env var, so it is not in the AWF host env and could not be
/// forwarded anyway — but excluding it explicitly makes the isolation
/// intent self-documenting and fail-safe rather than relying on that
/// implicit ADO behaviour.
fn copilot_byom_exclude_keys(is_copilot: bool, engine_config: &EngineConfig) -> Vec<String> {
    if !is_copilot {
        return Vec::new();
    }
    let mut keys = crate::engine::copilot_byom_credential_keys(engine_config);
    if engine_config
        .provider()
        .and_then(|p| p.token.as_ref())
        .is_some()
    {
        keys.push(crate::compile::types::PROVIDER_BEARER_TOKEN_VAR.to_string());
    }
    keys
}

/// Shared back-end for the three IR-driven target compilers
/// (standalone / stage / job). Performs all the heavy lifting:
/// validates the front matter, computes every scalar, fans out
/// extension declarations, builds the canonical 5-job graph with the
/// optional `prefix`, and returns the per-target wrap inputs.
/// Run every shared front-matter validator used by the IR-driven target
/// compilers. Split out of [`build_pipeline_context`] purely to keep that
/// function's cognitive complexity manageable — behaviour and error
/// propagation are unchanged.
fn validate_pipeline_front_matter(
    front_matter: &FrontMatter,
    threat_detection: &crate::compile::types::ThreatDetectionConfig,
    detection_engine_config: &crate::compile::types::EngineConfig,
) -> Result<()> {
    common::validate_front_matter_identity(front_matter)?;
    common::validate_permissions_read_policy(front_matter)?;
    common::validate_permissions_write_policy(front_matter)?;
    if let Some(minutes) = front_matter.engine.timeout_minutes() {
        common::validate_proxied_timeout(front_matter, minutes)?;
    }
    common::validate_variable_groups(front_matter)?;
    common::validate_safe_outputs_keys(front_matter)?;
    front_matter.validate_threat_detection_config(threat_detection, detection_engine_config)?;
    front_matter.validate_require_approval()?;
    front_matter.validate_staged()?;
    common::validate_github_issue_outputs_config(front_matter)?;
    common::validate_work_item_assignment_outputs_config(front_matter)?;
    common::validate_comment_target(front_matter)?;
    common::validate_update_work_item_target(front_matter)?;
    common::validate_submit_pr_review_events(front_matter)?;
    common::validate_update_pr_votes(front_matter)?;
    common::validate_resolve_pr_thread_statuses(front_matter)?;
    common::validate_ado_aw_debug_config(front_matter)?;
    if let Some(sc) = front_matter.supply_chain() {
        sc.validate()?;
    }
    Ok(())
}

/// Collect each extension's compile-time [`Declarations`], surfacing any
/// warnings to stderr as they are produced. Split out of
/// [`build_pipeline_context`] purely to keep that function's cognitive
/// complexity manageable — behaviour and error propagation are unchanged.
fn collect_extension_declarations(
    extensions: &[Extension],
    ctx: &CompileContext<'_>,
) -> Result<Vec<crate::compile::extensions::Declarations>> {
    let mut extension_declarations = Vec::with_capacity(extensions.len());
    for ext in extensions {
        let decl = ext.declarations(ctx)?;
        for warning in &decl.warnings {
            eprintln!("Warning: {}", warning);
        }
        extension_declarations.push(decl);
    }
    Ok(extension_declarations)
}

/// Fan out each extension's [`Declarations`] into the Agent job's setup
/// steps, agent-prepare steps, and agent conditions, appending any prompt
/// supplement as a raw-YAML step. Split out of [`build_pipeline_context`]
/// purely to keep that function's cognitive complexity manageable —
/// behaviour and error propagation are unchanged.
fn fanout_extension_declarations(
    extensions: &[Extension],
    extension_declarations: Vec<crate::compile::extensions::Declarations>,
) -> Result<(Vec<Step>, Vec<Step>, Vec<Condition>)> {
    let mut ext_setup_steps: Vec<Step> = Vec::new();
    let mut ext_agent_prepare: Vec<Step> = Vec::new();
    let mut ext_agent_conditions: Vec<Condition> = Vec::new();
    for (ext, decl) in extensions.iter().zip(extension_declarations) {
        ext_setup_steps.extend(decl.setup_steps);
        ext_agent_prepare.extend(decl.agent_prepare_steps);
        ext_agent_conditions.extend(decl.agent_conditions);
        // Prompt supplements append after the per-extension prepare
        // steps. `wrap_prompt_append` returns a YAML string for a
        // `bash: cat >> prompt …` step; emit as `Step::RawYaml`
        // (typing it would mean recreating the wrap helper as a typed
        // builder for no concrete benefit — the bash body is fixed).
        if let Some(prompt) = decl.prompt_supplement {
            ext_agent_prepare.push(Step::RawYaml(
                crate::compile::extensions::wrap_prompt_append(&prompt, ext.name())?,
            ));
        }
    }
    Ok((ext_setup_steps, ext_agent_prepare, ext_agent_conditions))
}

/// Bundle of engine-derived values computed once per pipeline compile:
/// prompt invocations, install steps, composed env blocks, and the
/// Copilot BYOM/BYOK exclusion keys for both the Agent and Detection
/// engines. Split out of [`build_pipeline_context`] purely to keep that
/// function's cognitive complexity manageable — behaviour is unchanged.
struct EngineSetup {
    compiler_version: String,
    engine_run: String,
    engine_run_detection: String,
    engine_install_steps_yaml: String,
    detection_engine_install_steps_yaml: String,
    engine_log_dir: String,
    engine_env: String,
    awf_paths: Vec<String>,
    byom_exclude_keys: Vec<String>,
    detection_byom_exclude_keys: Vec<String>,
    detection_engine_env: Vec<(String, String)>,
}

#[allow(clippy::too_many_arguments)]
fn build_engine_setup(
    front_matter: &FrontMatter,
    extensions: &[Extension],
    ctx: &CompileContext<'_>,
    extension_declarations: &[crate::compile::extensions::Declarations],
    threat_detection: &crate::compile::types::ThreatDetectionConfig,
    detection_engine_config: &crate::compile::types::EngineConfig,
) -> Result<EngineSetup> {
    let compiler_version = env!("CARGO_PKG_VERSION").to_string();
    let detection_engine = crate::engine::get_engine(detection_engine_config.engine_id())?;

    let engine_run = ctx.engine.invocation(
        ctx.front_matter,
        extension_declarations,
        "/tmp/awf-tools/agent-prompt.md",
        Some("/tmp/awf-tools/mcp-config.json"),
    )?;
    let engine_run_detection = detection_engine.invocation_with_config(
        detection_engine_config,
        ctx.front_matter,
        extension_declarations,
        "/tmp/awf-tools/threat-analysis-prompt.md",
        None,
    )?;
    let engine_install_steps_yaml =
        ctx.engine
            .install_steps(&front_matter.engine, &front_matter.target, ctx.ado_org())?;
    let detection_engine_install_steps_yaml = if threat_detection.is_enabled() {
        detection_engine.install_steps(
            detection_engine_config,
            &front_matter.target,
            ctx.ado_org(),
        )?
    } else {
        String::new()
    };
    let engine_log_dir = ctx.engine.log_dir().to_string();

    let mut engine_env = ctx.engine.env(&front_matter.engine)?;
    // BYOM/BYOK credential exclusion is Copilot-specific: gate on the engine type so a
    // future non-Copilot engine whose env happens to contain a COPILOT_PROVIDER_*
    // key is never treated as a Copilot provider credential.
    let is_copilot = matches!(ctx.engine, crate::engine::Engine::Copilot);
    let byom_exclude_keys = copilot_byom_exclude_keys(is_copilot, &front_matter.engine);
    let detection_is_copilot = matches!(detection_engine, crate::engine::Engine::Copilot);
    let detection_byom_exclude_keys =
        copilot_byom_exclude_keys(detection_is_copilot, detection_engine_config);
    let detection_engine_env = if detection_is_copilot {
        crate::engine::copilot_detection_env(detection_engine_config)?
    } else {
        Vec::new()
    };
    // AWF path env (when extensions declare path prepends)
    let awf_paths = common::collect_awf_path_prepends(extension_declarations);
    let has_awf_paths = !awf_paths.is_empty();
    let awf_path_env = common::generate_awf_path_env(has_awf_paths);
    if !awf_path_env.is_empty() {
        engine_env = format!("{engine_env}\n{awf_path_env}");
    }
    let agent_env = common::collect_agent_env_vars(extensions, extension_declarations)?;
    if !agent_env.is_empty() {
        engine_env = format!("{engine_env}\n{agent_env}");
    }

    Ok(EngineSetup {
        compiler_version,
        engine_run,
        engine_run_detection,
        engine_install_steps_yaml,
        detection_engine_install_steps_yaml,
        engine_log_dir,
        engine_env,
        awf_paths,
        byom_exclude_keys,
        detection_byom_exclude_keys,
        detection_engine_env,
    })
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn build_pipeline_context(
    front_matter: &FrontMatter,
    extensions: &[Extension],
    ctx: &CompileContext<'_>,
    input_path: &Path,
    output_path: &Path,
    markdown_body: &str,
    skip_integrity: bool,
    debug_pipeline: bool,
    prefix: Option<&str>,
) -> Result<BuiltPipelineContext> {
    let threat_detection = front_matter.threat_detection_config()?;
    let detection_engine_config = front_matter.effective_detection_engine(&threat_detection);

    // ─── Validations (reuse all shared validators) ────────────────
    validate_pipeline_front_matter(front_matter, &threat_detection, &detection_engine_config)?;

    let extension_declarations = collect_extension_declarations(extensions, ctx)?;

    // ─── Scalars ──────────────────────────────────────────────────
    let pipeline_name = format!(
        "{}{}",
        common::sanitize_pipeline_agent_name(&front_matter.name),
        ADO_BUILD_ID_SUFFIX
    );
    let agent_display_name = front_matter.name.clone();
    let effective_workspace = common::compute_effective_workspace(
        &front_matter.workspace,
        &front_matter.checkout,
        &front_matter.name,
    )?;
    let working_directory = common::generate_working_directory(&effective_workspace);
    let trigger_repo_directory = common::generate_trigger_repo_directory(&front_matter.checkout);
    // Identity of the `self` repository, resolved at compile time.
    //
    // `self` is the repository this workflow is compiled in, and (because the
    // executor's `--source` resolves beneath the `self` checkout) also the
    // repository whose pipeline runs it — for template targets that is the
    // parent pipeline's repository. The compiler therefore already knows the
    // name, and the `ado-aw-marker` extension already bakes it into the lock,
    // so `ado-aw check` fails loudly if the baked value ever drifts from the
    // repository the pipeline actually runs in.
    //
    // The `$(Build.Repository.Name)` fallback is deliberately last: it names
    // the *triggering* repository, which differs from `self` on
    // repository-resource-triggered runs (issue #1731). Warn rather than fail,
    // because compiling outside an ADO clone is a supported developer path.
    let self_repository_name = match ctx.ado_context.as_ref() {
        Some(ado) => EnvValue::literal(ado.repo_name.clone()),
        None => {
            eprintln!(
                "Warning: could not resolve the Azure DevOps repository for agent '{}' \
                (no ADO git remote and no ADO_AW_COMPILE_REMOTE_URL). Falling back to \
                $(Build.Repository.Name) for the 'self' repository identity, which names \
                the TRIGGERING repository — safe-outputs targeting `repository: self` may \
                resolve to the wrong repository on repository-resource-triggered runs. \
                Compile from an Azure DevOps clone, or set ADO_AW_COMPILE_REMOTE_URL.",
                front_matter.name
            );
            EnvValue::ado_macro("Build.Repository.Name")?
        }
    };
    let pools = common::resolve_pool_overrides_typed(
        front_matter.target.clone(),
        front_matter.pool.as_ref(),
        front_matter.pool_overrides(),
    )?;

    let engine_setup = build_engine_setup(
        front_matter,
        extensions,
        ctx,
        &extension_declarations,
        &threat_detection,
        &detection_engine_config,
    )?;
    let EngineSetup {
        compiler_version,
        engine_run,
        engine_run_detection,
        engine_install_steps_yaml,
        detection_engine_install_steps_yaml,
        engine_log_dir,
        engine_env,
        awf_paths,
        byom_exclude_keys,
        detection_byom_exclude_keys,
        detection_engine_env,
    } = engine_setup;

    // AWF mounts + allowlist
    let allowed_domains =
        common::generate_allowed_domains(front_matter, extensions, &extension_declarations)?;
    // With no engine overlay, Detection uses the same effective engine and
    // network inputs as Agent, so the already-computed allowlist is identical.
    // Any future detection-only network contributor must extend this branch
    // rather than relying on the clone.
    let detection_allowed_domains = if threat_detection.engine.is_some() {
        common::generate_allowed_domains_for_engine(
            front_matter,
            extensions,
            &extension_declarations,
            &detection_engine_config,
        )?
    } else {
        allowed_domains.clone()
    };
    let awf_mounts = common::generate_awf_mounts(extensions, &extension_declarations);
    let awf_path_step_yaml = common::generate_awf_path_step(&awf_paths);

    // MCPG config + compiler-generated dynamic SafeOutputs tool definitions.
    let custom_tool_schemas = super::custom_tools::generate_custom_tool_schemas(front_matter)?;
    let custom_tools_json = if custom_tool_schemas.is_empty() {
        None
    } else {
        Some(super::custom_tools::custom_tools_json(
            &custom_tool_schemas,
        )?)
    };
    let resolved_execution_config_json =
        super::custom_tools::resolved_execution_config_json(front_matter, &custom_tool_schemas)?;
    let mcpg_config_obj = common::generate_mcpg_config(front_matter, &extension_declarations)?;
    let mcpg_config_json = serde_json::to_string_pretty(&mcpg_config_obj)
        .map_err(|e| anyhow::anyhow!("Failed to serialize MCPG config: {e}"))?;
    let mcpg_docker_env = common::generate_mcpg_docker_env(front_matter, &extension_declarations);
    let mcpg_step_env = common::generate_mcpg_step_env(&extension_declarations);

    // Source / pipeline paths (for integrity check + metadata).
    // `source_path` embeds `{{ trigger_repo_directory }}` which the
    // legacy template fold substitutes — do the same eagerly so step
    // bodies receive a fully-resolved scalar.
    // Validate the exact user-controlled suffix produced by the canonical path
    // generator before the final path is embedded into compiler-authored bash.
    // The trigger-repository prefix itself is compiler-owned.
    let source_path_raw = common::generate_source_path(input_path);
    let source_path_suffix = source_path_raw
        .strip_prefix("{{ trigger_repo_directory }}/")
        .ok_or_else(|| {
            anyhow::anyhow!(
                "compiler-generated workflow source path is missing the trigger-repository prefix"
            )
        })?;
    crate::validate::reject_pipeline_injection(source_path_suffix, "workflow source path")?;
    let source_relative_path = source_path_suffix.to_string();
    let source_path =
        source_path_raw.replace("{{ trigger_repo_directory }}", &trigger_repo_directory);
    let pipeline_path = common::generate_pipeline_path(output_path);

    // Read / write tokens
    let acquire_read_token = common::generate_acquire_ado_token(
        front_matter
            .permissions
            .as_ref()
            .and_then(|p| p.read.as_ref())
            .map(crate::compile::types::ReadPermissionConfig::service_connection),
        "SC_READ_TOKEN",
    );
    let acquire_write_token = common::generate_acquire_ado_token(
        front_matter
            .permissions
            .as_ref()
            .and_then(|p| p.write.as_ref())
            .map(crate::compile::types::WritePermissionConfig::service_connection),
        "SC_WRITE_TOKEN",
    );
    // Skip integrity check resolution
    let skip_integrity = skip_integrity
        || front_matter
            .ado_aw_debug
            .as_ref()
            .map(|d| d.skip_integrity)
            .unwrap_or(false);
    let integrity_check_yaml = common::generate_integrity_check(skip_integrity);

    // Agent prompt content
    let agent_content_value = build_agent_content(
        front_matter,
        input_path,
        markdown_body,
        &ctx.imported_prompt_body,
        &source_path,
        &trigger_repo_directory,
    )?;

    // ─── Top-level pipeline fields ────────────────────────────────
    let parameters = build_parameters(front_matter)?;
    let resources = build_resources(&front_matter.repositories, &front_matter.on_config)?;
    let triggers = build_triggers(&front_matter.on_config, front_matter)?;

    // ─── Extension declaration fanout ─────────────────────────────
    let (ext_setup_steps, ext_agent_prepare, ext_agent_conditions) =
        fanout_extension_declarations(extensions, extension_declarations)?;

    // Aggregate config for per-job builders
    let cfg = StandaloneCtx {
        pools,
        agent_display_name: agent_display_name.clone(),
        self_checkout_fetch: front_matter
            .checkout_fetch
            .get(SELF_CHECKOUT_ALIAS)
            .cloned()
            .unwrap_or_default(),
        working_directory: working_directory.clone(),
        trigger_repo_directory: trigger_repo_directory.clone(),
        self_repository_name,
        compiler_version: compiler_version.clone(),
        engine_install_steps_yaml,
        detection_engine_install_steps_yaml,
        engine_run,
        engine_run_detection,
        detection_engine_config,
        threat_detection,
        engine_env,
        engine_log_dir,
        allowed_domains,
        detection_allowed_domains,
        awf_mounts,
        awf_path_step_yaml,
        mcpg_config_json,
        custom_tools_json,
        resolved_execution_config_json,
        mcpg_docker_env,
        mcpg_step_env,
        source_path,
        source_relative_path,
        pipeline_path: pipeline_path.clone(),
        acquire_read_token,
        acquire_write_token,
        integrity_check_yaml,
        agent_content_value,
        debug_pipeline,
        byom_exclude_keys,
        detection_byom_exclude_keys,
        detection_engine_env,
    };

    // ─── Build jobs ───────────────────────────────────────────────
    let jobs = build_canonical_jobs(
        front_matter,
        extensions,
        &cfg,
        &ext_setup_steps,
        &ext_agent_prepare,
        &ext_agent_conditions,
        prefix,
    )?;

    Ok(BuiltPipelineContext {
        pipeline_name,
        parameters,
        resources,
        triggers,
        jobs,
    })
}

/// Build the canonical job graph (Setup?, Agent, Detection,
/// SafeOutputs, Teardown?, Conclusion?) used by every target. The optional
/// `prefix` is applied to Agent / Detection / SafeOutputs job IDs
/// (matches the legacy template behaviour: Setup and Teardown stay
/// unprefixed even in `target: job|stage`, see `src/data/job-base.yml`
/// where `{{ setup_job }}` substitutes a literal `- job: Setup`).
///
/// `ext_agent_conditions` is the per-extension contribution to the
/// Agent job's `condition:`. The builder folds it into a single
/// `Condition::And([Condition::Succeeded, ...])` (an empty set
/// leaves the Agent job unconditional).
///
/// Returns jobs with their cross-job `depends_on` edges wired to the
/// correct (possibly prefixed) names.
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_canonical_jobs(
    front_matter: &FrontMatter,
    extensions: &[Extension],
    cfg: &StandaloneCtx,
    ext_setup_steps: &[Step],
    ext_agent_prepare: &[Step],
    ext_agent_conditions: &[Condition],
    prefix: Option<&str>,
) -> Result<Vec<Job>> {
    let p = JobPrefix(prefix);
    let mut jobs = Vec::new();
    if let Some(setup) = build_setup_job(front_matter, extensions, ext_setup_steps, cfg, &p)? {
        jobs.push(setup);
    }
    jobs.push(build_agent_job(
        front_matter,
        extensions,
        ext_agent_prepare,
        ext_agent_conditions,
        cfg,
        &p,
    )?);
    jobs.push(build_detection_job(front_matter, cfg, &p)?);
    if let Some(review) = build_manual_review_job(front_matter, cfg, &p)? {
        jobs.push(review);
    }
    let mut custom_defs = collect_custom_safe_output_job_defs(front_matter, &p)?;
    classify_custom_post_review_dependencies(&mut custom_defs)?;
    let custom_job_ids: Vec<JobId> = custom_defs.iter().map(|d| d.job_id.clone()).collect();
    let custom_direct_reviewed_job_ids: Vec<JobId> = custom_defs
        .iter()
        .filter(|d| d.reviewed)
        .map(|d| d.job_id.clone())
        .collect();
    let custom_automatic_job_ids: Vec<JobId> = custom_defs
        .iter()
        .filter(|d| !d.reviewed && !d.post_review)
        .map(|d| d.job_id.clone())
        .collect();
    for def in &custom_defs {
        jobs.push(build_custom_safe_output_job(def, front_matter, cfg)?);
    }
    // Safe-outputs execution. With manual review, execution may split into an
    // automatic job (runs immediately) and a reviewed job (gated behind the
    // ManualReview approval). Partition decides the shape:
    //   - no reviewed tools           → single default job (unchanged)
    //   - all reviewed tools          → single default job, gated by ManualReview
    //   - mixed (auto + reviewed)     → auto job + reviewed job
    let (auto_all, reviewed_all) = front_matter.partition_safe_outputs_by_approval();
    let custom_tool_names = front_matter.custom_safe_output_tool_names();
    let custom_tool_set: std::collections::HashSet<&str> =
        custom_tool_names.iter().map(String::as_str).collect();
    let auto: Vec<String> = auto_all
        .into_iter()
        .filter(|tool| !custom_tool_set.contains(tool.as_str()))
        .collect();
    let reviewed: Vec<String> = reviewed_all
        .into_iter()
        .filter(|tool| !custom_tool_set.contains(tool.as_str()))
        .collect();
    let has_reviewed_safeoutputs_job = !reviewed.is_empty() && !auto.is_empty();
    // Which variant actually runs `create-pull-request` (and thus needs the
    // `prepare-pr-base` fetch/deepen — issue #1453). In a split it lives in
    // exactly one variant; the other filters it out, so only the running
    // variant should pay for the bundle download + prepare step.
    let create_pr_configured = front_matter.create_pr_config().is_some();
    let create_pr_reviewed = reviewed.iter().any(|t| t == CREATE_PULL_REQUEST_TOOL);
    let safeoutputs_waits_for_review = !reviewed.is_empty() && auto.is_empty();
    let github_issue_tools_configured = front_matter.github_issue_tool_names();
    let github_issue_tools_reviewed: Vec<String> = github_issue_tools_configured
        .iter()
        .filter(|tool| reviewed.contains(tool))
        .cloned()
        .collect();
    let github_issue_tools_automatic: Vec<String> = github_issue_tools_configured
        .iter()
        .filter(|tool| !reviewed.contains(tool))
        .cloned()
        .collect();
    if reviewed.is_empty() || auto.is_empty() {
        jobs.push(build_safeoutputs_job(
            front_matter,
            cfg,
            &p,
            &SafeOutputsVariant::default_single(
                create_pr_configured,
                github_issue_tools_configured,
            )
            .with_excluded_tools(&custom_tool_names),
        )?);
    } else {
        jobs.push(build_safeoutputs_job(
            front_matter,
            cfg,
            &p,
            &SafeOutputsVariant::automatic(
                &reviewed,
                create_pr_configured && !create_pr_reviewed,
                github_issue_tools_automatic,
            )
            .with_excluded_tools(&custom_tool_names),
        )?);
        jobs.push(build_safeoutputs_job(
            front_matter,
            cfg,
            &p,
            &SafeOutputsVariant::reviewed(
                &reviewed,
                create_pr_configured && create_pr_reviewed,
                github_issue_tools_reviewed,
            ),
        )?);
    }
    if let Some(teardown) = build_teardown_job(front_matter, cfg, &p)? {
        jobs.push(teardown);
    }
    if let Some(conclusion) = build_conclusion_job(
        front_matter,
        cfg,
        &p,
        &custom_defs,
        has_reviewed_safeoutputs_job,
    )? {
        jobs.push(conclusion);
    }

    // Wire dependsOn between jobs (graph pass also derives but
    // explicit edges make the YAML match committed lock files).
    wire_explicit_dependencies(
        &mut jobs,
        &p,
        &custom_defs,
        &custom_direct_reviewed_job_ids,
        &custom_automatic_job_ids,
        &custom_job_ids,
        safeoutputs_waits_for_review,
    )?;
    Ok(jobs)
}

/// Job-id prefix helper. Encapsulates the legacy-template quirk that
/// Setup and Teardown jobs stay unprefixed even when other jobs in
/// the same target are prefixed by `generate_stage_prefix`.
pub(crate) struct JobPrefix<'a>(pub Option<&'a str>);

impl<'a> JobPrefix<'a> {
    /// Produce the `JobId` for a canonical job (`Setup` / `Agent` /
    /// `Detection` / `SafeOutputs` / `Teardown` / `Conclusion`).
    /// Setup, Teardown, and Conclusion are always unprefixed; Agent,
    /// Detection, and SafeOutputs are prefixed when a prefix is
    /// provided.
    pub(crate) fn id(&self, base: &str) -> Result<JobId> {
        match (self.0, base) {
            (
                Some(prefix),
                "Agent" | "Detection" | "ManualReview" | "SafeOutputs" | "SafeOutputs_Reviewed",
            ) => JobId::new(format!("{prefix}_{base}")),
            _ => JobId::new(base),
        }
    }

    fn custom_id(&self, tool: &str) -> Result<JobId> {
        let base = format!("Custom_{}", ado_identifier_suffix(tool));
        match self.0 {
            Some(prefix) => JobId::new(format!("{prefix}_{base}")),
            None => JobId::new(base),
        }
    }
}

/// Aggregates the precomputed scalars + YAML fragments threaded into
/// every per-job builder. Lives only inside this module; passed by
/// reference so builders don't take 20+ args each.
pub(crate) struct StandaloneCtx {
    pub(crate) pools: PerJobPools,
    pub(crate) agent_display_name: String,
    /// Fetch tuning for the auto-generated `checkout: self` step, resolved from
    /// a reserved `self` entry in `repos:` (empty ⇒ ADO defaults).
    pub(crate) self_checkout_fetch: CheckoutFetchOpts,
    pub(crate) working_directory: String,
    pub(crate) trigger_repo_directory: String,
    /// Identity of the `self` repository, resolved at compile time from the ADO
    /// git remote. Falls back to the `$(Build.Repository.Name)` macro (with a
    /// compile warning) when no ADO context is available.
    pub(crate) self_repository_name: EnvValue,
    pub(crate) compiler_version: String,
    /// Engine install steps as a YAML string (`Engine::install_steps`
    /// returns YAML today). Lowered through `Step::RawYaml` because
    /// it is opaque user-authored-shaped content from the engine
    /// impl. A future `Engine::install_steps_typed` would lift this
    /// to typed steps.
    pub(crate) engine_install_steps_yaml: String,
    pub(crate) detection_engine_install_steps_yaml: String,
    pub(crate) engine_run: String,
    pub(crate) engine_run_detection: String,
    pub(crate) detection_engine_config: EngineConfig,
    pub(crate) threat_detection: ThreatDetectionConfig,
    /// Composed engine env block — `KEY: VALUE` lines, one per line.
    /// Carried as a string and re-parsed during step emission.
    pub(crate) engine_env: String,
    pub(crate) engine_log_dir: String,
    pub(crate) allowed_domains: String,
    pub(crate) detection_allowed_domains: String,
    /// `--mount` flags for AWF (or `\` placeholder when no mounts).
    pub(crate) awf_mounts: String,
    /// `awf_path_step` YAML body (or empty when no path prepends).
    pub(crate) awf_path_step_yaml: String,
    pub(crate) mcpg_config_json: String,
    /// Compiler-generated dynamic SafeOutputs tool definitions. When present,
    /// the Agent job stages this beside the MCPG config and the hardened
    /// SafeOutputs stdio container reads it through its `/safeoutputs` mount.
    pub(crate) custom_tools_json: Option<String>,
    /// Fully merged, compiler-owned Stage 3 configuration.
    pub(crate) resolved_execution_config_json: String,
    /// `-e KEY=...` docker flags for MCPG.
    pub(crate) mcpg_docker_env: String,
    /// `env:` block for the MCPG step (`env:\n  KEY: ...`).
    pub(crate) mcpg_step_env: String,
    pub(crate) source_path: String,
    /// Validated path to the workflow source relative to the trigger repository.
    /// SafeOutputs variants combine this with their job-local checkout layout.
    pub(crate) source_relative_path: String,
    pub(crate) pipeline_path: String,
    /// `AzureCLI@2` task YAML body (or empty when no read service connection).
    pub(crate) acquire_read_token: String,
    pub(crate) acquire_write_token: String,
    /// `Verify pipeline integrity` step YAML (or empty when skipped).
    pub(crate) integrity_check_yaml: String,
    /// Agent prompt body (either inlined imports or
    /// `{{#runtime-import ...}}` marker).
    pub(crate) agent_content_value: String,
    pub(crate) debug_pipeline: bool,
    /// Actual provider credential env keys present to pass to AWF `--exclude-env`;
    /// empty for non-BYOM. AWF's API proxy itself is always enabled.
    pub(crate) byom_exclude_keys: Vec<String>,
    pub(crate) detection_byom_exclude_keys: Vec<String>,
    /// Validated inherited/overridden custom env for Detection.
    pub(crate) detection_engine_env: Vec<(String, String)>,
}

// ─────────────────────────────────────────────────────────────────────
// Top-level field builders
// ─────────────────────────────────────────────────────────────────────

fn build_parameters(front_matter: &FrontMatter) -> Result<Vec<Parameter>> {
    let has_memory = front_matter
        .tools
        .as_ref()
        .and_then(|t| t.cache_memory.as_ref())
        .is_some_and(|cm| cm.is_enabled());
    let is_template_target = matches!(
        front_matter.target,
        crate::compile::types::CompileTarget::Job | crate::compile::types::CompileTarget::Stage
    );
    let raw = common::build_parameters(&front_matter.parameters, has_memory, is_template_target)?;
    let mut out = Vec::with_capacity(raw.len());
    for p in raw {
        // Validate per existing rules (mirrors common::generate_parameters)
        if !crate::validate::is_valid_parameter_name(&p.name) {
            anyhow::bail!(
                "Invalid parameter name '{}': must match [A-Za-z_][A-Za-z0-9_]* (ADO identifier)",
                p.name
            );
        }
        if let Some(ref display_name) = p.display_name {
            crate::validate::reject_ado_expressions(display_name, &p.name, "displayName")?;
        }
        if let Some(ref default) = p.default {
            crate::validate::reject_ado_expressions_in_value(default, &p.name, "default")?;
        }

        let kind = match p.param_type.as_deref() {
            Some("boolean") => ParameterKind::Boolean,
            Some("number") => ParameterKind::Number,
            Some("object") => ParameterKind::Object,
            _ => ParameterKind::String,
        };
        let default = match (&kind, &p.default) {
            (_, None) => ParameterDefault::None,
            (ParameterKind::Boolean, Some(v)) => match v.as_bool() {
                Some(b) => ParameterDefault::Bool(b),
                None => match v.as_str() {
                    Some("true") => ParameterDefault::Bool(true),
                    Some("false") => ParameterDefault::Bool(false),
                    Some(s) => ParameterDefault::String(s.to_string()),
                    None => ParameterDefault::None,
                },
            },
            (ParameterKind::Number, Some(v)) => match v.as_i64() {
                Some(n) => ParameterDefault::Number(n),
                None => match v.as_str().and_then(|s| s.parse::<i64>().ok()) {
                    Some(n) => ParameterDefault::Number(n),
                    None => ParameterDefault::String(yaml_value_as_string(v)),
                },
            },
            (ParameterKind::Object, Some(v)) => match v {
                serde_yaml::Value::Sequence(items) => ParameterDefault::Sequence(items.clone()),
                _ => ParameterDefault::String(yaml_value_as_string(v)),
            },
            (ParameterKind::String, Some(v)) => ParameterDefault::String(yaml_value_as_string(v)),
        };
        out.push(Parameter {
            name: p.name.clone(),
            display_name: p.display_name.clone(),
            kind,
            default,
            values: p.values.clone().unwrap_or_default(),
        });
    }
    Ok(out)
}

fn yaml_value_as_string(v: &serde_yaml::Value) -> String {
    match v {
        serde_yaml::Value::String(s) => s.clone(),
        serde_yaml::Value::Number(n) => n.to_string(),
        serde_yaml::Value::Bool(b) => b.to_string(),
        _ => serde_yaml::to_string(v)
            .unwrap_or_default()
            .trim()
            .to_string(),
    }
}

fn build_resources(repos: &[RepoCfg], on: &Option<OnConfig>) -> Result<Resources> {
    let mut repositories: Vec<RepositoryResource> = vec![RepositoryResource::SelfRepo {
        clean: true,
        submodules: true,
    }];
    for r in repos {
        repositories.push(RepositoryResource::Named {
            identifier: r.repository.clone(),
            kind: r.repo_type.clone(),
            name: r.name.clone(),
            r#ref: Some(r.repo_ref.clone()),
            endpoint: r.endpoint.clone(),
        });
    }
    // Pipeline-completion triggers surface as `resources.pipelines[]`.
    // Mirrors legacy `generate_pipeline_resources`.
    let mut pipelines: Vec<PipelineResource> = Vec::new();
    if let Some(trigger_config) = on
        && let Some(pipeline) = &trigger_config.pipeline
    {
        // Snake-case identifier from the pipeline display name
        let identifier: String = pipeline
            .name
            .to_lowercase()
            .chars()
            .map(|c| if c.is_alphanumeric() { c } else { '_' })
            .collect();
        pipelines.push(PipelineResource {
            identifier,
            source: pipeline.name.clone(),
            project: pipeline.project.clone(),
            branches: pipeline.branches.clone(),
            // legacy emits `trigger: true` when branches is empty.
            // The lower_pipeline_resource codegen handles the
            // branches.include vs scalar shape.
            trigger: true,
        });
    }
    Ok(Resources {
        repositories,
        pipelines,
    })
}

fn build_triggers(on: &Option<OnConfig>, front_matter: &FrontMatter) -> Result<Triggers> {
    // Schedules — fuzzy schedule parsed once into typed Schedule items.
    let mut schedules: Vec<Schedule> = Vec::new();
    if let Some(s) = front_matter.schedule() {
        let parsed = crate::fuzzy_schedule::parse_fuzzy_schedule(s.expression())?;
        let cron = crate::fuzzy_schedule::generate_cron(&parsed, &front_matter.name);
        let branches = s.branches();
        let branches_include = if branches.is_empty() {
            vec!["main".to_string()]
        } else {
            branches.to_vec()
        };
        schedules.push(Schedule {
            cron,
            display_name: "Scheduled run".to_string(),
            branches_include,
            always: true,
        });
    }

    // `on:` declares when this pipeline runs, and both keys are ALWAYS
    // emitted: Azure DevOps reads a missing `trigger:` / `pr:` key as
    // "run on every branch", not "never run". So absence of an `on.*` key
    // means the corresponding trigger is explicitly disabled, and nothing
    // needs to "suppress" anything.

    // PR trigger — from `on.pr`, else `pr: none`.
    let pr = Some(match on.as_ref().and_then(|o| o.pr.as_ref()) {
        Some(pr_cfg) => build_pr_trigger_from_config(pr_cfg),
        None => PrTrigger::disabled(),
    });

    // CI/push trigger:
    //   - explicit `on.push` always wins, including over the synthetic
    //     mechanism below;
    //   - `on.pr` in the default `synthetic` mode needs CI-triggered builds
    //     to react to (it resolves the open PR for `Build.SourceBranch` at
    //     runtime), so the compiler emits the all-branches trigger as a
    //     MECHANISM for delivering `on.pr` — not as user intent;
    //   - otherwise the pipeline does not start on a push.
    let synthetic_pr = on
        .as_ref()
        .and_then(|o| o.pr.as_ref())
        .is_some_and(|pr_cfg| matches!(pr_cfg.mode, PrMode::Synthetic));
    let ci = Some(match on.as_ref().and_then(|o| o.push.as_ref()) {
        Some(push_cfg) => build_ci_trigger_from_config(push_cfg),
        None if synthetic_pr => CiTrigger::all_branches(),
        None => CiTrigger::disabled(),
    });

    // Pipeline resources — none for standalone today (handled via legacy
    // generate_pipeline_resources but standalone fixtures don't exercise it).
    Ok(Triggers { schedules, pr, ci })
}

/// Build the typed CI trigger from an explicit `on.push` block.
fn build_ci_trigger_from_config(push: &crate::compile::types::PushTriggerConfig) -> CiTrigger {
    use crate::compile::types::PushTriggerConfig;
    let filters = match push {
        PushTriggerConfig::Disabled(_) => return CiTrigger::disabled(),
        PushTriggerConfig::Filtered(f) => f,
    };
    let (b_inc, b_exc) = match &filters.branches {
        Some(b) => (b.include.clone(), b.exclude.clone()),
        None => (Vec::new(), Vec::new()),
    };
    let (p_inc, p_exc) = match &filters.paths {
        Some(p) => (p.include.clone(), p.exclude.clone()),
        None => (Vec::new(), Vec::new()),
    };
    // `push: {}` (or one with only empty filter lists) carries no
    // information; treat it as "every branch" rather than emitting an
    // invalid empty `trigger:` mapping.
    if b_inc.is_empty() && b_exc.is_empty() && p_inc.is_empty() && p_exc.is_empty() {
        return CiTrigger::all_branches();
    }
    CiTrigger {
        branches_include: b_inc,
        branches_exclude: b_exc,
        paths_include: p_inc,
        paths_exclude: p_exc,
        disabled: false,
    }
}

fn build_pr_trigger_from_config(pr: &crate::compile::types::PrTriggerConfig) -> PrTrigger {
    let (b_inc, b_exc) = match &pr.branches {
        Some(b) => (b.include.clone(), b.exclude.clone()),
        None => (Vec::new(), Vec::new()),
    };
    let (p_inc, p_exc) = match &pr.paths {
        Some(p) => (p.include.clone(), p.exclude.clone()),
        None => (Vec::new(), Vec::new()),
    };
    PrTrigger {
        branches_include: b_inc,
        branches_exclude: b_exc,
        paths_include: p_inc,
        paths_exclude: p_exc,
        disabled: false,
    }
}

// ─────────────────────────────────────────────────────────────────────
// Per-job builders
// ─────────────────────────────────────────────────────────────────────

/// Build the optional Setup job. Returns `None` when nothing requires
/// a Setup job (no user setup, no extension setup, no filters).
///
/// **Setup is always unprefixed** even when other jobs in the same
/// target are prefixed by `generate_stage_prefix`. This matches the
/// legacy `generate_setup_job` behaviour (which always emits
/// `- job: Setup` literally) — so the `prefix.id("Setup")` call below
/// returns `JobId::new("Setup")` regardless of prefix state.
fn build_setup_job(
    front_matter: &FrontMatter,
    _extensions: &[Extension],
    ext_setup_steps: &[Step],
    cfg: &StandaloneCtx,
    prefix: &JobPrefix<'_>,
) -> Result<Option<Job>> {
    let has_user_setup = !front_matter.setup.is_empty();
    let has_ext_setup = !ext_setup_steps.is_empty();

    if !has_user_setup && !has_ext_setup {
        return Ok(None);
    }
    let mut steps: Vec<Step> = Vec::new();
    steps.push(checkout_self_step(&cfg.self_checkout_fetch, false));
    steps.extend(ext_setup_steps.iter().cloned());

    // User setup steps as RawYaml — they're arbitrary user-authored ADO YAML
    // that the IR does not model. When filter gates are active, gate the user
    // steps by setting a `condition:` key on each step's mapping before lowering
    // to RawYaml.
    let pr_filters = front_matter.pr_filters();
    let pipeline_filters = front_matter.pipeline_filters();
    let has_pr_gate = pr_filters
        .map(|f| !super::filter_ir::lower_pr_filters(f).is_empty())
        .unwrap_or(false);
    let has_pipeline_gate = pipeline_filters
        .map(|f| !super::filter_ir::lower_pipeline_filters(f).is_empty())
        .unwrap_or(false);
    let gate_condition: Option<String> = match (has_pr_gate, has_pipeline_gate) {
        (true, true) => Some(
            "and(eq(variables['prGate.SHOULD_RUN'], 'true'), eq(variables['pipelineGate.SHOULD_RUN'], 'true'))"
                .to_string(),
        ),
        (true, false) => Some("eq(variables['prGate.SHOULD_RUN'], 'true')".to_string()),
        (false, true) => Some("eq(variables['pipelineGate.SHOULD_RUN'], 'true')".to_string()),
        (false, false) => None,
    };
    for user_step_val in &front_matter.setup {
        let yaml = match gate_condition.as_deref() {
            Some(cond) => {
                // Mutate a clone of the step mapping to inject `condition:`
                let mut step_val = user_step_val.clone();
                if let serde_yaml::Value::Mapping(m) = &mut step_val {
                    m.insert(
                        serde_yaml::Value::String("condition".to_string()),
                        serde_yaml::Value::String(cond.to_string()),
                    );
                }
                step_to_raw_yaml_string(&step_val)?
            }
            None => step_to_raw_yaml_string(user_step_val)?,
        };
        steps.push(Step::RawYaml(yaml));
    }

    let mut job = Job::new(prefix.id("Setup")?, "Setup", cfg.pools.setup.clone());
    job.steps = steps;
    Ok(Some(job))
}

fn build_agent_job(
    front_matter: &FrontMatter,
    extensions: &[Extension],
    ext_agent_prepare: &[Step],
    ext_agent_conditions: &[Condition],
    cfg: &StandaloneCtx,
    prefix: &JobPrefix<'_>,
) -> Result<Job> {
    let mut steps: Vec<Step> = Vec::new();

    // 1. checkout: self
    steps.push(checkout_self_step(
        &cfg.self_checkout_fetch,
        !front_matter.checkout.is_empty(),
    ));
    // 2. additional repo checkouts
    for repo in &front_matter.checkout {
        let fetch = front_matter
            .checkout_fetch
            .get(repo)
            .cloned()
            .unwrap_or_default();
        steps.push(Step::Checkout(CheckoutStep {
            repository: CheckoutRepo::Named(repo.clone()),
            path: Some(format!("s/{repo}")),
            clean: None,
            submodules: None,
            fetch_depth: fetch.depth_for_emit(),
            fetch_tags: fetch.fetch_tags,
            persist_credentials: None,
        }));
    }

    // 3. acquire ADO read token (AzureCLI@2 task) — only when configured.
    push_raw_yaml_if_nonempty(&mut steps, &cfg.acquire_read_token)?;

    // 4. engine install steps (Copilot CLI install). YAML string from
    //    `Engine::install_steps`; lowered through `Step::RawYaml`
    //    until a typed `Engine::install_steps_typed` lands.
    push_raw_yaml_if_nonempty(&mut steps, &cfg.engine_install_steps_yaml)?;

    // 5. Download agentic pipeline compiler
    //    Hoist one NuGetAuthenticate@1 for the whole job when the feed mirror
    //    is active, ahead of the compiler/AWF DownloadPackage@1 steps.
    if let Some(auth) = feed_auth_step(front_matter.supply_chain()) {
        steps.push(auth);
    }
    steps.extend(download_compiler_step(
        &cfg.compiler_version,
        front_matter.supply_chain(),
    ));

    // 6. Integrity check (when not skipped)
    push_raw_yaml_if_nonempty(
        &mut steps,
        &substitute_integrity_check(
            &cfg.integrity_check_yaml,
            &cfg.pipeline_path,
            &cfg.trigger_repo_directory,
        ),
    )?;

    // 7. Prepare tooling (generates MCPG API key, writes MCPG config to staging)
    steps.push(Step::Bash(prepare_mcpg_config_step(
        &cfg.mcpg_config_json,
        cfg.custom_tools_json.as_deref(),
    )?));

    // 8. Prepare tooling - copy binary + config to /tmp
    steps.push(Step::Bash(prepare_tooling_step()));

    // 9. Prepare agent prompt (heredoc)
    steps.push(Step::Bash(prepare_agent_prompt_step(
        &cfg.agent_content_value,
    )?));

    // 10. DockerInstaller@0
    steps.push(Step::Task(DockerInstaller::new("26.1.4").into_step()));

    // 11. Download AWF
    steps.extend(download_awf_step(front_matter.supply_chain()));

    // 12. Pre-pull AWF + MCPG container images.
    steps.extend(prepull_images_step(true, front_matter.supply_chain()));

    // 13. Extension prepare steps (typed) + user steps (RawYaml)
    steps.extend(ext_agent_prepare.iter().cloned());
    for user_step_val in &front_matter.steps {
        steps.push(Step::RawYaml(step_to_raw_yaml_string(user_step_val)?));
    }

    // 14. AWF path step (when extensions declare path prepends)
    push_raw_yaml_if_nonempty(&mut steps, &cfg.awf_path_step_yaml)?;

    // 14a. Credential-isolated Azure DevOps policy engine.
    //
    //      Must precede MCPG: the Azure DevOps MCP is redirected at the
    //      engine's container address, and that address does not exist until
    //      the engine is running.
    let ado_proxy_enabled = common::ado_proxy_enabled(front_matter);
    if ado_proxy_enabled {
        steps.push(Step::Bash(prepare_ado_proxy_network_step()));
        if common::ado_mcp_enabled(front_matter) {
            steps.push(Step::Bash(prepare_ado_mcp_step(common::ado_mcp_version(
                front_matter,
            ))));
        }
        steps.push(Step::Bash(start_ado_proxy_step(front_matter)));
    }

    // 15. MCP Gateway (MCPG), which launches SafeOutputs as a stdio child.
    steps.push(Step::Bash(start_mcpg_step(
        &cfg.mcpg_docker_env,
        &cfg.mcpg_step_env,
        cfg.debug_pipeline,
        front_matter.supply_chain(),
    )?));

    // Both peers must still exist immediately before AWF creates `awf-net`.
    // This catches a detached-process/lifecycle regression here, with each
    // container's own logs, instead of letting AWF fail later with only
    // "No such container".
    if ado_proxy_enabled {
        steps.push(Step::Bash(verify_trusted_topology_peers_step()));
    }

    // 16. Verify MCP backends (debug-only)
    if cfg.debug_pipeline {
        steps.push(Step::Bash(verify_mcp_backends_step()));
    }

    // 17. Run copilot (AWF network isolated) — the big one.
    //     When `create-pull-request` is configured, first fetch/deepen the
    //     target branch so the containerized SafeOutputs MCP server can compute a
    //     diff base on shallow-default agent pools (issue #1413). Runs after
    //     `checkout: self` (step 1) so the clone exists, and before the Copilot
    //     run so the refs are present when the agent proposes a PR. The
    //     `prepare-pr-base.js` bundle is staged by the ado-script extension's
    //     agent-prepare steps (`prepare_pr_base_active` is OR'd into that
    //     extension's Agent-job download predicate), so it is guaranteed present.
    if front_matter.create_pr_config().is_some() {
        // The prepare step deepens every checkout dir the SafeOutputs MCP server
        // may generate a patch from — see `create_pr_prepare_repos`. The
        // compile-time target-inference advisory is emitted here (Agent job)
        // only, so it never double-prints when the same step is also emitted in
        // the SafeOutputs job (issue #1453).
        warn_create_pr_target_inference(front_matter);
        let repos = create_pr_prepare_repos(front_matter, &cfg.trigger_repo_directory);
        steps.push(super::extensions::ado_script::prepare_pr_base_step_typed(
            super::extensions::ado_script::PreparePrBaseMode::PatchBase,
            &repos,
        ));
    }
    //     When GitHub App auth is configured, mint the installation token
    //     immediately before the Copilot run; `copilot_env` sources
    //     `GITHUB_TOKEN` from the masked same-job `GITHUB_APP_TOKEN` the mint
    //     step sets. Never runs for SafeOutputs/user steps.
    //
    //     The ado-script bundle is staged by the ado-script extension's
    //     agent-prepare steps: `github_app_token_active` is OR'd into that
    //     extension's Agent-job download predicate (mirroring
    //     `safe_outputs_summary_active`), so the bundle is guaranteed present by
    //     the time we reach this step — no need to inspect emitted steps or
    //     re-download here.
    if let Some(app_token) = front_matter.engine.github_app_token() {
        steps.push(super::extensions::ado_script::github_app_token_step_typed(
            app_token,
        )?);
    }
    // When an external provider token is configured, mint it in-job (same-job
    // secret) immediately before the Copilot run so COPILOT_PROVIDER_API_KEY
    // resolves via a plain macro. Coexists cleanly with the app-token mint above
    // (independent secret vars, both plain pre-run steps).
    if let Some(token) = front_matter
        .engine
        .provider()
        .and_then(|p| p.token.as_ref())
    {
        steps.push(Step::Task(provider_token_mint_step(token)));
    }
    steps.push(Step::Bash(run_agent_step(
        &cfg.allowed_domains,
        &cfg.awf_mounts,
        &cfg.working_directory,
        &cfg.engine_run,
        &cfg.engine_env,
        &cfg.byom_exclude_keys,
        front_matter.supply_chain(),
        ado_proxy_enabled,
    )?));

    // 18a. Revoke the GitHub App token (best-effort, always) once the Copilot
    //      run has returned, so the minted installation token does not remain
    //      valid for its full lifetime. Skipped when `skip-token-revocation`.
    if let Some(app_token) = front_matter.engine.github_app_token()
        && !app_token.skip_token_revocation
    {
        steps.push(super::extensions::ado_script::github_app_token_revoke_step_typed(app_token)?);
    }

    // 19. Collect safe outputs from AWF container
    steps.push(Step::Bash(collect_safe_outputs_step()));

    // 19a. Render the proposed safe outputs to the build summary tab. Always
    // emitted when any safe-output tool is enabled (transparency for every
    // run); when manual review is configured the reviewed proposals are listed
    // first. The ado-script bundle was delivered earlier in this job by the
    // ado-script extension, gated on the SAME predicate
    // (`has_any_safe_output_tool` → `safe_outputs_summary_active`), so the
    // bundle is downloaded iff this step is emitted.
    if front_matter.has_any_safe_output_tool() {
        let (_, reviewed_summary_tools) = front_matter.partition_safe_outputs_by_approval();
        steps.push(Step::Bash(safe_outputs_summary_step(
            front_matter,
            &reviewed_summary_tools,
        )?));
    }

    // 20. Stop MCPG and SafeOutputs
    steps.push(Step::Bash(stop_mcpg_step()));

    // 20a. Stop the policy engine, then remove its network. `--rm` only fires
    //      on a clean exit, so an OOM or SIGKILL would otherwise leave the
    //      container — and the credential it holds in memory — running past
    //      the job.
    if ado_proxy_enabled {
        steps.push(Step::Bash(stop_ado_proxy_step()));
        steps.push(Step::Bash(teardown_ado_proxy_network_step()));
    }

    // 21. User post_steps (finalize_steps)
    for user_step_val in &front_matter.post_steps {
        steps.push(Step::RawYaml(step_to_raw_yaml_string(user_step_val)?));
    }

    // 22. Copy logs
    steps.push(Step::Bash(copy_logs_step(&cfg.engine_log_dir, false)));

    // 23. Publish artifact
    steps.push(Step::Publish(PublishStep {
        path: "$(Agent.TempDirectory)/staging".to_string(),
        artifact: "agent_outputs_$(Build.BuildId)".to_string(),
        condition: Some(Condition::Always),
    }));

    let _ = extensions; // currently unused after typed declarations gather
    let _ = &cfg.agent_display_name; // friendly name is the pipeline `name:`, not the job displayName
    let mut job = Job::new(prefix.id("Agent")?, "Agent", cfg.pools.agent.clone());
    if let Some(minutes) = front_matter.engine.timeout_minutes() {
        job.timeout = Some(std::time::Duration::from_secs(60 * (minutes as u64)));
    }
    job.steps = steps;
    job.variables = agent_job_variables_hoist(front_matter)?;

    // Agent-job condition: every extension that wants to gate the
    // Agent job contributes typed clauses via
    // [`Declarations::agent_conditions`]. The fold AND-s them
    // together with a leading `succeeded()`; an empty contribution
    // set leaves the Agent job unconditional (matching the pre-lift
    // behaviour).
    //
    // No knowledge of which extensions contribute or what their step
    // IDs / signals are lives here — every clause is owned by the
    // extension that produces the underlying step output.
    job.condition = fold_agent_conditions(ext_agent_conditions);
    Ok(job)
}

/// Fold a slice of extension-supplied Agent-job condition clauses
/// into a single [`Condition::And`] led by [`Condition::Succeeded`].
///
/// Returns [`None`] for an empty slice — that matches the pre-lift
/// behaviour where the Agent job had no `condition:` when no
/// extension contributed. The leading `Succeeded` matches the
/// `succeeded()` atom the previous monolithic
/// `build_agentic_condition` emitted first.
fn fold_agent_conditions(clauses: &[Condition]) -> Option<Condition> {
    if clauses.is_empty() {
        return None;
    }
    let mut parts: Vec<Condition> = Vec::with_capacity(clauses.len() + 1);
    parts.push(Condition::Succeeded);
    parts.extend(clauses.iter().cloned());
    Some(Condition::And(parts))
}

/// Build the Agent-job-level `variables:` block. Typed sibling of
/// `common::generate_agent_job_variables`. Currently emits content
/// **only** when synthetic-PR-from-CI is active.
///
/// Each variable hoists a `synthPr` Setup-job step output to the
/// Agent-job scope via a typed
/// [`EnvValue::Coalesce`]([`EnvValue::StepOutput`]) — the lowering
/// picks the cross-job
/// `$[ coalesce(dependencies.Setup.outputs['synthPr.<name>'], '') ]`
/// form for the cross-job consumer (Agent reading from Setup), which
/// is the only form ADO reliably evaluates at the `variables:` scope.
///
/// Why job-level and not step-level env: ADO step `env:` does NOT
/// evaluate `$[ ... ]` runtime expressions reliably (see PR #956 —
/// empirically broken in msazuresphere/4x4 build #612290 / #612528).
/// Step env then reads the hoisted value via the same-job `$(name)`
/// macro form (see `exec_context/pr.rs::prepare_step_typed`).
fn agent_job_variables_hoist(
    front_matter: &FrontMatter,
) -> Result<Vec<crate::compile::ir::job::JobVariable>> {
    use crate::compile::ir::env::EnvValue;
    use crate::compile::ir::output::OutputRef;

    if !front_matter.is_synthetic_pr() {
        return Ok(Vec::new());
    }
    let synth = StepId::new("synthPr")?;
    let mut out: Vec<JobVariable> = Vec::new();
    for name in super::extensions::ado_script::SYNTH_PR_AGENT_HOIST_NAMES {
        // Single-child `Coalesce` lowers to
        // `coalesce(<child>, '')` so the variable is empty rather
        // than the unresolved literal `$[ ... ]` when the dependency
        // can't be resolved (e.g. Setup was skipped or synthPr did
        // not emit the output).
        out.push(JobVariable {
            name: (*name).to_string(),
            value: EnvValue::coalesce(vec![EnvValue::step_output(OutputRef::new(
                synth.clone(),
                *name,
            ))]),
        });
    }
    Ok(out)
}

/// The Agent-job condition fold lives inline in [`build_agent_job`].
/// Per-extension contributions arrive via
/// [`crate::compile::extensions::Declarations::agent_conditions`]
/// (see `AdoScriptExtension::build_agent_conditions` for today's
/// only contributor — synth-PR-skip, PR-filter gate, pipeline-filter
/// gate, and user `expression:` escape hatches).
/// Whether the Detection job must stage the `ado-script` bundle. The Detection
/// job has no extension-prepare phase (unlike the Agent job, whose bundle
/// download is contributed by `AdoScriptExtension`), so it stages the bundle
/// itself — but gated on this single predicate so exactly one download is
/// emitted. Today only the GitHub App token step needs it; future
/// detection-only bundle consumers should `||` their own condition in here
/// rather than adding a second `install_and_download_steps_typed` call.
fn detection_job_needs_ado_script_bundle(engine_config: &EngineConfig) -> bool {
    engine_config.github_app_token().is_some()
}

fn build_detection_job(
    front_matter: &FrontMatter,
    cfg: &StandaloneCtx,
    prefix: &JobPrefix<'_>,
) -> Result<Job> {
    let mut steps: Vec<Step> = Vec::new();
    steps.push(checkout_self_step(&cfg.self_checkout_fetch, false));
    // Detection job pulls the Agent's output artifact via cross-job download
    steps.push(Step::Download(DownloadStep {
        source: "current".to_string(),
        artifact: "agent_outputs_$(Build.BuildId)".to_string(),
        condition: None,
    }));

    // Prepare safe outputs for analysis
    steps.push(Step::Bash(prepare_safe_outputs_for_analysis(
        &cfg.working_directory,
    )));
    if cfg.threat_detection.is_enabled() {
        // Detection gets its own effective engine install/config path.
        push_raw_yaml_if_nonempty(&mut steps, &cfg.detection_engine_install_steps_yaml)?;
        if let Some(auth) = feed_auth_step(front_matter.supply_chain()) {
            steps.push(auth);
        }
        steps.extend(download_compiler_step(
            &cfg.compiler_version,
            front_matter.supply_chain(),
        ));
        steps.push(Step::Task(DockerInstaller::new("26.1.4").into_step()));
        steps.extend(download_awf_step(front_matter.supply_chain()));
        steps.extend(prepull_images_step(false, front_matter.supply_chain()));

        // include_str! may carry CRLF line endings on Windows; normalise to LF
        // before marker substitution and appending operator instructions.
        let threat_prompt_raw = include_str!("../data/threat-analysis.md");
        let mut threat_prompt = threat_prompt_raw
            .replace("\r\n", "\n")
            .replace("{{ source_path }}", &cfg.source_path)
            .replace("{{ agent_name }}", &cfg.agent_display_name)
            .replace("{{ agent_description }}", &front_matter.description)
            .replace("{{ working_directory }}", &cfg.working_directory);
        if let Some(custom_prompt) = cfg
            .threat_detection
            .prompt
            .as_deref()
            .filter(|prompt| !prompt.is_empty())
        {
            threat_prompt.push_str("\n\n## Additional Instructions\n\n");
            threat_prompt.push_str(&custom_prompt.replace("\r\n", "\n"));
        }
        steps.push(Step::Bash(prepare_threat_analysis_prompt_step(
            &threat_prompt,
        )?));
        steps.push(Step::Bash(setup_compiler_step()));

        // Stage auth support before custom pre-steps, but mint credentials only
        // after them so trusted setup code receives the least privilege needed.
        if detection_job_needs_ado_script_bundle(&cfg.detection_engine_config) {
            steps.extend(
                super::extensions::ado_script::install_and_download_steps_typed(
                    front_matter.supply_chain(),
                ),
            );
        }
        for user_step in &cfg.threat_detection.steps {
            steps.push(Step::RawYaml(step_to_raw_yaml_string(user_step)?));
        }

        if let Some(app_token) = cfg.detection_engine_config.github_app_token() {
            steps.push(super::extensions::ado_script::github_app_token_step_typed(
                app_token,
            )?);
        }
        if let Some(token) = cfg
            .detection_engine_config
            .provider()
            .and_then(|provider| provider.token.as_ref())
        {
            steps.push(Step::Task(provider_token_mint_step(token)));
        }
        steps.push(Step::Bash(run_threat_analysis_step(
            &cfg.detection_allowed_domains,
            &cfg.working_directory,
            &cfg.engine_run_detection,
            &cfg.detection_byom_exclude_keys,
            &cfg.detection_engine_env,
            crate::engine::github_token_source_var(&cfg.detection_engine_config),
            front_matter.supply_chain(),
        )?));
        if let Some(app_token) = cfg.detection_engine_config.github_app_token()
            && !app_token.skip_token_revocation
        {
            steps.push(
                super::extensions::ado_script::github_app_token_revoke_step_typed(app_token)?,
            );
        }
        for user_step in &cfg.threat_detection.post_steps {
            steps.push(Step::RawYaml(step_to_raw_yaml_string(user_step)?));
        }
        steps.push(Step::Bash(prepare_analyzed_outputs_step()));
        steps.push(Step::Bash(evaluate_threat_analysis_step()));
    } else {
        steps.push(Step::Bash(prepare_analyzed_outputs_passthrough_step()));
        steps.push(Step::Bash(threat_analysis_disabled_step()));
    }
    // When manual review is configured, detect whether the agent actually
    // proposed any approval-gated outputs — DECLARES TYPED OUTPUT. The
    // ManualReview gate is conditioned on this so the run never pauses for a
    // human when there is nothing to review.
    let (_, reviewed_tools) = front_matter.partition_safe_outputs_by_approval();
    if !reviewed_tools.is_empty() {
        steps.push(Step::Bash(detect_reviewed_proposals_step(
            &cfg.working_directory,
            &reviewed_tools,
        )));
    }
    let custom_tools = front_matter.custom_safe_output_tool_names();
    if !custom_tools.is_empty() {
        steps.push(Step::Bash(detect_custom_proposals_step(
            &cfg.working_directory,
            &custom_tools,
        )?));
    }
    if cfg.threat_detection.is_enabled() {
        steps.push(Step::Bash(copy_logs_step(&cfg.engine_log_dir, true)));
    }
    // Publish
    steps.push(Step::Publish(PublishStep {
        path: "$(Agent.TempDirectory)/analyzed_outputs".to_string(),
        artifact: "analyzed_outputs_$(Build.BuildId)".to_string(),
        condition: Some(Condition::Always),
    }));

    let mut job = Job::new(
        prefix.id("Detection")?,
        "Detection",
        cfg.pools.detection.clone(),
    );
    if cfg.threat_detection.is_enabled()
        && let Some(minutes) = cfg.detection_engine_config.timeout_minutes()
    {
        job.timeout = Some(std::time::Duration::from_secs(60 * minutes as u64));
    }
    job.steps = steps;
    Ok(job)
}

/// Describes one safe-outputs execution job. The canonical graph emits a
/// single default variant in the common case, or — when manual review splits
/// execution — an automatic variant (`--exclude` the reviewed tools) plus a
/// reviewed variant (`--only` the reviewed tools) gated behind ManualReview.
struct SafeOutputsVariant {
    /// Canonical job base name passed to `JobPrefix::id`.
    base: &'static str,
    /// Job `displayName`.
    display: &'static str,
    /// Published pipeline-artifact name (must be unique per run).
    artifact: &'static str,
    /// Trailing `--only`/`--exclude` flags for `ado-aw execute` (or empty).
    filter_args: String,
    /// Whether THIS variant actually executes `create-pull-request`. In a
    /// split-approval config only one of the two variants runs the tool (the
    /// other filters it out via `--only`/`--exclude`), so only that variant
    /// needs the `prepare-pr-base` fetch/deepen + the ado-script bundle download
    /// (issue #1453 review). Avoids a wasted Node install + bundle fetch +
    /// prepare step in the variant that will never open a PR.
    runs_create_pull_request: bool,
    github_issue_tools: Vec<String>,
    /// Whether this is the manual-review-gated `SafeOutputs_Reviewed` variant.
    /// Used to select the correct pool override without relying on the job name.
    is_reviewed: bool,
}

/// Checkout-dependent paths for one SafeOutputs job.
///
/// Split approval can produce two Stage 3 jobs with different checkout sets,
/// so workflow-global paths cannot be reused blindly by both variants.
struct SafeOutputsCheckoutLayout {
    source_path: String,
    self_repository_directory: String,
    multi_checkout: bool,
}

impl SafeOutputsCheckoutLayout {
    fn for_variant(
        front_matter: &FrontMatter,
        cfg: &StandaloneCtx,
        variant: &SafeOutputsVariant,
    ) -> Self {
        let has_additional_checkouts =
            variant.runs_create_pull_request && !front_matter.checkout.is_empty();
        let self_repository_directory = if has_additional_checkouts {
            // This job emits the same multi-checkout layout as the Agent job, so
            // `self` sits at the compiler-owned `MULTI_CHECKOUT_SELF_PATH`. That
            // is exactly what `generate_trigger_repo_directory` produces for a
            // non-empty checkout list, which is how `cfg.trigger_repo_directory`
            // was built — assert the two stay in agreement.
            debug_assert_eq!(
                cfg.trigger_repo_directory,
                common::MULTI_CHECKOUT_SELF_DIRECTORY,
                "multi-checkout self directory must match the fixed `s/self` path"
            );
            cfg.trigger_repo_directory.clone()
        } else {
            // Only `checkout: self` runs here, so ADO places it at the root
            // regardless of what the workflow-wide layout looks like.
            common::generate_trigger_repo_directory(&[])
        };
        let source_path = format!("{}/{}", self_repository_directory, cfg.source_relative_path);

        Self {
            source_path,
            self_repository_directory,
            multi_checkout: has_additional_checkouts,
        }
    }
}

impl SafeOutputsVariant {
    /// The default single-job variant: no filter, canonical names. Runs every
    /// configured tool, so it executes `create-pull-request` iff configured.
    fn default_single(runs_create_pull_request: bool, github_issue_tools: Vec<String>) -> Self {
        Self {
            base: "SafeOutputs",
            display: "SafeOutputs",
            artifact: "safe_outputs",
            filter_args: String::new(),
            runs_create_pull_request,
            github_issue_tools,
            is_reviewed: false,
        }
    }

    /// The automatic variant in a split: excludes every reviewed tool. Runs
    /// `create-pull-request` only when it is configured and NOT review-gated.
    fn automatic(
        reviewed: &[String],
        runs_create_pull_request: bool,
        github_issue_tools: Vec<String>,
    ) -> Self {
        Self {
            base: "SafeOutputs",
            display: "SafeOutputs",
            artifact: "safe_outputs",
            filter_args: filter_flags("--exclude", reviewed),
            runs_create_pull_request,
            github_issue_tools,
            is_reviewed: false,
        }
    }

    /// The reviewed variant in a split: runs only the reviewed tools. Runs
    /// `create-pull-request` only when it is configured and review-gated.
    fn reviewed(
        reviewed: &[String],
        runs_create_pull_request: bool,
        github_issue_tools: Vec<String>,
    ) -> Self {
        Self {
            base: "SafeOutputs_Reviewed",
            display: "SafeOutputs (reviewed)",
            artifact: "safe_outputs_reviewed",
            filter_args: filter_flags("--only", reviewed),
            runs_create_pull_request,
            github_issue_tools,
            is_reviewed: true,
        }
    }

    fn with_excluded_tools(mut self, excluded: &[String]) -> Self {
        if excluded.is_empty() {
            return self;
        }
        let mut flags = self.filter_args;
        flags.push_str(&filter_flags("--exclude", excluded));
        self.filter_args = flags;
        self
    }
}

/// Build a ` --<flag> <tool>` run for `ado-aw execute` (leading space so it
/// concatenates onto the fixed command). Tool names are spliced into the bash
/// command without per-name shell quoting; this is safe because they are
/// compiler-controlled safe-output identifiers restricted to ASCII
/// alphanumeric/hyphen (no shell metacharacters). The invariant is enforced by
/// `validate::is_safe_tool_name` via `common::validate_safe_outputs_keys`,
/// which `build_pipeline_context` runs before `build_canonical_jobs` reaches
/// this function.
fn filter_flags(flag: &str, tools: &[String]) -> String {
    let mut s = String::new();
    for t in tools {
        s.push_str(&format!(" {flag} {t}"));
    }
    s
}

#[derive(Debug, Clone)]
struct CustomSafeOutputJobDef {
    name: String,
    job_id: JobId,
    reviewed: bool,
    post_review: bool,
    env: Vec<(String, String)>,
    steps: Vec<serde_json::Value>,
    display_name: Option<String>,
    authored_condition: Option<String>,
    needs: Vec<String>,
    timeout_minutes: Option<u32>,
    staged: bool,
}

fn ado_identifier_suffix(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    if out
        .chars()
        .next()
        .is_some_and(|ch| ch.is_ascii_alphabetic() || ch == '_')
    {
        out
    } else {
        format!("_{out}")
    }
}

fn custom_tool_output_var(tool: &str) -> String {
    format!("HasCustom_{}", ado_identifier_suffix(tool))
}

fn collect_custom_safe_output_job_defs(
    front_matter: &FrontMatter,
    prefix: &JobPrefix<'_>,
) -> Result<Vec<CustomSafeOutputJobDef>> {
    let (_, reviewed) = front_matter.partition_safe_outputs_by_approval();
    let reviewed: std::collections::HashSet<&str> = reviewed.iter().map(String::as_str).collect();
    collect_custom_tool_definitions(front_matter)?
        .into_iter()
        .map(|definition| {
            let staged = front_matter.tool_is_staged(&definition.name);
            custom_job_def(definition, &reviewed, prefix, staged)
        })
        .collect()
}

fn custom_job_def(
    definition: CustomToolDefinition,
    reviewed: &std::collections::HashSet<&str>,
    prefix: &JobPrefix<'_>,
    staged: bool,
) -> Result<CustomSafeOutputJobDef> {
    for step in &definition.steps {
        validate_custom_job_step(&definition.name, step)?;
    }
    Ok(CustomSafeOutputJobDef {
        job_id: prefix.custom_id(&definition.name)?,
        reviewed: reviewed.contains(definition.name.as_str()),
        post_review: false,
        name: definition.name,
        env: definition.env,
        steps: definition.steps,
        display_name: definition.display_name,
        authored_condition: definition.condition,
        needs: definition.needs,
        timeout_minutes: definition.timeout_minutes,
        staged,
    })
}

fn classify_custom_post_review_dependencies(defs: &mut [CustomSafeOutputJobDef]) -> Result<()> {
    let indexes: std::collections::HashMap<String, usize> = defs
        .iter()
        .enumerate()
        .map(|(index, definition)| (definition.name.clone(), index))
        .collect();

    for definition in defs.iter() {
        for dependency in &definition.needs {
            anyhow::ensure!(
                indexes.contains_key(dependency)
                    || super::custom_tools::CUSTOM_JOB_SYSTEM_NEEDS.contains(&dependency.as_str()),
                "safe-outputs.jobs.{}.needs references unknown job '{}'",
                definition.name,
                dependency
            );
            anyhow::ensure!(
                dependency != &definition.name,
                "safe-outputs.jobs.{}.needs cannot depend on itself",
                definition.name
            );
        }
    }

    fn visit(
        index: usize,
        defs: &[CustomSafeOutputJobDef],
        indexes: &std::collections::HashMap<String, usize>,
        states: &mut [u8],
        stack: &mut Vec<String>,
    ) -> Result<()> {
        if states[index] == 2 {
            return Ok(());
        }
        if states[index] == 1 {
            stack.push(defs[index].name.clone());
            anyhow::bail!(
                "safe-outputs.jobs dependency cycle detected: {}",
                stack.join(" -> ")
            );
        }
        states[index] = 1;
        stack.push(defs[index].name.clone());
        for dependency in &defs[index].needs {
            if let Some(dependency_index) = indexes.get(dependency) {
                visit(*dependency_index, defs, indexes, states, stack)?;
            }
        }
        stack.pop();
        states[index] = 2;
        Ok(())
    }

    let mut states = vec![0_u8; defs.len()];
    for index in 0..defs.len() {
        visit(index, defs, &indexes, &mut states, &mut Vec::new())?;
    }

    let mut changed = true;
    while changed {
        changed = false;
        for index in 0..defs.len() {
            if defs[index].reviewed || defs[index].post_review {
                continue;
            }
            let follows_reviewed = defs[index].needs.iter().any(|dependency| {
                indexes.get(dependency).is_some_and(|dependency_index| {
                    defs[*dependency_index].reviewed || defs[*dependency_index].post_review
                }) || dependency == "safe-outputs-reviewed"
            });
            if follows_reviewed {
                defs[index].post_review = true;
                changed = true;
            }
        }
    }
    Ok(())
}

fn validate_custom_job_step(tool: &str, step: &serde_json::Value) -> Result<()> {
    let object = step.as_object().ok_or_else(|| {
        anyhow::anyhow!("safe-outputs.jobs.{tool}.steps entries must be mappings")
    })?;
    for forbidden in ["template", "checkout", "container", "target"] {
        anyhow::ensure!(
            !object.contains_key(forbidden),
            "safe-outputs.jobs.{tool}.steps: '{forbidden}' is not supported; custom jobs \
             must use self-contained inline steps or explicitly versioned ADO tasks"
        );
    }
    let execution_keys = ["bash", "powershell", "pwsh", "task"];
    let execution_count = execution_keys
        .iter()
        .filter(|key| object.contains_key(**key))
        .count();
    anyhow::ensure!(
        execution_count == 1,
        "safe-outputs.jobs.{tool}.steps entries must define exactly one of: {}",
        execution_keys.join(", ")
    );
    // Only the step-level map becomes process environment; `inputs.env` is
    // ordinary task input and cannot shadow compiler-owned job variables.
    if let Some(env) = object.get("env").and_then(serde_json::Value::as_object) {
        for key in ["ADO_AW_AGENT_OUTPUT", "ADO_AW_SAFE_OUTPUTS_STAGED"] {
            anyhow::ensure!(
                !env.contains_key(key),
                "safe-outputs.jobs.{tool}.steps env key '{key}' is compiler-owned"
            );
        }
    }
    if let Some(task) = object.get("task").and_then(serde_json::Value::as_str) {
        let Some((_, version)) = task.rsplit_once('@') else {
            anyhow::bail!(
                "safe-outputs.jobs.{tool}.steps task '{task}' must include an explicit version"
            );
        };
        anyhow::ensure!(
            !version.is_empty() && version.chars().all(|ch| ch.is_ascii_digit()),
            "safe-outputs.jobs.{tool}.steps task '{task}' must use an explicit numeric version"
        );
    }
    let yaml =
        serde_yaml::to_value(step).context("failed to convert custom job step for validation")?;
    if let Some(Err(message)) = super::ir::tasks::parse::validate_task_step(&yaml) {
        anyhow::bail!("safe-outputs.jobs.{tool}.steps has invalid task input: {message}");
    }
    for removed in ["ADO_AW_SAFE_OUTPUT_PROPOSALS", "ADO_AW_SAFE_OUTPUT_RESULTS"] {
        anyhow::ensure!(
            !custom_step_references_removed_variable(object, removed),
            "safe-outputs.jobs.{tool}.steps references removed variable {removed}; use \
             ADO_AW_AGENT_OUTPUT"
        );
    }
    Ok(())
}

fn custom_step_references_removed_variable(
    step: &serde_json::Map<String, serde_json::Value>,
    variable: &str,
) -> bool {
    let env_references = step
        .get("env")
        .and_then(serde_json::Value::as_object)
        .is_some_and(|env| {
            env.contains_key(variable)
                || env
                    .values()
                    .any(|value| json_value_references_variable(value, variable))
        });
    let runtime_field_references = ["bash", "powershell", "pwsh", "inputs", "workingDirectory"]
        .iter()
        .filter_map(|field| step.get(*field))
        .any(|value| json_value_references_variable(value, variable));
    let condition_references = step
        .get("condition")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|condition| condition.contains(variable));

    env_references || runtime_field_references || condition_references
}

fn json_value_references_variable(value: &serde_json::Value, variable: &str) -> bool {
    match value {
        serde_json::Value::String(value) => {
            value.contains(&format!("$({variable})"))
                || value.contains(&format!("${variable}"))
                || value.contains(&format!("${{{variable}}}"))
                || value
                    .to_ascii_uppercase()
                    .contains(&format!("$ENV:{variable}"))
                || value.contains(&format!("%{variable}%"))
        }
        serde_json::Value::Array(values) => values
            .iter()
            .any(|value| json_value_references_variable(value, variable)),
        serde_json::Value::Object(values) => values
            .values()
            .any(|value| json_value_references_variable(value, variable)),
        _ => false,
    }
}

fn build_custom_safe_output_job(
    def: &CustomSafeOutputJobDef,
    front_matter: &FrontMatter,
    cfg: &StandaloneCtx,
) -> Result<Job> {
    let mut steps = vec![checkout_none_step()];
    steps.push(Step::Download(DownloadStep {
        source: "current".to_string(),
        artifact: "analyzed_outputs_$(Build.BuildId)".to_string(),
        condition: None,
    }));
    if let Some(auth) = feed_auth_step(front_matter.supply_chain()) {
        steps.push(auth);
    }
    steps.extend(download_compiler_step(
        &cfg.compiler_version,
        front_matter.supply_chain(),
    ));
    steps.push(Step::Bash(prepare_custom_executor_binary_step()));
    let config_path = "$(Agent.TempDirectory)/ado-aw-custom-tools.json";
    let agent_output_path = "$(Agent.TempDirectory)/ado-aw-agent-output.json";
    steps.push(Step::Bash(write_custom_runtime_config_step(
        &cfg.resolved_execution_config_json,
        config_path,
    )?));
    steps.push(Step::Bash(prepare_custom_agent_output_step(
        config_path,
        agent_output_path,
    )));
    for component_step in &def.steps {
        let step =
            serde_yaml::to_value(component_step).context("failed to convert custom job step")?;
        steps.push(Step::RawYaml(component_step_with_custom_env(
            &step,
            &def.env,
            agent_output_path,
            def.staged,
        )?));
    }

    let custom_pool = if def.reviewed || def.post_review {
        cfg.pools.safe_outputs_reviewed.clone()
    } else {
        cfg.pools.safe_outputs.clone()
    };
    let mut job = Job::new(
        def.job_id.clone(),
        def.display_name
            .clone()
            .unwrap_or_else(|| format!("Custom safe output: {}", def.name)),
        custom_pool,
    );
    job.steps = steps;
    job.condition = Some(custom_job_condition(def)?);
    if let Some(minutes) = def.timeout_minutes {
        job.timeout = Some(std::time::Duration::from_secs(u64::from(minutes) * 60));
    }
    Ok(job)
}

fn custom_job_condition(def: &CustomSafeOutputJobDef) -> Result<Condition> {
    let mut parts = vec![
        Condition::Succeeded,
        Condition::Eq(
            Expr::StepOutput(OutputRef::new(
                StepId::new("threatAnalysis")?,
                "SafeToProcess",
            )),
            Expr::Literal("true".to_string()),
        ),
        Condition::Eq(
            Expr::StepOutput(OutputRef::new(
                StepId::new(CUSTOM_PROPOSALS_STEP_ID)?,
                custom_tool_output_var(&def.name),
            )),
            Expr::Literal("true".to_string()),
        ),
    ];
    if def.reviewed {
        parts.push(Condition::Eq(
            Expr::StepOutput(OutputRef::new(
                StepId::new("reviewedProposals")?,
                "HasReviewedProposals",
            )),
            Expr::Literal("true".to_string()),
        ));
    }
    if let Some(condition) = &def.authored_condition {
        parts.push(Condition::Custom(condition.clone()));
    }
    Ok(Condition::And(parts))
}

shell_script! {
    /// Prepare the compiler binary at the well-known `/tmp/awf-tools/ado-aw`
    /// location the custom safe-output executor picks up.
    PREPARE_CUSTOM_EXECUTOR_BINARY {
        interpreter: Bash,
        bindings: [],
        externals: [],
        fragments: [],
        body: r#"
mkdir -p /tmp/awf-tools
AGENTIC_PIPELINES_PATH="$(Pipeline.Workspace)/agentic-pipeline-compiler/ado-aw"
chmod +x "$AGENTIC_PIPELINES_PATH"
cp "$AGENTIC_PIPELINES_PATH" /tmp/awf-tools/ado-aw
chmod +x /tmp/awf-tools/ado-aw
"#,
    }
}

shell_script! {
    /// Add the downloaded compiler binary to PATH and mark it executable.
    ADD_COMPILER_TO_PATH {
        interpreter: Bash,
        bindings: [],
        externals: [],
        fragments: [],
        body: r###"
ls -la "$(Pipeline.Workspace)/agentic-pipeline-compiler"
chmod +x "$(Pipeline.Workspace)/agentic-pipeline-compiler/ado-aw"
echo "##vso[task.prependpath]$(Pipeline.Workspace)/agentic-pipeline-compiler"
"###,
    }
}

shell_script! {
    /// Create the per-job staging output directory.
    PREPARE_OUTPUT_DIRECTORY {
        interpreter: Bash,
        bindings: [AGENT_TEMP],
        externals: [],
        fragments: [],
        body: r#"
mkdir -p "$AGENT_TEMP/staging"
"#,
    }
}

fn prepare_custom_executor_binary_step() -> BashStep {
    ShellScript::new(&PREPARE_CUSTOM_EXECUTOR_BINARY)
        .into_step("Prepare custom safe-output executor")
}

shell_script! {
    /// Write the compiler-generated custom-tools runtime config to a file.
    /// The payload is base64-encoded at compile time so no re-parsing is
    /// needed at runtime — `base64 --decode` is the last command, so ADO's
    /// fail-on-last-command default surfaces a corrupted transfer.
    WRITE_CUSTOM_RUNTIME_CONFIG {
        interpreter: Bash,
        bindings: [ENCODED, AGENT_TEMP, CONFIG_FILENAME],
        externals: [],
        fragments: [],
        body: r#"
mkdir -p "$AGENT_TEMP/ado-aw-custom"
printf '%s' "$ENCODED" | base64 --decode > "$AGENT_TEMP/$CONFIG_FILENAME"
"#,
    }
}

fn write_custom_runtime_config_step(
    custom_tools_json: &str,
    config_path: &str,
) -> Result<BashStep> {
    let parsed: serde_json::Value = serde_json::from_str(custom_tools_json)
        .context("failed to parse compiler-generated custom tools config")?;
    let json = serde_json::to_string_pretty(&parsed)
        .context("failed to serialize custom job runtime config")?;
    let encoded = STANDARD.encode(json.as_bytes());
    let filename = agent_temp_filename(config_path);
    Ok(ShellScript::new(&WRITE_CUSTOM_RUNTIME_CONFIG)
        .bind_text("ENCODED", encoded)
        .bind("AGENT_TEMP", Binding::ado_macro("Agent.TempDirectory"))
        .bind_text("CONFIG_FILENAME", filename)
        .into_step("Write custom job runtime config"))
}

shell_script! {
    /// Materialise the aggregate `ADO_AW_AGENT_OUTPUT` payload for a
    /// custom safe-output job by invoking `ado-aw execute` in
    /// `--prepare-custom-agent-output` mode.
    PREPARE_CUSTOM_AGENT_OUTPUT {
        interpreter: Bash,
        bindings: [AGENT_TEMP, PIPELINE_WORKSPACE, BUILD_ID, CONFIG_FILENAME, OUTPUT_FILENAME],
        externals: [],
        fragments: [],
        body: r#"
/tmp/awf-tools/ado-aw execute \
  --safe-output-dir "$PIPELINE_WORKSPACE/analyzed_outputs_$BUILD_ID" \
  --resolved-config "$AGENT_TEMP/$CONFIG_FILENAME" \
  --prepare-custom-agent-output "$AGENT_TEMP/$OUTPUT_FILENAME"
"#,
    }
}

fn prepare_custom_agent_output_step(config_path: &str, output_path: &str) -> BashStep {
    ShellScript::new(&PREPARE_CUSTOM_AGENT_OUTPUT)
        .bind("AGENT_TEMP", Binding::ado_macro("Agent.TempDirectory"))
        .bind(
            "PIPELINE_WORKSPACE",
            Binding::ado_macro("Pipeline.Workspace"),
        )
        .bind("BUILD_ID", Binding::ado_macro("Build.BuildId"))
        .bind_text("CONFIG_FILENAME", agent_temp_filename(config_path))
        .bind_text("OUTPUT_FILENAME", agent_temp_filename(output_path))
        .into_step("Prepare custom Agent output")
}

/// Extract the filename portion of an `$(Agent.TempDirectory)/<filename>`
/// path so it can be passed through [`Binding::text`] (which forbids `$(`)
/// while the `$(Agent.TempDirectory)` prefix is contributed separately as
/// [`Binding::ado_macro`].
fn agent_temp_filename(path: &str) -> String {
    let prefix = "$(Agent.TempDirectory)/";
    path.strip_prefix(prefix)
        .unwrap_or_else(|| panic!(
            "custom-tools config path {path:?} must start with {prefix:?}"
        ))
        .to_string()
}

fn component_step_with_custom_env(
    step: &serde_yaml::Value,
    custom_env: &[(String, String)],
    agent_output_path: &str,
    staged: bool,
) -> Result<String> {
    let mut step = step.clone();
    let mapping = step.as_mapping_mut().ok_or_else(|| {
        anyhow::anyhow!("safe-outputs.jobs.<tool>.steps entries must be YAML mappings")
    })?;
    let env_map = mapping
        .entry(serde_yaml::Value::String("env".to_string()))
        .or_insert_with(|| serde_yaml::Value::Mapping(serde_yaml::Mapping::new()))
        .as_mapping_mut()
        .ok_or_else(|| {
            anyhow::anyhow!("safe-outputs.jobs.<tool>.steps env blocks must be mappings")
        })?;
    for (name, value) in custom_env {
        let key = serde_yaml::Value::String(name.clone());
        if !env_map.contains_key(&key) {
            env_map.insert(key, serde_yaml::Value::String(value.clone()));
        }
    }
    env_map.insert(
        serde_yaml::Value::String("ADO_AW_AGENT_OUTPUT".to_string()),
        serde_yaml::Value::String(agent_output_path.to_string()),
    );
    env_map.insert(
        serde_yaml::Value::String("ADO_AW_SAFE_OUTPUTS_STAGED".to_string()),
        serde_yaml::Value::String(staged.to_string()),
    );
    step_to_raw_yaml_string(&step)
}

fn shell_quote(raw: &str) -> String {
    format!("'{}'", raw.replace('\'', "'\\''"))
}

/// Build the `(dir, target-branch)` pairs the `prepare-pr-base` bundle must
/// fetch/deepen — one per allowed `create-pull-request` repo. `self` uses its
/// exact checkout directory, while named repositories are siblings beneath
/// `$(Build.SourcesDirectory)`. Each dir is paired with THAT repo's resolved
/// target branch
/// (`CreatePrConfig::resolve_target_branch` — explicit override, inferred
/// checkout ref, or the literal default), so a PR to any repo deepens the branch
/// it actually targets. A single `self` checkout ⇒ one pair. Returns an empty
/// vec when `create-pull-request` is not configured.
///
/// Pure (no diagnostics): the compile-time target-inference advisory is emitted
/// separately by [`warn_create_pr_target_inference`] so it prints exactly once
/// even though the prepare step is emitted in both the Agent and SafeOutputs
/// jobs (issue #1453).
fn create_pr_prepare_repos(
    front_matter: &FrontMatter,
    self_repository_directory: &str,
) -> Vec<super::extensions::ado_script::PreparePrBaseRepo> {
    use super::extensions::ado_script::PreparePrBaseRepo;

    let Some(pr_cfg) = front_matter.create_pr_config() else {
        return Vec::new();
    };
    let repo_refs = front_matter.checkout_repo_refs();
    let mut repos = vec![PreparePrBaseRepo {
        dir: self_repository_directory.to_string(),
        // Read BUILD_SOURCEBRANCH directly in the Node process. Embedding the
        // runtime branch value into bash argv would make valid `$()`/backtick
        // ref characters subject to shell command substitution.
        source_ref: None,
        target_branch: pr_cfg.resolve_target_branch("self", &repo_refs),
    }];
    for alias in &front_matter.checkout {
        repos.push(PreparePrBaseRepo {
            dir: format!("$(Build.SourcesDirectory)/{alias}"),
            source_ref: repo_refs.get(alias).cloned(),
            target_branch: pr_cfg.resolve_target_branch(alias, &repo_refs),
        });
    }
    repos
}

/// Emit the compile-time advisory when `create-pull-request`'s
/// `infer-target-from-checkout-ref` would resolve a non-branch ref (e.g. a tag)
/// as a PR base. `resolve_target_branch` would hand back the whole ref, and
/// Stage 3 builds `refs/heads/<ref>` → a PR into `refs/heads/refs/tags/v1` that
/// ADO rejects with a generic error. Advisory, not fatal: the repo may be a
/// dependency checkout the agent never opens a PR against — an explicit
/// `target-branches:` entry silences it. Called once (Agent job) so it never
/// double-prints alongside the SafeOutputs-job prepare step.
fn warn_create_pr_target_inference(front_matter: &FrontMatter) {
    let Some(pr_cfg) = front_matter.create_pr_config() else {
        return;
    };
    if !pr_cfg.infer_target_from_checkout_ref {
        return;
    }
    let repo_refs = front_matter.checkout_repo_refs();
    for alias in &front_matter.checkout {
        if !pr_cfg.target_branches.contains_key(alias)
            && let Some(git_ref) = repo_refs.get(alias)
            && !git_ref.starts_with("refs/heads/")
        {
            eprintln!(
                "Warning: create-pull-request infer-target-from-checkout-ref is set, but \
                checkout repo '{alias}' is at '{git_ref}', which is not a branch \
                (refs/heads/*). A PR into this repo would target an invalid ref. Set an \
                explicit `target-branches: {{ {alias}: <branch> }}` if the agent opens a PR \
                against it."
            );
        }
    }
}

fn build_safeoutputs_job(
    front_matter: &FrontMatter,
    cfg: &StandaloneCtx,
    prefix: &JobPrefix<'_>,
    variant: &SafeOutputsVariant,
) -> Result<Job> {
    let layout = SafeOutputsCheckoutLayout::for_variant(front_matter, cfg, variant);
    let github_auth = if !variant.github_issue_tools.is_empty() {
        front_matter.github_safe_outputs_auth_for_tools(&variant.github_issue_tools)?
    } else {
        None
    };
    let github_app = github_auth
        .as_ref()
        .and_then(crate::compile::types::GithubSafeOutputsAuth::app_config);
    let mut steps: Vec<Step> = Vec::new();
    steps.push(checkout_self_step(
        &cfg.self_checkout_fetch,
        layout.multi_checkout,
    ));
    // When `create-pull-request` is configured and there are additional
    // checked-out repos, the SafeOutputs job must replicate the Agent job's
    // multi-checkout layout (issue #1731). Without these checkouts:
    //   • The additional repo directories don't exist in the SafeOutputs
    //     workspace, so `prepare-pr-base.js` and `ado-aw execute` fail.
    // `self` uses the compiler-owned `s/self` path in this layout so neither a
    // repository-resource trigger nor an additional alias can change where the
    // executor finds the workflow source.
    // Only emit these for the variant that actually runs `create-pull-request`;
    // other variants (and split-approval auto-SafeOutputs) don't need them.
    if variant.runs_create_pull_request {
        for repo in &front_matter.checkout {
            let fetch = front_matter
                .checkout_fetch
                .get(repo)
                .cloned()
                .unwrap_or_default();
            steps.push(Step::Checkout(CheckoutStep {
                repository: CheckoutRepo::Named(repo.clone()),
                path: Some(format!("s/{repo}")),
                clean: None,
                submodules: None,
                fetch_depth: fetch.depth_for_emit(),
                fetch_tags: fetch.fetch_tags,
                persist_credentials: None,
            }));
        }
    }
    // Acquire write token (when configured)
    push_raw_yaml_if_nonempty(&mut steps, &cfg.acquire_write_token)?;
    // Download analyzed outputs
    steps.push(Step::Download(DownloadStep {
        source: "current".to_string(),
        artifact: "analyzed_outputs_$(Build.BuildId)".to_string(),
        condition: None,
    }));
    // Download compiler
    //    One NuGetAuthenticate@1 for the whole SafeOutputs job (feed mirror).
    if let Some(auth) = feed_auth_step(front_matter.supply_chain()) {
        steps.push(auth);
    }
    steps.extend(download_compiler_step(
        &cfg.compiler_version,
        front_matter.supply_chain(),
    ));
    // Add compiler to path
    steps.push(Step::Bash(
        ShellScript::new(&ADD_COMPILER_TO_PATH).into_step("Add agentic compiler to path"),
    ));
    // Prepare output directory
    steps.push(Step::Bash(
        ShellScript::new(&PREPARE_OUTPUT_DIRECTORY)
            .bind("AGENT_TEMP", Binding::ado_macro("Agent.TempDirectory"))
            .into_step("Prepare output directory"),
    ));
    // When `create-pull-request` is configured, fetch/deepen each target branch
    // in THIS job's checkout, immediately before the executor runs (issue
    // #1453). The prepare step also runs in the Agent job (for the containerized
    // SafeOutputs MCP diff base), but each ADO job gets an isolated checkout, so
    // the Agent-job fetch is invisible here — the `create-pull-request` executor
    // (`ado-aw execute`) builds its worktree from `origin/<target>` in the
    // SafeOutputs checkout and needs the ref landed locally. Stage the ado-script
    // bundle in this job (it is otherwise only staged in the Agent/Setup jobs),
    // then emit the same `prepare-pr-base` step. The bundle auth projects
    // `System.AccessToken` (the build identity the checkout persists credentials
    // for), so the git fetch is authenticated regardless of the write token.
    if variant.runs_create_pull_request || github_app.is_some() {
        steps.extend(
            super::extensions::ado_script::install_and_download_steps_typed(
                front_matter.supply_chain(),
            ),
        );
    }
    if variant.runs_create_pull_request {
        let repos = create_pr_prepare_repos(front_matter, &layout.self_repository_directory);
        steps.push(super::extensions::ado_script::prepare_pr_base_step_typed(
            super::extensions::ado_script::PreparePrBaseMode::TargetWorktree,
            &repos,
        ));
    }
    if let Some(app) = github_app {
        let permissions =
            front_matter.github_app_permissions_for_tools(&variant.github_issue_tools)?;
        steps.push(
            super::extensions::ado_script::github_app_token_step_typed_for(
                app,
                crate::compile::types::SAFE_OUTPUTS_GITHUB_APP_TOKEN_VAR,
                "Mint GitHub App token (SafeOutputs)",
                &permissions,
            )?,
        );
    }
    let executor_ado_env = common::generate_executor_ado_env(
        front_matter
            .permissions
            .as_ref()
            .and_then(|permissions| permissions.write.as_ref())
            .map(crate::compile::types::WritePermissionConfig::service_connection),
        github_auth.as_ref(),
    );
    let resolved_config_path = "$(Agent.TempDirectory)/ado-aw-resolved-config.json";
    steps.push(Step::Bash(write_custom_runtime_config_step(
        &cfg.resolved_execution_config_json,
        resolved_config_path,
    )?));
    // Execute safe outputs (Stage 3) — typed BashStep with typed env block
    steps.push(Step::Bash(execute_safe_outputs_step(
        &layout.source_path,
        resolved_config_path,
        &layout.self_repository_directory,
        &cfg.self_repository_name,
        &executor_ado_env,
        &variant.filter_args,
    )?));
    if let Some(app) = github_app
        && !app.skip_token_revocation
    {
        steps.push(
            super::extensions::ado_script::github_app_token_revoke_step_typed_for(
                app,
                crate::compile::types::SAFE_OUTPUTS_GITHUB_APP_TOKEN_VAR,
                "Revoke GitHub App token (SafeOutputs)",
            )?,
        );
    }
    // Copy logs
    steps.push(Step::Bash(copy_logs_safeoutputs_step(&cfg.engine_log_dir)));
    // Publish
    steps.push(Step::Publish(PublishStep {
        path: "$(Agent.TempDirectory)/staging".to_string(),
        artifact: variant.artifact.to_string(),
        condition: Some(Condition::Always),
    }));

    let safeoutputs_pool = if variant.is_reviewed {
        cfg.pools.safe_outputs_reviewed.clone()
    } else {
        cfg.pools.safe_outputs.clone()
    };
    let mut job = Job::new(prefix.id(variant.base)?, variant.display, safeoutputs_pool);
    job.steps = steps;
    // **Marquee**: condition uses typed Expr::StepOutput on Detection's
    // threatAnalysis.SafeToProcess output. Lowering picks the cross-job
    // `dependencies.Detection.outputs[...]` form (and automatically
    // uses the prefixed Detection job ID when `prefix` is `Some`).
    job.condition = Some(Condition::And(vec![
        Condition::Succeeded,
        Condition::Eq(
            Expr::StepOutput(OutputRef::new(
                StepId::new("threatAnalysis")?,
                "SafeToProcess",
            )),
            Expr::Literal("true".to_string()),
        ),
    ]));
    Ok(job)
}

/// Grace minutes added to the agentless `ManualReview` job-level timeout on top
/// of the task's `timeoutInMinutes`. Keeps the job timeout strictly larger than
/// the task timeout so the task's graceful `onTimeout` (reject/resume) always
/// fires before any job-level cancellation could preempt it.
const MANUAL_REVIEW_JOB_TIMEOUT_GRACE_MINUTES: u64 = 5;

/// Build the agentless **ManualReview** job (a `ManualValidation@1` server
/// task) when any enabled safe-output tool resolves to require manual review.
///
/// Returns `Ok(None)` when no tool requires approval (the common case — the
/// canonical graph is then unchanged). The gate sits between Detection and
/// SafeOutputs; its condition reuses Detection's `threatAnalysis.SafeToProcess`
/// output so a run flagged unsafe never pauses for a human, and a rejected
/// validation fails the gate so SafeOutputs (which depends on it) is skipped —
/// fail-closed by default.
fn build_manual_review_job(
    front_matter: &FrontMatter,
    cfg: &StandaloneCtx,
    prefix: &JobPrefix<'_>,
) -> Result<Option<Job>> {
    let (_, reviewed) = front_matter.partition_safe_outputs_by_approval();
    if reviewed.is_empty() {
        return Ok(None);
    }
    let approval = aggregate_approval_config(front_matter, &reviewed);

    let mut job = Job::new(prefix.id("ManualReview")?, "Manual Review", Pool::Server);
    job.steps = vec![Step::Task(build_manual_validation_step(
        &approval, &reviewed,
    ))];
    // The pending-period timeout is enforced on the TASK
    // (`ManualValidation@1`'s step `timeoutInMinutes`, set in
    // `build_manual_validation_step`) so that the task's `onTimeout`
    // handler (reject/resume) fires gracefully. The job-level timeout is kept
    // only as a strictly-larger outer hard bound: if it equalled the task
    // timeout it would race with — and could preempt — the task's `onTimeout`,
    // re-introducing the very cancellation that defeats `on-timeout: resume`.
    if let Some(mins) = approval.timeout_minutes {
        let job_bound = (mins as u64) + MANUAL_REVIEW_JOB_TIMEOUT_GRACE_MINUTES;
        job.timeout = Some(std::time::Duration::from_secs(60 * job_bound));
    }
    let _ = cfg; // pool/compiler context not needed for an agentless gate
    job.condition = Some(Condition::And(vec![
        Condition::Succeeded,
        Condition::Eq(
            Expr::StepOutput(OutputRef::new(
                StepId::new("threatAnalysis")?,
                "SafeToProcess",
            )),
            Expr::Literal("true".to_string()),
        ),
        // Only pause for a human when the agent actually proposed an
        // approval-gated output (set by Detection's reviewedProposals step).
        Condition::Eq(
            Expr::StepOutput(OutputRef::new(
                StepId::new("reviewedProposals")?,
                "HasReviewedProposals",
            )),
            Expr::Literal("true".to_string()),
        ),
    ]));
    Ok(Some(job))
}

/// Fold the per-tool/global approval settings of every reviewed tool into the
/// single settings object that drives the whole-pipeline `ManualValidation@1`
/// gate. Lists are unioned; the timeout is the strictest (smallest) provided;
/// `on-timeout` is fail-closed (`reject`) unless *every* contributing config
/// explicitly asks to `resume`.
///
/// **Instructions:** every reviewed tool is listed and **all** author-supplied
/// per-tool `instructions` are aggregated into the single gate message (grouped
/// when identical) — no tool's note is dropped. See
/// [`compose_review_instructions`].
fn aggregate_approval_config(front_matter: &FrontMatter, reviewed: &[String]) -> ApprovalConfig {
    use std::collections::BTreeSet;
    // The sole caller (`build_manual_review_job`) only invokes this when at
    // least one tool requires approval. Calling it with an empty slice would
    // return `on_timeout: Some(Resume)` (a fail-OPEN default), so enforce the
    // invariant with a release-build `assert!` — this is a security boundary
    // and the compiler is not a hot path, so the cost is irrelevant.
    assert!(
        !reviewed.is_empty(),
        "aggregate_approval_config called with no reviewed tools (would default to fail-open resume)"
    );
    let mut approvers: BTreeSet<String> = BTreeSet::new();
    let mut notify: BTreeSet<String> = BTreeSet::new();
    let mut timeout_minutes: Option<u32> = None;
    let mut all_resume = true;
    // Per-tool author instructions, in sorted (reviewed) order. A single
    // ManualReview gate covers every reviewed tool, so rather than silently
    // dropping all but the first note (the old behaviour), we keep them all and
    // compose a message that lists every tool and attaches its note — see
    // `compose_review_instructions`.
    let mut per_tool_instructions: Vec<(String, String)> = Vec::new();

    for tool in reviewed {
        let Some(cfg) = front_matter.tool_requires_approval(tool) else {
            // A tool in `reviewed` with no resolvable config should be
            // impossible (the partition is built from the same predicate), but
            // if a future regression produces one, fail closed rather than let
            // the aggregated gate silently default to `on-timeout: resume`.
            all_resume = false;
            continue;
        };
        approvers.extend(cfg.approvers);
        notify.extend(cfg.notify_users);
        if let Some(t) = cfg.timeout_minutes {
            timeout_minutes = Some(timeout_minutes.map_or(t, |existing| existing.min(t)));
        }
        match cfg.on_timeout {
            Some(ApprovalOnTimeout::Resume) => {}
            _ => all_resume = false,
        }
        if let Some(instr) = cfg.instructions {
            let instr = instr.trim();
            if !instr.is_empty() {
                per_tool_instructions.push((tool.clone(), instr.to_string()));
            }
        }
    }

    ApprovalConfig {
        approvers: approvers.into_iter().collect(),
        notify_users: notify.into_iter().collect(),
        timeout_minutes,
        on_timeout: Some(if all_resume {
            ApprovalOnTimeout::Resume
        } else {
            ApprovalOnTimeout::Reject
        }),
        instructions: Some(compose_review_instructions(
            reviewed,
            &per_tool_instructions,
        )),
    }
}

/// Compose the single `ManualValidation@1` reviewer message for a run.
///
/// Because one gate covers every reviewed tool, this **lists every reviewed
/// tool** (the actions pending approval) and attaches **all** author-supplied
/// per-tool notes — none is silently dropped. `per_tool` holds the non-empty
/// instructions in sorted reviewed order; tools sharing identical note text
/// (e.g. inherited from a section-level `require-approval`) are grouped so the
/// note appears once, attributed to every tool it covers.
///
/// - No author notes anywhere → the standard default listing every tool.
/// - Exactly one reviewed tool with a note → that note verbatim (unchanged
///   single-tool authoring experience).
/// - Multiple reviewed tools with at least one note → enumerated message.
fn compose_review_instructions(reviewed: &[String], per_tool: &[(String, String)]) -> String {
    if per_tool.is_empty() {
        return default_review_instructions(reviewed);
    }
    if reviewed.len() == 1 {
        return per_tool[0].1.clone();
    }

    let mut msg = format!(
        "This run is paused for manual review. The agent has proposed safe \
         outputs of the following type(s) that require approval before they \
         are applied: {}.",
        reviewed.join(", ")
    );
    msg.push_str("\n\nReviewer notes by tool:");
    // Group tools sharing identical note text, preserving first-seen order.
    let mut grouped: Vec<(String, Vec<String>)> = Vec::new();
    for (tool, instr) in per_tool {
        if let Some(entry) = grouped.iter_mut().find(|(text, _)| text == instr) {
            entry.1.push(tool.clone());
        } else {
            grouped.push((instr.clone(), vec![tool.clone()]));
        }
    }
    for (instr, tools) in &grouped {
        msg.push_str(&format!("\n- {}: {}", tools.join(", "), instr));
    }
    msg.push_str(
        "\n\nReview the proposed content in the 'ado-aw-safe-outputs' summary \
         tab on this run, then Approve (Resume) to apply them, or Reject to \
         discard them.",
    );
    msg
}

/// Build the `ManualValidation@1` step from the aggregated approval settings.
fn build_manual_validation_step(approval: &ApprovalConfig, reviewed: &[String]) -> TaskStep {
    let mut builder = ManualValidation::new(approval.notify_users.join(", "));
    if !approval.approvers.is_empty() {
        builder = builder.approvers(approval.approvers.join(", "));
    }
    let instructions = approval
        .instructions
        .clone()
        .unwrap_or_else(|| default_review_instructions(reviewed));
    builder = builder.instructions(instructions);
    let on_timeout = match approval.on_timeout {
        Some(ApprovalOnTimeout::Resume) => OnTimeout::Resume,
        _ => OnTimeout::Reject,
    };
    builder = builder.on_timeout(on_timeout);
    if let Some(mins) = approval.timeout_minutes {
        // Bound the pending period on the TASK so its `onTimeout` handler
        // (reject/resume) actually fires — a job-level timeout would instead
        // cancel the job and never apply `on-timeout: resume`.
        builder = builder.timeout_minutes(mins);
    }
    builder.into_step()
}

/// Default reviewer message when the author did not set `instructions`.
fn default_review_instructions(reviewed: &[String]) -> String {
    format!(
        "This run is paused for manual review. The agent has proposed safe \
         outputs of the following type(s) that require approval before they \
         are applied: {}. Review the proposed content in the \
         'ado-aw-safe-outputs' summary tab on this run, then Approve (Resume) \
         to apply them, or Reject to discard them.",
        reviewed.join(", ")
    )
}

fn build_teardown_job(
    front_matter: &FrontMatter,
    cfg: &StandaloneCtx,
    prefix: &JobPrefix<'_>,
) -> Result<Option<Job>> {
    if front_matter.teardown.is_empty() {
        return Ok(None);
    }
    let mut steps: Vec<Step> = Vec::new();
    steps.push(checkout_self_step(&cfg.self_checkout_fetch, false));
    for user_step_val in &front_matter.teardown {
        steps.push(Step::RawYaml(step_to_raw_yaml_string(user_step_val)?));
    }
    let mut job = Job::new(
        prefix.id("Teardown")?,
        "Teardown",
        cfg.pools.teardown.clone(),
    );
    job.steps = steps;
    job.condition = Some(Condition::Always);
    Ok(Some(job))
}

/// Apply a single per-tool config (`noop` / `missing-tool` / `missing-data`)
/// to the Conclusion step as flat `AW_<TOOL>_*` env vars (gh-aw pattern).
/// Each field gets its own env var — avoids JSON-in-env-var corruption in ADO.
fn apply_conclusion_tool_config_env(
    mut conclusion_step: BashStep,
    front_matter: &FrontMatter,
    tool_key: &str,
) -> BashStep {
    let Some(tool_config) = front_matter.safe_outputs.get(tool_key) else {
        return conclusion_step;
    };
    let env_prefix = format!("AW_{}", tool_key.to_uppercase().replace('-', "_"));

    // Tool disabled entirely (e.g. noop: false)
    if tool_config.is_boolean() {
        if tool_config.as_bool() == Some(false) {
            conclusion_step = conclusion_step.with_env(
                format!("{env_prefix}_REPORT_AS_WORK_ITEM"),
                EnvValue::Literal("false".to_string()),
            );
        }
        return conclusion_step;
    }

    let Some(obj) = tool_config.as_object() else {
        return conclusion_step;
    };

    // report-as-work-item: accept both YAML bool and string forms.
    // serde_json::Value::to_string() on String("false") would emit
    // "\"false\"" (JSON-encoded with quotes), which the TypeScript
    // readBooleanEnv would reject and default to true — silently
    // inverting the opt-out. Use as_bool()/as_str() instead.
    if let Some(v) = obj.get("report-as-work-item") {
        let bool_str = v
            .as_bool()
            .map(|b| b.to_string())
            .or_else(|| v.as_str().map(|s| s.to_string()));
        if let Some(s) = bool_str {
            conclusion_step = conclusion_step.with_env(
                format!("{env_prefix}_REPORT_AS_WORK_ITEM"),
                EnvValue::Literal(s),
            );
        }
    }
    if let Some(v) = obj.get("title-prefix").and_then(|v| v.as_str()) {
        conclusion_step = conclusion_step.with_env(
            format!("{env_prefix}_TITLE_PREFIX"),
            EnvValue::Literal(crate::sanitize::sanitize(v)),
        );
    }
    if let Some(v) = obj.get("work-item-type").and_then(|v| v.as_str()) {
        conclusion_step = conclusion_step.with_env(
            format!("{env_prefix}_WORK_ITEM_TYPE"),
            EnvValue::Literal(crate::sanitize::sanitize(v)),
        );
    }
    if let Some(v) = obj.get("area-path").and_then(|v| v.as_str()) {
        conclusion_step = conclusion_step.with_env(
            format!("{env_prefix}_AREA_PATH"),
            EnvValue::Literal(crate::sanitize::sanitize(v)),
        );
    }
    if let Some(v) = obj.get("iteration-path").and_then(|v| v.as_str()) {
        conclusion_step = conclusion_step.with_env(
            format!("{env_prefix}_ITERATION_PATH"),
            EnvValue::Literal(crate::sanitize::sanitize(v)),
        );
    }
    if let Some(tags) = obj.get("tags").and_then(|v| v.as_array()) {
        let tags_json = serde_json::to_string(tags).unwrap_or_else(|_| "[]".to_string());
        conclusion_step =
            conclusion_step.with_env(format!("{env_prefix}_TAGS"), EnvValue::Literal(tags_json));
    }
    conclusion_step
}

/// Hoist upstream job results (Agent/Detection/SafeOutputs[/_Reviewed]/custom
/// jobs) into job-level variables and wire them onto the Conclusion step as
/// `$(name)` macro env vars.
///
/// ADO only evaluates `$[...]` runtime expressions inside `variables:` and
/// `condition:` — NOT in step env blocks — so results are hoisted to job
/// variables here and consumed as `$(name)` macros in the step env.
fn hoist_conclusion_job_results(
    mut conclusion_step: BashStep,
    prefix: &JobPrefix<'_>,
    custom_defs: &[CustomSafeOutputJobDef],
    has_reviewed_job: bool,
) -> Result<(Vec<JobVariable>, BashStep)> {
    let agent_id = prefix.id("Agent")?;
    let detection_id = prefix.id("Detection")?;
    let safeoutputs_id = prefix.id("SafeOutputs")?;
    let reviewed_id = prefix.id("SafeOutputs_Reviewed")?;

    // In the mixed manual-review split both a SafeOutputs (automatic) and a
    // SafeOutputs_Reviewed (gated) job exist. Surface the reviewed job's result
    // too so a reviewer rejection (which fails SafeOutputs_Reviewed) is reported
    // instead of silently lost.
    let mut conclusion_variables = vec![
        // EnvValue::Literal deliberately carries a raw `$[...]` runtime expression:
        // ADO evaluates `$[...]` only in `variables:`/`condition:`, so the value is
        // hoisted here and consumed as a `$(name)` macro in the step env below
        // (not EnvValue::AdoMacro — the lower.rs guard rejects pre-wrapped macros).
        JobVariable {
            name: "AW_AGENT_RESULT".to_string(),
            value: EnvValue::Literal(format!("$[dependencies.{}.result]", agent_id.as_str())),
        },
        JobVariable {
            name: "AW_DETECTION_RESULT".to_string(),
            value: EnvValue::Literal(format!("$[dependencies.{}.result]", detection_id.as_str())),
        },
        JobVariable {
            name: "AW_SAFEOUTPUTS_RESULT".to_string(),
            value: EnvValue::Literal(format!(
                "$[dependencies.{}.result]",
                safeoutputs_id.as_str()
            )),
        },
    ];
    if has_reviewed_job {
        conclusion_variables.push(JobVariable {
            name: "AW_SAFEOUTPUTS_REVIEWED_RESULT".to_string(),
            value: EnvValue::Literal(format!("$[dependencies.{}.result]", reviewed_id.as_str())),
        });
    }
    for (index, def) in custom_defs.iter().enumerate() {
        let result_name = format!("AW_CUSTOM_JOB_{index}_RESULT");
        conclusion_variables.push(JobVariable {
            name: result_name,
            value: EnvValue::Literal(format!("$[dependencies.{}.result]", def.job_id.as_str())),
        });
    }

    conclusion_step = conclusion_step
        .with_env(
            "AW_AGENT_RESULT",
            EnvValue::PipelineVar("AW_AGENT_RESULT".to_string()),
        )
        .with_env(
            "AW_DETECTION_RESULT",
            EnvValue::PipelineVar("AW_DETECTION_RESULT".to_string()),
        )
        .with_env(
            "AW_SAFEOUTPUTS_RESULT",
            EnvValue::PipelineVar("AW_SAFEOUTPUTS_RESULT".to_string()),
        );
    if has_reviewed_job {
        conclusion_step = conclusion_step.with_env(
            "AW_SAFEOUTPUTS_REVIEWED_RESULT",
            EnvValue::PipelineVar("AW_SAFEOUTPUTS_REVIEWED_RESULT".to_string()),
        );
    }
    if !custom_defs.is_empty() {
        conclusion_step = conclusion_step.with_env(
            "AW_CUSTOM_JOB_COUNT",
            EnvValue::Literal(custom_defs.len().to_string()),
        );
        for (index, def) in custom_defs.iter().enumerate() {
            conclusion_step = conclusion_step
                .with_env(
                    format!("AW_CUSTOM_JOB_{index}_NAME"),
                    EnvValue::Literal(format!("Custom safe output: {}", def.name)),
                )
                .with_env(
                    format!("AW_CUSTOM_JOB_{index}_RESULT"),
                    EnvValue::PipelineVar(format!("AW_CUSTOM_JOB_{index}_RESULT")),
                );
        }
    }

    Ok((conclusion_variables, conclusion_step))
}

fn build_conclusion_job(
    front_matter: &FrontMatter,
    cfg: &StandaloneCtx,
    prefix: &JobPrefix<'_>,
    custom_defs: &[CustomSafeOutputJobDef],
    has_reviewed_job: bool,
) -> Result<Option<Job>> {
    use crate::compile::ado_bundle::{Bundle, apply_bundle_auth, token_source_for};
    // Conclusion job is always emitted when safe-outputs exist (gh-aw pattern).
    if front_matter.safe_outputs.is_empty() {
        return Ok(None);
    }

    let mut steps: Vec<Step> = Vec::new();
    steps.push(checkout_none_step());

    // Install Node + download/verify the ado-script bundle using the canonical
    // helper. This keeps the supply-chain mirror handling and the unzip layout
    // (`/tmp/ado-aw-scripts/ado-script/<bundle>.js`) consistent with the
    // Agent/Setup jobs — a hand-rolled copy here previously double-nested the
    // unzip path and bypassed the supply-chain feed.
    steps.extend(
        super::extensions::ado_script::install_and_download_steps_typed(
            front_matter.supply_chain.as_ref(),
        ),
    );

    // Acquire write token (when configured): same-job minting is required because
    // Azure Pipelines task.setvariable variables are job-scoped and NOT propagated
    // to downstream jobs without isOutput=true + dependsOn mapping. The SafeOutputs
    // job mints its own SC_WRITE_TOKEN copy; Conclusion must do the same.
    push_raw_yaml_if_nonempty(&mut steps, &cfg.acquire_write_token)?;

    let mut download_artifact = TaskStep::new(
        "DownloadPipelineArtifact@2",
        "Download SafeOutputs artifact",
    )
    .with_input("artifact", "safe_outputs")
    .with_input("path", "$(Pipeline.Workspace)/conclusion_inputs");
    download_artifact.condition = Some(Condition::Always);
    // The safe_outputs artifact may not exist when SafeOutputs was skipped;
    // ignore the download failure — conclusion.js handles a missing dir.
    download_artifact.continue_on_error = true;
    steps.push(Step::Task(download_artifact));

    let conclusion_path = super::extensions::ado_script::CONCLUSION_PATH;
    let mut conclusion_step = ShellScript::new(&REPORT_CONCLUSION)
        .bind_text("CONCLUSION_PATH", conclusion_path)
        .into_step("Report pipeline conclusion");
    conclusion_step = conclusion_step.with_condition(Condition::Always);
    // The Conclusion job's contract is "always runs, never fails": it exists to
    // surface OTHER jobs' failures, so it must not turn a non-zero exit of its
    // own (e.g. node OOM/SIGKILL, or an unhandled rejection escaping
    // conclusion.js's top-level `.then`) into a pipeline failure that masks the
    // real signal. Use ADO's `continueOnError` rather than a blanket `|| true`
    // in the bash body: the failure still shows up in the timeline as a warning
    // (preserving observability) instead of being silently swallowed.
    conclusion_step.continue_on_error = true;

    // Global opt-out: safe-outputs.report-failure-as-work-item (default: true)
    let report_failure = front_matter
        .safe_outputs
        .get("report-failure-as-work-item")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    conclusion_step = conclusion_step
        .with_env(
            "AW_REPORT_FAILURE_AS_WORK_ITEM",
            EnvValue::Literal(report_failure.to_string()),
        )
        .with_env(
            "AW_PIPELINE_NAME",
            // Sanitize for consistency with the per-tool config fields below:
            // the name flows verbatim into the ADO work-item title/body, and
            // operator-controlled strings are sanitized everywhere else.
            EnvValue::Literal(crate::sanitize::sanitize(&front_matter.name)),
        )
        .with_env(
            "AW_SAFE_OUTPUT_DIR",
            EnvValue::Literal("$(Pipeline.Workspace)/conclusion_inputs".to_string()),
        );

    // Use SC_WRITE_TOKEN when a write service connection is configured;
    // fall back to System.AccessToken otherwise. The token source is selected
    // by the shared `token_source_for` helper (same logic as the Stage 3
    // executor) and projected via the bundle-auth applier so the Conclusion
    // step can never ship without a bearer (the regression that was #1307).
    let write_sc = front_matter
        .permissions
        .as_ref()
        .and_then(|p| p.write.as_ref())
        .map(crate::compile::types::WritePermissionConfig::service_connection);
    conclusion_step = apply_bundle_auth(
        conclusion_step,
        Bundle::Conclusion,
        token_source_for(write_sc),
    );

    // Pass per-tool configs as individual flat env vars (gh-aw pattern).
    // Each field gets its own env var — avoids JSON-in-env-var corruption in ADO.
    //
    // Note: pipeline_failure has no per-tool config entry — it uses hardcoded
    // defaults (type: Task, no area/iteration path). The global
    // report-failure-as-work-item toggle controls whether it files at all.
    for tool_key in &["noop", "missing-tool", "missing-data"] {
        conclusion_step =
            apply_conclusion_tool_config_env(conclusion_step, front_matter, tool_key);
    }

    // Pass upstream job results via job-level variables hoist.
    // ADO only evaluates $[...] runtime expressions inside `variables:` and
    // `condition:` — NOT in step env blocks. We hoist to job variables and
    // reference them as $(name) macros in the step env.
    let (conclusion_variables, conclusion_step) = hoist_conclusion_job_results(
        conclusion_step,
        prefix,
        custom_defs,
        has_reviewed_job,
    )?;

    steps.push(Step::Bash(conclusion_step));

    let mut job = Job::new(
        prefix.id("Conclusion")?,
        "Conclusion",
        cfg.pools.conclusion.clone(),
    );
    job.variables = conclusion_variables;
    job.steps = steps;
    // Keep Conclusion's "run regardless of upstream result" behavior, but do
    // not continue running after an explicit pipeline cancellation request.
    job.condition = Some(Condition::And(vec![
        Condition::Always,
        Condition::Custom("not(canceled())".to_string()),
    ]));
    Ok(Some(job))
}

/// Wire explicit `depends_on` between the canonical jobs. The graph
/// pass also derives these from OutputRefs but explicit edges make
/// the emitted YAML match committed lock-file shapes exactly.
///
/// The `prefix` is threaded through so dependency edges use the
/// correct (possibly prefixed) target job IDs for `target: job|stage`.
///
/// # Errors
///
/// Returns `Err` if `prefix.id(...)` fails for any of the canonical
/// names. In the standard call graph the jobs were just constructed
/// from the same `prefix`, so a failure here would indicate an
/// invalid `JobPrefix` reaching this function — the typed error is
/// preferable to a panic for any future caller.
fn wire_explicit_dependencies(
    jobs: &mut [Job],
    prefix: &JobPrefix<'_>,
    custom_defs: &[CustomSafeOutputJobDef],
    custom_direct_reviewed_job_ids: &[JobId],
    custom_automatic_job_ids: &[JobId],
    custom_job_ids: &[JobId],
    safeoutputs_waits_for_review: bool,
) -> Result<()> {
    let setup_id = prefix.id("Setup")?;
    let agent_id = prefix.id("Agent")?;
    let detection_id = prefix.id("Detection")?;
    let manualreview_id = prefix.id("ManualReview")?;
    let safeoutputs_id = prefix.id("SafeOutputs")?;
    let reviewed_id = prefix.id("SafeOutputs_Reviewed")?;
    let teardown_id = prefix.id("Teardown")?;
    let conclusion_id = prefix.id("Conclusion")?;
    let has_setup = jobs.iter().any(|j| j.id == setup_id);
    let has_teardown = jobs.iter().any(|j| j.id == teardown_id);
    // The reviewed execution job only exists in the mixed (split) case.
    let has_reviewed_job = jobs.iter().any(|j| j.id == reviewed_id);
    let custom_by_id: std::collections::HashMap<&JobId, &CustomSafeOutputJobDef> = custom_defs
        .iter()
        .map(|definition| (&definition.job_id, definition))
        .collect();
    let custom_by_name: std::collections::HashMap<&str, &JobId> = custom_defs
        .iter()
        .map(|definition| (definition.name.as_str(), &definition.job_id))
        .collect();
    for j in jobs.iter_mut() {
        if j.id == agent_id && has_setup {
            j.depends_on = vec![setup_id.clone()];
        } else if j.id == detection_id {
            j.depends_on = vec![agent_id.clone()];
        } else if j.id == manualreview_id {
            // Agentless gate: depends on Detection (its condition reads
            // Detection's threatAnalysis.SafeToProcess output).
            j.depends_on = vec![agent_id.clone(), detection_id.clone()];
        } else if custom_job_ids.iter().any(|id| id == &j.id) {
            let definition = custom_by_id[&j.id];
            let mut deps = vec![agent_id.clone(), detection_id.clone()];
            if custom_direct_reviewed_job_ids.iter().any(|id| id == &j.id) {
                deps.push(manualreview_id.clone());
            }
            for dependency in &definition.needs {
                let id = match dependency.as_str() {
                    "agent" => agent_id.clone(),
                    "detection" => detection_id.clone(),
                    "safe-outputs" => safeoutputs_id.clone(),
                    "safe-outputs-reviewed" => {
                        anyhow::ensure!(
                            has_reviewed_job || safeoutputs_waits_for_review,
                            "safe-outputs.jobs.{}.needs references `safe-outputs-reviewed`, \
                             but no reviewed built-in SafeOutputs path is emitted",
                            definition.name
                        );
                        if has_reviewed_job {
                            reviewed_id.clone()
                        } else {
                            safeoutputs_id.clone()
                        }
                    }
                    custom => custom_by_name[custom].clone(),
                };
                if !deps.contains(&id) {
                    deps.push(id);
                }
            }
            j.depends_on = deps;
        } else if j.id == safeoutputs_id {
            // The "SafeOutputs" job is the automatic path. It is gated behind
            // ManualReview only when it is the *sole* execution job (all tools
            // reviewed); in the mixed split it runs immediately after Detection
            // alongside the separate reviewed job.
            j.depends_on = if safeoutputs_waits_for_review {
                vec![
                    agent_id.clone(),
                    detection_id.clone(),
                    manualreview_id.clone(),
                ]
            } else {
                vec![agent_id.clone(), detection_id.clone()]
            };
        } else if j.id == reviewed_id {
            // Reviewed execution runs only after the approval gate clears, so a
            // rejected review fails closed (this job is skipped).
            j.depends_on = vec![
                agent_id.clone(),
                detection_id.clone(),
                manualreview_id.clone(),
            ];
        } else if j.id == teardown_id {
            // Teardown is cleanup paired with the *automatic* execution path.
            // In the mixed split it deliberately does NOT depend on the
            // human-gated `SafeOutputs_Reviewed` job: that job is routinely
            // skipped (whenever the agent proposed no reviewed-type output) and
            // can stay paused on the approval gate indefinitely. Depending on it
            // under ADO's implicit `succeeded()` gate would skip Teardown on the
            // common no-reviewed-proposal path (and block cleanup behind a human
            // approval otherwise). Waiting only on the auto `SafeOutputs` job
            // keeps Teardown's behaviour identical to the single-job case.
            let mut deps = vec![safeoutputs_id.clone()];
            deps.extend(custom_automatic_job_ids.iter().cloned());
            j.depends_on = deps;
        } else if j.id == conclusion_id {
            let mut deps = vec![
                agent_id.clone(),
                detection_id.clone(),
                safeoutputs_id.clone(),
            ];
            if has_reviewed_job {
                // Mixed split: depend on the reviewed execution job too so a
                // reviewer rejection (which fails SafeOutputs_Reviewed) is
                // detected by Conclusion. Accepted trade-off: in the mixed case
                // Conclusion waits behind the manual-review gate. The job's
                // always() condition still fires when the reviewed job is
                // skipped or fails.
                deps.push(reviewed_id.clone());
            }
            deps.extend(custom_job_ids.iter().cloned());
            if has_teardown {
                deps.push(teardown_id.clone());
            }
            j.depends_on = deps;
        }
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────
// Step body builders — typed BashStep/TaskStep with format!() bodies
// ─────────────────────────────────────────────────────────────────────

fn checkout_self_step(fetch: &CheckoutFetchOpts, multi_checkout: bool) -> Step {
    checkout_self_step_with_path(
        fetch,
        multi_checkout.then_some(common::MULTI_CHECKOUT_SELF_PATH),
    )
}

fn checkout_self_step_with_path(fetch: &CheckoutFetchOpts, path: Option<&str>) -> Step {
    Step::Checkout(CheckoutStep {
        repository: CheckoutRepo::Self_,
        path: path.map(str::to_string),
        clean: None,
        submodules: None,
        fetch_depth: fetch.depth_for_emit(),
        fetch_tags: fetch.fetch_tags,
        persist_credentials: None,
    })
}

fn checkout_none_step() -> Step {
    Step::Checkout(CheckoutStep {
        repository: CheckoutRepo::None,
        path: None,
        clean: None,
        submodules: None,
        fetch_depth: None,
        fetch_tags: None,
        persist_credentials: None,
    })
}

/// Derive the ACR registry name (used by `az acr login --name`) from a
/// registry base path. Takes the host portion (before the first `/`), then
/// strips a trailing `.azurecr.io` when present; otherwise returns the portion
/// before the first `.` (falling back to the whole host).
///
/// NOTE: this assumes the standard `<name>.azurecr.io` login-server hostname.
/// For ACR accessed over Azure Private Link with a custom domain (e.g.
/// `myacr.internal.contoso.com`), the `.split('.').next()` fallback may not
/// yield the registry name `az acr login --name` expects — configure
/// `registry.name` with the canonical `*.azurecr.io` login server in that case.
fn acr_registry_name(registry_base: &str) -> &str {
    let host = registry_base.split('/').next().unwrap_or(registry_base);
    host.strip_suffix(".azurecr.io")
        .or_else(|| host.split('.').next())
        .unwrap_or(host)
}

/// `AzureCLI@2` step that runs `az acr login` against an internal registry so
/// subsequent `docker pull` calls in the same job are authenticated. Uses the
/// resolved registry service connection (an ARM/Azure service connection).
/// `registry_base` is the configured registry host or base path; the ACR name
/// is derived from its host portion.
fn acr_login_step(registry_base: &str, connection: &str) -> TaskStep {
    let name = acr_registry_name(registry_base);
    AzureCli::new(
        connection,
        ScriptType::Bash,
        ScriptLocation::Inline(format!("az acr login --name {name}\n")),
    )
    .with_display_name("Authenticate to internal container registry")
    .into_step()
}

/// `AzureCLI@2` step that mints the external model-provider credential
/// (`engine.provider.token`) **in the same job** as the engine run. Authenticated
/// by the ARM `service-connection`, it runs `az account get-access-token` for the
/// configured resource and publishes the result as the same-job secret
/// [`PROVIDER_BEARER_TOKEN_VAR`], which is referenced by `COPILOT_PROVIDER_API_KEY`
/// (the credential env var the AWF api-proxy sidecar reads and forwards as
/// `Authorization: Bearer <value>`).
///
/// Same-job minting is deliberate: it avoids the cross-job `isOutput`/`dependsOn`
/// plumbing (the #1372 failure) — a plain `$(...)` macro resolves the token. The
/// AWF api-proxy sidecar (`--exclude-env COPILOT_PROVIDER_API_KEY`) keeps the
/// value out of the sandbox; this step runs outside the sandbox.
///
/// Token lifetime: `az account get-access-token` returns a short-lived AAD token
/// (typically ~1h). Minting immediately before the Copilot run keeps it fresh for
/// normal workloads; a job that queues/idles for the full token lifetime *after*
/// this step (before the run) could see an expired token — mint is intentionally
/// the last step before the engine invocation to minimise that window.
fn provider_token_mint_step(token: &ProviderToken) -> TaskStep {
    let resource = token.resource();
    let var = crate::compile::types::PROVIDER_BEARER_TOKEN_VAR;
    // `resource` is a validated `ProviderResourceUrl` (shell-safe allowlist, no
    // single-quotes); single-quoting here is defense-in-depth so the value is
    // passed to `az` as one literal argument regardless.
    let script = format!(
        "set -eo pipefail\n\
         TOKEN=$(az account get-access-token --resource '{resource}' --query accessToken -o tsv)\n\
         echo \"##vso[task.setvariable variable={var};issecret=true]$TOKEN\"\n"
    );
    AzureCli::new(
        token.service_connection.as_str(),
        ScriptType::Bash,
        ScriptLocation::Inline(script),
    )
    .with_display_name("Acquire provider bearer token")
    .into_step()
}

/// `NuGetAuthenticate@1` step. When a service connection is resolved it is
/// passed via `nuGetServiceConnections` (cross-org/external feeds); otherwise
/// the task authenticates the build identity with `$(System.AccessToken)`.
pub(crate) fn nuget_authenticate_step(connection: Option<&str>) -> TaskStep {
    let mut auth = NuGetAuthenticate::new().with_display_name("Authenticate to internal feed");
    if let Some(conn) = connection {
        auth = auth.nuget_service_connections(conn);
    }
    auth.into_step()
}

/// `DownloadPackage@1` step pulling a single NuGet package by name+version
/// from the internal feed into `download_path`.
pub(crate) fn download_package_step(
    display: impl Into<String>,
    feed: &str,
    package: &str,
    version: &str,
    download_path: &str,
) -> TaskStep {
    DownloadPackage::nuget(feed, package, version, download_path)
        .with_display_name(display)
        .into_step()
}

/// Download one pinned candidate pipeline artifact to a compiler-owned path.
///
/// User-controlled project and artifact values remain typed task inputs; only
/// validated positive numeric IDs are converted to task-input strings.
pub(crate) fn download_candidate_artifact_step(
    config: &PipelineArtifactConfig,
    display: impl Into<String>,
    target_path: &str,
) -> TaskStep {
    DownloadPipelineArtifact::new(target_path)
        .source(ArtifactSource::Specific)
        .project(config.project.as_str())
        .pipeline(config.definition_id.to_string())
        .run_version(RunVersion::Specific)
        .run_id(config.run_id.to_string())
        .artifact(config.artifact.as_str())
        .with_display_name(display)
        .into_step()
}

// Keep this free of single quotes: the generated Bash passes it as one
// single-quoted `python3 -c` argument so the multiline validator stays
// readable without requiring shell escaping.
const CANDIDATE_PROVENANCE_VALIDATOR_PY: &str = r#"import json
import sys

provenance_path, definition_arg, build_arg = sys.argv[1:]
with open(provenance_path, encoding="utf-8") as handle:
    provenance = json.load(handle)

expected_definition = int(definition_arg)
expected_build = int(build_arg)
if provenance.get("schema") != "ado-aw/candidate-artifact/1":
    sys.exit("candidate provenance schema must be ado-aw/candidate-artifact/1")

definition = provenance.get("producer_definition_id")
build = provenance.get("producer_build_id")
if type(definition) is not int or definition != expected_definition:
    sys.exit(
        f"candidate producer_definition_id mismatch: "
        f"expected {expected_definition}, got {definition!r}"
    )
if type(build) is not int or build != expected_build:
    sys.exit(
        f"candidate producer_build_id mismatch: expected {expected_build}, got {build!r}"
    )

diagnostic = {
    "schema": provenance["schema"],
    "producer_definition_id": definition,
    "producer_build_id": build,
}
for key in (
    "repository",
    "source_ref",
    "source_version",
    "reason",
    "compiler_version",
    "awf_version",
):
    if key in provenance:
        diagnostic[key] = provenance[key]

print("Validated candidate provenance:")
print(json.dumps(diagnostic, indent=2, sort_keys=True))
"#;

shell_script! {
    /// Stage a payload out of a provenance-checked candidate pipeline
    /// artifact: locate exactly one payload, checksum manifest and provenance
    /// document, copy them into place, verify an *exact* filename checksum
    /// entry, then validate producer identity before the caller's tail runs.
    ///
    /// The exactly-one requirement is load-bearing. A `find` that matched two
    /// files would otherwise let an attacker who can add a file to the
    /// artifact decide which one is staged.
    STAGE_CANDIDATE_ARTIFACT_PAYLOAD {
        interpreter: Bash,
        bindings: [STAGING, DEST, PAYLOAD_NAME, PROVENANCE_VALIDATOR, DEFINITION_ID, RUN_ID],
        externals: [],
        fragments: [tail],
        body: r###"
set -eo pipefail
mkdir -p "$DEST"

locate_one() {
  local name="$1"
  mapfile -d '' -t matches < <(find "$STAGING" -type f -name "$name" -print0)
  if [ "${#matches[@]}" -ne 1 ]; then
    echo "##vso[task.complete result=Failed]Expected exactly one $name in candidate artifact, found ${#matches[@]}" >&2
    exit 1
  fi
  printf '%s' "${matches[0]}"
}

PAYLOAD="$(locate_one "$PAYLOAD_NAME")"
CHK="$(locate_one checksums.txt)"
PROVENANCE="$(locate_one provenance.json)"
cp "$PAYLOAD" "$DEST/$PAYLOAD_NAME"
cp "$CHK" "$DEST/checksums.txt"
cp "$PROVENANCE" "$DEST/provenance.json"

echo "Verifying exact checksum entry for $PAYLOAD_NAME..."
cd "$DEST" || exit 1
awk -v name="$PAYLOAD_NAME" '
  { candidate=$2; sub(/^\*/, "", candidate); if (candidate == name) { count++; line=$0 } }
  END { if (count != 1) exit 1; print line }
' checksums.txt | sha256sum -c -

python3 -c "$PROVENANCE_VALIDATOR" provenance.json "$DEFINITION_ID" "$RUN_ID"
# ado-aw:fragment tail
"###,
    }
}

/// Bash body that stages a payload out of a provenance-checked candidate
/// pipeline artifact, then runs the caller-supplied verify/relocate tail.
///
/// The producer contract is `schema: ado-aw/candidate-artifact/1` with numeric
/// `producer_definition_id` and `producer_build_id` fields.
///
/// Returns the rendered script rather than a step because two callers wrap it
/// differently — see [`download_compiler_step`] here and
/// `install_and_download_steps_typed` in `extensions/ado_script.rs`.
pub(crate) fn stage_candidate_artifact_payload_bash(
    config: &PipelineArtifactConfig,
    staging: &str,
    dest_dir: &str,
    payload: &str,
    tail: &str,
) -> String {
    ShellScript::new(&STAGE_CANDIDATE_ARTIFACT_PAYLOAD)
        .bind("STAGING", Binding::ado_path(staging))
        .bind("DEST", Binding::ado_path(dest_dir))
        .bind_text("PAYLOAD_NAME", payload)
        .bind(
            "PROVENANCE_VALIDATOR",
            Binding::document(CANDIDATE_PROVENANCE_VALIDATOR_PY),
        )
        .bind("DEFINITION_ID", Binding::number(config.definition_id))
        .bind("RUN_ID", Binding::number(config.run_id))
        .fragment("tail", tail)
        .render()
}

shell_script! {
    /// Locate a payload inside a `DownloadPackage@1` staging directory —
    /// handling both the extracted-tree and raw-`.nupkg` delivery shapes —
    /// copy it plus `checksums.txt` into place, verify the checksum, then run
    /// the caller's tail with `DEST` as the working directory.
    EXTRACT_PACKAGE_PAYLOAD {
        interpreter: Bash,
        bindings: [STAGING, DEST, PAYLOAD_NAME],
        externals: [],
        fragments: [tail],
        body: r###"
set -eo pipefail
mkdir -p "$DEST"

# DownloadPackage@1 may deliver an extracted tree or a raw .nupkg;
# handle both by unzipping any .nupkg when the payload is absent.
if [ -z "$(find "$STAGING" -name "$PAYLOAD_NAME" -print -quit)" ]; then
  NUPKG="$(find "$STAGING" -name '*.nupkg' -print -quit)"
  if [ -n "$NUPKG" ]; then
    unzip -o "$NUPKG" -d "$STAGING" >/dev/null
  fi
fi

BIN="$(find "$STAGING" -name "$PAYLOAD_NAME" -print -quit)"
CHK="$(find "$STAGING" -name 'checksums.txt' -print -quit)"
if [ -z "$BIN" ] || [ -z "$CHK" ]; then
  echo "##vso[task.complete result=Failed]$PAYLOAD_NAME or checksums.txt not found in package"
  exit 1
fi
cp "$BIN" "$DEST/$PAYLOAD_NAME"
cp "$CHK" "$DEST/checksums.txt"

echo "Verifying checksum..."
cd "$DEST" || exit 1
grep "$PAYLOAD_NAME" checksums.txt | sha256sum -c -
# ado-aw:fragment tail
"###,
    }
}

/// Bash body that stages a payload out of a `DownloadPackage@1` staging
/// directory and runs the caller-supplied verify/relocate tail.
///
/// `payload` is the artifact file name (e.g. `ado-aw-linux-x64`); `tail` is
/// appended once the files are staged in `dest_dir`, which is also the working
/// directory by then.
fn extract_package_payload_bash(
    staging: &str,
    dest_dir: &str,
    payload: &str,
    tail: &str,
) -> String {
    ShellScript::new(&EXTRACT_PACKAGE_PAYLOAD)
        .bind("STAGING", Binding::ado_path(staging))
        .bind("DEST", Binding::ado_path(dest_dir))
        .bind_text("PAYLOAD_NAME", payload)
        .fragment("tail", tail)
        .render()
}

/// `NuGetAuthenticate@1` step to emit **once per job** when the feed mirror is
/// active. Hoisting a single auth step (keyed on the resolved feed connection)
/// keeps the per-artifact `DownloadPackage@1` calls authenticated without
/// repeating the (idempotent) auth task for every binary. Returns `None` when
/// no feed is configured.
fn feed_auth_step(supply_chain: Option<&SupplyChainConfig>) -> Option<Step> {
    let sc = supply_chain?;
    sc.feed
        .as_ref()
        .map(|_| Step::Task(nuget_authenticate_step(sc.feed_connection())))
}

fn download_compiler_step(
    compiler_version: &str,
    supply_chain: Option<&SupplyChainConfig>,
) -> Vec<Step> {
    if let Some(artifact) = supply_chain.and_then(|sc| sc.pipeline_artifact.as_ref()) {
        let dest = "$(Pipeline.Workspace)/agentic-pipeline-compiler";
        let staging = "$(Pipeline.Workspace)/ado-aw-candidate/compiler";
        let tail = "mv ado-aw-linux-x64 ado-aw\n\
                    chmod +x ado-aw\n";
        let body = stage_candidate_artifact_payload_bash(
            artifact,
            staging,
            dest,
            "ado-aw-linux-x64",
            tail,
        );
        return vec![
            Step::Task(download_candidate_artifact_step(
                artifact,
                "Download candidate artifact for agentic pipeline compiler",
                staging,
            )),
            Step::Bash(BashStep::new(
                "Stage candidate agentic pipeline compiler",
                body,
            )),
        ];
    }

    if let Some(feed) = supply_chain.and_then(|sc| sc.feed.as_ref()) {
        let dest = "$(Pipeline.Workspace)/agentic-pipeline-compiler";
        let staging = "$(Pipeline.Workspace)/agentic-pipeline-compiler/_pkg";
        let tail = "mv ado-aw-linux-x64 ado-aw\n\
                    chmod +x ado-aw\n";
        let body = extract_package_payload_bash(staging, dest, "ado-aw-linux-x64", tail);
        // Auth is hoisted to the job builder via `feed_auth_step` (one
        // NuGetAuthenticate@1 per job, not per artifact).
        return vec![
            Step::Task(download_package_step(
                format!("Download agentic pipeline compiler (v{compiler_version})"),
                feed.name.as_str(),
                "ado-aw",
                compiler_version,
                staging,
            )),
            Step::Bash(BashStep::new(
                format!("Stage agentic pipeline compiler (v{compiler_version})"),
                body,
            )),
        ];
    }

    vec![Step::Bash(
        ShellScript::new(&DOWNLOAD_COMPILER_FROM_RELEASES)
            .bind_text("COMPILER_VERSION", compiler_version)
            .bind(
                "PIPELINE_WORKSPACE",
                Binding::ado_macro("Pipeline.Workspace"),
            )
            .into_step(format!(
                "Download agentic pipeline compiler (v{compiler_version})"
            )),
    )]
}

shell_script! {
    /// Fallback download path when no supply-chain feed or pipeline artifact
    /// is configured: fetch the `ado-aw` binary directly from the GitHub
    /// Releases page, verify its SHA-256 against the published
    /// `checksums.txt`, and stage it at `<Pipeline.Workspace>/agentic-pipeline-compiler/ado-aw`.
    DOWNLOAD_COMPILER_FROM_RELEASES {
        interpreter: Bash,
        bindings: [COMPILER_VERSION, PIPELINE_WORKSPACE],
        externals: [],
        fragments: [],
        body: r###"
set -eo pipefail
DOWNLOAD_DIR="$PIPELINE_WORKSPACE/agentic-pipeline-compiler"
DOWNLOAD_URL="https://github.com/githubnext/ado-aw/releases/download/v$COMPILER_VERSION/ado-aw-linux-x64"
CHECKSUM_URL="https://github.com/githubnext/ado-aw/releases/download/v$COMPILER_VERSION/checksums.txt"

mkdir -p "$DOWNLOAD_DIR"
echo "Downloading ado-aw v$COMPILER_VERSION from GitHub Releases..."
curl -fsSL -o "$DOWNLOAD_DIR/ado-aw-linux-x64" "$DOWNLOAD_URL"
curl -fsSL -o "$DOWNLOAD_DIR/checksums.txt" "$CHECKSUM_URL"

echo "Verifying checksum..."
cd "$DOWNLOAD_DIR" || exit 1
grep "ado-aw-linux-x64" checksums.txt | sha256sum -c -
mv ado-aw-linux-x64 ado-aw
chmod +x ado-aw
"###,
    }
}

shell_script! {
    /// Fallback download path for the AWF (Agentic Workflow Firewall)
    /// binary: fetch it directly from the GitHub Releases page of
    /// `github/gh-aw-firewall`, verify its SHA-256, and expose it on `PATH`.
    DOWNLOAD_AWF_FROM_RELEASES {
        interpreter: Bash,
        bindings: [AWF_VERSION, PIPELINE_WORKSPACE],
        externals: [],
        fragments: [],
        body: r###"
set -eo pipefail

DOWNLOAD_DIR="$PIPELINE_WORKSPACE/awf"
DOWNLOAD_URL="https://github.com/github/gh-aw-firewall/releases/download/v$AWF_VERSION/awf-linux-x64"
CHECKSUM_URL="https://github.com/github/gh-aw-firewall/releases/download/v$AWF_VERSION/checksums.txt"

mkdir -p "$DOWNLOAD_DIR"
echo "Downloading AWF v$AWF_VERSION from GitHub Releases..."
curl -fsSL -o "$DOWNLOAD_DIR/awf-linux-x64" "$DOWNLOAD_URL"
curl -fsSL -o "$DOWNLOAD_DIR/checksums.txt" "$CHECKSUM_URL"

echo "Verifying checksum..."
cd "$DOWNLOAD_DIR" || exit 1
grep "awf-linux-x64" checksums.txt | sha256sum -c -
mv awf-linux-x64 awf
chmod +x awf
echo "##vso[task.prependpath]$PIPELINE_WORKSPACE/awf"
./awf --version
"###,
    }
}

shell_script! {
    /// Pre-pull every AWF container image (and optionally MCPG) so that the
    /// subsequent `docker run` on the isolated agent network has all images
    /// available locally. The `mcpg_pull` fragment holds an optional extra
    /// `docker pull` line for the MCPG image.
    PREPULL_IMAGES {
        interpreter: Bash,
        bindings: [SQUID_IMAGE, AGENT_IMAGE, API_PROXY_IMAGE],
        externals: [],
        fragments: [mcpg_pull],
        body: r###"
set -eo pipefail

docker pull "$SQUID_IMAGE"
docker pull "$AGENT_IMAGE"
docker pull "$API_PROXY_IMAGE"
# ado-aw:fragment mcpg_pull
"###,
    }
}

fn substitute_integrity_check(yaml: &str, pipeline_path: &str, trigger_repo_dir: &str) -> String {
    if yaml.is_empty() {
        return String::new();
    }
    yaml.replace("{{ pipeline_path }}", pipeline_path)
        .replace("{{ trigger_repo_directory }}", trigger_repo_dir)
}

shell_script! {
    /// Stage the runtime MCPG config JSON, generate a per-run gateway API
    /// key, and (optionally) stage the compiler-generated custom-tools JSON.
    /// The two JSON payloads are spliced in via `mcpg_config_heredoc` and
    /// `custom_tools_block` fragments so the compiler owns the heredoc
    /// sentinels (each derived from the SHA of its own payload).
    PREPARE_MCPG_CONFIG {
        interpreter: Bash,
        bindings: [AGENT_TEMP, MCPG_PORT, MCPG_DOMAIN],
        externals: [],
        fragments: [mcpg_config_heredoc, custom_tools_block],
        body: r###"
mkdir -p "$AGENT_TEMP/staging"

# Generate MCPG API key early so it's available as an ADO secret variable
# for both the MCPG config and the agent's mcp-config.json
MCP_GATEWAY_API_KEY=$(openssl rand -base64 45 | tr -d '/+=')
echo "##vso[task.setvariable variable=MCP_GATEWAY_API_KEY;issecret=true]$MCP_GATEWAY_API_KEY"

# Export gateway port and domain as pipeline variables (matching gh-aw pattern).
# These duplicate the compile-time values baked into the YAML, but MCPG's
# Docker container requires MCP_GATEWAY_PORT and MCP_GATEWAY_DOMAIN env vars
# to start — the ADO variable indirection satisfies that contract.
echo "##vso[task.setvariable variable=MCP_GATEWAY_PORT]$MCPG_PORT"
echo "##vso[task.setvariable variable=MCP_GATEWAY_DOMAIN]$MCPG_DOMAIN"

# Write MCPG (MCP Gateway) configuration to a file
# ado-aw:fragment mcpg_config_heredoc

# ado-aw:fragment custom_tools_block
echo "MCPG config:"
cat "$AGENT_TEMP/staging/mcpg-config.json"

# Validate JSON
python3 -m json.tool "$AGENT_TEMP/staging/mcpg-config.json" > /dev/null && echo "JSON is valid"
"###,
    }
}

fn prepare_mcpg_config_step(
    mcpg_config_json: &str,
    custom_tools_json: Option<&str>,
) -> Result<BashStep> {
    let mcpg_sentinel = super::common::heredoc_sentinel("MCPG_CONFIG_EOF", mcpg_config_json)?;
    let mcpg_config_heredoc = format!(
        "cat > \"$AGENT_TEMP/staging/mcpg-config.json\" << '{mcpg_sentinel}'\n\
         {mcpg_config_json}\n\
         {mcpg_sentinel}"
    );
    let custom_tools_fragment = if let Some(custom_tools_json) = custom_tools_json {
        let sentinel =
            super::common::heredoc_sentinel("CUSTOM_TOOLS_JSON_EOF", custom_tools_json)?;
        format!(
            "# Write compiler-generated dynamic SafeOutputs tool definitions\n\
             cat > \"$AGENT_TEMP/staging/custom-tools.json\" << '{sentinel}'\n\
             {custom_tools_json}\n\
             {sentinel}\n\
             python3 -m json.tool \"$AGENT_TEMP/staging/custom-tools.json\" > /dev/null"
        )
    } else {
        String::new()
    };
    Ok(ShellScript::new(&PREPARE_MCPG_CONFIG)
        .bind("AGENT_TEMP", Binding::ado_macro("Agent.TempDirectory"))
        .bind("MCPG_PORT", Binding::number(MCPG_PORT.into()))
        .bind_text("MCPG_DOMAIN", MCPG_DOMAIN)
        .fragment("mcpg_config_heredoc", mcpg_config_heredoc)
        .fragment("custom_tools_block", custom_tools_fragment)
        .into_step("Prepare MCPG config"))
}

shell_script! {
    /// Prepare the AWF working directory (`/tmp/awf-tools/`) with the
    /// compiler binary and MCPG staging JSON. Copies the compiler out of the
    /// Pipeline.Workspace and into `/tmp/` so it is reachable inside the
    /// AWF-managed container (AWF auto-mounts `/tmp:/tmp:rw`).
    PREPARE_TOOLING {
        interpreter: Bash,
        bindings: [PIPELINE_WORKSPACE, AGENT_TEMP],
        externals: [HOME],
        fragments: [],
        body: r###"
mkdir -p /tmp/awf-tools/staging

echo "HOME: $HOME"

# Use absolute path since MCP subprocess may not inherit PATH
AGENTIC_PIPELINES_PATH="$PIPELINE_WORKSPACE/agentic-pipeline-compiler/ado-aw"

# Verify the binary exists and is executable
ls -la "$AGENTIC_PIPELINES_PATH"
chmod +x "$AGENTIC_PIPELINES_PATH"

$AGENTIC_PIPELINES_PATH -h

# Copy compiler binary to /tmp so it's accessible inside AWF container
cp "$AGENTIC_PIPELINES_PATH" /tmp/awf-tools/ado-aw
chmod +x /tmp/awf-tools/ado-aw

# Copy MCPG config to /tmp
cp "$AGENT_TEMP/staging/mcpg-config.json" /tmp/awf-tools/staging/mcpg-config.json
if [ -f "$AGENT_TEMP/staging/custom-tools.json" ]; then
  cp "$AGENT_TEMP/staging/custom-tools.json" /tmp/awf-tools/staging/custom-tools.json
fi
"###,
    }
}

fn prepare_tooling_step() -> BashStep {
    ShellScript::new(&PREPARE_TOOLING)
        .bind(
            "PIPELINE_WORKSPACE",
            Binding::ado_macro("Pipeline.Workspace"),
        )
        .bind("AGENT_TEMP", Binding::ado_macro("Agent.TempDirectory"))
        .into_step("Prepare tooling")
}

shell_script! {
    /// Write the agent-prompt markdown to `/tmp/awf-tools/agent-prompt.md`
    /// so that it is reachable inside the AWF container (AWF auto-mounts
    /// `/tmp:/tmp:rw`). The `heredoc` fragment carries a per-content
    /// SHA-derived sentinel so a malicious agent markdown body cannot
    /// terminate the heredoc early and inject shell into the Agent job.
    PREPARE_AGENT_PROMPT {
        interpreter: Bash,
        bindings: [],
        externals: [],
        fragments: [heredoc],
        body: r###"
# Write agent instructions to /tmp so it's accessible inside AWF container
# ado-aw:fragment heredoc

echo "Agent prompt:"
cat "/tmp/awf-tools/agent-prompt.md"
"###,
    }
}

fn prepare_agent_prompt_step(agent_content: &str) -> Result<BashStep> {
    let sentinel = super::common::heredoc_sentinel("AGENT_PROMPT_EOF", agent_content)?;
    let heredoc = format!(
        "cat > \"/tmp/awf-tools/agent-prompt.md\" << '{sentinel}'\n{agent_content}\n{sentinel}"
    );
    Ok(ShellScript::new(&PREPARE_AGENT_PROMPT)
        .fragment("heredoc", heredoc)
        .into_step("Prepare agent prompt"))
}

fn download_awf_step(supply_chain: Option<&SupplyChainConfig>) -> Vec<Step> {
    if let Some(artifact) = supply_chain.and_then(|sc| sc.pipeline_artifact.as_ref()) {
        let dest = "$(Pipeline.Workspace)/awf";
        let staging = "$(Pipeline.Workspace)/ado-aw-candidate/awf";
        let tail = "mv awf-linux-x64 awf\n\
                    chmod +x awf\n\
                    echo \"##vso[task.prependpath]$(Pipeline.Workspace)/awf\"\n\
                    ./awf --version\n";
        let body =
            stage_candidate_artifact_payload_bash(artifact, staging, dest, "awf-linux-x64", tail);
        return vec![
            Step::Task(download_candidate_artifact_step(
                artifact,
                "Download candidate artifact for AWF",
                staging,
            )),
            Step::Bash(BashStep::new(
                "Stage candidate AWF (Agentic Workflow Firewall)",
                body,
            )),
        ];
    }

    if let Some(feed) = supply_chain.and_then(|sc| sc.feed.as_ref()) {
        let dest = "$(Pipeline.Workspace)/awf";
        let staging = "$(Pipeline.Workspace)/awf/_pkg";
        let tail = "mv awf-linux-x64 awf\n\
                    chmod +x awf\n\
                    echo \"##vso[task.prependpath]$(Pipeline.Workspace)/awf\"\n\
                    ./awf --version\n";
        let body = extract_package_payload_bash(staging, dest, "awf-linux-x64", tail);
        // Auth is hoisted to the job builder via `feed_auth_step`.
        return vec![
            Step::Task(download_package_step(
                format!("Download AWF (Agentic Workflow Firewall) v{AWF_VERSION}"),
                feed.name.as_str(),
                "awf",
                AWF_VERSION,
                staging,
            )),
            Step::Bash(BashStep::new(
                format!("Stage AWF (Agentic Workflow Firewall) v{AWF_VERSION}"),
                body,
            )),
        ];
    }

    vec![Step::Bash(
        ShellScript::new(&DOWNLOAD_AWF_FROM_RELEASES)
            .bind_text("AWF_VERSION", AWF_VERSION)
            .bind(
                "PIPELINE_WORKSPACE",
                Binding::ado_macro("Pipeline.Workspace"),
            )
            .into_step(format!(
                "Download AWF (Agentic Workflow Firewall) v{AWF_VERSION}"
            )),
    )]
}

fn prepull_images_step(include_mcpg: bool, supply_chain: Option<&SupplyChainConfig>) -> Vec<Step> {
    let registry = supply_chain.and_then(|sc| sc.registry.as_ref());
    let registry_base = registry.map(|r| r.name.as_str());

    let squid = image_ref(
        "ghcr.io/github/gh-aw-firewall/squid",
        AWF_VERSION,
        registry_base,
    );
    let agent = image_ref(
        "ghcr.io/github/gh-aw-firewall/agent",
        AWF_VERSION,
        registry_base,
    );
    let api_proxy = image_ref(
        "ghcr.io/github/gh-aw-firewall/api-proxy",
        AWF_VERSION,
        registry_base,
    );

    let (display, mcpg_pull) = if include_mcpg {
        let mcpg = image_ref(MCPG_IMAGE, &format!("v{MCPG_VERSION}"), registry_base);
        (
            format!("Pre-pull AWF and MCPG container images (v{AWF_VERSION})"),
            format!("docker pull \"{mcpg}\""),
        )
    } else {
        (
            format!("Pre-pull AWF container images (v{AWF_VERSION})"),
            String::new(),
        )
    };

    let mut steps = Vec::new();
    // When using an internal registry, authenticate before pulling so the
    // job's docker daemon (shared with the subsequent `docker run` of MCPG)
    // can reach the registry.
    if let (Some(base), Some(conn)) = (
        registry_base,
        supply_chain.and_then(|sc| sc.registry_connection()),
    ) {
        steps.push(Step::Task(acr_login_step(base, conn)));
    }
    steps.push(Step::Bash(
        ShellScript::new(&PREPULL_IMAGES)
            .bind_text("SQUID_IMAGE", &squid)
            .bind_text("AGENT_IMAGE", &agent)
            .bind_text("API_PROXY_IMAGE", &api_proxy)
            .fragment("mcpg_pull", mcpg_pull)
            .into_step(display),
    ));
    steps
}

shell_script! {
    /// Start the MCP Gateway (MCPG) on the runner's Docker daemon so that AWF
    /// can later attach it to its isolated internal network. This is
    /// contractually a *single* Bash task — the API key never leaves the
    /// process — so the block-scalar body has to carry the full multi-line
    /// `docker run …` invocation. The compiler-owned `docker_env_lines` and
    /// `debug_flag` fragments splice any extra `-e VAR=…` and `-e DEBUG=…`
    /// continuation lines directly into the middle of that invocation.
    ///
    /// Uses:
    /// - the pipeline variables `MCP_GATEWAY_API_KEY` (secret) and
    ///   `ADO_PROXY_IP` (from the optional ado-proxy startup step) as
    ///   externals routed via `env:`, so the secret is not baked into the
    ///   emitted YAML prelude
    /// - `Binding::text` for the two static topology names (container +
    ///   image ref) and `Binding::number` for the fixed listen port
    /// - Compile-time constant `MCP_GATEWAY_DOMAIN` bound as text so the
    ///   docker container receives the same domain constant used elsewhere
    START_MCPG {
        interpreter: Bash,
        bindings: [MCPG_CONTAINER, MCPG_IMAGE, MCPG_PORT, MCPG_DOMAIN],
        externals: [MCP_GATEWAY_API_KEY, ADO_PROXY_IP],
        fragments: [debug_flag, docker_env_lines],
        body: r###"
# Substitute runtime values into MCPG config
MCP_RUNNER_UID=$(id -u)
MCP_RUNNER_GID=$(id -g)
MCPG_CONFIG=$(sed \
  -e "s|\${MCP_RUNNER_UID}|$MCP_RUNNER_UID|g" \
  -e "s|\${MCP_RUNNER_GID}|$MCP_RUNNER_GID|g" \
  -e "s|\${MCP_GATEWAY_API_KEY}|$MCP_GATEWAY_API_KEY|g" \
  -e "s|\${ADO_PROXY_IP}|${ADO_PROXY_IP:-}|g" \
  /tmp/awf-tools/staging/mcpg-config.json)

# A client redirected at an empty address would resolve the real
# Azure DevOps instead of the policy engine, quietly restoring the
# direct path this design removes. Fail loudly rather than start.
if grep -q 'ADO_PROXY_IP' /tmp/awf-tools/staging/mcpg-config.json \
   && [ -z "${ADO_PROXY_IP:-}" ]; then
  echo "##vso[task.complete result=Failed]ado-proxy address is unknown; refusing to start MCP clients unredirected"
  exit 1
fi

# Log the template config (before API key substitution) for debugging.
echo "Starting MCPG with config template:"
python3 -m json.tool < /tmp/awf-tools/staging/mcpg-config.json

# Remove any leftover container or stale output from a previous interrupted run
# (--rm only cleans up on clean exit; OOM/SIGKILL may leave it behind)
docker rm -f "$MCPG_CONTAINER" 2>/dev/null || true
GATEWAY_OUTPUT="/tmp/gh-aw/mcp-config/gateway-output.json"
mkdir -p "$(dirname "$GATEWAY_OUTPUT")" /tmp/gh-aw/mcp-logs
rm -f "$GATEWAY_OUTPUT"

# Start MCPG on Docker's bridge network. AWF attaches this named,
# trusted container to its internal network after creating awf-net.
# The Docker socket mount is required because MCPG spawns stdio-based MCP
# servers as sibling containers. This grants significant host access — acceptable
# here because the pipeline agent is already trusted and network-isolated by AWF.
#
# stdout → gateway-output.json (machine-readable config, read after health check)
echo "$MCPG_CONFIG" | docker run -i --rm \
  --name "$MCPG_CONTAINER" \
  --network bridge \
  -p "127.0.0.1:$MCPG_PORT:$MCPG_PORT" \
  --entrypoint /app/awmg \
  -v /var/run/docker.sock:/var/run/docker.sock \
  -e MCP_GATEWAY_PORT="$MCPG_PORT" \
  -e MCP_GATEWAY_DOMAIN="$MCPG_DOMAIN" \
  -e MCP_GATEWAY_API_KEY="$MCP_GATEWAY_API_KEY" \
  # ado-aw:fragment debug_flag
  # ado-aw:fragment docker_env_lines
  "$MCPG_IMAGE" \
  --routed --listen "0.0.0.0:$MCPG_PORT" --config-stdin --log-dir /tmp/gh-aw/mcp-logs \
  > "$GATEWAY_OUTPUT" 2> >(tee /tmp/gh-aw/mcp-logs/stderr.log >&2) &
MCPG_PID=$!
echo "MCPG started (PID: $MCPG_PID)"

# Wait for MCPG to be ready
READY=false
for _i in $(seq 1 30); do
  if curl -sf "http://localhost:$MCPG_PORT/health" > /dev/null 2>&1; then
    echo "MCPG is ready"
    READY=true
    break
  fi
  sleep 1
done
if [ "$READY" != "true" ]; then
  echo "##vso[task.complete result=Failed]MCPG did not become ready within 30s"
  exit 1
fi

# Wait for gateway output file to contain valid JSON with mcpServers.
# Health check passing doesn't guarantee stdout is flushed, so poll.
echo "Waiting for gateway output file..."
GATEWAY_READY=false
for _i in $(seq 1 15); do
  if [ -s "$GATEWAY_OUTPUT" ] && jq -e '.mcpServers' "$GATEWAY_OUTPUT" > /dev/null 2>&1; then
    echo "Gateway output is ready"
    GATEWAY_READY=true
    break
  fi
  sleep 1
done
if [ "$GATEWAY_READY" != "true" ]; then
  echo "##vso[task.complete result=Failed]Gateway output file not ready within 15s"
  echo "Gateway output content:"
  cat "$GATEWAY_OUTPUT" 2>/dev/null || echo "(empty or missing)"
  exit 1
fi

echo "Gateway output:"
cat "$GATEWAY_OUTPUT"

# Convert gateway output to Copilot CLI mcp-config.json.
# Mirrors gh-aw's convert_gateway_config_copilot.cjs:
#   - Rewrite gateway URLs to the stable MCPG container name that AWF
#     attaches to its internal network
#   - Ensure tools: ["*"] on each server entry (Copilot CLI requirement)
#   - Mark generated MCPG entries as default/trusted servers for Copilot CLI
#   - Preserve all other fields (headers, type, etc.)
jq --arg prefix "http://$MCPG_DOMAIN:$MCPG_PORT" \
  '.mcpServers |= (to_entries | sort_by(.key) | map(.value.url |= sub("^http://[^/]+/"; "\($prefix)/") | .value.tools = ["*"] | .value.isDefaultServer = true) | from_entries)' \
  "$GATEWAY_OUTPUT" > /tmp/awf-tools/mcp-config.json

chmod 600 /tmp/awf-tools/mcp-config.json

echo "Generated MCP config at: /tmp/awf-tools/mcp-config.json"
cat /tmp/awf-tools/mcp-config.json
"###,
    }
}

fn start_mcpg_step(
    mcpg_docker_env: &str,
    mcpg_step_env: &str,
    debug_pipeline: bool,
    supply_chain: Option<&SupplyChainConfig>,
) -> Result<BashStep> {
    let registry_base = supply_chain
        .and_then(|sc| sc.registry.as_ref())
        .map(|r| r.name.as_str());
    let mcpg_image_v = image_ref(MCPG_IMAGE, &format!("v{MCPG_VERSION}"), registry_base);

    // Match the legacy layout of two placeholder `\`-continuation lines when
    // no extensions contribute docker env — bash treats a lone `\` as a
    // no-op continuation and preserving the shape keeps the docker-run
    // command's argument boundaries identical to the pre-migration YAML.
    // `generate_mcpg_docker_env` returns a single `\` byte when no
    // extensions contribute, so match that sentinel as well as an empty
    // string.
    let docker_env_lines: String =
        if mcpg_docker_env.trim().is_empty() || mcpg_docker_env.trim() == "\\" {
            // Two empty continuation lines mirror the legacy template's
            // two-marker layout.
            "\\\n  \\".to_string()
        } else {
            // `generate_mcpg_docker_env` already terminates every line with
            // ` \` continuation, so re-indent the lines without appending
            // another ` \` (issue #1034).
            mcpg_docker_env.lines().collect::<Vec<_>>().join("\n  ")
        };
    // `--debug-pipeline` injects an extra `-e DEBUG="*" \` continuation line
    // into the `docker run …` invocation so MCPG (and the stdio backends it
    // spawns) emit verbose logs to the gateway stderr stream.
    let debug_flag = if debug_pipeline {
        "-e DEBUG=\"*\" \\".to_string()
    } else {
        "\\".to_string()
    };

    use super::ir::env::EnvValue;
    let mut step = ShellScript::new(&START_MCPG)
        .bind_text("MCPG_CONTAINER", MCPG_CONTAINER_NAME)
        .bind_text("MCPG_IMAGE", &mcpg_image_v)
        .bind("MCPG_PORT", Binding::number(MCPG_PORT.into()))
        .bind_text("MCPG_DOMAIN", MCPG_DOMAIN)
        .fragment("debug_flag", debug_flag)
        .fragment("docker_env_lines", docker_env_lines)
        .into_step("Start MCP Gateway (MCPG)")
        .with_env(
            "MCP_GATEWAY_API_KEY",
            EnvValue::pipeline_var("MCP_GATEWAY_API_KEY"),
        )
        .with_env("ADO_PROXY_IP", EnvValue::pipeline_var("ADO_PROXY_IP"));
    for (k, v) in parse_env_block(mcpg_step_env)? {
        step = step.with_env(k, v);
    }
    Ok(step)
}

/// Build AWF image-selection flags for the pre-pulled container set.
fn awf_image_flags(supply_chain: Option<&SupplyChainConfig>) -> String {
    let mut block = format!("  --image-tag \"{AWF_VERSION}\" \\\n");
    if let Some(registry) = supply_chain.and_then(|sc| sc.registry.as_ref()) {
        block.push_str(&format!(
            "  --image-registry \"{}\" \\\n",
            registry.name.as_str()
        ));
    }
    block.push_str("  ");
    block
}

/// Build AWF environment-exclusion flag lines for a Copilot BYOM/BYOK run.
///
/// `exclude_keys` are the provider credential env keys present in `engine.env`
/// (canonical uppercase `COPILOT_PROVIDER_*` names). AWF 0.27.32+ always enables
/// its API proxy, so only one `--exclude-env <key>` line is needed per key.
///
/// How the credential reaches the provider without reaching the agent: AWF's
/// api-proxy sidecar reads the *real*
/// `COPILOT_PROVIDER_*` values from the host process env, and injects
/// **placeholders** into the agent container regardless of `--env-all` —
/// `COPILOT_PROVIDER_BASE_URL` becomes the sidecar URL (e.g.
/// `http://172.30.0.30:10002`) and `COPILOT_PROVIDER_API_KEY` a dummy token (see
/// gh-aw-firewall `docs/api-proxy-sidecar.md`, "agent container env" table, and
/// `containers/api-proxy/providers/copilot.js`, verified against AWF v0.27.9).
/// The Copilot CLI therefore talks to the sidecar, which strips the client auth
/// header and injects the real credential on the outbound request. `--exclude-env`
/// keeps the raw value out of `--env-all` passthrough (defense-in-depth on top of
/// AWF's placeholder override). Because env-var names are case-sensitive and the
/// keys are the canonical uppercase names, the emitted `--exclude-env <key>`
/// matches exactly what AWF overrides and the CLI reads.
///
/// Shared by [`run_agent_step`] and [`run_threat_analysis_step`].
fn awf_exclude_env_flags(exclude_keys: &[String]) -> String {
    let mut block = String::new();
    for key in exclude_keys {
        block.push_str(&format!("  --exclude-env {key} \\\n"));
    }
    block
}

shell_script! {
    /// Invoke the AI agent inside AWF's network-isolated Docker topology.
    ///
    /// This is the workflow's *single* Bash task — the pre-signed engine
    /// command must reach `awf` without any wrapper mutating it, so the
    /// entire multi-line `awf …` invocation lives in a single block-scalar
    /// body. Everything variable is spliced via fragments:
    /// - `topology_attach` — one `--topology-attach` line per trusted peer
    ///   (MCPG always, ado-proxy when the policy engine is enabled)
    /// - `image_flags` — `--image-tag` plus optional `--image-registry`
    /// - `exclude_env` — one `--exclude-env <key>` line per BYOM/BYOK secret
    ///   AWF's api-proxy sidecar strips out of the agent env
    /// - `awf_mounts` — the compiler-supplied chain of `--mount "…"` args
    /// - `routed_engine_run` — the single-quoted `NO_PROXY` prefix + engine
    ///   command that AWF invokes inside the sandbox
    RUN_AGENT {
        interpreter: Bash,
        bindings: [AGENT_TEMP, PIPELINE_WORKSPACE, ALLOWED_DOMAINS],
        externals: [WORKING_DIRECTORY],
        fragments: [topology_attach, image_flags, exclude_env, awf_mounts, routed_engine_run],
        body: r###"
set -o pipefail

AGENT_OUTPUT_FILE="$AGENT_TEMP/staging/logs/agent-output.txt"
mkdir -p "$AGENT_TEMP/staging/logs"
AGENT_EXIT_CODE=0

echo "=== Running AI agent with AWF network isolation ==="
echo "Allowed domains: $ALLOWED_DOMAINS"

# AWF provides L7 domain whitelisting via a rootless Docker topology.
# The named MCPG container is attached to AWF's internal network as a
# trusted endpoint; the agent has no route to the host.
# AWF auto-mounts /tmp:/tmp:rw into the container, so copilot binary,
# agent prompt, and MCP config are placed under /tmp/awf-tools/.
# The argument list is assembled into an array so runtime-supplied
# fragments splice in as ordinary shell statements (`AWF_ARGS+=(...)`)
# — no `\`-continuation chain to break with fragment marker comments.
AWF_ARGS=(
  --allow-domains "$ALLOWED_DOMAINS"
  --network-isolation
)
# ado-aw:fragment topology_attach
# ado-aw:fragment image_flags
AWF_ARGS+=(--skip-pull --env-all)
# ado-aw:fragment exclude_env
# ado-aw:fragment awf_mounts
AWF_ARGS+=(
  --container-workdir "$WORKING_DIRECTORY"
  --log-level info
  --proxy-logs-dir "$AGENT_TEMP/staging/logs/firewall"
)
# ado-aw:fragment routed_engine_run

# Stream agent output in real-time while filtering VSO commands.
# sed -u = unbuffered (line-by-line) so output appears immediately.
# tee writes to both stdout (ADO pipeline log) and the artifact file.
# pipefail (set above) ensures AWF's exit code propagates through the pipe.
# shellcheck disable=SC2016 # The single-quoted engine command inside AWF_ARGS is intentionally expanded by AWF inside the sandbox
"$PIPELINE_WORKSPACE/awf/awf" "${AWF_ARGS[@]}" 2>&1 \
  | sed -u 's/##vso\[/[VSO-FILTERED] vso[/g; s/##\[/[VSO-FILTERED] [/g' \
  | tee "$AGENT_OUTPUT_FILE" \
  || AGENT_EXIT_CODE=$?

# Print firewall summary if available
if [ -x "$PIPELINE_WORKSPACE/awf/awf" ]; then
  echo "=== Firewall Summary ==="
  "$PIPELINE_WORKSPACE/awf/awf" logs summary --source "$AGENT_TEMP/staging/logs/firewall" 2>/dev/null || true
fi

exit "$AGENT_EXIT_CODE"
"###,
    }
}

#[allow(clippy::too_many_arguments)]
fn run_agent_step(
    allowed_domains: &str,
    awf_mounts: &str,
    working_directory: &str,
    engine_run: &str,
    engine_env: &str,
    byom_exclude_keys: &[String],
    supply_chain: Option<&SupplyChainConfig>,
    ado_proxy_enabled: bool,
) -> Result<BashStep> {
    // The awf_mounts string is a `\`-joined chain of `--mount "..."` lines;
    // splice it in at the fragment marker's own indent.
    let awf_mounts_block: String = if awf_mounts == "\\" {
        // "\\" is the sentinel for "no mounts" in the legacy string
        // shape; produce an empty append so the array stays unchanged.
        String::new()
    } else {
        // The legacy shape is `--mount "..." \` per line. Strip the trailing
        // `\`, split on whitespace-separated `--mount` occurrences, and rebuild
        // as an `AWF_ARGS+=(...)` statement.
        let mut lines: Vec<String> = Vec::new();
        for line in awf_mounts.lines() {
            let line = line.trim();
            let line = line.strip_suffix('\\').unwrap_or(line).trim_end();
            if line.is_empty() {
                continue;
            }
            lines.push(line.to_string());
        }
        if lines.is_empty() {
            String::new()
        } else {
            // The ADO macro `$(AW_AZ_MOUNTS)` (contributed by the Azure CLI
            // extension) is substituted at YAML load time before bash sees it.
            // shellcheck cannot see the ADO substitution and mis-reads it as
            // bash command substitution word-splitting into the array (SC2207),
            // which is precisely what we want here because ADO expands the
            // macro to zero or more `--mount ...` tokens.
            format!(
                "# shellcheck disable=SC2207 # $(AW_AZ_MOUNTS) is an ADO macro substituted at YAML load; word-splitting into the array is intentional.\nAWF_ARGS+=({})",
                lines.join(" ")
            )
        }
    };
    let image_flags_block = awf_image_flags(supply_chain);
    let exclude_env_block = awf_exclude_env_flags(byom_exclude_keys);

    // AWF attaches externally-launched trusted containers to its internal
    // network by name. The flag is repeatable, which is what lets the policy
    // engine join alongside MCPG. Attaching also gives the agent an
    // `/etc/hosts` entry for the container, so the `az` wrapper can resolve
    // the engine by name without relying on Docker's embedded DNS.
    let topology_attach_block = {
        let mut parts = vec![format!("--topology-attach \"{MCPG_CONTAINER_NAME}\"")];
        if ado_proxy_enabled {
            parts.push(format!("--topology-attach \"{ADO_PROXY_CONTAINER_NAME}\""));
        }
        format!("AWF_ARGS+=({})", parts.join(" "))
    };

    // Convert `awf_image_flags`'s legacy `  --flag "..." \\\n` shape into
    // `AWF_ARGS+=(--flag "..." ...)`.
    let image_flags_line = {
        let mut parts: Vec<String> = Vec::new();
        for line in image_flags_block.lines() {
            let line = line.trim();
            let line = line.strip_suffix('\\').unwrap_or(line).trim_end();
            if line.is_empty() {
                continue;
            }
            parts.push(line.to_string());
        }
        format!("AWF_ARGS+=({})", parts.join(" "))
    };

    // Same conversion for `awf_exclude_env_flags`.
    let exclude_env_line = {
        let mut parts: Vec<String> = Vec::new();
        for line in exclude_env_block.lines() {
            let line = line.trim();
            let line = line.strip_suffix('\\').unwrap_or(line).trim_end();
            if line.is_empty() {
                continue;
            }
            parts.push(line.to_string());
        }
        if parts.is_empty() {
            String::new()
        } else {
            format!("AWF_ARGS+=({})", parts.join(" "))
        }
    };

    // Trusted peers must bypass Squid: their names are not public DNS, and
    // routing them through the proxy would break the very connection that
    // reaches the policy engine.
    let no_proxy_peers = if ado_proxy_enabled {
        format!("{MCPG_CONTAINER_NAME},{ADO_PROXY_CONTAINER_NAME}")
    } else {
        MCPG_CONTAINER_NAME.to_string()
    };
    let routed_engine_run = format!(
        "AWF_ARGS+=(-- 'export NO_PROXY=\"${{NO_PROXY:+$NO_PROXY,}}{no_proxy_peers}\"; \
         export no_proxy=\"$NO_PROXY\"; {engine_run}')"
    );

    let mut step = ShellScript::new(&RUN_AGENT)
        .bind("AGENT_TEMP", Binding::ado_macro("Agent.TempDirectory"))
        .bind(
            "PIPELINE_WORKSPACE",
            Binding::ado_macro("Pipeline.Workspace"),
        )
        .bind_text("ALLOWED_DOMAINS", allowed_domains)
        .fragment("topology_attach", topology_attach_block)
        .fragment("image_flags", image_flags_line)
        .fragment("exclude_env", exclude_env_line)
        .fragment("awf_mounts", awf_mounts_block)
        .fragment("routed_engine_run", routed_engine_run)
        .into_step("Run copilot (AWF network isolated)");
    step.working_directory = Some(working_directory.to_string());
    // Engine env comes as a multi-line YAML env block — `KEY: VALUE` lines
    // joined by `\n`, no `env:` prefix (it's the value side of an env: mapping).
    let synthetic_block = format!(
        "env:\n{}",
        engine_env
            .lines()
            .map(|l| format!("  {l}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    use super::ir::env::EnvValue;
    // WORKING_DIRECTORY is passed via env: so ADO substitutes any `$(...)`
    // macros in the value before bash sees it. The prelude `Binding::text`
    // channel deliberately refuses `$(` to prevent unreviewed macro
    // substitutions.
    step = step.with_env("WORKING_DIRECTORY", EnvValue::literal(working_directory));
    for (k, v) in parse_env_block(&synthetic_block)? {
        step = step.with_env(k, v);
    }
    Ok(step)
}

shell_script! {
    /// Run `ado-aw execute` (Stage 3). Translates a `SucceededWithIssues`
    /// exit code (2) from the executor into an ADO SucceededWithIssues
    /// result rather than a hard failure.
    ///
    /// The path externals are supplied through the step `env:` block so ADO
    /// expands their `$(…)` macros before bash sees the value; the compiler
    /// itself would refuse to bake an unreviewed `$(` into a binding.
    ///
    /// `FILTER_ARGS` is intentionally expanded unquoted so an authored value
    /// like `--only foo --exclude bar` word-splits into individual flags.
    /// The producer restricts each token to the safe-output allow-list
    /// vocabulary (`is_safe_tool_name`), so no shell metacharacter can appear.
    EXECUTE_SAFE_OUTPUTS {
        interpreter: Bash,
        bindings: [FILTER_ARGS],
        externals: [
            ADO_AW_SOURCE_PATH,
            ADO_AW_RESOLVED_CONFIG_PATH,
            ADO_AW_SAFE_OUTPUT_DIR,
            ADO_AW_OUTPUT_DIR,
        ],
        fragments: [],
        body: r###"
# shellcheck disable=SC2086 # FILTER_ARGS is a compiler-owned run of --only/--exclude flags; unquoted expansion is intentional.
ado-aw execute --source "$ADO_AW_SOURCE_PATH" --resolved-config "$ADO_AW_RESOLVED_CONFIG_PATH" --safe-output-dir "$ADO_AW_SAFE_OUTPUT_DIR" --output-dir "$ADO_AW_OUTPUT_DIR" $FILTER_ARGS
EXIT_CODE=$?
if [ $EXIT_CODE -eq 2 ]; then
  echo "##vso[task.complete result=SucceededWithIssues;]Executor completed with warnings"
  exit 0
fi
exit $EXIT_CODE
"###,
    }
}

fn execute_safe_outputs_step(
    source_path: &str,
    resolved_config_path: &str,
    // Stage 3 runs git operations against `repository: self`, so the executor's
    // working directory is the exact self checkout — not the resolved
    // `workspace:` directory, which may point at another repository's alias.
    self_repository_directory: &str,
    self_repository_name: &EnvValue,
    executor_ado_env: &str,
    filter_args: &str,
) -> Result<BashStep> {
    // `filter_args` is either empty or a leading-space-prefixed run of
    // `--only <tool>` / `--exclude <tool>` flags appended to the command.
    let mut script = ShellScript::new(&EXECUTE_SAFE_OUTPUTS)
        .bind_text("FILTER_ARGS", filter_args.trim())
        .into_step("Execute safe outputs (Stage 3)");
    script.working_directory = Some(self_repository_directory.to_string());
    // Path externals reach bash through ADO env expansion, which is the
    // documented mechanism for values holding predefined macros.
    script = script
        .with_env("ADO_AW_SOURCE_PATH", EnvValue::literal(source_path))
        .with_env(
            "ADO_AW_RESOLVED_CONFIG_PATH",
            EnvValue::literal(resolved_config_path),
        )
        .with_env(
            "ADO_AW_SAFE_OUTPUT_DIR",
            EnvValue::literal("$(Pipeline.Workspace)/analyzed_outputs_$(Build.BuildId)"),
        )
        .with_env(
            "ADO_AW_OUTPUT_DIR",
            EnvValue::literal("$(Agent.TempDirectory)/staging"),
        );
    for (k, v) in parse_env_block(executor_ado_env)? {
        script = script.with_env(k, v);
    }
    script = script.with_env(
        "ADO_AW_SELF_REPOSITORY_DIRECTORY",
        // The value embeds `$(Build.SourcesDirectory)`, but it is still a
        // `Literal`: ADO expands `$(...)` macros in step `env:` values at agent
        // runtime, so the macro reaches the executor already resolved.
        // `EnvValue::AdoMacro` is for values that are *only* a macro; this one
        // is a macro-plus-suffix path, and `Concat` would add no value because
        // no part of it needs separate lowering.
        EnvValue::literal(self_repository_directory),
    );
    script = script.with_env(
        "ADO_AW_SELF_REPOSITORY_NAME",
        self_repository_name.clone(),
    );
    Ok(script)
}

shell_script! {
    /// Copy staged safe outputs from AWF's `/tmp` mount back into the
    /// ADO staging directory for artifact publish.
    COLLECT_SAFE_OUTPUTS {
        interpreter: Bash,
        bindings: [AGENT_TEMP],
        externals: [],
        fragments: [],
        body: r#"
# Copy safe outputs from /tmp back to staging for artifact publish
mkdir -p "$AGENT_TEMP/staging"
cp -r /tmp/awf-tools/staging/* "$AGENT_TEMP/staging/" 2>/dev/null || true
echo "Safe outputs copied to $AGENT_TEMP/staging"
ls -la "$AGENT_TEMP/staging" 2>/dev/null || echo "No safe outputs found"
"#,
    }
}

fn collect_safe_outputs_step() -> BashStep {
    ShellScript::new(&COLLECT_SAFE_OUTPUTS)
        .bind("AGENT_TEMP", Binding::ado_macro("Agent.TempDirectory"))
        .into_step("Collect safe outputs from AWF container")
        .with_condition(Condition::Always)
}

shell_script! {
    /// Render the proposed safe outputs to a sanitized markdown file for the
    /// build summary tab. Best-effort: a non-zero exit is downgraded to a
    /// warning so the summary can never block the review gate.
    SAFE_OUTPUTS_SUMMARY {
        interpreter: Bash,
        bindings: [APPROVAL_SUMMARY_PATH],
        externals: [],
        fragments: [],
        body: r###"
node "$APPROVAL_SUMMARY_PATH" || echo "##vso[task.logissue type=warning]approval-summary step failed (non-fatal)"
"###,
    }
}

/// Render the proposed safe outputs to a sanitized markdown file and attach it
/// to the build summary tab (`##vso[task.uploadsummary]`), via the
/// `approval-summary.js` ado-script bundle.
///
/// Emitted at the **end of the Agent job** (after `collect_safe_outputs_step`
/// has staged `safe_outputs.ndjson`), never in the Detection/threat-analysis
/// job. The ado-script bundle is delivered earlier in the same job by the
/// ado-script extension's agent-prepare steps (gated on
/// `safe_outputs_summary_active`).
///
/// `reviewed` is the compiler-resolved set of approval-gated tool names; when
/// non-empty the bundle lists those proposals first under a "Pending approval"
/// heading. It is passed through the typed env block (not spliced into the
/// shell command), so tool names never reach a shell word-split. Tool names are
/// joined with a newline (`\n`) rather than a comma: a `,` can legally appear in
/// an unrestricted YAML map key, so a comma delimiter could misparse such a key,
/// whereas a newline can never appear in a one-line map key. (`is_safe_tool_name`
/// already rejects both via `validate_safe_outputs_keys`, so this is
/// defense-in-depth.)
///
/// Best-effort: a non-zero exit from the bundle is downgraded to a warning so
/// rendering the summary can never fail the build or block the review gate.
/// The output base name is namespaced (`ado-aw-safe-outputs.md`) so the
/// ADO-derived summary-tab title never collides with a consumer/template-target
/// `task.uploadsummary` tab.
fn approval_summary_repository_policies(front_matter: &FrontMatter) -> Result<String> {
    let mut policies = serde_json::Map::new();
    for tool in front_matter.github_issue_tool_names() {
        let Some(config) = front_matter.github_issue_compiler_config(&tool)? else {
            continue;
        };
        policies.insert(
            tool,
            serde_json::json!({
                "targetRepo": config.target_repo,
                "allowedRepos": config.allowed_repos,
            }),
        );
    }
    Ok(serde_json::Value::Object(policies).to_string())
}

fn safe_outputs_summary_step(front_matter: &FrontMatter, reviewed: &[String]) -> Result<BashStep> {
    use super::ir::env::EnvValue;
    let approval_summary_path = super::extensions::ado_script::APPROVAL_SUMMARY_PATH;
    let repository_policies = approval_summary_repository_policies(front_matter)?;
    let github_api_url = front_matter
        .github_safe_outputs_auth()?
        .map(|auth| auth.api_url().to_string())
        .unwrap_or_default();
    Ok(ShellScript::new(&SAFE_OUTPUTS_SUMMARY)
        .bind_text("APPROVAL_SUMMARY_PATH", approval_summary_path)
        .into_step("Render safe-outputs summary")
        .with_env(
            "AW_SAFE_OUTPUTS_NDJSON",
            EnvValue::literal("$(Agent.TempDirectory)/staging/safe_outputs.ndjson"),
        )
        .with_env(
            "AW_APPROVAL_SUMMARY_OUT",
            EnvValue::literal("$(Agent.TempDirectory)/ado-aw-safe-outputs.md"),
        )
        .with_env("AW_REVIEWED_TOOLS", EnvValue::literal(reviewed.join("\n")))
        .with_env(
            "AW_GITHUB_REPOSITORY_POLICIES",
            EnvValue::literal(repository_policies),
        )
        .with_env(
            "AW_CURRENT_REPOSITORY",
            EnvValue::ado_macro("Build.Repository.Name")?,
        )
        .with_env(
            "AW_CURRENT_REPOSITORY_PROVIDER",
            EnvValue::ado_macro("Build.Repository.Provider")?,
        )
        .with_env("AW_GITHUB_API_URL", EnvValue::literal(github_api_url))
        .with_condition(Condition::Always))
}

shell_script! {
    /// Prepare the isolated Docker network shared by the proxy and optional
    /// MCP. `--internal` is load-bearing, not tidiness.
    PREPARE_ADO_PROXY_NETWORK {
        interpreter: Bash,
        bindings: [PROXY_NETWORK],
        externals: [],
        fragments: [],
        body: r#"
set -euo pipefail

# Network shared by the policy engine and the Azure DevOps MCP.
#
# `--internal` is load-bearing, not tidiness. A normal user-defined
# bridge has outbound NAT, so the MCP would keep a direct route to the
# internet — including Azure DevOps hosts that are not redirected —
# and the engine would police only the one hostname we happen to
# override. Measured: a container on a normal bridge reaches the
# internet; on an internal bridge it cannot, while still reaching its
# peers. The engine keeps its own egress because AWF dual-homes it
# onto awf-net, where Squid lives.
if ! docker network inspect "$PROXY_NETWORK" >/dev/null 2>&1; then
  docker network create --internal "$PROXY_NETWORK"
fi
"#,
    }
}

/// Prepare the isolated Docker network shared by the proxy and optional MCP.
fn prepare_ado_proxy_network_step() -> BashStep {
    ShellScript::new(&PREPARE_ADO_PROXY_NETWORK)
        .bind_text("PROXY_NETWORK", ADO_PROXY_NETWORK_NAME)
        .into_step("Prepare ado-proxy network")
}

shell_script! {
    /// Stage the Azure DevOps MCP package on the runner. It is installed on
    /// the runner (which has registry access) and mounted read-only into a
    /// container that does not. The mount point is load-bearing: Node
    /// resolves dependencies by walking upward from the importing file, so
    /// the tree must land at `/app/node_modules`.
    PREPARE_ADO_MCP {
        interpreter: Bash,
        bindings: [MCP_HOST_NODE_MODULES, MCP_PACKAGE, MCP_VERSION],
        externals: [],
        fragments: [],
        body: r###"
set -euo pipefail

# Install the MCP on the runner and stage it for mounting. The
# container it is mounted into can reach nothing but the engine, so it
# cannot fetch this itself.
MCP_STAGE="$(dirname "$MCP_HOST_NODE_MODULES")"
rm -rf "$MCP_STAGE"
mkdir -p "$MCP_STAGE"
cd "$MCP_STAGE"
npm init -y >/dev/null 2>&1
npm install --omit=dev --no-audit --no-fund --save-exact \
  "$MCP_PACKAGE@$MCP_VERSION"

# Verify the pin actually took. `npm install` resolves a *range* for
# anything it also has to satisfy transitively, so a matching request
# does not by itself guarantee a matching tree — and the agent's tool
# surface is defined by whatever ends up on disk here.
MCP_INSTALLED=$(node -p \
  "require('$MCP_HOST_NODE_MODULES/$MCP_PACKAGE/package.json').version")
if [ "$MCP_INSTALLED" != "$MCP_VERSION" ]; then
  echo "##vso[task.complete result=Failed]Azure DevOps MCP resolved to $MCP_INSTALLED, expected $MCP_VERSION"
  exit 1
fi

# Fail here rather than at MCP start time, where a missing entry
# script surfaces as an opaque MCPG backend error.
if [ ! -f "$MCP_HOST_NODE_MODULES/$MCP_PACKAGE/dist/index.js" ]; then
  echo "##vso[task.complete result=Failed]Azure DevOps MCP package did not install"
  exit 1
fi
echo "Azure DevOps MCP $MCP_INSTALLED staged at $MCP_HOST_NODE_MODULES"
"###,
    }
}

/// Stage the Azure DevOps MCP package only when its tool is enabled.
///
/// It is installed on the runner, which has registry access, and mounted
/// read-only into a container that does not. The mount point is load-bearing:
/// Node resolves dependencies by walking upward from the importing file, so
/// the tree must land at `/app/node_modules`.
fn prepare_ado_mcp_step(version: &str) -> BashStep {
    ShellScript::new(&PREPARE_ADO_MCP)
        .bind_text("MCP_HOST_NODE_MODULES", ADO_MCP_HOST_NODE_MODULES)
        .bind_text("MCP_PACKAGE", ADO_MCP_PACKAGE)
        .bind_text("MCP_VERSION", version)
        .into_step("Prepare Azure DevOps MCP")
}

shell_script! {
    /// Remove the network created for the policy engine and its clients.
    TEARDOWN_ADO_PROXY_NETWORK {
        interpreter: Bash,
        bindings: [PROXY_NETWORK],
        externals: [],
        fragments: [],
        body: r#"
# Remove the policy-engine network once its containers are gone
docker network rm "$PROXY_NETWORK" 2>/dev/null || true
"#,
    }
}

/// Remove the network created for the policy engine and its clients.
fn teardown_ado_proxy_network_step() -> BashStep {
    ShellScript::new(&TEARDOWN_ADO_PROXY_NETWORK)
        .bind_text("PROXY_NETWORK", ADO_PROXY_NETWORK_NAME)
        .into_step("Remove ado-proxy network")
        .with_condition(Condition::Always)
}

shell_script! {
    /// Stop the MCPG container.
    STOP_MCPG {
        interpreter: Bash,
        bindings: [MCPG_CONTAINER],
        externals: [],
        fragments: [],
        body: r#"
# Stop MCPG container
echo "Stopping MCPG..."
docker stop "$MCPG_CONTAINER" 2>/dev/null || true
echo "MCPG and stdio child containers stopped"
"#,
    }
}

fn stop_mcpg_step() -> BashStep {
    ShellScript::new(&STOP_MCPG)
        .bind_text("MCPG_CONTAINER", MCPG_CONTAINER_NAME)
        .into_step("Stop MCPG")
        .with_condition(Condition::Always)
}

/// Start the `ado-proxy` policy engine as a host container.
///
/// Mirrors [`start_mcpg_step`]: an ordinary bridge-networked container started
/// before AWF, which AWF then joins to its own network via
/// `--topology-attach`. It must start *before* MCPG, because the ADO MCP is
/// redirected at the proxy's container IP and that IP does not exist until the
/// container does.
///
/// # Why the interception material never touches disk
///
/// AWF chroots the agent with `/tmp` mounted at both `/tmp` and `/host/tmp`,
/// so *anything* written under `/tmp` is agent-readable. The CA private key
/// and the ADO bearer are therefore generated into the agent-private work
/// directory, streamed into the container on stdin, and deleted — the
/// container holds them in memory only. Writing them where the bundle could
/// read them from a file would hand the agent the exact credential this whole
/// design exists to withhold.
///
/// # Structure
///
/// The step's ~200-line body is composed from ordered phases, each registered
/// as its own [`shell_script!`] so shellcheck sees it in isolation and the
/// declared variable surface at each phase boundary is visible in the source.
/// See [`docs/ado-script.md`] and issue #1833 for the design rationale. All
/// phases still emit into a **single ADO Bash task**: the credential-custody
/// contract (bearer via `env:` only, private material streamed on stdin,
/// destroyed before readiness polling) requires atomic execution.
///
/// Not yet emitted: see [`stop_ado_proxy_step`].
fn start_ado_proxy_step(front_matter: &FrontMatter) -> BashStep {
    let policy = PolicyDocument::new(front_matter).to_json();
    let hosts: Vec<&str> = catalog::catalog().protected_hosts.to_vec();

    ShellScript::new(&START_ADO_PROXY)
        .bind_text("PROXY_CONTAINER", ADO_PROXY_CONTAINER_NAME)
        .bind_text("PROXY_IMAGE", ADO_PROXY_IMAGE)
        .bind_text("PROXY_NETWORK", ADO_PROXY_NETWORK_NAME)
        .bind_text("PROXY_SCRIPT_PATH", paths::ADO_PROXY_PATH)
        .bind_text("AZ_WRAPPER_DIR", AZ_WRAPPER_DIR)
        .bind_text("CA_HOST_PATH", ADO_PROXY_PUBLIC_CA_HOST_PATH)
        .bind("AGENT_TEMP", Binding::ado_macro("Agent.TempDirectory"))
        .bind(
            "ADO_PROXY_PROJECT",
            Binding::ado_macro("System.TeamProject"),
        )
        .bind(
            "ADO_PROXY_PROJECT_ID",
            Binding::ado_macro("System.TeamProjectId"),
        )
        .bind(
            "ADO_PROXY_REPOSITORY",
            Binding::ado_macro("Build.Repository.Name"),
        )
        .bind(
            "ADO_PROXY_REPOSITORY_ID",
            Binding::ado_macro("Build.Repository.ID"),
        )
        .bind("LEAF_HOSTS", Binding::words(&hosts))
        .bind("POLICY", Binding::document(policy))
        .bind(
            "CONTAINER_ENTRYPOINT",
            Binding::text(ado_proxy_container_entrypoint_flattened()),
        )
        .fragment("resolve_org", common::resolve_ado_organization_bash())
        .fragment(
            "setup_workdir",
            phase_body(&START_ADO_PROXY_SETUP_WORKDIR),
        )
        .fragment("write_policy", phase_body(&START_ADO_PROXY_WRITE_POLICY))
        .fragment(
            "mint_material",
            phase_body(&START_ADO_PROXY_MINT_MATERIAL),
        )
        .fragment(
            "build_material",
            phase_body(&START_ADO_PROXY_BUILD_MATERIAL),
        )
        .fragment(
            "run_container",
            phase_body(&START_ADO_PROXY_RUN_CONTAINER),
        )
        .fragment(
            "handover_material",
            phase_body(&START_ADO_PROXY_HANDOVER_MATERIAL),
        )
        .fragment(
            "destroy_private",
            phase_body(&START_ADO_PROXY_DESTROY_PRIVATE),
        )
        .fragment("wait_ready", phase_body(&START_ADO_PROXY_WAIT_READY))
        .into_step("Start ado-proxy policy engine")
        // The bearer is read from the environment here and immediately
        // base64-encoded into the stdin document; it is never written to a
        // file and never reaches the container's `Env`.
        .with_env("ADO_PROXY_BEARER", EnvValue::secret("SC_READ_TOKEN"))
}

/// Return a phase's body verbatim (trimmed and dedented) so it can be
/// spliced as a `# ado-aw:fragment` in [`START_ADO_PROXY`]. Each phase is
/// still registered — and therefore shellchecked — in isolation.
fn phase_body(def: &crate::compile::shell::ShellScriptDef) -> String {
    // Skip a leading shebang line if present (phases don't carry one; guard
    // is defence-in-depth), then dedent the raw body.
    let body = def.body.trim_start_matches('\n');
    crate::compile::shell::dedent(body).trim().to_string()
}

/// The one-liner passed to the container's `sh -c`. Kept in sync with the
/// registered [`START_ADO_PROXY_CONTAINER_ENTRYPOINT_SH`] script via
/// `container_entrypoint_matches_registered_body`.
fn ado_proxy_container_entrypoint_flattened() -> String {
    format!(
        "set -eu; umask 077; MATERIAL_FIFO=/tmp/ado-proxy-material; \
         mkfifo \"$MATERIAL_FIFO\"; \
         exec node /app/ado-proxy.js \
         --policy-file /etc/ado-proxy/policy.json \
         --public-ca-file /var/lib/ado-proxy/ado-proxy-ca.pem \
         --upstream-proxy {url} \
         --listen-port {lp} \
         --tls-port {tp} \
         --log-dir /var/log/ado-proxy < \"$MATERIAL_FIFO\"",
        url = AWF_SQUID_URL,
        lp = ADO_PROXY_LISTEN_PORT,
        tp = ADO_PROXY_TLS_PORT,
    )
}

// ── ado-proxy phase scripts ─────────────────────────────────────────────
//
// The start_ado_proxy step's ~200-line body is composed from these ordered
// phases. Each is registered so `src/compile/shell/lint.rs` shellchecks it in
// isolation, and each phase's declared `externals:` list makes the
// inter-phase variable contract visible in the source.
//
// The full script still runs as a **single trusted Bash task** because the
// credential-custody contract (bearer via env only, private material on
// stdin, destroyed before polling) requires atomic execution.

shell_script! {
    /// Phase 1: create the agent-private work directory outside `/tmp` and
    /// register a cleanup trap. AWF mounts `/tmp` into the agent chroot, so
    /// any private material generated under `/tmp` would be agent-readable.
    START_ADO_PROXY_SETUP_WORKDIR {
        interpreter: Bash,
        bindings: [],
        externals: [AGENT_TEMP],
        fragments: [],
        body: r###"
set -euo pipefail

# Generate into the agent work directory, NOT /tmp: AWF mounts /tmp
# into the agent chroot, so /tmp is readable by the agent.
umask 077
PROXY_DIR=$(mktemp -d "$AGENT_TEMP/ado-proxy.XXXXXX")
cleanup_material() { rm -rf "$PROXY_DIR"; }
trap cleanup_material EXIT
"###,
    }
}

shell_script! {
    /// Phase 3: write the policy document, substitute the scope identifiers
    /// resolved at pipeline runtime, refuse to start if any placeholder
    /// survives, then dump the fully substituted policy for auditability.
    ///
    /// Both name and GUID of project and repository are supplied because
    /// clients may address either — `az` substitutes whichever it cached.
    /// The bundle treats an absent identifier as matching nothing, so
    /// omitting one is a silent denial.
    START_ADO_PROXY_WRITE_POLICY {
        interpreter: Bash,
        bindings: [],
        externals: [
            PROXY_DIR, POLICY,
            ADO_PROXY_ORGANIZATION,
            ADO_PROXY_PROJECT, ADO_PROXY_PROJECT_ID,
            ADO_PROXY_REPOSITORY, ADO_PROXY_REPOSITORY_ID
        ],
        fragments: [],
        body: r###"
# Policy document. Non-secret, so it is mounted rather than streamed.
# Scope is substituted here rather than at compile time so the same
# compiled pipeline can be queued against a different project.
mkdir -p "$PROXY_DIR/policy"
printf '%s\n' "$POLICY" > "$PROXY_DIR/policy/policy.json"
sed -i \
  -e "s|\${ADO_PROXY_ORGANIZATION}|$ADO_PROXY_ORGANIZATION|g" \
  -e "s|\${ADO_PROXY_PROJECT}|$ADO_PROXY_PROJECT|g" \
  -e "s|\${ADO_PROXY_PROJECT_ID}|$ADO_PROXY_PROJECT_ID|g" \
  -e "s|\${ADO_PROXY_REPOSITORY}|$ADO_PROXY_REPOSITORY|g" \
  -e "s|\${ADO_PROXY_REPOSITORY_ID}|$ADO_PROXY_REPOSITORY_ID|g" \
  "$PROXY_DIR/policy/policy.json"

# A surviving placeholder would be read as a literal organization or
# repository name, matching nothing — a total denial that reads as a
# policy decision rather than a bug.
if grep -q 'ADO_PROXY_' "$PROXY_DIR/policy/policy.json"; then
  echo "##vso[task.complete result=Failed]ado-proxy policy still contains an unsubstituted placeholder"
  exit 1
fi
echo "ado-proxy policy:"
python3 -m json.tool < "$PROXY_DIR/policy/policy.json"
"###,
    }
}

shell_script! {
    /// Phase 4: mint the interception CA plus one leaf per catalogued
    /// protected host, then publish the CA path via `task.setvariable` for
    /// clients (the `az` wrapper, the ADO MCP mount). The matching private
    /// key never leaves `$PROXY_DIR` and is destroyed in
    /// [`START_ADO_PROXY_DESTROY_PRIVATE`].
    ///
    /// `keyUsage=critical,keyCertSign,cRLSign` is load-bearing: OpenSSL 3
    /// (as used by Python `requests`) rejects `pathlen` without an explicit
    /// `keyCertSign`, so a lax CA would fail strict verifiers.
    START_ADO_PROXY_MINT_MATERIAL {
        interpreter: Bash,
        bindings: [],
        externals: [PROXY_DIR, LEAF_HOSTS, AZ_WRAPPER_DIR, CA_HOST_PATH],
        fragments: [],
        body: r###"
# Interception certificate authority and one leaf per protected host.
openssl req -x509 -newkey rsa:2048 -nodes -days 2 \
  -subj "/CN=ado-aw ado-proxy interception CA" \
  -keyout "$PROXY_DIR/ca.key" -out "$PROXY_DIR/ca.pem" \
  -addext "basicConstraints=critical,CA:TRUE,pathlen:0" \
  -addext "keyUsage=critical,keyCertSign,cRLSign" 2>/dev/null
# shellcheck disable=SC2086 # LEAF_HOSTS is Binding::words; unquoted expansion is the documented word-list contract.
for PROXY_HOST in $LEAF_HOSTS; do
  printf 'basicConstraints=CA:FALSE\nkeyUsage=critical,digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\nsubjectAltName=DNS:%s\n' "$PROXY_HOST" > "$PROXY_DIR/leaf.ext"
  openssl req -new -newkey rsa:2048 -nodes -subj "/CN=$PROXY_HOST" \
    -keyout "$PROXY_DIR/$PROXY_HOST.key" -out "$PROXY_DIR/$PROXY_HOST.csr" 2>/dev/null
  openssl x509 -req -in "$PROXY_DIR/$PROXY_HOST.csr" \
    -CA "$PROXY_DIR/ca.pem" -CAkey "$PROXY_DIR/ca.key" -CAcreateserial \
    -days 2 -extfile "$PROXY_DIR/leaf.ext" -out "$PROXY_DIR/$PROXY_HOST.pem" 2>/dev/null
done

# The proxy publishes its own interception CA certificate for clients
# to trust. It goes under /tmp deliberately: AWF mounts /tmp into the
# agent chroot, so this one file is what the az wrapper reads and what
# the MCP container mounts. Publishing once means no client can trust
# a stale copy. The matching private key never leaves $PROXY_DIR and
# is destroyed below.
mkdir -p "$AZ_WRAPPER_DIR"
echo "##vso[task.setvariable variable=ADO_PROXY_CA_FILE]$CA_HOST_PATH"
"###,
    }
}

shell_script! {
    /// Phase 5: assemble the material document with `jq` so a value
    /// containing JSON metacharacters cannot alter the document shape. The
    /// bearer is read from the environment here and immediately base64-
    /// encoded into the JSON string — it never lands in argv, in a file, or
    /// in the container `Env`.
    START_ADO_PROXY_BUILD_MATERIAL {
        interpreter: Bash,
        bindings: [],
        externals: [PROXY_DIR, LEAF_HOSTS, ADO_PROXY_BEARER],
        fragments: [],
        body: r###"
# Build the material document. jq assembles it so that a value
# containing JSON metacharacters cannot alter the document shape.
PROXY_MATERIAL=$(jq -n \
  --arg schema 'ado-aw/ado-proxy-material/v1' \
  --arg ca_cert "$(base64 -w0 < "$PROXY_DIR/ca.pem")" \
  --arg token "$(printf '%s' "$ADO_PROXY_BEARER" | base64 -w0)" \
  '{schema: $schema, ca_cert: $ca_cert, token: $token, leaves: {}}')
# shellcheck disable=SC2086 # LEAF_HOSTS is Binding::words; unquoted expansion is the documented word-list contract.
for PROXY_HOST in $LEAF_HOSTS; do
  PROXY_MATERIAL=$(printf '%s' "$PROXY_MATERIAL" | jq \
    --arg host "$PROXY_HOST" \
    --arg key "$(base64 -w0 < "$PROXY_DIR/$PROXY_HOST.key")" \
    --arg cert "$(base64 -w0 < "$PROXY_DIR/$PROXY_HOST.pem")" \
    '.leaves[$host] = {key: $key, cert: $cert}')
done
"###,
    }
}

shell_script! {
    /// Phase 6: start the proxy container detached, so the container
    /// lifetime belongs to Docker, not to this Bash task's attached STDIO.
    /// Azure Pipelines cleans up inherited child streams between tasks; an
    /// attached `docker run -i ... &` was observed to exit and `--rm` itself
    /// before AWF could attach it.
    ///
    /// A container-local FIFO preserves the stdin-only custody contract:
    /// material is streamed through `docker exec -i`, never written to a
    /// runner path, container layer, argv, or environment.
    START_ADO_PROXY_RUN_CONTAINER {
        interpreter: Bash,
        bindings: [],
        externals: [
            PROXY_CONTAINER, PROXY_NETWORK, PROXY_SCRIPT_PATH,
            PROXY_DIR, PROXY_IMAGE, CONTAINER_ENTRYPOINT, AZ_WRAPPER_DIR
        ],
        fragments: [],
        body: r###"
# Remove any container left behind by an interrupted run.
docker rm -f "$PROXY_CONTAINER" 2>/dev/null || true
mkdir -p /tmp/gh-aw/ado-proxy-logs

# Start detached so the container lifetime belongs to Docker, not to
# this Bash task's attached STDIO. Azure Pipelines cleans up inherited
# child streams between tasks; an attached `docker run -i ... &` was
# observed to exit and `--rm` itself before AWF could attach it.
#
# A container-local FIFO preserves the stdin-only custody contract:
# material is streamed through `docker exec -i`, never written to a
# runner path, container layer, argv, or environment.
docker run -d \
  --name "$PROXY_CONTAINER" \
  --network "$PROXY_NETWORK" \
  --entrypoint sh \
  -v "$PROXY_SCRIPT_PATH:/app/ado-proxy.js:ro" \
  -v "$PROXY_DIR/policy:/etc/ado-proxy:ro" \
  -v "$AZ_WRAPPER_DIR:/var/lib/ado-proxy" \
  -v /tmp/gh-aw/ado-proxy-logs:/var/log/ado-proxy \
  "$PROXY_IMAGE" \
  -c "$CONTAINER_ENTRYPOINT" \
  >/dev/null
"###,
    }
}

shell_script! {
    /// Phase 7: wait for the container to open its private material FIFO,
    /// then stream the assembled material in over `docker exec -i`. A
    /// failed transfer prints durable Docker log + inspect state before
    /// failing the pipeline so the true cause reaches the audit trail.
    START_ADO_PROXY_HANDOVER_MATERIAL {
        interpreter: Bash,
        bindings: [],
        externals: [PROXY_CONTAINER, PROXY_MATERIAL],
        fragments: [],
        body: r###"
# Wait until the detached container is blocked on its private FIFO,
# then hand over the one-shot material. A transfer failure prints the
# durable Docker log and container state before failing the pipeline.
FIFO_READY=false
for _i in $(seq 1 30); do
  if docker exec "$PROXY_CONTAINER" test -p /tmp/ado-proxy-material 2>/dev/null; then
    FIFO_READY=true
    break
  fi
  sleep 1
done
if [ "$FIFO_READY" != "true" ]; then
  echo "##vso[task.logissue type=error]ado-proxy container did not create its private material channel"
  docker inspect -f 'state={{.State.Status}} exit={{.State.ExitCode}} error={{.State.Error}}' "$PROXY_CONTAINER" 2>/dev/null || true
  docker logs --tail 200 "$PROXY_CONTAINER" 2>&1 || true
  exit 1
fi
if ! printf '%s' "$PROXY_MATERIAL" | docker exec -i "$PROXY_CONTAINER" sh -c 'cat > /tmp/ado-proxy-material'; then
  echo "##vso[task.logissue type=error]ado-proxy material handover failed"
  docker inspect -f 'state={{.State.Status}} exit={{.State.ExitCode}} error={{.State.Error}}' "$PROXY_CONTAINER" 2>/dev/null || true
  docker logs --tail 200 "$PROXY_CONTAINER" 2>&1 || true
  exit 1
fi
"###,
    }
}

shell_script! {
    /// Phase 8: destroy the private material as soon as the container has
    /// consumed it. The container holds material in memory only; nothing
    /// else needs it. Ordering matters: this must run before readiness
    /// polling so a hung poll cannot keep the CA private key on disk.
    START_ADO_PROXY_DESTROY_PRIVATE {
        interpreter: Bash,
        bindings: [],
        externals: [PROXY_DIR],
        fragments: [],
        body: r###"
# Drop the private material as soon as it has been handed over. The
# container has it in memory; nothing else needs it again.
#
# Blanking before `unset` is deliberate: `unset` alone removes the name
# binding but a shell is free to leave the value in the freed slot, and this
# value is the ADO bearer plus the CA private key. Assigning "" overwrites it
# first.
# shellcheck disable=SC2034  # write-only by design; the assignment *is* the erasure
PROXY_MATERIAL=""
unset PROXY_MATERIAL
shred -u "$PROXY_DIR/ca.key" "$PROXY_DIR"/*.key 2>/dev/null || rm -f "$PROXY_DIR/ca.key" "$PROXY_DIR"/*.key
"###,
    }
}

shell_script! {
    /// Phase 9: resolve the container IP after the engine has parsed
    /// policy, published its public CA, and reached its listening state.
    /// Publish the IP so downstream MCPG config substitution can redirect
    /// clients at the engine.
    START_ADO_PROXY_WAIT_READY {
        interpreter: Bash,
        bindings: [],
        externals: [PROXY_CONTAINER, CA_HOST_PATH],
        fragments: [],
        body: r###"
# Resolve the container IP only after the engine has parsed policy,
# published its public CA and reached its listening state.
PROXY_READY=false
PROXY_STATE=""
ADO_PROXY_IP=""
for _i in $(seq 1 30); do
  PROXY_STATE=$(docker inspect -f '{{.State.Status}}' "$PROXY_CONTAINER" 2>/dev/null || true)
  if [ "$PROXY_STATE" = "exited" ] || [ "$PROXY_STATE" = "dead" ]; then
    break
  fi
  ADO_PROXY_IP=$(docker inspect -f '{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}' "$PROXY_CONTAINER" 2>/dev/null || true)
  if [ -n "$ADO_PROXY_IP" ] \
     && [ -f "$CA_HOST_PATH" ] \
     && docker logs "$PROXY_CONTAINER" 2>&1 | grep -q '\[ado-proxy\] listening'; then
    PROXY_READY=true
    break
  fi
  sleep 1
done
if [ "$PROXY_READY" != "true" ]; then
  echo "##vso[task.logissue type=error]ado-proxy did not become ready within 30s (state=${PROXY_STATE:-missing})"
  docker inspect -f 'state={{.State.Status}} exit={{.State.ExitCode}} error={{.State.Error}}' "$PROXY_CONTAINER" 2>/dev/null || true
  docker logs --tail 200 "$PROXY_CONTAINER" 2>&1 || true
  exit 1
fi
echo "ado-proxy is ready at $ADO_PROXY_IP"
docker logs --tail 1 "$PROXY_CONTAINER" 2>&1 || true
echo "##vso[task.setvariable variable=ADO_PROXY_IP]$ADO_PROXY_IP"
"###,
    }
}

shell_script! {
    /// The container's `sh -c` entrypoint. Registered as its own `Sh`
    /// script so shellcheck lints the multi-line source; the actual
    /// `docker run` invocation embeds the semicolon-flattened form
    /// returned by [`ado_proxy_container_entrypoint_flattened`], which
    /// `container_entrypoint_matches_registered_body` keeps in sync.
    ADO_PROXY_CONTAINER_ENTRYPOINT_SH {
        interpreter: Sh,
        bindings: [],
        externals: [],
        fragments: [],
        body: r###"
set -eu
umask 077
MATERIAL_FIFO=/tmp/ado-proxy-material
mkfifo "$MATERIAL_FIFO"
exec node /app/ado-proxy.js \
  --policy-file /etc/ado-proxy/policy.json \
  --public-ca-file /var/lib/ado-proxy/ado-proxy-ca.pem \
  --upstream-proxy http://172.30.0.10:3128 \
  --listen-port 11080 \
  --tls-port 443 \
  --log-dir /var/log/ado-proxy < "$MATERIAL_FIFO"
"###,
    }
}

shell_script! {
    /// The **atomic** `start_ado_proxy` step: composed from
    /// independently-registered phases, still emitted as a single trusted
    /// Bash task so the credential-custody contract holds. See
    /// [`start_ado_proxy_step`] for the phase catalogue and issue #1833 for
    /// the design.
    START_ADO_PROXY {
        interpreter: Bash,
        bindings: [
            PROXY_CONTAINER, PROXY_IMAGE, PROXY_NETWORK,
            PROXY_SCRIPT_PATH, AZ_WRAPPER_DIR, CA_HOST_PATH,
            AGENT_TEMP,
            ADO_PROXY_PROJECT, ADO_PROXY_PROJECT_ID,
            ADO_PROXY_REPOSITORY, ADO_PROXY_REPOSITORY_ID,
            LEAF_HOSTS, POLICY, CONTAINER_ENTRYPOINT
        ],
        externals: [],
        fragments: [
            setup_workdir,
            resolve_org,
            write_policy,
            mint_material,
            build_material,
            run_container,
            handover_material,
            destroy_private,
            wait_ready
        ],
        // Every phase but `resolve_org` is a registered script, so the lint
        // shellchecks the *composed* body rather than an outline of markers.
        // That is what catches a variable one phase sets and another reads —
        // the one new risk splitting a script into phases introduces.
        // `resolve_org` is supplied at runtime by `common`, so it keeps its
        // marker and is linted where it is produced.
        phases: [
            setup_workdir = START_ADO_PROXY_SETUP_WORKDIR,
            write_policy = START_ADO_PROXY_WRITE_POLICY,
            mint_material = START_ADO_PROXY_MINT_MATERIAL,
            build_material = START_ADO_PROXY_BUILD_MATERIAL,
            run_container = START_ADO_PROXY_RUN_CONTAINER,
            handover_material = START_ADO_PROXY_HANDOVER_MATERIAL,
            destroy_private = START_ADO_PROXY_DESTROY_PRIVATE,
            wait_ready = START_ADO_PROXY_WAIT_READY,
        ],
        body: r###"
# Start the ado-proxy policy engine.
#
# The agent never receives an Azure DevOps credential. This container
# holds it, and serves only the operations in the versioned catalog.

# ado-aw:fragment setup_workdir

# ado-aw:fragment resolve_org

# ado-aw:fragment write_policy

# ado-aw:fragment mint_material

# ado-aw:fragment build_material

# ado-aw:fragment run_container

# ado-aw:fragment handover_material

# ado-aw:fragment destroy_private

# ado-aw:fragment wait_ready
"###,
    }
}

/// Stop the `ado-proxy` container.
///
/// `--rm` only fires on a clean exit, so an OOM or SIGKILL would otherwise
/// leave the container — and the credential it holds in memory — running past
/// the job.
///
/// Not yet emitted: the Agent job gains these steps in `proxy-topology-attach`,
/// once AWF is also told to attach the container. Landing the lifecycle first
/// keeps that change to the wiring alone.
fn stop_ado_proxy_step() -> BashStep {
    ShellScript::new(&STOP_ADO_PROXY)
        .bind_text("PROXY_CONTAINER", ADO_PROXY_CONTAINER_NAME)
        .into_step("Stop ado-proxy")
        .with_condition(Condition::Always)
}

shell_script! {
    /// Stop the ado-proxy container, preserving auditable lifecycle output
    /// (`docker inspect` state + `docker logs` tail) even when the
    /// container disappeared between the start step and here — that
    /// scenario is precisely the one worth debugging, so a missing
    /// container publishes an explicit warning rather than a silent skip.
    STOP_ADO_PROXY {
        interpreter: Bash,
        bindings: [PROXY_CONTAINER],
        externals: [],
        fragments: [],
        body: r###"
# Preserve auditable lifecycle output before stopping the policy engine
mkdir -p /tmp/gh-aw/ado-proxy-logs
if docker inspect "$PROXY_CONTAINER" >/dev/null 2>&1; then
  docker inspect -f 'state={{.State.Status}} exit={{.State.ExitCode}} error={{.State.Error}}' "$PROXY_CONTAINER" \
    > /tmp/gh-aw/ado-proxy-logs/container-state.txt 2>&1 || true
  docker logs "$PROXY_CONTAINER" \
    > /tmp/gh-aw/ado-proxy-logs/container.log 2>&1 || true
else
  echo 'state=missing before teardown' > /tmp/gh-aw/ado-proxy-logs/container-state.txt
  echo "##vso[task.logissue type=warning]ado-proxy container was already missing at teardown; inspect the preflight/AWF step and ado-proxy log artifact"
fi

# Stop the ado-proxy policy engine
echo "Stopping ado-proxy..."
docker stop "$PROXY_CONTAINER" 2>/dev/null || true
docker rm -f "$PROXY_CONTAINER" 2>/dev/null || true
echo "ado-proxy stopped"
"###,
    }
}

/// Verify externally-launched peers still exist immediately before AWF tries
/// to attach them to its internal network.
///
/// A peer may start successfully and disappear between pipeline tasks if its
/// lifecycle accidentally remains attached to the launching task's STDIO.
/// Reporting the peer's Docker state and logs here turns AWF's otherwise opaque
/// "No such container" error into an actionable startup/lifecycle failure.
fn verify_trusted_topology_peers_step() -> BashStep {
    ShellScript::new(&VERIFY_TRUSTED_TOPOLOGY_PEERS)
        .bind_text("MCPG_CONTAINER", MCPG_CONTAINER_NAME)
        .bind_text("PROXY_CONTAINER", ADO_PROXY_CONTAINER_NAME)
        .bind_text("CA_HOST_PATH", ADO_PROXY_PUBLIC_CA_HOST_PATH)
        .into_step("Verify trusted topology peers")
}

shell_script! {
    /// Verify externally-launched trusted peers (MCPG, the ado-proxy) are
    /// still running immediately before AWF attaches them to its internal
    /// network. Turns AWF's opaque "No such container" into an actionable
    /// startup / lifecycle failure with `docker ps -a` and `docker logs`
    /// tails; also asserts the ado-proxy public CA is readable, since a
    /// too-restrictive container umask leaves it owner-only and clients
    /// then can't verify the intercepted chain.
    VERIFY_TRUSTED_TOPOLOGY_PEERS {
        interpreter: Bash,
        bindings: [MCPG_CONTAINER, PROXY_CONTAINER, CA_HOST_PATH],
        externals: [],
        fragments: [],
        body: r###"
set -euo pipefail
mkdir -p /tmp/gh-aw/ado-proxy-logs
for PEER in "$MCPG_CONTAINER" "$PROXY_CONTAINER"; do
  PEER_STATE=$(docker inspect -f '{{.State.Status}}' "$PEER" 2>/dev/null || true)
  if [ "$PEER_STATE" != "running" ]; then
    echo "##vso[task.logissue type=error]trusted topology peer $PEER is not running before AWF attachment (state=${PEER_STATE:-missing})"
    docker ps -a --filter "name=^/${PEER}$" --no-trunc || true
    if [ "$PEER" = "$PROXY_CONTAINER" ]; then
      docker logs --tail 200 "$PEER" 2>&1 \
        | tee /tmp/gh-aw/ado-proxy-logs/container.log || true
    else
      docker logs --tail 200 "$PEER" 2>&1 || true
    fi
    exit 1
  fi
  echo "Trusted topology peer $PEER is running"
done
if [ ! -r "$CA_HOST_PATH" ]; then
  echo "##vso[task.logissue type=error]ado-proxy public CA is not readable by the runner/agent identity: $CA_HOST_PATH"
  ls -l "$CA_HOST_PATH" 2>&1 || true
  echo "The proxy publishes this intentionally public certificate for the wrapped az process and Azure DevOps MCP. A restrictive container umask must not leave it owner-only."
  docker logs --tail 200 "$PROXY_CONTAINER" 2>&1 || true
  exit 1
fi
CA_MODE=$(stat -c '%a' "$CA_HOST_PATH" 2>/dev/null || echo unknown)
echo "ado-proxy public CA is readable (mode=$CA_MODE)"
echo "ado-proxy policy and client configuration are ready; runtime denials will include the policy reason and sanitized decision logs"
"###,
    }
}

shell_script! {
    /// Report the overall pipeline conclusion. Best-effort: falls back to a
    /// warning when `conclusion.js` (delivered by the ado-script extension)
    /// is unavailable, since the Conclusion job is contractually always
    /// runs / never fails.
    REPORT_CONCLUSION {
        interpreter: Bash,
        bindings: [CONCLUSION_PATH],
        externals: [],
        fragments: [],
        body: r###"
if command -v node >/dev/null 2>&1 && [ -f "$CONCLUSION_PATH" ]; then
  node "$CONCLUSION_PATH"
else
  echo "##vso[task.logissue type=warning]conclusion.js unavailable; skipping conclusion reporting"
fi
"###,
    }
}

shell_script! {
    /// Copy every engine + ado-aw log to the Detection job's analyzed-outputs
    /// artifact directory (each source landing in its own subdirectory).
    /// The `engine_log_dir` fragment assigns `ENGINE_LOG_DIR` from a
    /// compile-time-constant literal so a shell like `$HOME` re-expands.
    COPY_LOGS_DETECTION {
        interpreter: Bash,
        bindings: [AGENT_TEMP],
        externals: [ADO_AW_LOG_DIR, ENGINE_LOG_DIR, HOME],
        fragments: [engine_log_dir],
        body: r###"
# Copy all logs to analyzed outputs for artifact upload
mkdir -p "$AGENT_TEMP/analyzed_outputs/logs"
# ado-aw:fragment engine_log_dir
if [ -d "$ENGINE_LOG_DIR" ]; then
  mkdir -p "$AGENT_TEMP/analyzed_outputs/logs/copilot"
  cp -r "$ENGINE_LOG_DIR"/* "$AGENT_TEMP/analyzed_outputs/logs/copilot/" 2>/dev/null || true
fi
ADO_AW_LOG_DIR="${ADO_AW_LOG_DIR:-$HOME/.ado-aw/logs}"
if [ -d "$ADO_AW_LOG_DIR" ]; then
  mkdir -p "$AGENT_TEMP/analyzed_outputs/logs/ado-aw"
  cp -r "$ADO_AW_LOG_DIR"/* "$AGENT_TEMP/analyzed_outputs/logs/ado-aw/" 2>/dev/null || true
fi
echo "Logs copied to $AGENT_TEMP/analyzed_outputs/logs"
ls -laR "$AGENT_TEMP/analyzed_outputs/logs" 2>/dev/null || echo "No logs found"
"###,
    }
}

shell_script! {
    /// Copy every engine + ado-aw + MCPG + ado-proxy log to the Agent job's
    /// staging/logs directory (each source landing in its own subdirectory
    /// when it exists at runtime).
    COPY_LOGS_AGENT {
        interpreter: Bash,
        bindings: [AGENT_TEMP],
        externals: [ADO_AW_LOG_DIR, ENGINE_LOG_DIR, HOME],
        fragments: [engine_log_dir],
        body: r###"
# Copy all logs to output directory for artifact upload
mkdir -p "$AGENT_TEMP/staging/logs"
# ado-aw:fragment engine_log_dir
if [ -d "$ENGINE_LOG_DIR" ]; then
  cp -r "$ENGINE_LOG_DIR"/* "$AGENT_TEMP/staging/logs/" 2>/dev/null || true
fi
ADO_AW_LOG_DIR="${ADO_AW_LOG_DIR:-$HOME/.ado-aw/logs}"
if [ -d "$ADO_AW_LOG_DIR" ]; then
  cp -r "$ADO_AW_LOG_DIR"/* "$AGENT_TEMP/staging/logs/" 2>/dev/null || true
fi
if [ -d /tmp/gh-aw/mcp-logs ]; then
  mkdir -p "$AGENT_TEMP/staging/logs/mcpg"
  cp -r /tmp/gh-aw/mcp-logs/* "$AGENT_TEMP/staging/logs/mcpg/" 2>/dev/null || true
fi
if [ -d /tmp/gh-aw/ado-proxy-logs ]; then
  mkdir -p "$AGENT_TEMP/staging/logs/ado-proxy"
  cp -r /tmp/gh-aw/ado-proxy-logs/* "$AGENT_TEMP/staging/logs/ado-proxy/" 2>/dev/null || true
fi
echo "Logs copied to $AGENT_TEMP/staging/logs"
ls -la "$AGENT_TEMP/staging/logs" 2>/dev/null || echo "No logs found"
"###,
    }
}

fn copy_logs_step(engine_log_dir: &str, is_detection: bool) -> BashStep {
    // Fragment content assigns ENGINE_LOG_DIR from a double-quoted literal so
    // that a shell variable such as `$HOME` re-expands at runtime — a
    // `Binding::text` value is single-quoted and would leave `$HOME` literal.
    // The value is a compiler-controlled constant (`Engine::log_dir`), never
    // a runtime input, so quoting it verbatim carries no injection risk.
    let engine_log_dir_fragment = format!("ENGINE_LOG_DIR=\"{engine_log_dir}\"");
    if is_detection {
        return ShellScript::new(&COPY_LOGS_DETECTION)
            .bind("AGENT_TEMP", Binding::ado_macro("Agent.TempDirectory"))
            .fragment("engine_log_dir", engine_log_dir_fragment)
            .into_step("Copy logs to output directory")
            .with_condition(Condition::Always);
    }
    ShellScript::new(&COPY_LOGS_AGENT)
        .bind("AGENT_TEMP", Binding::ado_macro("Agent.TempDirectory"))
        .fragment("engine_log_dir", engine_log_dir_fragment)
        .into_step("Copy logs to output directory")
        .with_condition(Condition::Always)
}

shell_script! {
    /// Copy the SafeOutputs job's own logs to `staging/logs/`, plus the
    /// Agent job's `agent-output.txt` and the executed-outputs NDJSON so the
    /// Conclusion job can read diagnostic signals from the SafeOutputs
    /// artifact.
    COPY_LOGS_SAFEOUTPUTS {
        interpreter: Bash,
        bindings: [AGENT_TEMP, PIPELINE_WORKSPACE, BUILD_ID],
        externals: [ADO_AW_LOG_DIR, ENGINE_LOG_DIR, HOME],
        fragments: [engine_log_dir],
        body: r###"
# Copy all logs to output directory for artifact upload
mkdir -p "$AGENT_TEMP/staging/logs"
# Copy agent output log from analyzed_outputs for optimisation use
cp "$PIPELINE_WORKSPACE/analyzed_outputs_$BUILD_ID/logs/agent-output.txt" \
  "$AGENT_TEMP/staging/logs/agent-output.txt" 2>/dev/null || true
# Copy executed NDJSON manifest so the Conclusion job can read diagnostic signals
cp "$PIPELINE_WORKSPACE/analyzed_outputs_$BUILD_ID/safe-outputs-executed.ndjson" \
  "$AGENT_TEMP/staging/safe-outputs-executed.ndjson" 2>/dev/null || true
# ado-aw:fragment engine_log_dir
if [ -d "$ENGINE_LOG_DIR" ]; then
  mkdir -p "$AGENT_TEMP/staging/logs/copilot"
  cp -r "$ENGINE_LOG_DIR"/* "$AGENT_TEMP/staging/logs/copilot/" 2>/dev/null || true
fi
ADO_AW_LOG_DIR="${ADO_AW_LOG_DIR:-$HOME/.ado-aw/logs}"
if [ -d "$ADO_AW_LOG_DIR" ]; then
  mkdir -p "$AGENT_TEMP/staging/logs/ado-aw"
  cp -r "$ADO_AW_LOG_DIR"/* "$AGENT_TEMP/staging/logs/ado-aw/" 2>/dev/null || true
fi
echo "Logs copied to $AGENT_TEMP/staging/logs"
ls -laR "$AGENT_TEMP/staging/logs" 2>/dev/null || echo "No logs found"
"###,
    }
}

fn copy_logs_safeoutputs_step(engine_log_dir: &str) -> BashStep {
    let engine_log_dir_fragment = format!("ENGINE_LOG_DIR=\"{engine_log_dir}\"");
    ShellScript::new(&COPY_LOGS_SAFEOUTPUTS)
        .bind("AGENT_TEMP", Binding::ado_macro("Agent.TempDirectory"))
        .bind(
            "PIPELINE_WORKSPACE",
            Binding::ado_macro("Pipeline.Workspace"),
        )
        .bind("BUILD_ID", Binding::ado_macro("Build.BuildId"))
        .fragment("engine_log_dir", engine_log_dir_fragment)
        .into_step("Copy logs to output directory")
        .with_condition(Condition::Always)
}

shell_script! {
    /// Copy the Agent job's proposed safe outputs into the Detection job's
    /// working directory for analysis.
    PREPARE_SAFE_OUTPUTS_FOR_ANALYSIS {
        interpreter: Bash,
        bindings: [PIPELINE_WORKSPACE, BUILD_ID],
        externals: [WORKING_DIRECTORY],
        fragments: [],
        body: r#"
mkdir -p "$WORKING_DIRECTORY/safe_outputs"
cp -a "$PIPELINE_WORKSPACE/agent_outputs_$BUILD_ID/." "$WORKING_DIRECTORY/safe_outputs"
"#,
    }
}

fn prepare_safe_outputs_for_analysis(working_directory: &str) -> BashStep {
    use super::ir::env::EnvValue;
    ShellScript::new(&PREPARE_SAFE_OUTPUTS_FOR_ANALYSIS)
        .bind(
            "PIPELINE_WORKSPACE",
            Binding::ado_macro("Pipeline.Workspace"),
        )
        .bind("BUILD_ID", Binding::ado_macro("Build.BuildId"))
        .into_step("Prepare safe outputs for analysis")
        .with_env("WORKING_DIRECTORY", EnvValue::literal(working_directory))
}

shell_script! {
    /// Write the threat-analysis prompt to
    /// `/tmp/awf-tools/threat-analysis-prompt.md`. The `heredoc` fragment
    /// carries a per-content SHA-derived sentinel — the same mitigation
    /// used in [`PREPARE_AGENT_PROMPT`] — so a malicious front-matter
    /// `description:` (which lands inside this prompt body) cannot
    /// terminate the heredoc early and inject commands into the Detection
    /// job.
    PREPARE_THREAT_ANALYSIS_PROMPT {
        interpreter: Bash,
        bindings: [],
        externals: [],
        fragments: [heredoc],
        body: r###"
# Write threat analysis prompt to /tmp (accessible inside AWF container)
# ado-aw:fragment heredoc

echo "Threat analysis prompt:"
cat "/tmp/awf-tools/threat-analysis-prompt.md"
"###,
    }
}

fn prepare_threat_analysis_prompt_step(threat_prompt: &str) -> Result<BashStep> {
    let sentinel = super::common::heredoc_sentinel("THREAT_ANALYSIS_EOF", threat_prompt)?;
    let heredoc = format!(
        "cat > \"/tmp/awf-tools/threat-analysis-prompt.md\" << '{sentinel}'\n\
         {threat_prompt}\n\
         {sentinel}"
    );
    Ok(ShellScript::new(&PREPARE_THREAT_ANALYSIS_PROMPT)
        .fragment("heredoc", heredoc)
        .into_step("Prepare threat analysis prompt"))
}

shell_script! {
    /// Ensure the downloaded compiler is executable at the well-known path.
    SETUP_COMPILER {
        interpreter: Bash,
        bindings: [],
        externals: [],
        fragments: [],
        body: r###"
AGENTIC_PIPELINES_PATH="$(Pipeline.Workspace)/agentic-pipeline-compiler/ado-aw"
chmod +x "$AGENTIC_PIPELINES_PATH"
"###,
    }
}

fn setup_compiler_step() -> BashStep {
    ShellScript::new(&SETUP_COMPILER).into_step("Setup agentic pipeline compiler")
}

shell_script! {
    /// Invoke the Detection stage's threat-analysis agent inside AWF's
    /// network-isolated Docker topology. Structured like [`RUN_AGENT`] but
    /// without a topology-attach block (Detection has no MCPG/ado-proxy
    /// peers) and with the single-quoted engine command passed through
    /// verbatim.
    RUN_THREAT_ANALYSIS {
        interpreter: Bash,
        bindings: [AGENT_TEMP, PIPELINE_WORKSPACE, ALLOWED_DOMAINS],
        externals: [WORKING_DIRECTORY],
        fragments: [image_flags, exclude_env, engine_run_detection],
        body: r###"
set -o pipefail

# Run threat analysis with AWF network isolation
THREAT_OUTPUT_FILE="$AGENT_TEMP/threat-analysis-output.txt"
AGENT_EXIT_CODE=0

# The argument list is assembled into an array so runtime-supplied
# fragments splice in as ordinary shell statements (`AWF_ARGS+=(...)`)
# — no `\`-continuation chain to break with fragment marker comments.
AWF_ARGS=(
  --allow-domains "$ALLOWED_DOMAINS"
  --network-isolation
)
# ado-aw:fragment image_flags
AWF_ARGS+=(--skip-pull --env-all)
# ado-aw:fragment exclude_env
AWF_ARGS+=(
  --container-workdir "$WORKING_DIRECTORY"
  --log-level info
  --proxy-logs-dir "$AGENT_TEMP/threat-analysis-logs/firewall"
)
# ado-aw:fragment engine_run_detection

# Stream threat analysis output in real-time with VSO command filtering
# shellcheck disable=SC2016 # The single-quoted engine command inside AWF_ARGS is intentionally expanded by AWF inside the sandbox
"$PIPELINE_WORKSPACE/awf/awf" "${AWF_ARGS[@]}" 2>&1 \
  | sed -u 's/##vso\[/[VSO-FILTERED] vso[/g; s/##\[/[VSO-FILTERED] [/g' \
  | tee "$THREAT_OUTPUT_FILE" \
  || AGENT_EXIT_CODE=$?

exit "$AGENT_EXIT_CODE"
"###,
    }
}

fn run_threat_analysis_step(
    allowed_domains: &str,
    working_directory: &str,
    engine_run_detection: &str,
    byom_exclude_keys: &[String],
    detection_engine_env: &[(String, String)],
    github_token_var: &str,
    supply_chain: Option<&SupplyChainConfig>,
) -> Result<BashStep> {
    let image_flags_block = awf_image_flags(supply_chain);
    let exclude_env_block = awf_exclude_env_flags(byom_exclude_keys);
    let image_flags_line = {
        let mut parts: Vec<String> = Vec::new();
        for line in image_flags_block.lines() {
            let line = line.trim();
            let line = line.strip_suffix('\\').unwrap_or(line).trim_end();
            if line.is_empty() {
                continue;
            }
            parts.push(line.to_string());
        }
        format!("AWF_ARGS+=({})", parts.join(" "))
    };
    let exclude_env_line = {
        let mut parts: Vec<String> = Vec::new();
        for line in exclude_env_block.lines() {
            let line = line.trim();
            let line = line.strip_suffix('\\').unwrap_or(line).trim_end();
            if line.is_empty() {
                continue;
            }
            parts.push(line.to_string());
        }
        if parts.is_empty() {
            String::new()
        } else {
            format!("AWF_ARGS+=({})", parts.join(" "))
        }
    };
    let engine_run_detection_line = format!("AWF_ARGS+=(-- '{engine_run_detection}')");

    let mut step = ShellScript::new(&RUN_THREAT_ANALYSIS)
        .bind("AGENT_TEMP", Binding::ado_macro("Agent.TempDirectory"))
        .bind(
            "PIPELINE_WORKSPACE",
            Binding::ado_macro("Pipeline.Workspace"),
        )
        .bind_text("ALLOWED_DOMAINS", allowed_domains)
        .fragment("image_flags", image_flags_line)
        .fragment("exclude_env", exclude_env_line)
        .fragment("engine_run_detection", engine_run_detection_line)
        .into_step("Run threat analysis (AWF network isolated)");
    step.working_directory = Some(working_directory.to_string());
    // env block: GITHUB_TOKEN + GITHUB_READ_ONLY — emit the latter as
    // a typed YAML integer so it round-trips unquoted (matching the
    // legacy copilot_env output of `GITHUB_READ_ONLY: 1`, not `'1'`).
    // WORKING_DIRECTORY is passed via env: so ADO substitutes any `$(...)`
    // macros in the value before bash sees it.
    use super::ir::env::EnvValue;
    step = step
        .with_env("WORKING_DIRECTORY", EnvValue::literal(working_directory))
        .with_env("GITHUB_TOKEN", EnvValue::pipeline_var(github_token_var))
        .with_env(
            "GITHUB_READ_ONLY",
            EnvValue::RawYamlScalar(serde_yaml::Value::Number(1.into())),
        );
    // Apply validated Detection engine env directly (macro → PipelineVar, else
    // Literal) without a YAML render/reparse round-trip.
    for (k, raw) in detection_engine_env {
        step = step.with_env(k.clone(), env_value_from_str(raw));
    }
    Ok(step)
}

shell_script! {
    /// Detection job: copy the original Agent proposal payload into
    /// `analyzed_outputs/`, then extract the JSON verdict from the
    /// `THREAT_DETECTION_RESULT:` line printed by the threat-analysis
    /// engine run.
    PREPARE_ANALYZED_OUTPUTS {
        interpreter: Bash,
        bindings: [AGENT_TEMP, PIPELINE_WORKSPACE, BUILD_ID],
        externals: [],
        fragments: [],
        body: r###"
# Create analyzed outputs directory with original safe outputs and analysis
mkdir -p "$AGENT_TEMP/analyzed_outputs"

# Copy original safe outputs
cp -a "$PIPELINE_WORKSPACE/agent_outputs_$BUILD_ID/." "$AGENT_TEMP/analyzed_outputs/"

# Copy threat analysis output
if [ -f "$AGENT_TEMP/threat-analysis-output.txt" ]; then
  cp "$AGENT_TEMP/threat-analysis-output.txt" "$AGENT_TEMP/analyzed_outputs/"
fi

# Extract JSON from THREAT_DETECTION_RESULT line in threat analysis output
if [ -f "$AGENT_TEMP/threat-analysis-output.txt" ]; then
  RESULT_LINE=$(grep "THREAT_DETECTION_RESULT:" "$AGENT_TEMP/threat-analysis-output.txt" | tail -1)
  if [ -n "$RESULT_LINE" ]; then
    # Extract JSON after the prefix
    JSON_CONTENT="${RESULT_LINE##*THREAT_DETECTION_RESULT:}"
    echo "$JSON_CONTENT" > "$AGENT_TEMP/analyzed_outputs/threat-analysis.json"
    echo "Extracted threat analysis JSON:"
    cat "$AGENT_TEMP/analyzed_outputs/threat-analysis.json"
  else
    echo "Warning: No THREAT_DETECTION_RESULT found in threat analysis output"
  fi
else
  echo "Warning: No threat analysis output file found"
fi

echo "Analyzed outputs directory contents:"
ls -laR "$AGENT_TEMP/analyzed_outputs"
"###,
    }
}

fn prepare_analyzed_outputs_step() -> BashStep {
    ShellScript::new(&PREPARE_ANALYZED_OUTPUTS)
        .bind("AGENT_TEMP", Binding::ado_macro("Agent.TempDirectory"))
        .bind(
            "PIPELINE_WORKSPACE",
            Binding::ado_macro("Pipeline.Workspace"),
        )
        .bind("BUILD_ID", Binding::ado_macro("Build.BuildId"))
        .into_step("Prepare analyzed outputs")
        .with_condition(Condition::Always)
}

shell_script! {
    /// Detection job (AI threat detection disabled): copy Agent proposals to
    /// `analyzed_outputs/` unchanged. The Detection stage still runs as a
    /// pipeline boundary even when analysis is skipped.
    PREPARE_ANALYZED_OUTPUTS_PASSTHROUGH {
        interpreter: Bash,
        bindings: [AGENT_TEMP, PIPELINE_WORKSPACE, BUILD_ID],
        externals: [],
        fragments: [],
        body: r###"
set -eo pipefail
mkdir -p "$AGENT_TEMP/analyzed_outputs"
cp -a "$PIPELINE_WORKSPACE/agent_outputs_$BUILD_ID/." "$AGENT_TEMP/analyzed_outputs/"
echo "AI threat detection is disabled; copied Agent outputs unchanged."
"###,
    }
}

fn prepare_analyzed_outputs_passthrough_step() -> BashStep {
    ShellScript::new(&PREPARE_ANALYZED_OUTPUTS_PASSTHROUGH)
        .bind("AGENT_TEMP", Binding::ado_macro("Agent.TempDirectory"))
        .bind(
            "PIPELINE_WORKSPACE",
            Binding::ado_macro("Pipeline.Workspace"),
        )
        .bind("BUILD_ID", Binding::ado_macro("Build.BuildId"))
        .into_step("Prepare analyzed outputs (detection disabled)")
}

shell_script! {
    /// Detection-disabled short-circuit: publish `SafeToProcess=true` so
    /// downstream jobs consuming this output variable behave as if analysis
    /// had run and passed.
    THREAT_ANALYSIS_DISABLED {
        interpreter: Bash,
        bindings: [],
        externals: [],
        fragments: [],
        body: r###"
echo "AI threat detection was explicitly disabled by workflow configuration."
echo "##vso[task.setvariable variable=SafeToProcess;isOutput=true]true"
echo "SafeToProcess set to: true"
"###,
    }
}

fn threat_analysis_disabled_step() -> BashStep {
    ShellScript::new(&THREAT_ANALYSIS_DISABLED)
        .into_step("Bypass AI threat analysis")
        .with_id(
            StepId::new("threatAnalysis")
                .expect("threatAnalysis is a valid StepId — see StepId::new contract"),
        )
        .with_output(OutputDecl::new("SafeToProcess"))
}

shell_script! {
    /// Detection stage: read the JSON verdict extracted by
    /// [`PREPARE_ANALYZED_OUTPUTS`] and publish a `SafeToProcess` output
    /// variable driving the SafeOutputs job's `condition:`. Defaults to
    /// `false` (unsafe) on any parse/read failure so the pipeline
    /// fails safe.
    EVALUATE_THREAT_ANALYSIS {
        interpreter: Bash,
        bindings: [AGENT_TEMP],
        externals: [],
        fragments: [],
        body: r###"
SAFE_TO_PROCESS="false"
JSON_FILE="$AGENT_TEMP/analyzed_outputs/threat-analysis.json"

if [ -f "$JSON_FILE" ]; then
  if jq -e . "$JSON_FILE" > /dev/null 2>&1; then
    echo "JSON is valid"

    # Check if any threat field is true
    if jq -e '.prompt_injection or .secret_leak or .malicious_patch' "$JSON_FILE" > /dev/null 2>&1; then
      echo "##vso[task.logissue type=warning]Threats detected - safe outputs will NOT be processed"
      jq -r '.reasons[]? // empty' "$JSON_FILE" | sed 's/^/  - /'
    else
      echo "No threats detected - safe outputs will be processed"
      SAFE_TO_PROCESS="true"
    fi
  else
    echo "##vso[task.logissue type=warning]Invalid JSON in threat analysis - defaulting to unsafe"
  fi
else
  echo "##vso[task.logissue type=warning]No threat analysis JSON found - defaulting to unsafe"
fi

echo "##vso[task.setvariable variable=SafeToProcess;isOutput=true]$SAFE_TO_PROCESS"
echo "SafeToProcess set to: $SAFE_TO_PROCESS"
"###,
    }
}

fn evaluate_threat_analysis_step() -> BashStep {
    ShellScript::new(&EVALUATE_THREAT_ANALYSIS)
        .bind("AGENT_TEMP", Binding::ado_macro("Agent.TempDirectory"))
        .into_step("Evaluate threat analysis")
        .with_id(
            StepId::new("threatAnalysis")
                .expect("threatAnalysis is a valid StepId — see StepId::new contract"),
        )
        .with_output(OutputDecl::new("SafeToProcess"))
        .with_condition(Condition::Always)
}

shell_script! {
    /// Scan the agent's proposed safe-output NDJSON for any approval-gated
    /// tool and publish a `HasReviewedProposals` output variable. The
    /// ManualReview gate is conditioned on this so a run never pauses for a
    /// human when the agent did not propose anything that requires review.
    DETECT_REVIEWED_PROPOSALS {
        interpreter: Bash,
        bindings: [ALTERNATION],
        externals: [WORKING_DIRECTORY],
        fragments: [],
        body: r###"
HAS_REVIEWED="false"
NAMES=""
PROPOSALS=$(find "$WORKING_DIRECTORY/safe_outputs" -name "safe_outputs.ndjson" 2>/dev/null | head -n 1)
if [ -n "$PROPOSALS" ] && [ -f "$PROPOSALS" ]; then
  if command -v jq >/dev/null 2>&1; then
    # Match only the top-level "name" of each NDJSON object so a
    # "name" key nested inside a tool's params can't false-positive.
    if NAMES=$(jq -r 'select(type=="object") | .name // empty' "$PROPOSALS" 2>/dev/null); then
      if printf '%s\n' "$NAMES" | grep -Eqx "($ALTERNATION)"; then
        HAS_REVIEWED="true"
      fi
    else
      # jq failed (e.g. corrupt/truncated proposals). Fall back to the
      # broad raw scan so detection fails safe (over-match, never under-
      # match) and record that detection was inconclusive.
      echo "##vso[task.logissue type=warning]approval-gate: jq failed to parse $PROPOSALS; using raw scan for reviewed-proposal detection"
      if grep -Eq "\"name\"[[:space:]]*:[[:space:]]*\"($ALTERNATION)\"" "$PROPOSALS"; then
        HAS_REVIEWED="true"
      fi
    fi
  elif grep -Eq "\"name\"[[:space:]]*:[[:space:]]*\"($ALTERNATION)\"" "$PROPOSALS"; then
    # jq unavailable: fall back to a broad scan. May over-match (pause
    # unnecessarily) but never under-matches, so the gate stays fail-safe.
    HAS_REVIEWED="true"
  fi
fi
echo "##vso[task.setvariable variable=HasReviewedProposals;isOutput=true]$HAS_REVIEWED"
echo "HasReviewedProposals set to: $HAS_REVIEWED"
"###,
    }
}

fn detect_reviewed_proposals_step(working_directory: &str, reviewed: &[String]) -> BashStep {
    use super::ir::env::EnvValue;
    // `reviewed` are compiler-controlled safe-output names (ASCII
    // alphanumeric/hyphen only — see `validate::is_safe_tool_name`), so they
    // are safe to embed directly in a jq/grep alternation.
    let alternation = reviewed.join("|");
    ShellScript::new(&DETECT_REVIEWED_PROPOSALS)
        .bind_text("ALTERNATION", alternation)
        .into_step("Detect reviewed proposals")
        .with_id(
            StepId::new("reviewedProposals")
                .expect("reviewedProposals is a valid StepId — see StepId::new contract"),
        )
        .with_output(OutputDecl::new("HasReviewedProposals"))
        .with_condition(Condition::Always)
        .with_env("WORKING_DIRECTORY", EnvValue::literal(working_directory))
}

shell_script! {
    /// Scan the analyzed proposal NDJSON once and publish one output variable
    /// per custom tool. The `tool_checks` fragment is populated by
    /// [`detect_custom_proposals_step`] with one block of shell per registered
    /// custom tool. Custom executor jobs use these booleans in their job-level
    /// `condition:` so an empty/no-op custom proposal set does not start a
    /// job.
    DETECT_CUSTOM_PROPOSALS {
        interpreter: Bash,
        bindings: [],
        externals: [WORKING_DIRECTORY],
        fragments: [tool_checks],
        body: r###"
PROPOSALS=$(find "$WORKING_DIRECTORY/safe_outputs" -name "safe_outputs.ndjson" 2>/dev/null | head -n 1)
NAMES=""
RAW_SCAN="false"
if [ -n "$PROPOSALS" ] && [ -f "$PROPOSALS" ]; then
  if command -v jq >/dev/null 2>&1; then
    if ! NAMES=$(jq -r 'select(type=="object") | .name // empty' "$PROPOSALS" 2>/dev/null); then
      echo "##vso[task.logissue type=warning]custom-proposals: jq failed to parse $PROPOSALS; using raw scan"
      RAW_SCAN="true"
    fi
  else
    RAW_SCAN="true"
  fi
fi
# Fake use so shellcheck (which cannot see the compiler-spliced
# tool_checks fragment) does not flag NAMES / RAW_SCAN as SC2034
# unused. This is a runtime no-op — `:` discards its arguments.
: "${NAMES}" "${RAW_SCAN}"
# ado-aw:fragment tool_checks
"###,
    }
}

fn detect_custom_proposals_step(working_directory: &str, tools: &[String]) -> Result<BashStep> {
    use super::ir::env::EnvValue;
    let mut tool_checks = String::new();
    let mut outputs = Vec::with_capacity(tools.len());
    for tool in tools {
        let output = custom_tool_output_var(tool);
        tool_checks.push_str(&format!(
            "{output}=\"false\"\n\
             if [ -n \"$NAMES\" ] && printf '%s\\n' \"$NAMES\" | grep -Fxq {tool_q}; then\n  \
               {output}=\"true\"\n\
             elif [ \"$RAW_SCAN\" = \"true\" ] && [ -n \"$PROPOSALS\" ] && grep -Eq '\"name\"[[:space:]]*:[[:space:]]*\"{tool}\"' \"$PROPOSALS\"; then\n  \
               {output}=\"true\"\n\
             fi\n\
             echo \"##vso[task.setvariable variable={output};isOutput=true]${output}\"\n\
             echo \"{output} set to: ${output}\"\n",
            tool_q = shell_quote(tool),
        ));
        outputs.push(output);
    }
    let mut step = ShellScript::new(&DETECT_CUSTOM_PROPOSALS)
        .fragment("tool_checks", tool_checks)
        .into_step("Detect custom proposals")
        .with_env("WORKING_DIRECTORY", EnvValue::literal(working_directory));
    for output in outputs {
        step = step.with_output(OutputDecl::new(output));
    }
    Ok(step
        .with_id(StepId::new(CUSTOM_PROPOSALS_STEP_ID)?)
        .with_condition(Condition::Always))
}

shell_script! {
    /// Debug-only probe (emitted when `--debug-pipeline` is on). Probes every
    /// MCPG backend via MCP `initialize` + `tools/list` to surface broken
    /// backends early. Mirrors the legacy
    /// `generate_debug_pipeline_replacements` bash body.
    VERIFY_MCP_BACKENDS {
        interpreter: Bash,
        bindings: [MCPG_PORT],
        externals: [MCPG_API_KEY],
        fragments: [],
        body: r###"
echo "=== Probing MCP backends ==="
PROBE_FAILED=false
for server in $(jq -r '.mcpServers | keys[]' /tmp/awf-tools/mcp-config.json); do
  echo ""
  echo "--- Probing: $server ---"
  # MCP requires initialize handshake before tools/list.
  # Send initialize first, then tools/list in a second request
  # using the session ID from the initialize response.
  INIT_RESPONSE=$(curl -s -D /tmp/probe-headers.txt -o /tmp/probe-init.json -w "%{http_code}" --max-time 120 -X POST \
    -H "Authorization: $MCPG_API_KEY" \
    -H "Content-Type: application/json" \
    -H "Accept: application/json, text/event-stream" \
    -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"ado-aw-probe","version":"1.0"}}}' \
    "http://localhost:$MCPG_PORT/mcp/$server" 2>&1)
  SESSION_ID=$(grep -i "mcp-session-id" /tmp/probe-headers.txt 2>/dev/null | tr -d '\r' | awk '{print $2}')
  echo "Initialize: HTTP $INIT_RESPONSE, session=$SESSION_ID"

  if [ -z "$SESSION_ID" ]; then
    echo "##vso[task.logissue type=warning]MCP backend '$server' did not return a session ID"
    cat /tmp/probe-init.json 2>/dev/null || true
    PROBE_FAILED=true
    continue
  fi

  # Now send tools/list with the session
  HTTP_CODE=$(curl -s -o /tmp/probe-response.json -w "%{http_code}" --max-time 120 -X POST \
    -H "Authorization: $MCPG_API_KEY" \
    -H "Content-Type: application/json" \
    -H "Accept: application/json, text/event-stream" \
    -H "Mcp-Session-Id: $SESSION_ID" \
    -d '{"jsonrpc":"2.0","id":2,"method":"tools/list"}' \
    "http://localhost:$MCPG_PORT/mcp/$server" 2>&1)
  BODY=$(cat /tmp/probe-response.json 2>/dev/null || echo "(empty)")
  # Extract tool count from SSE data line
  TOOL_COUNT=$(echo "$BODY" | grep '^data:' | sed 's/^data: //' | jq -r '.result.tools | length' 2>/dev/null || echo "?")
  echo "tools/list: HTTP $HTTP_CODE"
  if [ "$HTTP_CODE" -ge 200 ] && [ "$HTTP_CODE" -lt 300 ] && [ "$TOOL_COUNT" != "?" ]; then
    echo "✓ $server: $TOOL_COUNT tools available"
  else
    echo "##vso[task.logissue type=warning]MCP backend '$server' tools/list returned HTTP $HTTP_CODE"
    echo "Response: $BODY"
    PROBE_FAILED=true
  fi
done

echo ""
echo "=== MCPG health after probes ==="
curl -sf "http://localhost:$MCPG_PORT/health" | jq . || true

if [ "$PROBE_FAILED" = "true" ]; then
  echo "##vso[task.logissue type=warning]One or more MCP backends failed to initialize — check logs above"
fi
"###,
    }
}

fn verify_mcp_backends_step() -> BashStep {
    use super::ir::env::EnvValue;
    ShellScript::new(&VERIFY_MCP_BACKENDS)
        .bind("MCPG_PORT", Binding::number(MCPG_PORT.into()))
        .into_step("Verify MCP backends")
        .with_env("MCPG_API_KEY", EnvValue::pipeline_var("MCP_GATEWAY_API_KEY"))
}

// ─────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────

/// Classify a single raw env-var value string into a typed [`EnvValue`].
///
/// An ADO **macro** `$(NAME)` (with no nested `$` or `(`) becomes an
/// [`EnvValue::PipelineVar`] so lowering re-emits the unquoted `$(NAME)` form;
/// anything else becomes an [`EnvValue::Literal`]. Single source of truth for
/// macro-vs-literal classification, shared by [`parse_env_block`] (which also
/// handles YAML-typed scalars) and the detection provider-env path.
///
/// Only a value that is *exactly* one `$(NAME)` wrapper is treated as a macro.
/// Compound values (e.g. `$(A)$(B)`, or `prefix-$(X)`) intentionally fall through
/// to `Literal` — they are emitted as a quoted YAML scalar. This is still
/// correct at runtime: ADO expands `$( )` macro references inside step-env values
/// regardless of quoting, so both references still expand. The only observable
/// difference is the quoted-vs-unquoted rendering in the compiled YAML.
fn env_value_from_str(raw: &str) -> super::ir::env::EnvValue {
    use super::ir::env::EnvValue;
    if let Some(inner) = raw.strip_prefix("$(").and_then(|s| s.strip_suffix(')'))
        && !inner.contains('$')
        && !inner.contains('(')
    {
        EnvValue::pipeline_var(inner.to_string())
    } else {
        EnvValue::literal(raw.to_string())
    }
}

/// Parse a legacy YAML env block (`env:\n  KEY: VALUE\n  KEY: VALUE`)
/// into typed `(name, EnvValue)` pairs preserving insertion order.
///
/// Each value is round-tripped through `serde_yaml` so quoted forms
/// (`"true"`, `"file"`) become bare literals; string values are then
/// classified by [`env_value_from_str`] (ADO macros → `PipelineVar`,
/// otherwise `Literal`) and non-string scalars are preserved as
/// `RawYamlScalar`.
///
/// # Errors
///
/// Returns `Err` if the input fails to parse as YAML or does not
/// match the `env: { KEY: VALUE, … }` shape. The inputs are
/// compiler-generated from validated front-matter, so a parse
/// failure here indicates a compiler bug rather than user error —
/// surfacing it loudly is much better than the previous silent
/// empty-vec fallback (which produced runtime "GITHUB_TOKEN missing"
/// failures in the pipeline with no compile-time signal).
fn parse_env_block(yaml_block: &str) -> Result<Vec<(String, super::ir::env::EnvValue)>> {
    use super::ir::env::EnvValue;
    if yaml_block.trim().is_empty() {
        return Ok(Vec::new());
    }
    let parsed: serde_yaml::Value = serde_yaml::from_str(yaml_block).map_err(|e| {
        anyhow::anyhow!(
            "ir::standalone: parse_env_block failed to parse compiler-generated YAML \
             ({e}); this is a compiler bug. Block was:\n{yaml_block}"
        )
    })?;
    let env_map = match parsed {
        serde_yaml::Value::Mapping(mut m) => {
            match m.shift_remove(serde_yaml::Value::String("env".into())) {
                Some(serde_yaml::Value::Mapping(inner)) => inner,
                Some(other) => anyhow::bail!(
                    "ir::standalone: parse_env_block: top-level `env:` value must be a \
                     mapping, got {:?}",
                    other
                ),
                None => anyhow::bail!(
                    "ir::standalone: parse_env_block: top-level YAML mapping is missing \
                     `env:` key"
                ),
            }
        }
        other => anyhow::bail!(
            "ir::standalone: parse_env_block: top-level YAML must be a mapping with an \
             `env:` key, got {:?}",
            other
        ),
    };
    let mut out = Vec::with_capacity(env_map.len());
    for (k, v) in env_map {
        let key = match k {
            serde_yaml::Value::String(s) => s,
            _ => continue,
        };
        match &v {
            // String values: route ADO macros through PipelineVar so
            // lowering preserves the `$(X)` form unquoted; everything
            // else lands as a Literal.
            serde_yaml::Value::String(raw_value) => {
                out.push((key, env_value_from_str(raw_value)));
            }
            // Non-string scalars (numbers / bools): preserve the
            // typed scalar identity through RawYamlScalar so the
            // emitter doesn't quote them.
            other => {
                out.push((key, EnvValue::RawYamlScalar(other.clone())));
            }
        }
    }
    Ok(out)
}

fn step_to_raw_yaml_string(step: &serde_yaml::Value) -> Result<String> {
    // Serialise the user-supplied step value as a leading-`- ` sequence
    // item so lower_raw_yaml's leading-`- ` stripper handles it.
    let yaml = serde_yaml::to_string(step)
        .map_err(|e| anyhow::anyhow!("Failed to serialize user step: {e}"))?;
    // The yaml ends with a newline; prepend `- ` and indent continuation
    // lines by 2 spaces.
    let mut out = String::new();
    for (i, line) in yaml.lines().enumerate() {
        if i == 0 {
            out.push_str("- ");
            out.push_str(line);
        } else {
            out.push('\n');
            out.push_str("  ");
            out.push_str(line);
        }
    }
    Ok(out)
}

fn push_raw_yaml_if_nonempty(steps: &mut Vec<Step>, yaml: &str) -> Result<()> {
    if yaml.trim().is_empty() {
        return Ok(());
    }
    // The body may contain one or more top-level `- ...` items (e.g.
    // engine_install_steps_yaml is two steps: install + version output).
    // Split them through serde_yaml so each item lands as a separate
    // Step::RawYaml that lower_raw_yaml can parse individually — this
    // gives us a real YAML parse instead of relying on blank-line
    // separators in the input. Any parse failure is a compiler bug
    // (the producer just emitted invalid YAML) and surfaces loudly.
    for chunk in split_yaml_step_sequence(yaml)? {
        steps.push(Step::RawYaml(chunk));
    }
    Ok(())
}

/// Split a YAML string of the form
///
/// ```yaml
/// - bash: |
///     ...
///   displayName: ...
///
/// - bash: |
///     ...
/// ```
///
/// into individual sequence items (`- bash: ...`), preserving each
/// item's body via `serde_yaml::to_string` so `lower_raw_yaml` can
/// handle it directly. Each returned string starts with `- `.
///
/// Single-item inputs return a one-element `Vec`. Inputs that are a
/// bare mapping (no leading `- `) are treated as a single item.
///
/// # Errors
///
/// Returns `Err` if the input does not parse as YAML, or if it
/// parses as something other than a sequence of mappings / a bare
/// mapping. Inputs are compiler-generated, so any failure is a
/// compiler bug.
fn split_yaml_step_sequence(yaml: &str) -> Result<Vec<String>> {
    let parsed: serde_yaml::Value = serde_yaml::from_str(yaml).map_err(|e| {
        anyhow::anyhow!(
            "ir::standalone: split_yaml_step_sequence failed to parse compiler-generated \
             step YAML ({e}); this is a compiler bug. Input was:\n{yaml}"
        )
    })?;
    let items: Vec<serde_yaml::Value> = match parsed {
        serde_yaml::Value::Sequence(seq) => seq,
        bare @ serde_yaml::Value::Mapping(_) => vec![bare],
        other => anyhow::bail!(
            "ir::standalone: split_yaml_step_sequence: expected a sequence of step mappings \
             or a single step mapping, got {:?}",
            other
        ),
    };
    items.into_iter().map(step_value_to_dash_yaml).collect()
}

/// Render a single YAML mapping value as a `- key: value\n  …` chunk
/// (i.e. as one item of a YAML sequence). The output starts with
/// `- ` so [`lower_raw_yaml`] can de-indent it.
fn step_value_to_dash_yaml(v: serde_yaml::Value) -> Result<String> {
    let yaml = serde_yaml::to_string(&v)
        .map_err(|e| anyhow::anyhow!("ir::standalone: failed to re-serialize step value ({e})"))?;
    let mut out = String::with_capacity(yaml.len() + 4);
    for (i, line) in yaml.lines().enumerate() {
        if i == 0 {
            out.push_str("- ");
            out.push_str(line);
        } else {
            out.push('\n');
            out.push_str("  ");
            out.push_str(line);
        }
    }
    out.push('\n');
    Ok(out)
}

/// Build the agent prompt body.
///
/// In `inlined-imports: true` mode the entire body (imported + consumer) is
/// already in `markdown_body`, so it is resolved inline verbatim. In the
/// default mode the consumer body is delivered by a `{{#runtime-import}}`
/// marker (so authors can edit it without recompiling), but any imported
/// component bodies (`imported_prompt_body`) are inlined **ahead** of that
/// marker: they were substituted at compile time and cannot be re-derived at
/// runtime from the consumer's own source. Mirrors gh-aw, which compile-inlines
/// input-bearing imports and runtime-imports only the main body.
fn build_agent_content(
    front_matter: &FrontMatter,
    input_path: &Path,
    markdown_body: &str,
    imported_prompt_body: &str,
    source_path: &str,
    trigger_repo_directory: &str,
) -> Result<String> {
    if front_matter.inlined_imports {
        let base_dir = input_path
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."));
        return crate::compile::extensions::ado_script::resolve_imports_inline(
            markdown_body,
            base_dir,
        );
    }
    // Runtime-import marker path: source_path may embed
    // `{{ trigger_repo_directory }}`; substitute, then strip the
    // `$(Build.SourcesDirectory)/` prefix to yield a relative path.
    let absolute = source_path.replace("{{ trigger_repo_directory }}", trigger_repo_directory);
    let marker_path = absolute
        .strip_prefix("$(Build.SourcesDirectory)/")
        .unwrap_or(&absolute)
        .to_string();
    anyhow::ensure!(
        !marker_path.chars().any(char::is_whitespace),
        "runtime-import: agent source path '{}' contains whitespace, which is not supported by the runtime resolver (rename the path to remove spaces, or set `inlined-imports: true`)",
        marker_path
    );
    anyhow::ensure!(
        !marker_path.contains('}'),
        "runtime-import: agent source path '{}' contains '}}', which is not supported by the runtime resolver (rename the path to remove '}}' characters, or set `inlined-imports: true`)",
        marker_path
    );
    let consumer_marker = format!("{{{{#runtime-import {}}}}}", marker_path);

    // Prepend the compile-time-substituted imported component bodies (if any)
    // ahead of the consumer's runtime-import marker (imports-first ordering).
    if imported_prompt_body.trim().is_empty() {
        Ok(consumer_marker)
    } else {
        Ok(format!("{imported_prompt_body}\n\n{consumer_marker}"))
    }
}

// Suppress unused warnings on imports retained for clarity / future use.
#[allow(dead_code)]
const _MCPG_CONFIG_TYPE_BIND: Option<McpgConfig> = None;
#[allow(dead_code)]
const _DECLARATIONS_BIND: Option<Declarations> = None;
#[allow(dead_code)]
const _HEADER_MARKER_BIND: &str = HEADER_MARKER;
#[allow(dead_code)]
const _PIPELINE_VAR_BIND: Option<PipelineVar> = None;
#[allow(dead_code)]
const _PIPELINE_RESOURCE_BIND: Option<PipelineResource> = None;
#[allow(dead_code)]
const _SUBMODULES_OPT_BIND: Option<SubmodulesOpt> = None;

#[cfg(test)]
mod tests {
    use super::*;

    fn test_front_matter(yaml: &str) -> FrontMatter {
        serde_yaml::from_str(yaml).expect("front matter should parse")
    }

    fn test_ctx() -> StandaloneCtx {
        let test_pool = Pool::VmImage("ubuntu-latest".to_string());
        StandaloneCtx {
            pools: PerJobPools {
                setup: test_pool.clone(),
                agent: test_pool.clone(),
                detection: test_pool.clone(),
                safe_outputs: test_pool.clone(),
                safe_outputs_reviewed: test_pool.clone(),
                teardown: test_pool.clone(),
                conclusion: test_pool.clone(),
            },
            agent_display_name: "Test".to_string(),
            self_checkout_fetch: CheckoutFetchOpts::default(),
            working_directory: "$(Build.SourcesDirectory)".to_string(),
            trigger_repo_directory: "$(Build.SourcesDirectory)".to_string(),
            self_repository_name: EnvValue::literal("test-repo"),
            compiler_version: "0.0.0-test".to_string(),
            engine_install_steps_yaml: String::new(),
            detection_engine_install_steps_yaml: String::new(),
            engine_run: "echo agent".to_string(),
            engine_run_detection: "echo detection".to_string(),
            detection_engine_config: EngineConfig::default(),
            threat_detection: ThreatDetectionConfig::default(),
            engine_env: "GITHUB_READ_ONLY: 1".to_string(),
            engine_log_dir: "/tmp/logs".to_string(),
            allowed_domains: "example.com".to_string(),
            detection_allowed_domains: "example.com".to_string(),
            awf_mounts: "\\".to_string(),
            awf_path_step_yaml: String::new(),
            mcpg_config_json: "{}".to_string(),
            custom_tools_json: None,
            resolved_execution_config_json: "{}".to_string(),
            mcpg_docker_env: String::new(),
            mcpg_step_env: String::new(),
            source_path: "$(Build.SourcesDirectory)/agents/test.md".to_string(),
            source_relative_path: "agents/test.md".to_string(),
            pipeline_path: "$(Build.SourcesDirectory)/agents/test.lock.yml".to_string(),
            acquire_read_token: String::new(),
            acquire_write_token: String::new(),
            integrity_check_yaml: String::new(),
            agent_content_value: "Test prompt".to_string(),
            debug_pipeline: false,
            byom_exclude_keys: Vec::new(),
            detection_byom_exclude_keys: Vec::new(),
            detection_engine_env: Vec::new(),
        }
    }

    fn canonical_jobs_for(yaml: &str) -> Vec<Job> {
        let fm = test_front_matter(yaml);
        let mut cfg = test_ctx();
        let schemas = super::super::custom_tools::generate_custom_tool_schemas(&fm).unwrap();
        cfg.resolved_execution_config_json =
            super::super::custom_tools::resolved_execution_config_json(&fm, &schemas).unwrap();
        cfg.custom_tools_json = (!schemas.is_empty())
            .then(|| super::super::custom_tools::custom_tools_json(&schemas).unwrap());
        build_canonical_jobs(&fm, &[], &cfg, &[], &[], &[], None).unwrap()
    }

    fn job_by_id<'a>(jobs: &'a [Job], id: &str) -> &'a Job {
        jobs.iter().find(|job| job.id.as_str() == id).unwrap()
    }

    #[test]
    fn custom_job_uses_aggregate_agent_output_without_checkout_or_result_artifact() {
        let jobs = canonical_jobs_for(
            r#"
name: Test
description: Test
safe-outputs:
  jobs:
    notify-team:
      display-name: Notify team
      description: Send a notification.
      timeout-minutes: 10
      inputs:
        title:
          type: string
          description: Notification title.
          required: true
      env:
        TOKEN: $(SHARED_TOKEN)
        ENDPOINT: $(SHARED_ENDPOINT)
      steps:
        - bash: echo notify
          displayName: Component notify
          env:
            TOKEN: step-token
"#,
        );
        let custom = job_by_id(&jobs, "Custom_notify_team");
        assert_eq!(custom.display_name, "Notify team");
        assert_eq!(custom.timeout, Some(std::time::Duration::from_secs(600)));
        assert!(matches!(
            custom.steps.first(),
            Some(Step::Checkout(CheckoutStep {
                repository: CheckoutRepo::None,
                ..
            }))
        ));
        assert!(custom.steps.iter().any(|step| {
            matches!(step, Step::Bash(step)
                if step.script.contains("--prepare-custom-agent-output")
                    && step.script.contains("--resolved-config"))
        }));
        assert!(custom.steps.iter().any(|step| {
            matches!(step, Step::RawYaml(yaml)
                if yaml.contains("Component notify")
                    && yaml.contains("ADO_AW_AGENT_OUTPUT")
                    && yaml.contains("ADO_AW_SAFE_OUTPUTS_STAGED")
                    && yaml.contains("TOKEN: step-token")
                    && yaml.contains("ENDPOINT: $(SHARED_ENDPOINT)")
                    && !yaml.contains("TOKEN: $(SHARED_TOKEN)"))
        }));
        assert!(
            !custom
                .steps
                .iter()
                .any(|step| matches!(step, Step::Publish(_)))
        );
        let conclusion = job_by_id(&jobs, "Conclusion");
        assert!(conclusion.variables.iter().any(|variable| {
            variable.name == "AW_CUSTOM_JOB_0_RESULT"
                && matches!(
                    &variable.value,
                    EnvValue::Literal(value)
                        if value == "$[dependencies.Custom_notify_team.result]"
                )
        }));
        assert!(conclusion.steps.iter().any(|step| {
            matches!(step, Step::Bash(step)
                if step.env.contains_key("AW_CUSTOM_JOB_COUNT")
                    && step.env.contains_key("AW_CUSTOM_JOB_0_NAME")
                    && step.env.contains_key("AW_CUSTOM_JOB_0_RESULT"))
        }));
    }

    #[test]
    fn custom_job_staged_and_authored_condition_are_additive() {
        let jobs = canonical_jobs_for(
            r#"
name: Test
description: Test
safe-outputs:
  staged: true
  jobs:
    notify:
      description: Notify.
      condition: eq(variables['EnableNotify'], 'true')
      steps:
        - bash: echo notify
"#,
        );
        let custom = job_by_id(&jobs, "Custom_notify");
        assert!(matches!(
            &custom.condition,
            Some(Condition::And(parts))
                if parts.iter().any(|part| matches!(
                    part,
                    Condition::Custom(value)
                        if value == "eq(variables['EnableNotify'], 'true')"
                ))
        ));
        assert!(custom.steps.iter().any(|step| {
            matches!(step, Step::RawYaml(yaml)
                if yaml.contains("ADO_AW_SAFE_OUTPUTS_STAGED: 'true'")
                    || yaml.contains("ADO_AW_SAFE_OUTPUTS_STAGED: \"true\"")
                    || yaml.contains("ADO_AW_SAFE_OUTPUTS_STAGED: true"))
        }));
    }

    #[test]
    fn reviewed_custom_job_does_not_create_phantom_reviewed_safeoutputs_result() {
        let jobs = canonical_jobs_for(
            r#"
name: Test
description: Test
safe-outputs:
  noop: {}
  notify:
    require-approval: true
  jobs:
    notify:
      description: Notify.
      steps:
        - bash: echo notify
"#,
        );

        assert!(
            !jobs
                .iter()
                .any(|job| job.id.as_str() == "SafeOutputs_Reviewed")
        );
        let conclusion = job_by_id(&jobs, "Conclusion");
        assert!(
            !conclusion
                .variables
                .iter()
                .any(|variable| variable.name == "AW_SAFEOUTPUTS_REVIEWED_RESULT")
        );
        assert!(conclusion.steps.iter().all(|step| {
            !matches!(step, Step::Bash(step)
                if step.env.contains_key("AW_SAFEOUTPUTS_REVIEWED_RESULT"))
        }));
    }

    #[test]
    fn custom_dependency_on_reviewed_job_is_post_review_not_separately_reviewed() {
        let jobs = canonical_jobs_for(
            r#"
name: Test
description: Test
safe-outputs:
  notify:
    require-approval: true
  jobs:
    notify:
      description: Notify.
      steps:
        - bash: echo notify
    publish-summary:
      description: Publish summary.
      needs: notify
      steps:
        - bash: echo summary
"#,
        );
        let notify = job_by_id(&jobs, "Custom_notify");
        assert!(
            notify
                .depends_on
                .iter()
                .any(|id| id.as_str() == "ManualReview")
        );
        let summary = job_by_id(&jobs, "Custom_publish_summary");
        assert!(
            summary
                .depends_on
                .iter()
                .any(|id| id.as_str() == "Custom_notify")
        );
        assert!(
            !summary
                .depends_on
                .iter()
                .any(|id| id.as_str() == "ManualReview")
        );
    }

    #[test]
    fn teardown_waits_for_automatic_custom_jobs_and_runs_as_cleanup() {
        let jobs = canonical_jobs_for(
            r#"
name: Test
description: Test
safe-outputs:
  jobs:
    notify:
      description: Notify.
      steps:
        - bash: echo notify
teardown:
  - bash: echo cleanup
"#,
        );
        let teardown = job_by_id(&jobs, "Teardown");
        assert!(
            teardown
                .depends_on
                .iter()
                .any(|id| id.as_str() == "SafeOutputs")
        );
        assert!(
            teardown
                .depends_on
                .iter()
                .any(|id| id.as_str() == "Custom_notify")
        );
        assert_eq!(teardown.condition, Some(Condition::Always));
    }

    #[test]
    fn custom_job_compiler_steps_do_not_require_an_interpreter() {
        let jobs = canonical_jobs_for(
            r#"
name: Test
description: Test
safe-outputs:
  jobs:
    notify:
      description: Notify.
      steps:
        - bash: echo notify
"#,
        );
        let job = job_by_id(&jobs, "Custom_notify");
        // Custom jobs run on consumer-owned pools, which are not guaranteed to
        // ship python3. Every compiler-generated step must stay within bash plus
        // the downloaded `ado-aw` binary; only the authored component steps may
        // pull in extra tooling.
        let generated_bash: Vec<&str> = job
            .steps
            .iter()
            .filter_map(|step| match step {
                Step::Bash(bash) => Some(bash.script.as_str()),
                _ => None,
            })
            .collect();
        assert!(
            !generated_bash.is_empty(),
            "custom job should emit compiler-generated bash steps"
        );
        for script in generated_bash {
            assert!(
                !script.contains("python3"),
                "compiler-generated custom job step must not depend on python3: {script}"
            );
        }
    }

    #[test]
    fn custom_job_dependency_cycles_fail_compilation() {
        let fm = test_front_matter(
            r#"
name: Test
description: Test
safe-outputs:
  jobs:
    first:
      description: First.
      needs: second
      steps:
        - bash: echo first
    second:
      description: Second.
      needs: first
      steps:
        - bash: echo second
"#,
        );
        let cfg = test_ctx();
        let error = build_canonical_jobs(&fm, &[], &cfg, &[], &[], &[], None).unwrap_err();
        assert!(error.to_string().contains("dependency cycle"), "{error:#}");
    }

    #[test]
    fn unavailable_reviewed_safeoutputs_dependency_fails_compilation() {
        let fm = test_front_matter(
            r#"
name: Test
description: Test
safe-outputs:
  noop: {}
  jobs:
    publish:
      description: Publish.
      needs: safe-outputs-reviewed
      steps:
        - bash: echo publish
"#,
        );
        let cfg = test_ctx();
        let error = build_canonical_jobs(&fm, &[], &cfg, &[], &[], &[], None).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("no reviewed built-in SafeOutputs path"),
            "{error:#}"
        );
    }

    #[test]
    fn removed_custom_variable_check_ignores_descriptive_text() {
        let step = serde_json::json!({
            "bash": "echo ok",
            "displayName": "Migrated from ADO_AW_SAFE_OUTPUT_PROPOSALS",
        });
        assert!(validate_custom_job_step("notify", &step).is_ok());
    }

    #[test]
    fn removed_custom_variable_check_rejects_runtime_references() {
        for step in [
            serde_json::json!({
                "bash": "cat \"$ADO_AW_SAFE_OUTPUT_PROPOSALS\"",
            }),
            serde_json::json!({
                "pwsh": "Get-Content $env:ADO_AW_SAFE_OUTPUT_RESULTS",
            }),
            serde_json::json!({
                "powershell": "Write-Output $Env:ADO_AW_SAFE_OUTPUT_PROPOSALS",
            }),
            serde_json::json!({
                "bash": "echo ok",
                "env": {
                    "LEGACY": "$(ADO_AW_SAFE_OUTPUT_PROPOSALS)",
                },
            }),
        ] {
            let error = validate_custom_job_step("notify", &step).unwrap_err();
            assert!(error.to_string().contains("removed variable"), "{error:#}");
        }
    }

    #[test]
    fn removed_custom_variable_reference_syntaxes_are_detected() {
        let variable = "ADO_AW_SAFE_OUTPUT_PROPOSALS";
        for value in [
            "$(ADO_AW_SAFE_OUTPUT_PROPOSALS)",
            "$ADO_AW_SAFE_OUTPUT_PROPOSALS",
            "${ADO_AW_SAFE_OUTPUT_PROPOSALS}",
            "$env:ADO_AW_SAFE_OUTPUT_PROPOSALS",
            "%ADO_AW_SAFE_OUTPUT_PROPOSALS%",
        ] {
            assert!(
                json_value_references_variable(
                    &serde_json::Value::String(value.to_string()),
                    variable
                ),
                "{value}"
            );
        }
    }

    // ── fold_agent_conditions (issue #987) ─────────────────────────────────

    #[test]
    fn fold_agent_conditions_empty_returns_none() {
        // Pre-lift behaviour: when no extension contributes a clause,
        // the Agent job has no `condition:` at all (so it inherits the
        // default `succeeded()` from ADO). The fold MUST preserve
        // that — emitting `condition: succeeded()` explicitly would
        // be a fixture drift.
        assert!(fold_agent_conditions(&[]).is_none());
    }

    #[test]
    fn fold_agent_conditions_leads_with_succeeded() {
        // The previous monolithic `build_agentic_condition` emitted
        // `succeeded()` as the first And() part. The fold owns that
        // prefix now so individual extensions don't have to duplicate
        // it.
        let clauses = vec![Condition::Custom("eq(variables['X'], 'y')".into())];
        let cond = fold_agent_conditions(&clauses).expect("non-empty fold");
        let Condition::And(parts) = cond else {
            panic!("expected And, got {cond:?}");
        };
        assert_eq!(parts.len(), 2);
        assert!(matches!(parts[0], Condition::Succeeded));
        assert!(matches!(&parts[1], Condition::Custom(s) if s == "eq(variables['X'], 'y')"));
    }

    #[test]
    fn fold_agent_conditions_preserves_clause_order() {
        // Declaration order matters for `condition:` readability AND
        // for fixture parity. The fold must AND-append clauses in
        // input order with no reordering, deduplication, or
        // simplification.
        let clauses = vec![
            Condition::Custom("A".into()),
            Condition::Custom("B".into()),
            Condition::Custom("C".into()),
        ];
        let cond = fold_agent_conditions(&clauses).unwrap();
        let Condition::And(parts) = cond else {
            panic!("expected And, got {cond:?}")
        };
        assert_eq!(parts.len(), 4);
        assert!(matches!(parts[0], Condition::Succeeded));
        for (i, expected) in ["A", "B", "C"].iter().enumerate() {
            match &parts[i + 1] {
                Condition::Custom(s) => assert_eq!(s, expected),
                other => panic!("part {} expected Custom, got {other:?}", i + 1),
            }
        }
    }

    // ── parse_env_block ────────────────────────────────────────────────────

    #[test]
    fn parse_env_block_empty_input_is_ok_empty_vec() {
        let pairs = parse_env_block("").unwrap();
        assert!(pairs.is_empty());
    }

    #[test]
    fn parse_env_block_routes_ado_macro_through_pipeline_var() {
        let pairs = parse_env_block("env:\n  GITHUB_TOKEN: $(GITHUB_TOKEN)\n").unwrap();
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].0, "GITHUB_TOKEN");
        assert!(matches!(
            pairs[0].1,
            crate::compile::ir::env::EnvValue::PipelineVar(ref name) if name == "GITHUB_TOKEN"
        ));
    }

    #[test]
    fn env_value_from_str_single_macro_is_pipeline_var() {
        use crate::compile::ir::env::EnvValue;
        assert!(matches!(
            env_value_from_str("$(Setup.Token)"),
            EnvValue::PipelineVar(ref n) if n == "Setup.Token"
        ));
    }

    #[test]
    fn env_value_from_str_compound_or_partial_macro_is_literal() {
        use crate::compile::ir::env::EnvValue;
        // Concatenated / partial macros are NOT single-wrapper macros, so they
        // fall through to Literal. They are still correct at runtime: ADO expands
        // $( ) references inside the (quoted) literal value. This pins the
        // documented classification boundary.
        for raw in ["$(A)$(B)", "prefix-$(X)", "$(X)-suffix", "plain-literal"] {
            assert!(
                matches!(env_value_from_str(raw), EnvValue::Literal(ref v) if v == raw),
                "value {raw:?} should classify as a verbatim Literal"
            );
        }
    }

    #[test]
    fn parse_env_block_bails_on_malformed_yaml() {
        // `KEY: : value` is ambiguous/invalid YAML: the bare value
        // starts with `: `, which the YAML parser cannot interpret as
        // a plain scalar.  Callers should never produce such a block,
        // so the typed Result surface should bail loudly.
        let err = parse_env_block("env:\n  KEY: : value\n").unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("parse_env_block failed to parse compiler-generated YAML"),
            "expected compiler-bug parse-failure message, got: {msg}"
        );
    }

    #[test]
    fn parse_env_block_bails_when_top_level_is_not_a_mapping() {
        let err = parse_env_block("just a string\n").unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("top-level YAML must be a mapping"),
            "got: {msg}"
        );
    }

    #[test]
    fn parse_env_block_bails_when_env_key_is_missing() {
        let err = parse_env_block("other: value\n").unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("missing `env:` key"), "got: {msg}");
    }

    #[test]
    fn the_policy_engine_starts_before_the_mcp_gateway() {
        // The Azure DevOps MCP is redirected at the engine's container
        // address, which does not exist until the engine is running. Starting
        // MCPG first would leave the redirect unresolvable.
        let network_script = prepare_ado_proxy_network_step().script;
        assert!(
            network_script.contains(&format!("PROXY_NETWORK='{ADO_PROXY_NETWORK_NAME}'"))
                && network_script.contains(r#"docker network create --internal "$PROXY_NETWORK""#),
            "the internal network must be created from the compiler-supplied \
             PROXY_NETWORK binding: {network_script}"
        );
        let script = prepare_ado_mcp_step(common::ADO_MCP_VERSION).script;
        assert!(
            script.contains(&format!("MCP_PACKAGE='{ADO_MCP_PACKAGE}'"))
                && script.contains(&format!("MCP_VERSION='{}'", common::ADO_MCP_VERSION))
                && script.contains(r#""$MCP_PACKAGE@$MCP_VERSION""#),
            "the MCP package must be pinned, not floating: {script}"
        );
        assert!(
            script.contains("--save-exact"),
            "an unpinned resolve would vary the agent's tool surface between runs"
        );
        assert!(
            script.contains("$MCP_INSTALLED\" != \""),
            "the resolved version must be verified, not just requested: {script}"
        );

        // A caller-supplied version must reach the script, and the compiled-in
        // default must not survive alongside it. The version is a binding now,
        // so assert on the prelude — that proves the producer supplied it,
        // where a bare substring would also match the verification message.
        let override_script = prepare_ado_mcp_step("2.9.0").script;
        assert!(
            override_script.contains("MCP_VERSION='2.9.0'"),
            "the override must be bound: {override_script}"
        );
        assert!(
            !override_script.contains(&format!("MCP_VERSION='{}'", common::ADO_MCP_VERSION)),
            "the default version must not survive an override: {override_script}"
        );
        // The pin and its verification both read the binding, so an override
        // cannot be applied to one and not the other.
        assert!(
            override_script.contains(r#""$MCP_PACKAGE@$MCP_VERSION""#)
                && override_script.contains("expected $MCP_VERSION"),
            "install and verification must share one version: {override_script}"
        );
    }

    #[test]
    fn the_interception_ca_is_usable_by_strict_verifiers() {
        // `pathlen` without `keyCertSign` is accepted by Node but rejected by
        // OpenSSL 3 with "Path length given without key usage keyCertSign".
        // Real `az` hit exactly that: Python's requests verified the chain
        // strictly and refused, while every Node client had been happy. The
        // key usage must therefore be declared explicitly.
        let script = start_ado_proxy_step(&proxy_fm()).script;
        assert!(
            script.contains("keyUsage=critical,keyCertSign,cRLSign"),
            "the CA must declare keyCertSign or strict verifiers reject it: {script}"
        );
    }

    #[test]
    fn the_mcp_network_has_no_route_to_the_internet() {
        // Measured, not assumed: a container on a normal user-defined bridge
        // reaches the internet through Docker's outbound NAT. Without
        // `--internal` the MCP would keep a direct route to every Azure DevOps
        // host the redirect does not override, and the engine would police one
        // hostname rather than the boundary.
        let script = prepare_ado_proxy_network_step().script;
        assert!(
            script.contains("--internal"),
            "the MCP must not be able to route past the policy engine: {script}"
        );
    }

    #[test]
    fn ado_proxy_supplies_every_current_scope_identifier() {
        // The bundle treats an absent identifier as matching nothing, so a
        // missing one is a silent denial rather than an error. `repository`
        // was previously omitted entirely, which killed all twelve catalogued
        // repository operations without any test noticing.
        let script = start_ado_proxy_step(&proxy_fm()).script;
        // `ShellScript` renders ADO predefined macros as `Binding::ado_macro`
        // — single-quoted so the value can never break out of the RHS — so the
        // prelude carries `NAME='$(macro)'` rather than the older
        // double-quoted `NAME="$(macro)"` form.
        for (variable, macro_name) in [
            ("ADO_PROXY_PROJECT", "System.TeamProject"),
            ("ADO_PROXY_PROJECT_ID", "System.TeamProjectId"),
            ("ADO_PROXY_REPOSITORY", "Build.Repository.Name"),
            ("ADO_PROXY_REPOSITORY_ID", "Build.Repository.ID"),
        ] {
            assert!(
                script.contains(&format!("{variable}='$({macro_name})'")),
                "{variable} must be sourced from $({macro_name}): {script}"
            );
            assert!(
                script.contains(&format!("s|\\${{{variable}}}|${variable}|g")),
                "{variable} must be substituted into the policy: {script}"
            );
        }
    }

    #[test]
    fn ado_proxy_refuses_to_start_on_an_unsubstituted_placeholder() {
        // A surviving `${ADO_PROXY_*}` would be read as a literal
        // organization or repository name, matching nothing — a total denial
        // that reads as a policy decision rather than a bug.
        let script = start_ado_proxy_step(&proxy_fm()).script;
        assert!(script.contains("grep -q 'ADO_PROXY_' \"$PROXY_DIR/policy/policy.json\""));
        assert!(script.contains("unsubstituted placeholder"));
    }

    #[test]
    fn ado_proxy_derives_the_organization_from_the_collection_uri() {
        // A fixed-prefix strip is a no-op for a *.visualstudio.com collection
        // URL, and a bare last-path-segment rule returns the whole host for
        // it. Both shapes are handled by the shared helper.
        let script = start_ado_proxy_step(&proxy_fm()).script;
        assert!(
            script.contains("if (NF>1) print $NF"),
            "organization derivation must handle both collection forms: {script}"
        );
        assert_eq!(
            script.matches("ADO_PROXY_ORGANIZATION=$(").count(),
            1,
            "exactly one derivation, shared with engine.rs"
        );
    }

    // ── run_agent_step topology attachment ──────────────────────────────────

    fn agent_step_for_test(ado_proxy_enabled: bool) -> String {
        run_agent_step(
            "example.com",
            "\\",
            "/work",
            "copilot -p prompt",
            "FOO: bar",
            &[],
            None,
            ado_proxy_enabled,
        )
        .expect("run_agent_step should build")
        .script
    }

    #[test]
    fn agent_attaches_only_mcpg_when_the_policy_engine_is_disabled() {
        let script = agent_step_for_test(false);
        assert_eq!(
            script.matches("--topology-attach").count(),
            1,
            "compiled output must be unchanged while the engine is unwired: {script}"
        );
        assert!(!script.contains(ADO_PROXY_CONTAINER_NAME));
    }

    #[test]
    fn agent_attaches_both_peers_when_the_policy_engine_is_enabled() {
        let script = agent_step_for_test(true);
        // Verified against the pinned AWF v0.27.32 binary, whose --help states
        // the flag is "Repeatable" and gives a two-peer example.
        assert_eq!(script.matches("--topology-attach").count(), 2);
        assert!(script.contains(&format!("--topology-attach \"{MCPG_CONTAINER_NAME}\"")));
        assert!(script.contains(&format!("--topology-attach \"{ADO_PROXY_CONTAINER_NAME}\"")));
    }

    #[test]
    fn trusted_peers_bypass_squid() {
        // The peer names are not public DNS. Routing them through Squid would
        // break the very connection that reaches the policy engine.
        let disabled = agent_step_for_test(false);
        assert!(disabled.contains(&format!("NO_PROXY:+$NO_PROXY,}}{MCPG_CONTAINER_NAME}")));
        assert!(!disabled.contains(ADO_PROXY_CONTAINER_NAME));

        let enabled = agent_step_for_test(true);
        assert!(enabled.contains(&format!(
            "NO_PROXY:+$NO_PROXY,}}{MCPG_CONTAINER_NAME},{ADO_PROXY_CONTAINER_NAME}"
        )));
    }

    #[test]
    fn enabling_the_policy_engine_changes_only_attachment_and_no_proxy() {
        // Guards against the continuation-indent damage that a hand-built
        // multi-line flag block can silently do to the surrounding invocation.
        let disabled = agent_step_for_test(false);
        let enabled = agent_step_for_test(true);

        let normalize = |script: &str| {
            script
                .lines()
                .filter(|line| !line.contains("--topology-attach") && !line.contains("NO_PROXY"))
                .collect::<Vec<_>>()
                .join("\n")
        };
        assert_eq!(
            normalize(&disabled),
            normalize(&enabled),
            "no other part of the AWF invocation may shift"
        );
    }

    /// Front matter for the proxy step tests: the ADO tool enabled with a read
    /// service connection, which is the configuration that turns the engine on.
    fn proxy_fm() -> FrontMatter {
        crate::compile::parse_markdown(
            "---\nname: t\ndescription: x\ntools:\n  azure-devops:\n    org: myorg\npermissions:\n  read: my-read-sc\n---\n",
        )
        .unwrap()
        .0
    }

    // ── start_ado_proxy_step / stop_ado_proxy_step ──────────────────────────

    #[test]
    fn ado_proxy_never_writes_the_bearer_or_ca_key_under_tmp() {
        // AWF chroots the agent with /tmp mounted at both /tmp and /host/tmp,
        // so anything the step writes under /tmp is agent-readable. The
        // credential this design exists to withhold must not land there.
        let script = start_ado_proxy_step(&proxy_fm()).script;

        assert!(
            script.contains("mktemp -d \"$AGENT_TEMP/ado-proxy."),
            "private material must be generated outside /tmp: {script}"
        );
        // AGENT_TEMP is bound from the ADO `Agent.TempDirectory` macro, which
        // is confined to the agent work directory — never under /tmp.
        assert!(
            script.contains("AGENT_TEMP='$(Agent.TempDirectory)'"),
            "AGENT_TEMP must come from Agent.TempDirectory: {script}"
        );
        for private in ["ca.key", "$ADO_PROXY_BEARER", "PROXY_MATERIAL"] {
            for line in script.lines().filter(|line| line.contains(private)) {
                let container_private_fifo =
                    line.contains("docker exec -i") && line.contains("/tmp/ado-proxy-material");
                assert!(
                    !line.contains("/tmp/gh-aw")
                        && (!line.contains("> /tmp") || container_private_fifo),
                    "{private} must never be written under /tmp: {line}"
                );
            }
        }
    }

    #[test]
    fn ado_proxy_streams_material_on_stdin_rather_than_via_env_or_argv() {
        let step = start_ado_proxy_step(&proxy_fm());

        // The container name reaches the body through the `PROXY_CONTAINER`
        // binding, so the docker invocations reference `$PROXY_CONTAINER`
        // rather than the literal `awmg-ado-proxy`.
        assert!(
            step.script.contains("PROXY_CONTAINER='awmg-ado-proxy'"),
            "PROXY_CONTAINER must be bound to the ado-proxy container name: {}",
            step.script
        );
        assert!(
            step.script.contains(
                "printf '%s' \"$PROXY_MATERIAL\" | docker exec -i \"$PROXY_CONTAINER\""
            ) && step.script.contains("cat > /tmp/ado-proxy-material"),
            "material must stream through the container-private FIFO: {}",
            step.script
        );
        assert!(
            step.script.contains("docker run -d")
                && step.script.contains("mkfifo \"$MATERIAL_FIFO\""),
            "the container must be detached from the Bash task before material handover"
        );
        // A `-e` would put it in the container's `Env`, readable by anyone who
        // can call `docker inspect`; an argv flag would expose it in the
        // process table.
        assert!(
            !step.script.contains("-e ADO_PROXY_BEARER"),
            "the bearer must not reach the container environment"
        );
        assert!(
            !step.script.contains("--token"),
            "the bearer must not be passed as an argument"
        );
    }

    #[test]
    fn ado_proxy_container_lifecycle_is_independent_of_the_start_task() {
        let script = start_ado_proxy_step(&proxy_fm()).script;
        assert!(script.contains("docker run -d"));
        assert!(
            !script.contains("docker run -i --rm"),
            "attached --rm containers disappear when Azure Pipelines cleans up task STDIO"
        );
        assert!(script.contains("docker logs --tail 200"));
        assert!(script.contains("state={{.State.Status}} exit={{.State.ExitCode}}"));
    }

    #[test]
    fn trusted_topology_preflight_reports_missing_peer_logs() {
        let step = verify_trusted_topology_peers_step();
        assert!(step.script.contains(MCPG_CONTAINER_NAME));
        assert!(step.script.contains(ADO_PROXY_CONTAINER_NAME));
        assert!(
            step.script
                .contains("trusted topology peer $PEER is not running")
        );
        assert!(step.script.contains("docker logs --tail 200"));
        assert!(step.script.contains("public CA is not readable"));
        assert!(step.script.contains(ADO_PROXY_PUBLIC_CA_HOST_PATH));
        assert_eq!(step.display_name, "Verify trusted topology peers");
    }

    #[test]
    fn ado_proxy_lifecycle_and_decision_logs_are_published() {
        let stop = stop_ado_proxy_step();
        assert!(stop.script.contains("container-state.txt"));
        assert!(stop.script.contains("container.log"));
        assert!(stop.script.contains("already missing at teardown"));

        let copy = copy_logs_step("/tmp/copilot", false);
        assert!(copy.script.contains("/tmp/gh-aw/ado-proxy-logs"));
        assert!(
            copy.script
                .contains("AGENT_TEMP='$(Agent.TempDirectory)'")
                && copy
                    .script
                    .contains(r#""$AGENT_TEMP/staging/logs/ado-proxy""#),
            "proxy lifecycle and sanitized decision logs must reach the agent artifact"
        );
    }

    #[test]
    fn ado_proxy_destroys_the_signing_key_after_handover() {
        let script = start_ado_proxy_step(&proxy_fm()).script;
        assert!(
            script.contains("shred -u \"$PROXY_DIR/ca.key\""),
            "the CA signing key must not outlive handover: {script}"
        );
        assert!(
            script.contains("trap cleanup_material EXIT"),
            "the work directory must be removed even on failure: {script}"
        );
    }

    #[test]
    fn ado_proxy_publishes_only_the_ca_certificate() {
        let script = start_ado_proxy_step(&proxy_fm()).script;
        // `--public-ca-file` is an *output*: the proxy writes its interception
        // CA there so clients can trust it. It must land somewhere the agent
        // can read (AWF mounts /tmp into the chroot) — unlike the signing key.
        // The flag lives inside the flattened container entrypoint that the
        // `CONTAINER_ENTRYPOINT` binding carries into the prelude.
        assert!(script.contains("--public-ca-file /var/lib/ado-proxy/ado-proxy-ca.pem"));
        assert!(
            script.contains(&format!("AZ_WRAPPER_DIR='{AZ_WRAPPER_DIR}'"))
                && script.contains("-v \"$AZ_WRAPPER_DIR:/var/lib/ado-proxy\""),
            "the wrapper directory must be bound and mounted at /var/lib/ado-proxy: {script}"
        );
        assert!(
            script.contains(&format!("CA_HOST_PATH='{ADO_PROXY_PUBLIC_CA_HOST_PATH}'"))
                && script.contains(
                    "##vso[task.setvariable variable=ADO_PROXY_CA_FILE]$CA_HOST_PATH"
                ),
            "clients need the published certificate's path: {script}"
        );
        assert!(
            !script.contains("ado-proxy-ca.key") && !script.contains("public/ca.key"),
            "the CA private key must never be published"
        );
    }

    #[test]
    fn ado_proxy_mints_a_leaf_for_every_catalogued_protected_host() {
        let script = start_ado_proxy_step(&proxy_fm()).script;
        for host in catalog::catalog().protected_hosts {
            assert!(
                script.contains(&format!("\"{host}\"")),
                "{host} is catalogued as protected but gets no interception leaf, \
                 so it could not be policed: {script}"
            );
        }
    }

    #[test]
    fn ado_proxy_egresses_only_through_squid() {
        let script = start_ado_proxy_step(&proxy_fm()).script;
        assert!(
            script.contains(&format!("--upstream-proxy {AWF_SQUID_URL}")),
            "the only egress must be Squid, so an outage is a 502 not a direct socket"
        );
        // Upstream Azure DevOps certificates are verified against Node's own
        // bundled roots (measured: 144 in node:20-slim, which ships no OS
        // trust store at all). Nothing needs mounting for that, and
        // `rejectUnauthorized` is never disabled.
        assert!(
            !script.contains("NODE_TLS_REJECT_UNAUTHORIZED"),
            "upstream verification must never be disabled: {script}"
        );
    }

    #[test]
    fn ado_proxy_reuses_the_existing_node_image() {
        // The proxy ships as an ado-script bundle already downloaded onto the
        // runner, so it must not introduce an image to build, pin or mirror.
        let script = start_ado_proxy_step(&proxy_fm()).script;
        assert_eq!(ADO_PROXY_IMAGE, common::ADO_MCP_IMAGE);
        // The image and the bundle path both reach the body through bindings,
        // so the docker invocation references them as `$PROXY_IMAGE` and
        // `$PROXY_SCRIPT_PATH` while the concrete values live in the prelude.
        assert!(
            script.contains(&format!("PROXY_IMAGE='{ADO_PROXY_IMAGE}'"))
                && script.contains("\"$PROXY_IMAGE\" \\"),
            "docker run must reuse the bound $PROXY_IMAGE: {script}"
        );
        assert!(
            script.contains(&format!(
                "PROXY_SCRIPT_PATH='{}'",
                paths::ADO_PROXY_PATH
            )) && script.contains("\"$PROXY_SCRIPT_PATH:/app/ado-proxy.js:ro\""),
            "docker run must mount the bound ado-proxy bundle: {script}"
        );
    }

    #[test]
    fn ado_proxy_embeds_a_policy_the_bundle_will_accept() {
        let narrowed = crate::compile::parse_markdown(
            "---\nname: t\ndescription: x\ntools:\n  azure-devops:\n    org: myorg\n\
             permissions:\n  read:\n    service-connection: my-read-sc\n    capabilities: [repos]\n---\n",
        )
        .unwrap()
        .0;
        let script = start_ado_proxy_step(&narrowed).script;
        assert!(script.contains("\"catalog_version\""));
        assert!(
            script.contains("\"discovery\""),
            "discovery is always on; without it no client completes startup"
        );
        assert!(script.contains("\"repos\""));
        assert!(
            !script.contains("\"boards\""),
            "an unrequested capability must not be granted"
        );
    }

    #[test]
    fn ado_proxy_recovers_from_an_interrupted_previous_run() {
        // --rm only fires on clean exit; an OOM or SIGKILL leaves the
        // container, and with it a live credential, behind. Both scripts now
        // read the container name through the `PROXY_CONTAINER` binding.
        let start = start_ado_proxy_step(&proxy_fm()).script;
        assert!(
            start.contains(&format!("PROXY_CONTAINER='{ADO_PROXY_CONTAINER_NAME}'"))
                && start.contains("docker rm -f \"$PROXY_CONTAINER\""),
            "start_ado_proxy_step must reap a stale container: {start}"
        );
        let stop = stop_ado_proxy_step().script;
        assert!(
            stop.contains(&format!("PROXY_CONTAINER='{ADO_PROXY_CONTAINER_NAME}'"))
                && stop.contains("docker rm -f \"$PROXY_CONTAINER\""),
            "stop_ado_proxy_step must reap the container: {stop}"
        );
    }

    #[test]
    fn ado_proxy_is_stopped_even_when_the_job_fails() {
        assert_eq!(stop_ado_proxy_step().condition, Some(Condition::Always));
    }

    // ── start_mcpg_step ─────────────────────────────────────────────────────

    #[test]
    fn start_mcpg_step_marks_copilot_mcp_servers_as_default() {
        let step = start_mcpg_step("", "", false, None).unwrap();

        assert!(
            step.script.contains(".value.tools = [\"*\"]"),
            "Copilot mcp-config conversion should preserve wildcard tools: {}",
            step.script
        );
        assert!(
            step.script.contains(".value.isDefaultServer = true"),
            "Copilot mcp-config conversion should mark generated MCP servers as default/trusted: {}",
            step.script
        );
    }

    // ── split_yaml_step_sequence ───────────────────────────────────────────

    #[test]
    fn split_yaml_step_sequence_single_step() {
        let yaml = "- bash: echo hi\n  displayName: greet\n";
        let chunks = split_yaml_step_sequence(yaml).unwrap();
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].starts_with("- bash:"));
        assert!(chunks[0].contains("displayName: greet"));
    }

    #[test]
    fn split_yaml_step_sequence_multiple_steps_without_blank_line_separator() {
        // The previous blank-line-based splitter would have merged
        // these two adjacent steps into a single garbage chunk. The
        // serde_yaml-based splitter correctly returns one chunk per
        // sequence item regardless of whitespace between them.
        let yaml = "- bash: echo first\n  displayName: First\n- bash: echo second\n  displayName: Second\n";
        let chunks = split_yaml_step_sequence(yaml).unwrap();
        assert_eq!(chunks.len(), 2, "got chunks: {chunks:?}");
        assert!(chunks[0].starts_with("- bash:"), "chunk[0]: {}", chunks[0]);
        assert!(chunks[1].starts_with("- bash:"), "chunk[1]: {}", chunks[1]);
        assert!(chunks[0].contains("First"));
        assert!(chunks[1].contains("Second"));
    }

    #[test]
    fn split_yaml_step_sequence_bails_on_invalid_yaml() {
        let yaml = "- bash: |\n  unterminated [ block\n  more\n] mismatched\n";
        let err = split_yaml_step_sequence(yaml).unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("split_yaml_step_sequence failed to parse"),
            "got: {msg}"
        );
    }

    // ── pool-overrides integration ──────────────────────────────────────────

    /// Parse front matter from a markdown string, resolve repos, and
    /// sanitize — mirroring what compile_pipeline_inner does before
    /// calling build_pipeline_context.
    fn parse_and_resolve(source: &str) -> super::super::types::FrontMatter {
        use super::super::common::{parse_markdown, resolve_repos};
        use crate::sanitize::SanitizeConfig;
        let (mut fm, _) = parse_markdown(source).unwrap();
        fm.sanitize_config_fields();
        let (repos, checkout, fetch) = resolve_repos(&fm).unwrap();
        fm.repositories = repos;
        fm.checkout = checkout;
        fm.checkout_fetch = fetch;
        fm
    }

    fn pool_name(pool: &Pool) -> String {
        match pool {
            Pool::Named { name, .. } => name.clone(),
            Pool::VmImage(s) => s.clone(),
            Pool::Server => "server".to_string(),
        }
    }

    fn build_jobs(source: &str) -> Vec<super::super::ir::job::Job> {
        let fm = parse_and_resolve(source);
        let threat_detection = fm.threat_detection_config().unwrap();
        let detection_engine_config = fm.effective_detection_engine(&threat_detection);
        let ctx = super::super::extensions::CompileContext::for_test(&fm);
        let extensions = super::super::extensions::collect_extensions(&fm);
        let decls: Vec<_> = extensions
            .iter()
            .map(|e| e.declarations(&ctx).unwrap())
            .collect();
        let mut ext_setup_steps = vec![];
        let mut ext_agent_prepare = vec![];
        let mut ext_agent_conditions = vec![];
        for d in &decls {
            ext_setup_steps.extend(d.setup_steps.clone());
            ext_agent_prepare.extend(d.agent_prepare_steps.clone());
            ext_agent_conditions.extend(d.agent_conditions.clone());
        }
        let pools = super::super::common::resolve_pool_overrides_typed(
            fm.target.clone(),
            fm.pool.as_ref(),
            fm.pool_overrides(),
        )
        .unwrap();
        let cfg = StandaloneCtx {
            pools,
            agent_display_name: fm.name.clone(),
            self_checkout_fetch: fm
                .checkout_fetch
                .get(super::SELF_CHECKOUT_ALIAS)
                .cloned()
                .unwrap_or_default(),
            working_directory: super::super::common::generate_working_directory(
                &super::super::common::compute_effective_workspace(
                    &fm.workspace,
                    &fm.checkout,
                    &fm.name,
                )
                .unwrap(),
            ),
            trigger_repo_directory: super::super::common::generate_trigger_repo_directory(
                &fm.checkout,
            ),
            self_repository_name: EnvValue::literal("test-repo"),
            compiler_version: "0.0.0-test".to_string(),
            engine_install_steps_yaml: String::new(),
            detection_engine_install_steps_yaml: String::new(),
            engine_run: String::new(),
            engine_run_detection: String::new(),
            detection_engine_config,
            threat_detection,
            engine_env: "env:\n  GITHUB_TOKEN: $(GITHUB_TOKEN)\n".to_string(),
            engine_log_dir: "/tmp/logs".to_string(),
            allowed_domains: String::new(),
            detection_allowed_domains: String::new(),
            awf_mounts: "\\".to_string(),
            awf_path_step_yaml: String::new(),
            mcpg_config_json: "{}".to_string(),
            custom_tools_json: None,
            resolved_execution_config_json: "{}".to_string(),
            mcpg_docker_env: String::new(),
            mcpg_step_env: String::new(),
            source_path: "source.md".to_string(),
            source_relative_path: "source.md".to_string(),
            pipeline_path: "source.lock.yml".to_string(),
            acquire_read_token: String::new(),
            acquire_write_token: String::new(),
            integrity_check_yaml: String::new(),
            agent_content_value: String::new(),
            debug_pipeline: false,
            byom_exclude_keys: vec![],
            detection_byom_exclude_keys: vec![],
            detection_engine_env: vec![],
        };
        build_canonical_jobs(
            &fm,
            &extensions,
            &cfg,
            &ext_setup_steps,
            &ext_agent_prepare,
            &ext_agent_conditions,
            None,
        )
        .unwrap()
    }

    fn job_pool_by_id<'a>(jobs: &'a [super::super::ir::job::Job], id: &str) -> Option<&'a Pool> {
        jobs.iter().find(|j| j.id.as_ref() == id).map(|j| &j.pool)
    }

    #[test]
    fn threat_detection_enabled_and_disabled_match_expected_ir_graph() {
        use std::collections::{BTreeMap, BTreeSet};

        use super::super::ir::{
            Pipeline, PipelineBody, PipelineShape, Resources, Triggers, graph::build_graph,
        };

        let common = concat!(
            "name: test\n",
            "description: test\n",
            "safe-outputs:\n",
            "  require-approval: true\n",
            "  create-pull-request: {}\n",
            "  add-pr-comment:\n",
            "    require-approval: false\n",
        );
        let enabled = format!("---\n{common}  threat-detection: true\n---\nbody\n");
        let disabled = format!(
            "---\n{common}  threat-detection:\n    enabled: false\n    steps:\n      \
             - bash: echo should-not-run\n        displayName: SHOULD_NOT_RUN_PRE\n    \
             post-steps:\n      - bash: echo should-not-run\n        displayName: \
             SHOULD_NOT_RUN_POST\n---\nbody\n"
        );

        let enabled_jobs = build_jobs(&enabled);
        let disabled_jobs = build_jobs(&disabled);

        let graph_for = |jobs| {
            let pipeline = Pipeline {
                name: "test".to_string(),
                parameters: vec![],
                resources: Resources::default(),
                triggers: Triggers::default(),
                variables: vec![],
                body: PipelineBody::Jobs(jobs),
                shape: PipelineShape::Standalone,
            };
            build_graph(&pipeline).unwrap()
        };
        let enabled_graph = graph_for(enabled_jobs.clone());
        let disabled_graph = graph_for(disabled_jobs.clone());

        let job = |id: &str| JobId::new(id).unwrap();
        let expected_edges = BTreeSet::from([
            (job("Conclusion"), job("Agent")),
            (job("Conclusion"), job("Detection")),
            (job("Conclusion"), job("SafeOutputs")),
            (job("Conclusion"), job("SafeOutputs_Reviewed")),
            (job("Detection"), job("Agent")),
            (job("ManualReview"), job("Agent")),
            (job("ManualReview"), job("Detection")),
            (job("SafeOutputs"), job("Agent")),
            (job("SafeOutputs"), job("Detection")),
            (job("SafeOutputs_Reviewed"), job("Agent")),
            (job("SafeOutputs_Reviewed"), job("Detection")),
            (job("SafeOutputs_Reviewed"), job("ManualReview")),
        ]);
        let expected_outputs = BTreeMap::from([
            (
                StepId::new("reviewedProposals").unwrap(),
                BTreeSet::from(["HasReviewedProposals".to_string()]),
            ),
            (
                StepId::new("threatAnalysis").unwrap(),
                BTreeSet::from(["SafeToProcess".to_string()]),
            ),
        ]);

        for graph in [&enabled_graph, &disabled_graph] {
            assert_eq!(graph.job_edges, expected_edges);
            assert!(graph.stage_edges.is_empty());
            assert_eq!(graph.outputs_needing_is_output, expected_outputs);
            assert_eq!(graph.step_locations.len(), 2);
            for (step, outputs) in &expected_outputs {
                let location = graph.step_locations.get(step).unwrap();
                assert_eq!(location.job, job("Detection"));
                assert_eq!(&location.outputs, outputs);
            }
        }

        let disabled_detection = disabled_jobs
            .iter()
            .find(|job| job.id.as_ref() == "Detection")
            .unwrap();
        assert!(disabled_detection.steps.iter().any(|step| {
            matches!(step, Step::Bash(step) if step.display_name == "Bypass AI threat analysis")
        }));
        assert!(
            !disabled_detection.steps.iter().any(|step| {
                matches!(step, Step::RawYaml(raw) if raw.contains("SHOULD_NOT_RUN"))
            })
        );

        let enabled_detection = enabled_jobs
            .iter()
            .find(|job| job.id.as_ref() == "Detection")
            .unwrap();
        let reviewed_index = enabled_detection
            .steps
            .iter()
            .position(|step| {
                step.id()
                    .is_some_and(|id| id.as_ref() == "reviewedProposals")
            })
            .unwrap();
        let copy_logs_index = enabled_detection
            .steps
            .iter()
            .position(|step| {
                matches!(step, Step::Bash(step) if step.display_name == "Copy logs to output directory")
            })
            .unwrap();
        assert!(reviewed_index < copy_logs_index);
    }

    #[test]
    fn pool_overrides_detection_only_flows_to_compiled_job() {
        let source = concat!(
            "---\nname: test\ndescription: test\n",
            "pool:\n  name: SpecializedPool\n  overrides:\n    detection:\n      vmImage: ubuntu-22.04\n",
            "safe-outputs:\n  noop: {}\n",
            "---\nbody\n"
        );
        let jobs = build_jobs(source);
        // Agent → SpecializedPool
        assert_eq!(
            pool_name(job_pool_by_id(&jobs, "Agent").unwrap()),
            "SpecializedPool"
        );
        // Detection → ubuntu-22.04
        assert_eq!(
            pool_name(job_pool_by_id(&jobs, "Detection").unwrap()),
            "ubuntu-22.04"
        );
        // SafeOutputs → SpecializedPool (no override)
        assert_eq!(
            pool_name(job_pool_by_id(&jobs, "SafeOutputs").unwrap()),
            "SpecializedPool"
        );
    }

    #[test]
    fn pool_overrides_empty_does_not_change_default() {
        // pool: {overrides: {}} is identical to pool: with no overrides key.
        let with_overrides = concat!(
            "---\nname: test\ndescription: test\n",
            "pool:\n  vmImage: ubuntu-22.04\n  overrides: {}\n",
            "safe-outputs:\n  noop: {}\n",
            "---\nbody\n"
        );
        let without_overrides = concat!(
            "---\nname: test\ndescription: test\n",
            "pool:\n  vmImage: ubuntu-22.04\n",
            "safe-outputs:\n  noop: {}\n",
            "---\nbody\n"
        );
        let jobs_with = build_jobs(with_overrides);
        let jobs_without = build_jobs(without_overrides);
        for job_id in ["Agent", "Detection", "SafeOutputs"] {
            assert_eq!(
                job_pool_by_id(&jobs_with, job_id).unwrap(),
                job_pool_by_id(&jobs_without, job_id).unwrap(),
                "pool mismatch for job {job_id}"
            );
        }
    }

    #[test]
    fn pool_overrides_all_downstream_override() {
        let source = concat!(
            "---\nname: test\ndescription: test\n",
            "pool:\n  name: SpecializedPool\n",
            "  overrides:\n",
            "    detection:\n      vmImage: ubuntu-22.04\n",
            "    safe-outputs:\n      vmImage: ubuntu-22.04\n",
            "    conclusion:\n      vmImage: ubuntu-22.04\n",
            "safe-outputs:\n  noop: {}\n",
            "---\nbody\n"
        );
        let jobs = build_jobs(source);
        assert_eq!(
            pool_name(job_pool_by_id(&jobs, "Agent").unwrap()),
            "SpecializedPool"
        );
        assert_eq!(
            pool_name(job_pool_by_id(&jobs, "Detection").unwrap()),
            "ubuntu-22.04"
        );
        assert_eq!(
            pool_name(job_pool_by_id(&jobs, "SafeOutputs").unwrap()),
            "ubuntu-22.04"
        );
        assert_eq!(
            pool_name(job_pool_by_id(&jobs, "Conclusion").unwrap()),
            "ubuntu-22.04"
        );
    }
    // ─── build_agent_content: imported-body delivery ─────────────────────────

    #[test]
    fn build_agent_content_default_mode_inlines_imported_body_before_marker() {
        let fm = test_front_matter("name: t\ndescription: d\n");
        let out = build_agent_content(
            &fm,
            std::path::Path::new("agents/test.md"),
            // markdown_body (combined) is ignored in default mode.
            "IGNORED COMBINED BODY",
            "Imported guidance line.",
            "$(Build.SourcesDirectory)/agents/test.md",
            "$(Build.SourcesDirectory)",
        )
        .unwrap();
        assert_eq!(
            out,
            "Imported guidance line.\n\n{{#runtime-import agents/test.md}}"
        );
    }

    #[test]
    fn build_agent_content_default_mode_without_imports_is_marker_only() {
        let fm = test_front_matter("name: t\ndescription: d\n");
        let out = build_agent_content(
            &fm,
            std::path::Path::new("agents/test.md"),
            "IGNORED",
            "",
            "$(Build.SourcesDirectory)/agents/test.md",
            "$(Build.SourcesDirectory)",
        )
        .unwrap();
        assert_eq!(out, "{{#runtime-import agents/test.md}}");
    }

    #[test]
    fn build_agent_content_inlined_mode_uses_combined_body() {
        // In inlined mode the combined body (imported + consumer) is already in
        // markdown_body and is emitted verbatim; the separate
        // imported_prompt_body arg is not appended a second time.
        let fm = test_front_matter("name: t\ndescription: d\ninlined-imports: true\n");
        let out = build_agent_content(
            &fm,
            std::path::Path::new("agents/test.md"),
            "Imported guidance line.\n\nConsumer body.",
            "Imported guidance line.",
            "$(Build.SourcesDirectory)/agents/test.md",
            "$(Build.SourcesDirectory)",
        )
        .unwrap();
        assert_eq!(out, "Imported guidance line.\n\nConsumer body.");
    }

    // ─── `on.push` / trigger truth table ────────────────────────────────
    //
    // `on:` is the complete declaration of when a pipeline runs. Azure
    // DevOps reads a *missing* `trigger:` / `pr:` key as "run on every
    // branch", so the compiler always emits both keys — absence of an
    // `on.*` key means that trigger is explicitly disabled.

    fn triggers_for(yaml: &str) -> Triggers {
        let fm = test_front_matter(yaml);
        build_triggers(&fm.on_config, &fm).expect("build_triggers should succeed")
    }

    const BASE: &str = "name: t\ndescription: d\n";

    #[test]
    fn build_triggers_no_on_config_yields_manual_only_pipeline() {
        let t = triggers_for(BASE);
        assert!(
            t.ci.as_ref().expect("ci trigger emitted").disabled,
            "no `on:` must emit `trigger: none` — a pipeline that never self-starts"
        );
        assert!(
            t.pr.as_ref().expect("pr trigger emitted").disabled,
            "no `on:` must emit `pr: none`"
        );
        assert!(t.schedules.is_empty());
    }

    #[test]
    fn build_triggers_push_none_disables_ci() {
        let t = triggers_for(&format!("{BASE}on:\n  push: none\n"));
        assert!(t.ci.as_ref().unwrap().disabled);
    }

    #[test]
    fn build_triggers_push_filters_emit_branches_and_paths() {
        let t = triggers_for(&format!(
            "{BASE}on:\n  push:\n    branches:\n      include: [main]\n      exclude: [wip/*]\n    paths:\n      include: [\"src/**\"]\n      exclude: [\"docs/**\"]\n"
        ));
        let ci = t.ci.as_ref().unwrap();
        assert!(!ci.disabled);
        assert_eq!(ci.branches_include, vec!["main".to_string()]);
        assert_eq!(ci.branches_exclude, vec!["wip/*".to_string()]);
        assert_eq!(ci.paths_include, vec!["src/**".to_string()]);
        assert_eq!(ci.paths_exclude, vec!["docs/**".to_string()]);
    }

    #[test]
    fn build_triggers_synthetic_pr_emits_all_branches_ci_trigger() {
        // `mode: synthetic` (the default) resolves the open PR for
        // `Build.SourceBranch` at runtime, so it needs CI-triggered builds
        // to react to. The all-branches trigger is the MECHANISM that
        // delivers `on.pr`, not independent user intent.
        let t = triggers_for(&format!(
            "{BASE}on:\n  pr:\n    branches:\n      include: [main]\n"
        ));
        let ci = t.ci.as_ref().unwrap();
        assert!(
            !ci.disabled,
            "synthetic PR mode needs push-triggered builds"
        );
        assert_eq!(ci.branches_include, vec!["*".to_string()]);
        assert!(!t.pr.as_ref().unwrap().disabled);
    }

    #[test]
    fn build_triggers_policy_pr_mode_disables_ci() {
        // A Build Validation policy fires real PR builds, so a CI trigger
        // would only queue duplicate feature-branch builds alongside it.
        let t = triggers_for(&format!(
            "{BASE}on:\n  pr:\n    mode: policy\n    branches:\n      include: [main]\n"
        ));
        assert!(t.ci.as_ref().unwrap().disabled);
        assert!(!t.pr.as_ref().unwrap().disabled);
    }

    #[test]
    fn build_triggers_schedule_alone_disables_ci_and_pr() {
        let t = triggers_for(&format!("{BASE}on:\n  schedule: daily around 03:00\n"));
        assert!(t.ci.as_ref().unwrap().disabled);
        assert!(t.pr.as_ref().unwrap().disabled);
        assert_eq!(t.schedules.len(), 1);
    }

    #[test]
    fn build_triggers_explicit_push_wins_over_schedule() {
        // "Run nightly, and also whenever `main` moves" is a legitimate
        // shape — the schedule must not silently swallow `on.push`.
        let t = triggers_for(&format!(
            "{BASE}on:\n  schedule: daily around 03:00\n  push:\n    branches:\n      include: [main]\n"
        ));
        let ci = t.ci.as_ref().unwrap();
        assert!(
            !ci.disabled,
            "explicit `on.push` must override the schedule"
        );
        assert_eq!(ci.branches_include, vec!["main".to_string()]);
        assert_eq!(t.schedules.len(), 1);
        assert!(
            t.pr.as_ref().unwrap().disabled,
            "`on.push` controls only `trigger:` — `pr:` stays driven by `on.pr`"
        );
    }

    #[test]
    fn build_triggers_explicit_push_wins_over_policy_pr_mode() {
        let t = triggers_for(&format!(
            "{BASE}on:\n  pr:\n    mode: policy\n    branches:\n      include: [main]\n  push:\n    branches:\n      include: [main]\n"
        ));
        let ci = t.ci.as_ref().unwrap();
        assert!(!ci.disabled);
        assert_eq!(ci.branches_include, vec!["main".to_string()]);
    }

    #[test]
    fn build_triggers_explicit_push_none_wins_over_synthetic_pr_mode() {
        // Opting out of push builds defeats synthetic PR resolution, but
        // it is what the author asked for and must not be second-guessed.
        let t = triggers_for(&format!(
            "{BASE}on:\n  push: none\n  pr:\n    branches:\n      include: [main]\n"
        ));
        assert!(t.ci.as_ref().unwrap().disabled);
        assert!(!t.pr.as_ref().unwrap().disabled);
    }

    #[test]
    fn build_triggers_empty_push_mapping_means_every_branch() {
        // `push: {}` carries no filter information; emitting `trigger: {}`
        // would be invalid ADO, so it degrades to the all-branches form.
        let t = triggers_for(&format!("{BASE}on:\n  push: {{}}\n"));
        let ci = t.ci.as_ref().unwrap();
        assert!(!ci.disabled);
        assert_eq!(ci.branches_include, vec!["*".to_string()]);
    }

    #[test]
    fn on_push_rejects_unknown_scalar_with_actionable_message() {
        // `push` is an untagged enum; serde's default failure message is
        // "data did not match any variant", so `expecting` supplies a hint.
        let err = serde_yaml::from_str::<FrontMatter>(&format!("{BASE}on:\n  push: always\n"))
            .expect_err("`push: always` must not parse");
        let msg = err.to_string();
        assert!(
            msg.contains("none") && msg.contains("branches"),
            "parse error should name the accepted shapes, got: {msg}"
        );
    }

    // ─── copilot_byom_exclude_keys ────────────────────────────────────────────

    #[test]
    fn copilot_byom_exclude_keys_non_copilot_always_empty() {
        // is_copilot = false must short-circuit to [] regardless of engine
        // config — even when the engine env contains BYOM credential keys or a
        // provider token is configured. This prevents a future non-Copilot
        // engine whose env happens to contain COPILOT_PROVIDER_* keys from
        // accidentally leaking those keys into AWF --exclude-env.
        let fm_with_keys = test_front_matter(
            "name: t\ndescription: d\nengine:\n  id: copilot\n  env:\n    COPILOT_PROVIDER_API_KEY: sk-123\n",
        );
        assert!(
            copilot_byom_exclude_keys(false, &fm_with_keys.engine).is_empty(),
            "non-copilot must return empty even when BYOM credential env keys are present"
        );

        // Also verify that a provider-token-configured engine is still excluded
        // when is_copilot is false.
        let fm_with_token = test_front_matter(
            "name: t\ndescription: d\nengine:\n  id: copilot\n  provider:\n    base-url: https://example.com/v1\n    token:\n      service-connection: sc\n",
        );
        assert!(
            copilot_byom_exclude_keys(false, &fm_with_token.engine).is_empty(),
            "non-copilot must return empty even when provider.token is configured"
        );
    }

    #[test]
    fn copilot_byom_exclude_keys_copilot_no_credentials_empty() {
        // A plain Copilot engine with no provider config at all produces no
        // --exclude-env keys.
        let fm = test_front_matter("name: t\ndescription: d\n");
        assert!(
            copilot_byom_exclude_keys(true, &fm.engine).is_empty(),
            "default copilot engine should produce no exclude keys"
        );

        // An engine with only non-credential COPILOT_PROVIDER_WIRE_API should
        // also produce no exclude keys (WIRE_API is config, not a credential).
        let fm_wire = test_front_matter(
            "name: t\ndescription: d\nengine:\n  id: copilot\n  env:\n    COPILOT_PROVIDER_WIRE_API: responses\n",
        );
        assert!(
            copilot_byom_exclude_keys(true, &fm_wire.engine).is_empty(),
            "COPILOT_PROVIDER_WIRE_API alone must not produce exclude keys"
        );
    }

    #[test]
    fn copilot_byom_exclude_keys_copilot_env_credential_keys_no_token() {
        // When BYOM credential env keys are present but no provider.token is
        // configured, the helper returns exactly those keys (sorted), with no
        // AW_PROVIDER_BEARER_TOKEN appended.
        let fm = test_front_matter(
            "name: t\ndescription: d\nengine:\n  id: copilot\n  env:\n    COPILOT_PROVIDER_BASE_URL: https://example.com/v1\n    COPILOT_PROVIDER_API_KEY: sk-abc\n",
        );
        let keys = copilot_byom_exclude_keys(true, &fm.engine);
        assert_eq!(
            keys,
            vec![
                "COPILOT_PROVIDER_API_KEY".to_string(),
                "COPILOT_PROVIDER_BASE_URL".to_string(),
            ],
            "should return sorted credential keys only, with no bearer-token var appended"
        );
    }

    #[test]
    fn copilot_byom_exclude_keys_copilot_provider_token_includes_derived_and_bearer_var() {
        // When provider.token is set, the compiler derives COPILOT_PROVIDER_BASE_URL
        // (from provider.base-url) and COPILOT_PROVIDER_API_KEY (because the minted
        // AW_PROVIDER_BEARER_TOKEN is wired into that slot) — these appear as
        // credential keys from copilot_byom_credential_keys. On top of those,
        // AW_PROVIDER_BEARER_TOKEN is appended by the helper because provider.token
        // is present.
        let fm = test_front_matter(
            "name: t\ndescription: d\nengine:\n  id: copilot\n  provider:\n    base-url: https://example.com/v1\n    token:\n      service-connection: sc\n",
        );
        let keys = copilot_byom_exclude_keys(true, &fm.engine);
        assert_eq!(
            keys,
            vec![
                "COPILOT_PROVIDER_API_KEY".to_string(),
                "COPILOT_PROVIDER_BASE_URL".to_string(),
                crate::compile::types::PROVIDER_BEARER_TOKEN_VAR.to_string(),
            ],
            "provider.token should produce derived credential keys + AW_PROVIDER_BEARER_TOKEN"
        );
    }

    #[test]
    fn copilot_byom_exclude_keys_copilot_provider_api_key_no_bearer_token_var() {
        // When provider.api-key (static key) is used instead of provider.token,
        // AW_PROVIDER_BEARER_TOKEN must NOT be appended. The helper only appends
        // that var when provider.token is present because only then does the compiler
        // mint the same-job secret that needs to be excluded from AWF --env-all.
        let fm = test_front_matter(
            "name: t\ndescription: d\nengine:\n  id: copilot\n  provider:\n    base-url: https://example.com/v1\n    api-key: $(FOUNDRY_KEY)\n",
        );
        let keys = copilot_byom_exclude_keys(true, &fm.engine);
        assert_eq!(
            keys,
            vec![
                "COPILOT_PROVIDER_API_KEY".to_string(),
                "COPILOT_PROVIDER_BASE_URL".to_string(),
            ],
            "provider.api-key (no token) must not append AW_PROVIDER_BEARER_TOKEN"
        );
    }
}
