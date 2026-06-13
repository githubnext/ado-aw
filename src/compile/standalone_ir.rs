//! Typed-IR builder for the standalone compile target.
//!
//! This module replaces `src/data/base.yml` for the standalone
//! pipeline shape: instead of interpolating values into a YAML
//! string template, [`build_standalone_pipeline`] composes a typed
//! [`Pipeline`] programmatically that the [`crate::compile::ir::lower`]
//! pass serialises.
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
//! The standalone pipeline always has:
//!
//! - `Setup` (optional): user `setup:` steps + extension setup steps.
//!   Emitted when filters / synthPr / user setup are present.
//! - `Agent`: extensions + the static AWF / MCPG / agent-run scaffold.
//! - `Detection`: threat-analysis pass that produces the
//!   `threatAnalysis.SafeToProcess` output.
//! - `SafeOutputs`: gated on Detection's `SafeToProcess` output via
//!   typed [`Condition::Eq`] over a typed
//!   [`crate::compile::ir::output::OutputRef`]. The lowering pass
//!   picks `dependencies.Detection.outputs['threatAnalysis.SafeToProcess']`
//!   — first production use of typed cross-job OutputRef in a
//!   condition.
//! - `Teardown` (optional): user `teardown:` steps.

use anyhow::Result;
use std::path::Path;

use super::common::{
    self, ADO_BUILD_ID_SUFFIX, AWF_VERSION, HEADER_MARKER, MCPG_DOMAIN, MCPG_IMAGE, MCPG_PORT,
    MCPG_VERSION,
};
use super::extensions::{CompileContext, CompilerExtension, Declarations, Extension, McpgConfig};
use super::ir::condition::{Condition, Expr};
use super::ir::ids::{JobId, StepId};
use super::ir::job::{Job, Pool};
use super::ir::output::{OutputDecl, OutputRef};
use super::ir::step::{
    BashStep, CheckoutRepo, CheckoutStep, DownloadStep, PublishStep, Step, SubmodulesOpt, TaskStep,
};
use super::ir::{
    CiTrigger, Parameter, ParameterDefault, ParameterKind, Pipeline, PipelineBody,
    PipelineResource, PipelineShape, PipelineVar, PrTrigger, RepositoryResource, Resources,
    Schedule, Triggers,
};
use super::types::{FrontMatter, OnConfig, PrMode, Repository as RepoCfg};

// Suppress unused; this module is wired up in a sibling commit.
#[allow(unused_imports)]
use super::common::{generate_acquire_ado_token, generate_executor_ado_env};

/// Build the typed [`Pipeline`] for the standalone target.
///
/// Mirrors the flow of `compile_shared` but composes a typed IR
/// instead of templating a YAML string. Callers thread the result
/// through [`crate::compile::ir::emit::emit`] to produce the final
/// YAML.
/// Build the typed [`Pipeline`] for the standalone target.
///
/// Mirrors the flow of `compile_shared` but composes a typed IR
/// instead of templating a YAML string. Callers thread the result
/// through [`crate::compile::ir::emit::emit`] to produce the final
/// YAML.
#[allow(clippy::too_many_arguments)]
pub fn build_standalone_pipeline(
    front_matter: &FrontMatter,
    extensions: &[Extension],
    ctx: &CompileContext<'_>,
    input_path: &Path,
    output_path: &Path,
    markdown_body: &str,
    skip_integrity: bool,
    debug_pipeline: bool,
) -> Result<Pipeline> {
    let built = build_pipeline_context(
        front_matter,
        extensions,
        ctx,
        input_path,
        output_path,
        markdown_body,
        skip_integrity,
        debug_pipeline,
        None,
    )?;
    Ok(Pipeline {
        name: built.pipeline_name,
        parameters: built.parameters,
        resources: built.resources,
        triggers: built.triggers,
        variables: Vec::new(),
        body: PipelineBody::Jobs(built.jobs),
        shape: PipelineShape::Standalone,
    })
}

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

/// Shared back-end for the three IR-driven target compilers
/// (standalone / stage / job). Performs all the heavy lifting:
/// validates the front matter, computes every scalar, fans out
/// extension declarations, builds the canonical 5-job graph with the
/// optional `prefix`, and returns the per-target wrap inputs.
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
    // ─── Validations (reuse all shared validators) ────────────────
    common::validate_front_matter_identity(front_matter)?;
    common::validate_checkout_self_collision(
        &front_matter.repositories,
        &front_matter.checkout,
        ctx.ado_context.as_ref().map(|c| c.repo_name.as_str()),
    )?;
    common::validate_safe_outputs_keys(front_matter)?;
    common::validate_comment_target(front_matter)?;
    common::validate_update_work_item_target(front_matter)?;
    common::validate_submit_pr_review_events(front_matter)?;
    common::validate_update_pr_votes(front_matter)?;
    common::validate_resolve_pr_thread_statuses(front_matter)?;
    common::validate_ado_aw_debug_config(front_matter)?;

    let mut extension_declarations = Vec::with_capacity(extensions.len());
    for ext in extensions {
        let decl = ext.declarations(ctx)?;
        for warning in &decl.warnings {
            eprintln!("Warning: {}", warning);
        }
        extension_declarations.push(decl);
    }

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
    let pool = common::resolve_pool_typed(front_matter.target.clone(), front_matter.pool.as_ref())?;

    let compiler_version = env!("CARGO_PKG_VERSION").to_string();

    let engine_run = ctx.engine.invocation(
        ctx.front_matter,
        &extension_declarations,
        "/tmp/awf-tools/agent-prompt.md",
        Some("/tmp/awf-tools/mcp-config.json"),
    )?;
    let engine_run_detection = ctx.engine.invocation(
        ctx.front_matter,
        &extension_declarations,
        "/tmp/awf-tools/threat-analysis-prompt.md",
        None,
    )?;
    let engine_install_steps_yaml =
        ctx.engine
            .install_steps(&front_matter.engine, &front_matter.target, ctx.ado_org())?;
    let engine_log_dir = ctx.engine.log_dir().to_string();

    let mut engine_env = ctx.engine.env(&front_matter.engine)?;
    // AWF path env (when extensions declare path prepends)
    let awf_paths = common::collect_awf_path_prepends(&extension_declarations);
    let has_awf_paths = !awf_paths.is_empty();
    let awf_path_env = common::generate_awf_path_env(has_awf_paths);
    if !awf_path_env.is_empty() {
        engine_env = format!("{engine_env}\n{awf_path_env}");
    }
    let agent_env = common::collect_agent_env_vars(extensions, &extension_declarations)?;
    if !agent_env.is_empty() {
        engine_env = format!("{engine_env}\n{agent_env}");
    }

    // AWF mounts + allowlist
    let allowed_domains =
        common::generate_allowed_domains(front_matter, extensions, &extension_declarations)?;
    let awf_mounts = common::generate_awf_mounts(extensions, &extension_declarations);
    let awf_path_step_yaml = common::generate_awf_path_step(&awf_paths);
    let enabled_tools_args = common::generate_enabled_tools_args(front_matter);

    // MCPG config
    let mcpg_config_obj = common::generate_mcpg_config(front_matter, &extension_declarations)?;
    let mcpg_config_json = serde_json::to_string_pretty(&mcpg_config_obj)
        .map_err(|e| anyhow::anyhow!("Failed to serialize MCPG config: {e}"))?;
    let mcpg_docker_env = common::generate_mcpg_docker_env(front_matter, &extension_declarations);
    let mcpg_step_env = common::generate_mcpg_step_env(&extension_declarations);

    // Source / pipeline paths (for integrity check + metadata).
    // `source_path` embeds `{{ trigger_repo_directory }}` which the
    // legacy template fold substitutes — do the same eagerly so step
    // bodies receive a fully-resolved scalar.
    let source_path_raw = common::generate_source_path(input_path);
    let source_path =
        source_path_raw.replace("{{ trigger_repo_directory }}", &trigger_repo_directory);
    let pipeline_path = common::generate_pipeline_path(output_path);

    // Read / write tokens
    let acquire_read_token = common::generate_acquire_ado_token(
        front_matter
            .permissions
            .as_ref()
            .and_then(|p| p.read.as_deref()),
        "SC_READ_TOKEN",
    );
    let acquire_write_token = common::generate_acquire_ado_token(
        front_matter
            .permissions
            .as_ref()
            .and_then(|p| p.write.as_deref()),
        "SC_WRITE_TOKEN",
    );
    let executor_ado_env = common::generate_executor_ado_env(
        front_matter
            .permissions
            .as_ref()
            .and_then(|p| p.write.as_deref()),
        common::debug_create_issue_enabled(front_matter),
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
        &source_path,
        &trigger_repo_directory,
    )?;

    // ─── Top-level pipeline fields ────────────────────────────────
    let parameters = build_parameters(front_matter)?;
    let resources = build_resources(&front_matter.repositories, &front_matter.on_config);
    let triggers = build_triggers(&front_matter.on_config, front_matter)?;

    // ─── Extension declaration fanout ─────────────────────────────
    let mut ext_setup_steps: Vec<Step> = Vec::new();
    let mut ext_agent_prepare: Vec<Step> = Vec::new();
    for (ext, decl) in extensions.iter().zip(extension_declarations) {
        ext_setup_steps.extend(decl.setup_steps);
        ext_agent_prepare.extend(decl.agent_prepare_steps);
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

    // Aggregate config for per-job builders
    let cfg = StandaloneCtx {
        pool: pool.clone(),
        agent_display_name: agent_display_name.clone(),
        working_directory: working_directory.clone(),
        trigger_repo_directory: trigger_repo_directory.clone(),
        compiler_version: compiler_version.clone(),
        engine_install_steps_yaml,
        engine_run,
        engine_run_detection,
        engine_env,
        engine_log_dir,
        allowed_domains,
        awf_mounts,
        awf_path_step_yaml,
        enabled_tools_args,
        mcpg_config_json,
        mcpg_docker_env,
        mcpg_step_env,
        source_path,
        pipeline_path: pipeline_path.clone(),
        acquire_read_token,
        acquire_write_token,
        executor_ado_env,
        integrity_check_yaml,
        agent_content_value,
        debug_pipeline,
    };

    // ─── Build jobs ───────────────────────────────────────────────
    let jobs = build_canonical_jobs(
        front_matter,
        extensions,
        &cfg,
        &ext_setup_steps,
        &ext_agent_prepare,
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

/// Build the canonical 5-job graph (Setup?, Agent, Detection,
/// SafeOutputs, Teardown?) used by every target. The optional
/// `prefix` is applied to Agent / Detection / SafeOutputs job IDs
/// (matches the legacy template behaviour: Setup and Teardown stay
/// unprefixed even in `target: job|stage`, see `src/data/job-base.yml`
/// where `{{ setup_job }}` substitutes a literal `- job: Setup`).
///
/// Returns jobs with their cross-job `depends_on` edges wired to the
/// correct (possibly prefixed) names.
pub(crate) fn build_canonical_jobs(
    front_matter: &FrontMatter,
    extensions: &[Extension],
    cfg: &StandaloneCtx,
    ext_setup_steps: &[Step],
    ext_agent_prepare: &[Step],
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
        cfg,
        &p,
    )?);
    jobs.push(build_detection_job(front_matter, cfg, &p)?);
    jobs.push(build_safeoutputs_job(front_matter, cfg, &p)?);
    if let Some(teardown) = build_teardown_job(front_matter, cfg, &p)? {
        jobs.push(teardown);
    }

    // Wire dependsOn between jobs (graph pass also derives but
    // explicit edges make the YAML match committed lock files).
    wire_explicit_dependencies(&mut jobs, &p)?;
    Ok(jobs)
}

/// Job-id prefix helper. Encapsulates the legacy-template quirk that
/// Setup and Teardown jobs stay unprefixed even when other jobs in
/// the same target are prefixed by `generate_stage_prefix`.
pub(crate) struct JobPrefix<'a>(pub Option<&'a str>);

impl<'a> JobPrefix<'a> {
    /// Produce the `JobId` for a canonical job (`Setup` / `Agent` /
    /// `Detection` / `SafeOutputs` / `Teardown`). Setup and Teardown
    /// are always unprefixed; the other three are prefixed when a
    /// prefix is provided.
    pub(crate) fn id(&self, base: &str) -> Result<JobId> {
        match (self.0, base) {
            (Some(prefix), "Agent" | "Detection" | "SafeOutputs") => {
                JobId::new(format!("{prefix}_{base}"))
            }
            _ => JobId::new(base),
        }
    }
}

/// Aggregates the precomputed scalars + YAML fragments threaded into
/// every per-job builder. Lives only inside this module; passed by
/// reference so builders don't take 20+ args each.
pub(crate) struct StandaloneCtx {
    pub(crate) pool: Pool,
    pub(crate) agent_display_name: String,
    pub(crate) working_directory: String,
    pub(crate) trigger_repo_directory: String,
    pub(crate) compiler_version: String,
    /// Engine install steps as a YAML string (`Engine::install_steps`
    /// returns YAML today). Lowered through `Step::RawYaml` because
    /// it is opaque user-authored-shaped content from the engine
    /// impl. A future `Engine::install_steps_typed` would lift this
    /// to typed steps.
    pub(crate) engine_install_steps_yaml: String,
    pub(crate) engine_run: String,
    pub(crate) engine_run_detection: String,
    /// Composed engine env block — `KEY: VALUE` lines, one per line.
    /// Carried as a string and re-parsed during step emission.
    pub(crate) engine_env: String,
    pub(crate) engine_log_dir: String,
    pub(crate) allowed_domains: String,
    /// `--mount` flags for AWF (or `\` placeholder when no mounts).
    pub(crate) awf_mounts: String,
    /// `awf_path_step` YAML body (or empty when no path prepends).
    pub(crate) awf_path_step_yaml: String,
    /// `--enabled-tools` args for SafeOutputs HTTP server (with trailing space).
    pub(crate) enabled_tools_args: String,
    pub(crate) mcpg_config_json: String,
    /// `-e KEY=...` docker flags for MCPG.
    pub(crate) mcpg_docker_env: String,
    /// `env:` block for the MCPG step (`env:\n  KEY: ...`).
    pub(crate) mcpg_step_env: String,
    pub(crate) source_path: String,
    pub(crate) pipeline_path: String,
    /// `AzureCLI@2` task YAML body (or empty when no read service connection).
    pub(crate) acquire_read_token: String,
    pub(crate) acquire_write_token: String,
    /// `env:` block for executor step (always non-empty — has
    /// SYSTEM_ACCESSTOKEN at minimum).
    pub(crate) executor_ado_env: String,
    /// `Verify pipeline integrity` step YAML (or empty when skipped).
    pub(crate) integrity_check_yaml: String,
    /// Agent prompt body (either inlined imports or
    /// `{{#runtime-import ...}}` marker).
    pub(crate) agent_content_value: String,
    pub(crate) debug_pipeline: bool,
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

fn build_resources(repos: &[RepoCfg], on: &Option<OnConfig>) -> Resources {
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
    Resources {
        repositories,
        pipelines,
    }
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

    let has_schedule = !schedules.is_empty();
    let has_pipeline_trigger = on.as_ref().and_then(|t| t.pipeline.as_ref()).is_some();

    // PR trigger — three branches mirroring `generate_pr_trigger`:
    //   - explicit `triggers.pr` override → typed PrTrigger { disabled: false, … }
    //   - suppression (pipeline or schedule configured) → pr: none
    //   - otherwise → no key (None)
    let pr = match on.as_ref().and_then(|o| o.pr.as_ref()) {
        Some(pr_cfg) => Some(build_pr_trigger_from_config(pr_cfg)),
        None => {
            if has_pipeline_trigger || has_schedule {
                Some(PrTrigger {
                    branches_include: Vec::new(),
                    branches_exclude: Vec::new(),
                    paths_include: Vec::new(),
                    paths_exclude: Vec::new(),
                    disabled: true,
                })
            } else {
                None
            }
        }
    };

    // CI trigger — `trigger: none` when pipeline/schedule or policy mode active.
    let ci = if has_pipeline_trigger || has_schedule {
        Some(CiTrigger { disabled: true })
    } else if let Some(pr_cfg) = on.as_ref().and_then(|o| o.pr.as_ref())
        && matches!(pr_cfg.mode, PrMode::Policy)
    {
        Some(CiTrigger { disabled: true })
    } else {
        None
    };

    // Pipeline resources — none for standalone today (handled via legacy
    // generate_pipeline_resources but standalone fixtures don't exercise it).
    Ok(Triggers { schedules, pr, ci })
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
    steps.push(checkout_self_step());
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

    let mut job = Job::new(prefix.id("Setup")?, "Setup", cfg.pool.clone());
    job.steps = steps;
    Ok(Some(job))
}

fn build_agent_job(
    front_matter: &FrontMatter,
    extensions: &[Extension],
    ext_agent_prepare: &[Step],
    cfg: &StandaloneCtx,
    prefix: &JobPrefix<'_>,
) -> Result<Job> {
    let mut steps: Vec<Step> = Vec::new();

    // 1. checkout: self
    steps.push(checkout_self_step());
    // 2. additional repo checkouts
    for repo in &front_matter.checkout {
        steps.push(Step::Checkout(CheckoutStep {
            repository: CheckoutRepo::Named(repo.clone()),
            clean: None,
            submodules: None,
            fetch_depth: None,
            persist_credentials: None,
        }));
    }

    // 3. acquire ADO read token (AzureCLI@2 task) — only when configured.
    push_raw_yaml_if_nonempty(&mut steps, &cfg.acquire_read_token);

    // 4. engine install steps (Copilot CLI install). YAML string from
    //    `Engine::install_steps`; lowered through `Step::RawYaml`
    //    until a typed `Engine::install_steps_typed` lands.
    push_raw_yaml_if_nonempty(&mut steps, &cfg.engine_install_steps_yaml);

    // 5. Download agentic pipeline compiler
    steps.push(Step::Bash(download_compiler_step(&cfg.compiler_version)));

    // 6. Integrity check (when not skipped)
    push_raw_yaml_if_nonempty(
        &mut steps,
        &substitute_integrity_check(
            &cfg.integrity_check_yaml,
            &cfg.pipeline_path,
            &cfg.trigger_repo_directory,
        ),
    );

    // 7. Prepare tooling (generates MCPG API key, writes MCPG config to staging)
    steps.push(Step::Bash(prepare_mcpg_config_step(&cfg.mcpg_config_json)));

    // 8. Prepare tooling - copy binary + config to /tmp
    steps.push(Step::Bash(prepare_tooling_step()));

    // 9. Prepare agent prompt (heredoc)
    steps.push(Step::Bash(prepare_agent_prompt_step(
        &cfg.agent_content_value,
    )?));

    // 10. DockerInstaller@0
    steps.push(Step::Task(
        TaskStep::new("DockerInstaller@0", "Install Docker").with_input("dockerVersion", "26.1.4"),
    ));

    // 11. Download AWF
    steps.push(Step::Bash(download_awf_step()));

    // 12. Pre-pull AWF + MCPG container images
    steps.push(Step::Bash(prepull_images_step(true)));

    // 13. Extension prepare steps (typed) + user steps (RawYaml)
    steps.extend(ext_agent_prepare.iter().cloned());
    for user_step_val in &front_matter.steps {
        steps.push(Step::RawYaml(step_to_raw_yaml_string(user_step_val)?));
    }

    // 14. AWF path step (when extensions declare path prepends)
    push_raw_yaml_if_nonempty(&mut steps, &cfg.awf_path_step_yaml);

    // 15. SafeOutputs HTTP server
    steps.push(Step::Bash(start_safeoutputs_server_step(
        &cfg.enabled_tools_args,
        &cfg.working_directory,
    )));

    // 16. MCP Gateway (MCPG)
    steps.push(Step::Bash(start_mcpg_step(
        &cfg.mcpg_docker_env,
        &cfg.mcpg_step_env,
        cfg.debug_pipeline,
    )));

    // 17. Verify MCP backends (debug-only)
    if cfg.debug_pipeline {
        steps.push(Step::Bash(verify_mcp_backends_step()));
    }

    // 18. Run copilot (AWF network isolated) — the big one
    steps.push(Step::Bash(run_agent_step(
        &cfg.allowed_domains,
        &cfg.awf_mounts,
        &cfg.working_directory,
        &cfg.engine_run,
        &cfg.engine_env,
    )));

    // 19. Collect safe outputs from AWF container
    steps.push(Step::Bash(collect_safe_outputs_step()));

    // 20. Stop MCPG and SafeOutputs
    steps.push(Step::Bash(stop_mcpg_step()));

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
    let mut job = Job::new(prefix.id("Agent")?, "Agent", cfg.pool.clone());
    if let Some(minutes) = front_matter.engine.timeout_minutes() {
        job.timeout = Some(std::time::Duration::from_secs(60 * (minutes as u64)));
    }
    job.steps = steps;
    job.variables = agent_job_variables_hoist(front_matter)?;

    // Agent-job condition: when PR/pipeline filters or synthetic-PR
    // are active, the agent must wait on Setup-job gate outputs.
    // Mirrors legacy `generate_agentic_depends_on` for standalone.
    if let Some(cond) = build_agentic_condition(front_matter) {
        job.condition = Some(cond);
    }
    Ok(job)
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
    use crate::compile::ir::job::JobVariable;
    use crate::compile::ir::output::OutputRef;

    if !front_matter.is_synthetic_pr() {
        return Ok(Vec::new());
    }
    let synth = StepId::new("synthPr")?;
    let mut out: Vec<JobVariable> = Vec::new();
    for name in &[
        "AW_PR_ID",
        "AW_PR_TARGETBRANCH",
        "AW_PR_SOURCEBRANCH",
        "AW_SYNTHETIC_PR",
    ] {
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

/// Build the typed Agent-job condition mirroring
/// `common::generate_agentic_depends_on` for the standalone target.
///
/// Encodes the same semantics:
/// - When `synthetic_pr_active`, honour the Setup-job
///   `synthPr.AW_SYNTHETIC_PR_SKIP=true` self-skip signal.
/// - When `has_pr_filters`, REQUIRE the `prGate.SHOULD_RUN=true`
///   output for any build that is a real PR OR a synth-promoted
///   build; otherwise (non-PR, non-synth) bypass the gate.
/// - When `has_pipeline_filters`, REQUIRE the
///   `pipelineGate.SHOULD_RUN=true` output for `ResourceTrigger`
///   builds; otherwise bypass.
/// - User filter `expression:` escape hatches are AND-ed in as
///   `Condition::Custom` atoms (their injection-vector check applies
///   at codegen time).
fn build_agentic_condition(front_matter: &FrontMatter) -> Option<Condition> {
    let pr_filters = front_matter.pr_filters();
    let pipeline_filters = front_matter.pipeline_filters();
    let has_pr_filters = pr_filters
        .map(|f| !super::filter_ir::lower_pr_filters(f).is_empty())
        .unwrap_or(false);
    let has_pipeline_filters = pipeline_filters
        .map(|f| !super::filter_ir::lower_pipeline_filters(f).is_empty())
        .unwrap_or(false);
    let synthetic_pr_active = front_matter.is_synthetic_pr();
    let pr_expression = pr_filters.and_then(|f| f.expression.as_deref());
    let pipeline_expression = pipeline_filters.and_then(|f| f.expression.as_deref());
    let has_expressions = pr_expression.is_some() || pipeline_expression.is_some();

    if !has_pr_filters && !has_pipeline_filters && !synthetic_pr_active && !has_expressions {
        return None;
    }

    let mut parts: Vec<Condition> = vec![Condition::Succeeded];

    if synthetic_pr_active {
        // ne(dependencies.Setup.outputs['synthPr.AW_SYNTHETIC_PR_SKIP'], 'true')
        parts.push(Condition::Custom(
            "ne(dependencies.Setup.outputs['synthPr.AW_SYNTHETIC_PR_SKIP'], 'true')".to_string(),
        ));
    }

    if has_pr_filters {
        if synthetic_pr_active {
            // or(
            //   and(
            //     ne(variables['Build.Reason'], 'PullRequest'),
            //     ne(dependencies.Setup.outputs['synthPr.AW_SYNTHETIC_PR'], 'true')
            //   ),
            //   eq(dependencies.Setup.outputs['prGate.SHOULD_RUN'], 'true')
            // )
            parts.push(Condition::Custom(
                "or(and(ne(variables['Build.Reason'], 'PullRequest'), ne(dependencies.Setup.outputs['synthPr.AW_SYNTHETIC_PR'], 'true')), eq(dependencies.Setup.outputs['prGate.SHOULD_RUN'], 'true'))"
                    .to_string(),
            ));
        } else {
            parts.push(Condition::Custom(
                "or(ne(variables['Build.Reason'], 'PullRequest'), eq(dependencies.Setup.outputs['prGate.SHOULD_RUN'], 'true'))"
                    .to_string(),
            ));
        }
    }

    if has_pipeline_filters {
        parts.push(Condition::Custom(
            "or(ne(variables['Build.Reason'], 'ResourceTrigger'), eq(dependencies.Setup.outputs['pipelineGate.SHOULD_RUN'], 'true'))"
                .to_string(),
        ));
    }

    if let Some(e) = pr_expression {
        parts.push(Condition::Custom(e.to_string()));
    }
    if let Some(e) = pipeline_expression {
        parts.push(Condition::Custom(e.to_string()));
    }

    Some(Condition::And(parts))
}

fn build_detection_job(
    front_matter: &FrontMatter,
    cfg: &StandaloneCtx,
    prefix: &JobPrefix<'_>,
) -> Result<Job> {
    let mut steps: Vec<Step> = Vec::new();
    steps.push(checkout_self_step());
    // Detection job pulls the Agent's output artifact via cross-job download
    steps.push(Step::Download(DownloadStep {
        source: "current".to_string(),
        artifact: "agent_outputs_$(Build.BuildId)".to_string(),
        condition: None,
    }));

    // Engine install
    push_raw_yaml_if_nonempty(&mut steps, &cfg.engine_install_steps_yaml);
    // Download compiler
    steps.push(Step::Bash(download_compiler_step(&cfg.compiler_version)));
    // DockerInstaller
    steps.push(Step::Task(
        TaskStep::new("DockerInstaller@0", "Install Docker").with_input("dockerVersion", "26.1.4"),
    ));
    // Download AWF
    steps.push(Step::Bash(download_awf_step()));
    // Pre-pull AWF (no MCPG image for detection)
    steps.push(Step::Bash(prepull_images_step(false)));
    // Prepare safe outputs for analysis
    steps.push(Step::Bash(prepare_safe_outputs_for_analysis(
        &cfg.working_directory,
    )));
    // Prepare threat analysis prompt
    // include_str! may carry CRLF line endings on Windows; normalise to LF
    // so the resulting block scalar emits cleanly. Then substitute the
    // template markers the threat prompt embeds (source_path, agent_name,
    // agent_description, working_directory) — these match the legacy
    // template fold's behaviour.
    let threat_prompt_raw = include_str!("../data/threat-analysis.md");
    let threat_prompt = threat_prompt_raw
        .replace("\r\n", "\n")
        .replace("{{ source_path }}", &cfg.source_path)
        .replace("{{ agent_name }}", &cfg.agent_display_name)
        .replace("{{ agent_description }}", &front_matter.description)
        .replace("{{ working_directory }}", &cfg.working_directory);
    steps.push(Step::Bash(prepare_threat_analysis_prompt_step(
        &threat_prompt,
    )?));
    // Setup compiler
    steps.push(Step::Bash(setup_compiler_step()));
    // Run threat analysis
    steps.push(Step::Bash(run_threat_analysis_step(
        &cfg.allowed_domains,
        &cfg.working_directory,
        &cfg.engine_run_detection,
    )));
    // Prepare analyzed outputs
    steps.push(Step::Bash(prepare_analyzed_outputs_step()));
    // Evaluate threat analysis — DECLARES TYPED OUTPUT
    steps.push(Step::Bash(evaluate_threat_analysis_step()));
    // Copy logs
    steps.push(Step::Bash(copy_logs_step(&cfg.engine_log_dir, true)));
    // Publish
    steps.push(Step::Publish(PublishStep {
        path: "$(Agent.TempDirectory)/analyzed_outputs".to_string(),
        artifact: "analyzed_outputs_$(Build.BuildId)".to_string(),
        condition: Some(Condition::Always),
    }));

    let mut job = Job::new(prefix.id("Detection")?, "Detection", cfg.pool.clone());
    job.steps = steps;
    Ok(job)
}

fn build_safeoutputs_job(
    _front_matter: &FrontMatter,
    cfg: &StandaloneCtx,
    prefix: &JobPrefix<'_>,
) -> Result<Job> {
    let mut steps: Vec<Step> = Vec::new();
    steps.push(checkout_self_step());
    // Acquire write token (when configured)
    push_raw_yaml_if_nonempty(&mut steps, &cfg.acquire_write_token);
    // Download analyzed outputs
    steps.push(Step::Download(DownloadStep {
        source: "current".to_string(),
        artifact: "analyzed_outputs_$(Build.BuildId)".to_string(),
        condition: None,
    }));
    // Download compiler
    steps.push(Step::Bash(download_compiler_step(&cfg.compiler_version)));
    // Add compiler to path
    steps.push(Step::Bash(bash(
        "Add agentic compiler to path",
        "ls -la \"$(Pipeline.Workspace)/agentic-pipeline-compiler\"\n\
         chmod +x \"$(Pipeline.Workspace)/agentic-pipeline-compiler/ado-aw\"\n\
         echo \"##vso[task.prependpath]$(Pipeline.Workspace)/agentic-pipeline-compiler\"\n",
    )));
    // Prepare output directory
    steps.push(Step::Bash(bash(
        "Prepare output directory",
        "mkdir -p \"$(Agent.TempDirectory)/staging\"\n",
    )));
    // Execute safe outputs (Stage 3) — typed BashStep with typed env block
    steps.push(Step::Bash(execute_safe_outputs_step(
        &cfg.source_path,
        &cfg.working_directory,
        &cfg.executor_ado_env,
    )));
    // Copy logs
    steps.push(Step::Bash(copy_logs_safeoutputs_step(&cfg.engine_log_dir)));
    // Publish
    steps.push(Step::Publish(PublishStep {
        path: "$(Agent.TempDirectory)/staging".to_string(),
        artifact: "safe_outputs".to_string(),
        condition: Some(Condition::Always),
    }));

    let mut job = Job::new(prefix.id("SafeOutputs")?, "SafeOutputs", cfg.pool.clone());
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

fn build_teardown_job(
    front_matter: &FrontMatter,
    cfg: &StandaloneCtx,
    prefix: &JobPrefix<'_>,
) -> Result<Option<Job>> {
    if front_matter.teardown.is_empty() {
        return Ok(None);
    }
    let mut steps: Vec<Step> = Vec::new();
    steps.push(checkout_self_step());
    for user_step_val in &front_matter.teardown {
        steps.push(Step::RawYaml(step_to_raw_yaml_string(user_step_val)?));
    }
    let mut job = Job::new(prefix.id("Teardown")?, "Teardown", cfg.pool.clone());
    job.steps = steps;
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
fn wire_explicit_dependencies(jobs: &mut [Job], prefix: &JobPrefix<'_>) -> Result<()> {
    let setup_id = prefix.id("Setup")?;
    let agent_id = prefix.id("Agent")?;
    let detection_id = prefix.id("Detection")?;
    let safeoutputs_id = prefix.id("SafeOutputs")?;
    let has_setup = jobs.iter().any(|j| j.id == setup_id);
    for j in jobs.iter_mut() {
        if j.id == agent_id && has_setup {
            j.depends_on = vec![setup_id.clone()];
        } else if j.id == detection_id {
            j.depends_on = vec![agent_id.clone()];
        } else if j.id == safeoutputs_id {
            j.depends_on = vec![agent_id.clone(), detection_id.clone()];
        } else if j.id.as_str() == "Teardown" {
            j.depends_on = vec![safeoutputs_id.clone()];
        }
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────
// Step body builders — typed BashStep/TaskStep with format!() bodies
// ─────────────────────────────────────────────────────────────────────

fn checkout_self_step() -> Step {
    Step::Checkout(CheckoutStep {
        repository: CheckoutRepo::Self_,
        clean: None,
        submodules: None,
        fetch_depth: None,
        persist_credentials: None,
    })
}

fn download_compiler_step(compiler_version: &str) -> BashStep {
    let script = format!(
        "set -eo pipefail\n\
         COMPILER_VERSION=\"{compiler_version}\"\n\
         DOWNLOAD_DIR=\"$(Pipeline.Workspace)/agentic-pipeline-compiler\"\n\
         DOWNLOAD_URL=\"https://github.com/githubnext/ado-aw/releases/download/v${{COMPILER_VERSION}}/ado-aw-linux-x64\"\n\
         CHECKSUM_URL=\"https://github.com/githubnext/ado-aw/releases/download/v${{COMPILER_VERSION}}/checksums.txt\"\n\
         \n\
         mkdir -p \"$DOWNLOAD_DIR\"\n\
         echo \"Downloading ado-aw v${{COMPILER_VERSION}} from GitHub Releases...\"\n\
         curl -fsSL -o \"$DOWNLOAD_DIR/ado-aw-linux-x64\" \"$DOWNLOAD_URL\"\n\
         curl -fsSL -o \"$DOWNLOAD_DIR/checksums.txt\" \"$CHECKSUM_URL\"\n\
         \n\
         echo \"Verifying checksum...\"\n\
         cd \"$DOWNLOAD_DIR\" || exit 1\n\
         grep \"ado-aw-linux-x64\" checksums.txt | sha256sum -c -\n\
         mv ado-aw-linux-x64 ado-aw\n\
         chmod +x ado-aw\n"
    );
    bash(
        format!("Download agentic pipeline compiler (v{compiler_version})"),
        script,
    )
}

fn substitute_integrity_check(yaml: &str, pipeline_path: &str, trigger_repo_dir: &str) -> String {
    if yaml.is_empty() {
        return String::new();
    }
    yaml.replace("{{ pipeline_path }}", pipeline_path)
        .replace("{{ trigger_repo_directory }}", trigger_repo_dir)
}

fn prepare_mcpg_config_step(mcpg_config_json: &str) -> BashStep {
    // mcpg_config_json is pretty-printed JSON. We want `{` to align with
    // the surrounding `cat`/`echo` lines (no extra leading indent) so the
    // emitted block-scalar bash body matches base.yml.
    let script = format!(
        "mkdir -p \"$(Agent.TempDirectory)/staging\"\n\
         \n\
         # Generate MCPG API key early so it's available as an ADO secret variable\n\
         # for both the MCPG config and the agent's mcp-config.json\n\
         MCP_GATEWAY_API_KEY=$(openssl rand -base64 45 | tr -d '/+=')\n\
         echo \"##vso[task.setvariable variable=MCP_GATEWAY_API_KEY;issecret=true]$MCP_GATEWAY_API_KEY\"\n\
         \n\
         # Export gateway port and domain as pipeline variables (matching gh-aw pattern).\n\
         # These duplicate the compile-time values baked into the YAML, but MCPG's\n\
         # Docker container requires MCP_GATEWAY_PORT and MCP_GATEWAY_DOMAIN env vars\n\
         # to start — the ADO variable indirection satisfies that contract.\n\
         echo \"##vso[task.setvariable variable=MCP_GATEWAY_PORT]{MCPG_PORT}\"\n\
         echo \"##vso[task.setvariable variable=MCP_GATEWAY_DOMAIN]{MCPG_DOMAIN}\"\n\
         \n\
         # Write MCPG (MCP Gateway) configuration to a file\n\
         cat > \"$(Agent.TempDirectory)/staging/mcpg-config.json\" << 'MCPG_CONFIG_EOF'\n\
{mcpg_config_json}\n\
         MCPG_CONFIG_EOF\n\
         \n\
         echo \"MCPG config:\"\n\
         cat \"$(Agent.TempDirectory)/staging/mcpg-config.json\"\n\
         \n\
         # Validate JSON\n\
         python3 -m json.tool \"$(Agent.TempDirectory)/staging/mcpg-config.json\" > /dev/null && echo \"JSON is valid\"\n"
    );
    bash("Prepare MCPG config", script)
}

fn prepare_tooling_step() -> BashStep {
    let script = "mkdir -p /tmp/awf-tools/staging\n\
                  \n\
                  echo \"HOME: $HOME\"\n\
                  \n\
                  # Use absolute path since MCP subprocess may not inherit PATH\n\
                  AGENTIC_PIPELINES_PATH=\"$(Pipeline.Workspace)/agentic-pipeline-compiler/ado-aw\"\n\
                  \n\
                  # Verify the binary exists and is executable\n\
                  ls -la \"$AGENTIC_PIPELINES_PATH\"\n\
                  chmod +x \"$AGENTIC_PIPELINES_PATH\"\n\
                  \n\
                  $AGENTIC_PIPELINES_PATH -h\n\
                  \n\
                  # Copy compiler binary to /tmp so it's accessible inside AWF container\n\
                  cp \"$AGENTIC_PIPELINES_PATH\" /tmp/awf-tools/ado-aw\n\
                  chmod +x /tmp/awf-tools/ado-aw\n\
                  \n\
                  # Copy MCPG config to /tmp\n\
                  cp \"$(Agent.TempDirectory)/staging/mcpg-config.json\" /tmp/awf-tools/staging/mcpg-config.json\n";
    bash("Prepare tooling", script)
}

fn prepare_agent_prompt_step(agent_content: &str) -> Result<BashStep> {
    // The agent_content lands inside a bash heredoc at the same indent as
    // `cat > ...` (no extra prefix), matching base.yml's emission.
    // The template uses leading-9-space `\n\` continuations; `dedent()`
    // strips them uniformly so the resulting bash body has 0-indent
    // surrounding lines and the interpolated content lands flush left.
    //
    // The sentinel is per-content SHA-derived so a malicious agent
    // markdown body cannot terminate the heredoc early and inject
    // shell commands into the Agent job. See
    // [`crate::compile::common::heredoc_sentinel`].
    let sentinel = super::common::heredoc_sentinel("AGENT_PROMPT_EOF", agent_content)?;
    let template = format!(
        "\
         # Write agent instructions to /tmp so it's accessible inside AWF container\n\
         cat > \"/tmp/awf-tools/agent-prompt.md\" << '{sentinel}'\n\
         {{INTERP}}\n\
         {sentinel}\n\
         \n\
         echo \"Agent prompt:\"\n\
         cat \"/tmp/awf-tools/agent-prompt.md\"\n"
    );
    let script = dedent(&template).replace("{INTERP}", agent_content);
    Ok(bash("Prepare agent prompt", script))
}

fn download_awf_step() -> BashStep {
    let script = format!(
        "set -eo pipefail\n\
         \n\
         AWF_VERSION=\"{AWF_VERSION}\"\n\
         DOWNLOAD_DIR=\"$(Pipeline.Workspace)/awf\"\n\
         DOWNLOAD_URL=\"https://github.com/github/gh-aw-firewall/releases/download/v${{AWF_VERSION}}/awf-linux-x64\"\n\
         CHECKSUM_URL=\"https://github.com/github/gh-aw-firewall/releases/download/v${{AWF_VERSION}}/checksums.txt\"\n\
         \n\
         mkdir -p \"$DOWNLOAD_DIR\"\n\
         echo \"Downloading AWF v${{AWF_VERSION}} from GitHub Releases...\"\n\
         curl -fsSL -o \"$DOWNLOAD_DIR/awf-linux-x64\" \"$DOWNLOAD_URL\"\n\
         curl -fsSL -o \"$DOWNLOAD_DIR/checksums.txt\" \"$CHECKSUM_URL\"\n\
         \n\
         echo \"Verifying checksum...\"\n\
         cd \"$DOWNLOAD_DIR\" || exit 1\n\
         grep \"awf-linux-x64\" checksums.txt | sha256sum -c -\n\
         mv awf-linux-x64 awf\n\
         chmod +x awf\n\
         echo \"##vso[task.prependpath]$(Pipeline.Workspace)/awf\"\n\
         ./awf --version\n"
    );
    bash(
        format!("Download AWF (Agentic Workflow Firewall) v{AWF_VERSION}"),
        script,
    )
}

fn prepull_images_step(include_mcpg: bool) -> BashStep {
    let mut script = format!(
        "set -eo pipefail\n\
         \n\
         docker pull ghcr.io/github/gh-aw-firewall/squid:{AWF_VERSION}\n\
         docker pull ghcr.io/github/gh-aw-firewall/agent:{AWF_VERSION}\n\
         docker tag ghcr.io/github/gh-aw-firewall/squid:{AWF_VERSION} ghcr.io/github/gh-aw-firewall/squid:latest\n\
         docker tag ghcr.io/github/gh-aw-firewall/agent:{AWF_VERSION} ghcr.io/github/gh-aw-firewall/agent:latest\n"
    );
    if include_mcpg {
        script.push_str(&format!("docker pull {MCPG_IMAGE}:v{MCPG_VERSION}\n"));
        bash(
            format!("Pre-pull AWF and MCPG container images (v{AWF_VERSION})"),
            script,
        )
    } else {
        bash(
            format!("Pre-pull AWF container images (v{AWF_VERSION})"),
            script,
        )
    }
}

fn start_safeoutputs_server_step(enabled_tools_args: &str, working_directory: &str) -> BashStep {
    let script = format!(
        "SAFE_OUTPUTS_PORT=8100\n\
         SAFE_OUTPUTS_API_KEY=$(openssl rand -base64 45 | tr -d '/+=')\n\
         echo \"##vso[task.setvariable variable=SAFE_OUTPUTS_PORT]$SAFE_OUTPUTS_PORT\"\n\
         echo \"##vso[task.setvariable variable=SAFE_OUTPUTS_API_KEY;issecret=true]$SAFE_OUTPUTS_API_KEY\"\n\
         \n\
         mkdir -p \"$(Agent.TempDirectory)/staging/logs\"\n\
         \n\
         # Start SafeOutputs as HTTP server in the background\n\
         # NOTE: {enabled_tools_args} expands to either \"\" or \"--enabled-tools X ... \"\n\
         # (with trailing space). The value MUST be newline-free; is_safe_tool_name enforces this.\n\
         # Positional args (output_directory, bounding_directory) MUST come after all named\n\
         # options — clap parses them positionally and reordering would break the command.\n\
         nohup /tmp/awf-tools/ado-aw mcp-http \\\n  \
           --port \"$SAFE_OUTPUTS_PORT\" \\\n  \
           --api-key \"$SAFE_OUTPUTS_API_KEY\" \\\n  \
           {enabled_tools_args}\"/tmp/awf-tools/staging\" \\\n  \
           \"{working_directory}\" \\\n  \
           > \"$(Agent.TempDirectory)/staging/logs/safeoutputs.log\" 2>&1 &\n\
         SAFE_OUTPUTS_PID=$!\n\
         echo \"##vso[task.setvariable variable=SAFE_OUTPUTS_PID]$SAFE_OUTPUTS_PID\"\n\
         echo \"SafeOutputs HTTP server started on port $SAFE_OUTPUTS_PORT (PID: $SAFE_OUTPUTS_PID)\"\n\
         \n\
         # Wait for server to be ready\n\
         READY=false\n\
         # shellcheck disable=SC2034 # i is intentionally unused; wait-N-times loop\n\
         for i in $(seq 1 30); do\n  \
           if curl -sf \"http://localhost:$SAFE_OUTPUTS_PORT/health\" > /dev/null 2>&1; then\n    \
             echo \"SafeOutputs HTTP server is ready\"\n    \
             READY=true\n    \
             break\n  \
           fi\n  \
           sleep 1\n\
         done\n\
         if [ \"$READY\" != \"true\" ]; then\n  \
           echo \"##vso[task.complete result=Failed]SafeOutputs HTTP server did not become ready within 30s\"\n  \
           exit 1\n\
         fi\n"
    );
    bash("Start SafeOutputs HTTP server", script)
}

fn start_mcpg_step(mcpg_docker_env: &str, mcpg_step_env: &str, debug_pipeline: bool) -> BashStep {
    let mcpg_image_v = format!("{MCPG_IMAGE}:v{MCPG_VERSION}");
    // Build the docker-env block as additional `-e VAR=...` lines, one per
    // line, joined with `\n  ` (newline + 2-space continuation indent to
    // match the surrounding `-e MCP_GATEWAY_*` lines). When no extensions
    // contribute docker env, emit two empty `\`-continuation lines as
    // placeholders for the legacy `{{ mcpg_debug_flags }}` and
    // `{{ mcpg_docker_env }}` markers — bash treats them as no-op
    // continuations and ignoring them keeps the lock file shape stable.
    // Build the docker-env block as additional `-e VAR=...` lines, one per
    // line, joined with `\n  ` (newline + 2-space continuation indent to
    // match the surrounding `-e MCP_GATEWAY_*` lines). When no extensions
    // contribute docker env, emit two empty `\`-continuation lines as
    // placeholders for the legacy `{{ mcpg_debug_flags }}` and
    // `{{ mcpg_docker_env }}` markers — bash treats them as no-op
    // continuations and ignoring them keeps the lock file shape stable.
    //
    // `generate_mcpg_docker_env` returns a single `\` byte when no
    // extensions contribute, so check for that sentinel as well as a
    // literal empty string.
    let docker_env_lines: String =
        if mcpg_docker_env.trim().is_empty() || mcpg_docker_env.trim() == "\\" {
            // Two empty continuation lines mirror the legacy template's
            // two-marker layout.
            "\\\n  \\".to_string()
        } else {
            mcpg_docker_env
                .lines()
                .map(|l| format!("{l} \\"))
                .collect::<Vec<_>>()
                .join("\n  ")
        };
    // `--debug-pipeline` injects an extra `-e DEBUG="*" \` continuation
    // line into the `docker run …` invocation so MCPG (and the stdio
    // backends it spawns) emit verbose logs to the gateway stderr stream.
    // Mirrors the legacy `{{ mcpg_debug_flags }}` template marker; emits
    // the trailing `\n  ` so the next continuation line aligns under it.
    let debug_flag = if debug_pipeline {
        "-e DEBUG=\"*\" \\\n  ".to_string()
    } else {
        String::new()
    };
    let script = format!(
        "# Substitute runtime values into MCPG config\n\
         MCPG_CONFIG=$(sed \\\n  \
           -e \"s|\\${{SAFE_OUTPUTS_PORT}}|$(SAFE_OUTPUTS_PORT)|g\" \\\n  \
           -e \"s|\\${{SAFE_OUTPUTS_API_KEY}}|$(SAFE_OUTPUTS_API_KEY)|g\" \\\n  \
           -e \"s|\\${{MCP_GATEWAY_API_KEY}}|$(MCP_GATEWAY_API_KEY)|g\" \\\n  \
           /tmp/awf-tools/staging/mcpg-config.json)\n\
         \n\
         # Log the template config (before API key substitution) for debugging.\n\
         echo \"Starting MCPG with config template:\"\n\
         python3 -m json.tool < /tmp/awf-tools/staging/mcpg-config.json\n\
         \n\
         # Remove any leftover container or stale output from a previous interrupted run\n\
         # (--rm only cleans up on clean exit; OOM/SIGKILL may leave it behind)\n\
         docker rm -f mcpg 2>/dev/null || true\n\
         GATEWAY_OUTPUT=\"/tmp/gh-aw/mcp-config/gateway-output.json\"\n\
         mkdir -p \"$(dirname \"$GATEWAY_OUTPUT\")\" /tmp/gh-aw/mcp-logs\n\
         rm -f \"$GATEWAY_OUTPUT\"\n\
         \n\
         # Start MCPG Docker container on host network.\n\
         # The Docker socket mount is required because MCPG spawns stdio-based MCP\n\
         # servers as sibling containers. This grants significant host access — acceptable\n\
         # here because the pipeline agent is already trusted and network-isolated by AWF.\n\
         #\n\
         # WORKAROUND: Override entrypoint to bypass run_containerized.sh which has a\n\
         # validate_port_mapping() bug — it calls `docker inspect .NetworkSettings.Ports`\n\
         # which is empty with --network host (by design), causing a spurious error:\n\
         #   [ERROR] Port 80 is not exposed from the container\n\
         # Upstream fix: https://github.com/github/gh-aw-mcpg/issues/TBD\n\
         #\n\
         # stdout → gateway-output.json (machine-readable config, read after health check)\n\
         echo \"$MCPG_CONFIG\" | docker run -i --rm \\\n  \
           --name mcpg \\\n  \
           --network host \\\n  \
           --entrypoint /app/awmg \\\n  \
           -v /var/run/docker.sock:/var/run/docker.sock \\\n  \
           -e MCP_GATEWAY_PORT=\"$(MCP_GATEWAY_PORT)\" \\\n  \
           -e MCP_GATEWAY_DOMAIN=\"$(MCP_GATEWAY_DOMAIN)\" \\\n  \
           -e MCP_GATEWAY_API_KEY=\"$(MCP_GATEWAY_API_KEY)\" \\\n  \
           {debug_flag}{docker_env_lines}\n  \
           {mcpg_image_v} \\\n  \
           --routed --listen 0.0.0.0:{MCPG_PORT} --config-stdin --log-dir /tmp/gh-aw/mcp-logs \\\n  \
           > \"$GATEWAY_OUTPUT\" 2> >(tee /tmp/gh-aw/mcp-logs/stderr.log >&2) &\n\
         MCPG_PID=$!\n\
         echo \"MCPG started (PID: $MCPG_PID)\"\n\
         \n\
         # Wait for MCPG to be ready\n\
         READY=false\n\
         # shellcheck disable=SC2034 # i is intentionally unused; wait-N-times loop\n\
         for i in $(seq 1 30); do\n  \
           if curl -sf \"http://localhost:{MCPG_PORT}/health\" > /dev/null 2>&1; then\n    \
             echo \"MCPG is ready\"\n    \
             READY=true\n    \
             break\n  \
           fi\n  \
           sleep 1\n\
         done\n\
         if [ \"$READY\" != \"true\" ]; then\n  \
           echo \"##vso[task.complete result=Failed]MCPG did not become ready within 30s\"\n  \
           exit 1\n\
         fi\n\
         \n\
         # Wait for gateway output file to contain valid JSON with mcpServers.\n\
         # Health check passing doesn't guarantee stdout is flushed, so poll.\n\
         echo \"Waiting for gateway output file...\"\n\
         GATEWAY_READY=false\n\
         # shellcheck disable=SC2034 # i is intentionally unused; wait-N-times loop\n\
         for i in $(seq 1 15); do\n  \
           if [ -s \"$GATEWAY_OUTPUT\" ] && jq -e '.mcpServers' \"$GATEWAY_OUTPUT\" > /dev/null 2>&1; then\n    \
             echo \"Gateway output is ready\"\n    \
             GATEWAY_READY=true\n    \
             break\n  \
           fi\n  \
           sleep 1\n\
         done\n\
         if [ \"$GATEWAY_READY\" != \"true\" ]; then\n  \
           echo \"##vso[task.complete result=Failed]Gateway output file not ready within 15s\"\n  \
           echo \"Gateway output content:\"\n  \
           cat \"$GATEWAY_OUTPUT\" 2>/dev/null || echo \"(empty or missing)\"\n  \
           exit 1\n\
         fi\n\
         \n\
         echo \"Gateway output:\"\n\
         cat \"$GATEWAY_OUTPUT\"\n\
         \n\
         # Convert gateway output to Copilot CLI mcp-config.json.\n\
         # Mirrors gh-aw's convert_gateway_config_copilot.cjs:\n\
         #   - Rewrite URLs from 127.0.0.1 to host.docker.internal (AWF container needs\n\
         #     host.docker.internal to reach MCPG on the host; 127.0.0.1 is container loopback)\n\
         #   - Ensure tools: [\"*\"] on each server entry (Copilot CLI requirement)\n\
         #   - Preserve all other fields (headers, type, etc.)\n\
         jq --arg prefix \"http://$(MCP_GATEWAY_DOMAIN):$(MCP_GATEWAY_PORT)\" \\\n  \
           '.mcpServers |= (to_entries | sort_by(.key) | map(.value.url |= sub(\"^http://[^/]+/\"; \"\\($prefix)/\") | .value.tools = [\"*\"]) | from_entries)' \\\n  \
           \"$GATEWAY_OUTPUT\" > /tmp/awf-tools/mcp-config.json\n\
         \n\
         chmod 600 /tmp/awf-tools/mcp-config.json\n\
         \n\
         echo \"Generated MCP config at: /tmp/awf-tools/mcp-config.json\"\n\
         cat /tmp/awf-tools/mcp-config.json\n"
    );
    let mut step = bash("Start MCP Gateway (MCPG)", script);
    for (k, v) in parse_env_block(mcpg_step_env) {
        step = step.with_env(k, v);
    }
    step
}

fn run_agent_step(
    allowed_domains: &str,
    awf_mounts: &str,
    working_directory: &str,
    engine_run: &str,
    engine_env: &str,
) -> BashStep {
    // The awf_mounts string is a `\`-joined chain of `--mount "..."` lines.
    // Render each at 2-space indent inside the bash body (the surrounding
    // `--allow-domains` line is at 2-space indent too — the block-scalar
    // body indent is set by the first non-empty line).
    let awf_mounts_block: String = if awf_mounts == "\\" {
        "  \\".to_string()
    } else {
        awf_mounts
            .lines()
            .map(|l| format!("  {l}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let script = format!(
        "set -o pipefail\n\
         \n\
         AGENT_OUTPUT_FILE=\"$(Agent.TempDirectory)/staging/logs/agent-output.txt\"\n\
         mkdir -p \"$(Agent.TempDirectory)/staging/logs\"\n\
         \n\
         echo \"=== Running AI agent with AWF network isolation ===\"\n\
         echo \"Allowed domains: {allowed_domains}\"\n\
         \n\
         # AWF provides L7 domain whitelisting via Squid proxy + Docker containers.\n\
         # --enable-host-access allows the AWF container to reach host services\n\
         # (MCPG and SafeOutputs) via host.docker.internal.\n\
         # AWF auto-mounts /tmp:/tmp:rw into the container, so copilot binary,\n\
         # agent prompt, and MCP config are placed under /tmp/awf-tools/.\n\
         # Stream agent output in real-time while filtering VSO commands.\n\
         # sed -u = unbuffered (line-by-line) so output appears immediately.\n\
         # tee writes to both stdout (ADO pipeline log) and the artifact file.\n\
         # pipefail (set above) ensures AWF's exit code propagates through the pipe.\n\
         # shellcheck disable=SC2046 # $(AW_AZ_MOUNTS) is an ADO macro substituted before bash sees it, not bash command substitution; word-splitting the expanded value into separate --mount tokens is intentional\n\
         sudo -E \"$(Pipeline.Workspace)/awf/awf\" \\\n  \
           --allow-domains \"{allowed_domains}\" \\\n  \
           --skip-pull \\\n  \
           --env-all \\\n  \
           --enable-host-access \\\n\
{awf_mounts_block}\n  \
           --container-workdir \"{working_directory}\" \\\n  \
           --log-level info \\\n  \
           --proxy-logs-dir \"$(Agent.TempDirectory)/staging/logs/firewall\" \\\n  \
           -- '{engine_run}' \\\n  \
           2>&1 \\\n  \
           | sed -u 's/##vso\\[/[VSO-FILTERED] vso[/g; s/##\\[/[VSO-FILTERED] [/g' \\\n  \
           | tee \"$AGENT_OUTPUT_FILE\" \\\n  \
           && AGENT_EXIT_CODE=0 || AGENT_EXIT_CODE=$?\n\
         \n\
         # Print firewall summary if available\n\
         if [ -x \"$(Pipeline.Workspace)/awf/awf\" ]; then\n  \
           echo \"=== Firewall Summary ===\"\n  \
           \"$(Pipeline.Workspace)/awf/awf\" logs summary --source \"$(Agent.TempDirectory)/staging/logs/firewall\" 2>/dev/null || true\n\
         fi\n\
         \n\
         exit \"$AGENT_EXIT_CODE\"\n"
    );
    let mut step = bash("Run copilot (AWF network isolated)", script);
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
    for (k, v) in parse_env_block(&synthetic_block) {
        step = step.with_env(k, v);
    }
    step
}

fn execute_safe_outputs_step(
    source_path: &str,
    working_directory: &str,
    executor_ado_env: &str,
) -> BashStep {
    let script = format!(
        "ado-aw execute --source \"{source_path}\" --safe-output-dir \"$(Pipeline.Workspace)/analyzed_outputs_$(Build.BuildId)\" --output-dir \"$(Agent.TempDirectory)/staging\"\n\
         EXIT_CODE=$?\n\
         if [ $EXIT_CODE -eq 2 ]; then\n  \
           echo \"##vso[task.complete result=SucceededWithIssues;]Executor completed with warnings\"\n  \
           exit 0\n\
         fi\n\
         exit $EXIT_CODE\n"
    );
    let mut step = bash("Execute safe outputs (Stage 3)", script);
    step.working_directory = Some(working_directory.to_string());
    for (k, v) in parse_env_block(executor_ado_env) {
        step = step.with_env(k, v);
    }
    step
}

fn collect_safe_outputs_step() -> BashStep {
    let script = "# Copy safe outputs from /tmp back to staging for artifact publish\n\
                  mkdir -p \"$(Agent.TempDirectory)/staging\"\n\
                  cp -r /tmp/awf-tools/staging/* \"$(Agent.TempDirectory)/staging/\" 2>/dev/null || true\n\
                  echo \"Safe outputs copied to $(Agent.TempDirectory)/staging\"\n\
                  ls -la \"$(Agent.TempDirectory)/staging\" 2>/dev/null || echo \"No safe outputs found\"\n";
    bash("Collect safe outputs from AWF container", script).with_condition(Condition::Always)
}

fn stop_mcpg_step() -> BashStep {
    let script = "# Stop MCPG container\n\
                  echo \"Stopping MCPG...\"\n\
                  docker stop mcpg 2>/dev/null || true\n\
                  echo \"MCPG stopped\"\n\
                  \n\
                  # Stop SafeOutputs HTTP server\n\
                  if [ -n \"$(SAFE_OUTPUTS_PID)\" ]; then\n  \
                    echo \"Stopping SafeOutputs (PID: $(SAFE_OUTPUTS_PID))...\"\n  \
                    kill \"$(SAFE_OUTPUTS_PID)\" 2>/dev/null || true\n  \
                    echo \"SafeOutputs stopped\"\n\
                  fi\n";
    bash("Stop MCPG and SafeOutputs", script).with_condition(Condition::Always)
}

fn copy_logs_step(engine_log_dir: &str, is_detection: bool) -> BashStep {
    if is_detection {
        // Detection job copies its logs into analyzed_outputs/logs (the
        // artifact published from that job), with per-subdir nesting.
        let script = format!(
            "# Copy all logs to analyzed outputs for artifact upload\n\
             mkdir -p \"$(Agent.TempDirectory)/analyzed_outputs/logs\"\n\
             if [ -d \"{engine_log_dir}\" ]; then\n  \
               mkdir -p \"$(Agent.TempDirectory)/analyzed_outputs/logs/copilot\"\n  \
               cp -r \"{engine_log_dir}\"/* \"$(Agent.TempDirectory)/analyzed_outputs/logs/copilot/\" 2>/dev/null || true\n\
             fi\n\
             ADO_AW_LOG_DIR=\"${{ADO_AW_LOG_DIR:-$HOME/.ado-aw/logs}}\"\n\
             if [ -d \"$ADO_AW_LOG_DIR\" ]; then\n  \
               mkdir -p \"$(Agent.TempDirectory)/analyzed_outputs/logs/ado-aw\"\n  \
               cp -r \"$ADO_AW_LOG_DIR\"/* \"$(Agent.TempDirectory)/analyzed_outputs/logs/ado-aw/\" 2>/dev/null || true\n\
             fi\n\
             echo \"Logs copied to $(Agent.TempDirectory)/analyzed_outputs/logs\"\n\
             ls -laR \"$(Agent.TempDirectory)/analyzed_outputs/logs\" 2>/dev/null || echo \"No logs found\"\n"
        );
        return bash("Copy logs to output directory", script).with_condition(Condition::Always);
    }
    let script = format!(
        "# Copy all logs to output directory for artifact upload\n\
         mkdir -p \"$(Agent.TempDirectory)/staging/logs\"\n\
         if [ -d \"{engine_log_dir}\" ]; then\n  \
           cp -r \"{engine_log_dir}\"/* \"$(Agent.TempDirectory)/staging/logs/\" 2>/dev/null || true\n\
         fi\n\
         ADO_AW_LOG_DIR=\"${{ADO_AW_LOG_DIR:-$HOME/.ado-aw/logs}}\"\n\
         if [ -d \"$ADO_AW_LOG_DIR\" ]; then\n  \
           cp -r \"$ADO_AW_LOG_DIR\"/* \"$(Agent.TempDirectory)/staging/logs/\" 2>/dev/null || true\n\
         fi\n\
         if [ -d /tmp/gh-aw/mcp-logs ]; then\n  \
           mkdir -p \"$(Agent.TempDirectory)/staging/logs/mcpg\"\n  \
           cp -r /tmp/gh-aw/mcp-logs/* \"$(Agent.TempDirectory)/staging/logs/mcpg/\" 2>/dev/null || true\n\
         fi\n\
         echo \"Logs copied to $(Agent.TempDirectory)/staging/logs\"\n\
         ls -la \"$(Agent.TempDirectory)/staging/logs\" 2>/dev/null || echo \"No logs found\"\n"
    );
    bash("Copy logs to output directory", script).with_condition(Condition::Always)
}

fn copy_logs_safeoutputs_step(engine_log_dir: &str) -> BashStep {
    let script = format!(
        "# Copy all logs to output directory for artifact upload\n\
         mkdir -p \"$(Agent.TempDirectory)/staging/logs\"\n\
         # Copy agent output log from analyzed_outputs for optimisation use\n\
         cp \"$(Pipeline.Workspace)/analyzed_outputs_$(Build.BuildId)/logs/agent-output.txt\" \\\n  \
           \"$(Agent.TempDirectory)/staging/logs/agent-output.txt\" 2>/dev/null || true\n\
         if [ -d \"{engine_log_dir}\" ]; then\n  \
           mkdir -p \"$(Agent.TempDirectory)/staging/logs/copilot\"\n  \
           cp -r \"{engine_log_dir}\"/* \"$(Agent.TempDirectory)/staging/logs/copilot/\" 2>/dev/null || true\n\
         fi\n\
         ADO_AW_LOG_DIR=\"${{ADO_AW_LOG_DIR:-$HOME/.ado-aw/logs}}\"\n\
         if [ -d \"$ADO_AW_LOG_DIR\" ]; then\n  \
           mkdir -p \"$(Agent.TempDirectory)/staging/logs/ado-aw\"\n  \
           cp -r \"$ADO_AW_LOG_DIR\"/* \"$(Agent.TempDirectory)/staging/logs/ado-aw/\" 2>/dev/null || true\n\
         fi\n\
         echo \"Logs copied to $(Agent.TempDirectory)/staging/logs\"\n\
         ls -laR \"$(Agent.TempDirectory)/staging/logs\" 2>/dev/null || echo \"No logs found\"\n"
    );
    bash("Copy logs to output directory", script).with_condition(Condition::Always)
}

fn prepare_safe_outputs_for_analysis(working_directory: &str) -> BashStep {
    let script = format!(
        "mkdir -p \"{working_directory}/safe_outputs\"\n\
         cp -a \"$(Pipeline.Workspace)/agent_outputs_$(Build.BuildId)/.\"  \"{working_directory}/safe_outputs\"\n"
    );
    bash("Prepare safe outputs for analysis", script)
}

fn prepare_threat_analysis_prompt_step(threat_prompt: &str) -> Result<BashStep> {
    // Same heredoc-injection mitigation as `prepare_agent_prompt_step`:
    // the sentinel is SHA-derived per content so a malicious
    // front-matter `description:` (which lands inside this prompt
    // body) cannot terminate the heredoc early and inject commands
    // into the Detection job.
    let sentinel = super::common::heredoc_sentinel("THREAT_ANALYSIS_EOF", threat_prompt)?;
    let template = format!(
        "\
         # Write threat analysis prompt to /tmp (accessible inside AWF container)\n\
         cat > \"/tmp/awf-tools/threat-analysis-prompt.md\" << '{sentinel}'\n\
         {{INTERP}}\n\
         {sentinel}\n\
         \n\
         echo \"Threat analysis prompt:\"\n\
         cat \"/tmp/awf-tools/threat-analysis-prompt.md\"\n"
    );
    let script = dedent(&template).replace("{INTERP}", threat_prompt);
    Ok(bash("Prepare threat analysis prompt", script))
}

fn setup_compiler_step() -> BashStep {
    let script = "AGENTIC_PIPELINES_PATH=\"$(Pipeline.Workspace)/agentic-pipeline-compiler/ado-aw\"\n\
                  chmod +x \"$AGENTIC_PIPELINES_PATH\"\n";
    bash("Setup agentic pipeline compiler", script)
}

fn run_threat_analysis_step(
    allowed_domains: &str,
    working_directory: &str,
    engine_run_detection: &str,
) -> BashStep {
    let script = format!(
        "set -o pipefail\n\
         \n\
         # Run threat analysis with AWF network isolation\n\
         THREAT_OUTPUT_FILE=\"$(Agent.TempDirectory)/threat-analysis-output.txt\"\n\
         \n\
         # Stream threat analysis output in real-time with VSO command filtering\n\
         sudo -E \"$(Pipeline.Workspace)/awf/awf\" \\\n  \
           --allow-domains \"{allowed_domains}\" \\\n  \
           --skip-pull \\\n  \
           --env-all \\\n  \
           --container-workdir \"{working_directory}\" \\\n  \
           --log-level info \\\n  \
           --proxy-logs-dir \"$(Agent.TempDirectory)/threat-analysis-logs/firewall\" \\\n  \
           -- '{engine_run_detection}' \\\n  \
           2>&1 \\\n  \
           | sed -u 's/##vso\\[/[VSO-FILTERED] vso[/g; s/##\\[/[VSO-FILTERED] [/g' \\\n  \
           | tee \"$THREAT_OUTPUT_FILE\" \\\n  \
           && AGENT_EXIT_CODE=0 || AGENT_EXIT_CODE=$?\n\
         \n\
         exit \"$AGENT_EXIT_CODE\"\n"
    );
    let mut step = bash("Run threat analysis (AWF network isolated)", script);
    step.working_directory = Some(working_directory.to_string());
    // env block: GITHUB_TOKEN + GITHUB_READ_ONLY — emit the latter as
    // a typed YAML integer so it round-trips unquoted (matching the
    // legacy copilot_env output of `GITHUB_READ_ONLY: 1`, not `'1'`).
    use super::ir::env::EnvValue;
    step = step
        .with_env("GITHUB_TOKEN", EnvValue::pipeline_var("GITHUB_TOKEN"))
        .with_env(
            "GITHUB_READ_ONLY",
            EnvValue::RawYamlScalar(serde_yaml::Value::Number(1.into())),
        );
    step
}

fn prepare_analyzed_outputs_step() -> BashStep {
    let script = "# Create analyzed outputs directory with original safe outputs and analysis\n\
                  mkdir -p \"$(Agent.TempDirectory)/analyzed_outputs\"\n\
                  \n\
                  # Copy original safe outputs\n\
                  cp -a \"$(Pipeline.Workspace)/agent_outputs_$(Build.BuildId)/.\" \"$(Agent.TempDirectory)/analyzed_outputs/\"\n\
                  \n\
                  # Copy threat analysis output\n\
                  if [ -f \"$(Agent.TempDirectory)/threat-analysis-output.txt\" ]; then\n  \
                    cp \"$(Agent.TempDirectory)/threat-analysis-output.txt\" \"$(Agent.TempDirectory)/analyzed_outputs/\"\n\
                  fi\n\
                  \n\
                  # Extract JSON from THREAT_DETECTION_RESULT line in threat analysis output\n\
                  if [ -f \"$(Agent.TempDirectory)/threat-analysis-output.txt\" ]; then\n  \
                    RESULT_LINE=$(grep \"THREAT_DETECTION_RESULT:\" \"$(Agent.TempDirectory)/threat-analysis-output.txt\" | tail -1)\n  \
                    if [ -n \"$RESULT_LINE\" ]; then\n    \
                      # Extract JSON after the prefix\n    \
                      JSON_CONTENT=\"${RESULT_LINE##*THREAT_DETECTION_RESULT:}\"\n    \
                      echo \"$JSON_CONTENT\" > \"$(Agent.TempDirectory)/analyzed_outputs/threat-analysis.json\"\n    \
                      echo \"Extracted threat analysis JSON:\"\n    \
                      cat \"$(Agent.TempDirectory)/analyzed_outputs/threat-analysis.json\"\n  \
                    else\n    \
                      echo \"Warning: No THREAT_DETECTION_RESULT found in threat analysis output\"\n  \
                    fi\n\
                  else\n  \
                    echo \"Warning: No threat analysis output file found\"\n\
                  fi\n\
                  \n\
                  echo \"Analyzed outputs directory contents:\"\n\
                  ls -laR \"$(Agent.TempDirectory)/analyzed_outputs\"\n";
    bash("Prepare analyzed outputs", script).with_condition(Condition::Always)
}

fn evaluate_threat_analysis_step() -> BashStep {
    let script = "SAFE_TO_PROCESS=\"false\"\n\
                  JSON_FILE=\"$(Agent.TempDirectory)/analyzed_outputs/threat-analysis.json\"\n\
                  \n\
                  if [ -f \"$JSON_FILE\" ]; then\n  \
                    if jq -e . \"$JSON_FILE\" > /dev/null 2>&1; then\n    \
                      echo \"JSON is valid\"\n    \
                      \n    \
                      # Check if any threat field is true\n    \
                      if jq -e '.prompt_injection or .secret_leak or .malicious_patch' \"$JSON_FILE\" > /dev/null 2>&1; then\n      \
                        echo \"##vso[task.logissue type=warning]Threats detected - safe outputs will NOT be processed\"\n      \
                        jq -r '.reasons[]? // empty' \"$JSON_FILE\" | sed 's/^/  - /'\n    \
                      else\n      \
                        echo \"No threats detected - safe outputs will be processed\"\n      \
                        SAFE_TO_PROCESS=\"true\"\n    \
                      fi\n  \
                    else\n    \
                      echo \"##vso[task.logissue type=warning]Invalid JSON in threat analysis - defaulting to unsafe\"\n  \
                    fi\n\
                  else\n  \
                    echo \"##vso[task.logissue type=warning]No threat analysis JSON found - defaulting to unsafe\"\n\
                  fi\n\
                  \n\
                  echo \"##vso[task.setvariable variable=SafeToProcess;isOutput=true]$SAFE_TO_PROCESS\"\n\
                  echo \"SafeToProcess set to: $SAFE_TO_PROCESS\"\n";
    bash("Evaluate threat analysis", script)
        .with_id(StepId::new("threatAnalysis").unwrap())
        .with_output(OutputDecl::new("SafeToProcess"))
        .with_condition(Condition::Always)
}

fn verify_mcp_backends_step() -> BashStep {
    // Debug-only probe (emitted when --debug-pipeline is on). Probes every
    // MCPG backend via MCP initialize + tools/list to surface broken
    // backends early. Mirrors the legacy `generate_debug_pipeline_replacements`
    // bash body. `{{ mcpg_port }}` in the legacy template is interpolated
    // here as the `MCPG_PORT` const value.
    let script = format!(
        "echo \"=== Probing MCP backends ===\"\n\
PROBE_FAILED=false\n\
for server in $(jq -r '.mcpServers | keys[]' /tmp/awf-tools/mcp-config.json); do\n  \
  echo \"\"\n  \
  echo \"--- Probing: $server ---\"\n  \
  # MCP requires initialize handshake before tools/list.\n  \
  # Send initialize first, then tools/list in a second request\n  \
  # using the session ID from the initialize response.\n  \
  INIT_RESPONSE=$(curl -s -D /tmp/probe-headers.txt -o /tmp/probe-init.json -w \"%{{http_code}}\" --max-time 120 -X POST \\\n    \
    -H \"Authorization: $MCPG_API_KEY\" \\\n    \
    -H \"Content-Type: application/json\" \\\n    \
    -H \"Accept: application/json, text/event-stream\" \\\n    \
    -d '{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{{\"protocolVersion\":\"2025-03-26\",\"capabilities\":{{}},\"clientInfo\":{{\"name\":\"ado-aw-probe\",\"version\":\"1.0\"}}}}}}' \\\n    \
    \"http://localhost:{MCPG_PORT}/mcp/$server\" 2>&1)\n  \
  SESSION_ID=$(grep -i \"mcp-session-id\" /tmp/probe-headers.txt 2>/dev/null | tr -d '\\r' | awk '{{print $2}}')\n  \
  echo \"Initialize: HTTP $INIT_RESPONSE, session=$SESSION_ID\"\n  \
\n  \
  if [ -z \"$SESSION_ID\" ]; then\n    \
    echo \"##vso[task.logissue type=warning]MCP backend '$server' did not return a session ID\"\n    \
    cat /tmp/probe-init.json 2>/dev/null || true\n    \
    PROBE_FAILED=true\n    \
    continue\n  \
  fi\n  \
\n  \
  # Now send tools/list with the session\n  \
  HTTP_CODE=$(curl -s -o /tmp/probe-response.json -w \"%{{http_code}}\" --max-time 120 -X POST \\\n    \
    -H \"Authorization: $MCPG_API_KEY\" \\\n    \
    -H \"Content-Type: application/json\" \\\n    \
    -H \"Accept: application/json, text/event-stream\" \\\n    \
    -H \"Mcp-Session-Id: $SESSION_ID\" \\\n    \
    -d '{{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\"}}' \\\n    \
    \"http://localhost:{MCPG_PORT}/mcp/$server\" 2>&1)\n  \
  BODY=$(cat /tmp/probe-response.json 2>/dev/null || echo \"(empty)\")\n  \
  # Extract tool count from SSE data line\n  \
  TOOL_COUNT=$(echo \"$BODY\" | grep '^data:' | sed 's/^data: //' | jq -r '.result.tools | length' 2>/dev/null || echo \"?\")\n  \
  echo \"tools/list: HTTP $HTTP_CODE\"\n  \
  if [ \"$HTTP_CODE\" -ge 200 ] && [ \"$HTTP_CODE\" -lt 300 ] && [ \"$TOOL_COUNT\" != \"?\" ]; then\n    \
    echo \"\u{2713} $server: $TOOL_COUNT tools available\"\n  \
  else\n    \
    echo \"##vso[task.logissue type=warning]MCP backend '$server' tools/list returned HTTP $HTTP_CODE\"\n    \
    echo \"Response: $BODY\"\n    \
    PROBE_FAILED=true\n  \
  fi\n\
done\n\
\n\
echo \"\"\n\
echo \"=== MCPG health after probes ===\"\n\
curl -sf \"http://localhost:{MCPG_PORT}/health\" | jq . || true\n\
\n\
if [ \"$PROBE_FAILED\" = \"true\" ]; then\n  \
  echo \"##vso[task.logissue type=warning]One or more MCP backends failed to initialize \u{2014} check logs above\"\n\
fi\n"
    );
    use super::ir::env::EnvValue;
    bash("Verify MCP backends", script).with_env(
        "MCPG_API_KEY",
        EnvValue::pipeline_var("MCP_GATEWAY_API_KEY"),
    )
}

// ─────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────

/// Construct a [`BashStep`] with its script body run through
/// [`dedent`]. Every compiler-generated bash body in this module is
/// built by `format!()` with `\n\` continuations whose source
/// indentation leaks into the emitted YAML; `dedent()` strips it.
fn bash(name: impl Into<String>, script: impl Into<String>) -> BashStep {
    BashStep::new(name, dedent(&script.into()))
}

/// Strip the common leading whitespace from every non-empty line of
/// `s`, **and** strip trailing whitespace from every line. The
/// trailing-whitespace strip is critical for block-scalar emission:
/// serde_yaml falls back to the double-quoted form when a block
/// scalar contains lines with trailing spaces (because the scalar's
/// re-parse would lose them), which produces hard-to-read YAML.
///
/// Used to clean Rust source-string indentation out of the bash
/// bodies we hand to [`BashStep::new`]. Without this, the
/// `\n\`-continuation indent in Rust source ends up inside the
/// emitted YAML block scalar.
fn dedent(s: &str) -> String {
    let min = s
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.chars().take_while(|c| *c == ' ').count())
        .min()
        .unwrap_or(0);
    let mut out = String::with_capacity(s.len());
    let mut first = true;
    for line in s.lines() {
        if !first {
            out.push('\n');
        }
        first = false;
        // Only strip the leading `min` chars when the line actually
        // has that many leading spaces; otherwise leave it alone
        // (this avoids mangling interpolated content whose indent is
        // intentionally lower than the surrounding template indent).
        let leading_spaces = line.chars().take_while(|c| *c == ' ').count();
        let strip = leading_spaces.min(min);
        let stripped_leading = &line[strip..];
        let stripped = stripped_leading.trim_end_matches([' ', '\t']);
        out.push_str(stripped);
    }
    if s.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// Parse a legacy YAML env block (`env:\n  KEY: VALUE\n  KEY: VALUE`)
/// into typed `(name, EnvValue)` pairs preserving insertion order.
///
/// Each value is round-tripped through `serde_yaml` so quoted forms
/// (`"true"`, `"file"`) become bare literals and ADO macros (`$(X)`)
/// land as `EnvValue::PipelineVar` so the lowering pass re-emits the
/// macro form. Anything else lands as `EnvValue::Literal`.
fn parse_env_block(yaml_block: &str) -> Vec<(String, super::ir::env::EnvValue)> {
    use super::ir::env::EnvValue;
    if yaml_block.trim().is_empty() {
        return Vec::new();
    }
    let parsed: serde_yaml::Value = match serde_yaml::from_str(yaml_block) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let env_map = match parsed {
        serde_yaml::Value::Mapping(mut m) => {
            match m.shift_remove(serde_yaml::Value::String("env".into())) {
                Some(serde_yaml::Value::Mapping(inner)) => inner,
                _ => return Vec::new(),
            }
        }
        _ => return Vec::new(),
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
                if let Some(inner) = raw_value
                    .strip_prefix("$(")
                    .and_then(|s| s.strip_suffix(')'))
                    && !inner.contains('$')
                    && !inner.contains('(')
                {
                    out.push((key, EnvValue::pipeline_var(inner.to_string())));
                } else {
                    out.push((key, EnvValue::literal(raw_value.clone())));
                }
            }
            // Non-string scalars (numbers / bools): preserve the
            // typed scalar identity through RawYamlScalar so the
            // emitter doesn't quote them.
            other => {
                out.push((key, EnvValue::RawYamlScalar(other.clone())));
            }
        }
    }
    out
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

fn push_raw_yaml_if_nonempty(steps: &mut Vec<Step>, yaml: &str) {
    if yaml.trim().is_empty() {
        return;
    }
    // The body may contain one or more top-level `- ...` items (e.g.
    // engine_install_steps_yaml is two steps: install + version output).
    // Split them so each lands as a separate Step::RawYaml that
    // lower_raw_yaml can parse individually.
    for chunk in split_yaml_step_sequence(yaml) {
        steps.push(Step::RawYaml(chunk));
    }
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
/// item's body verbatim including its trailing newline. Each
/// returned string starts with `- ` so `lower_raw_yaml` can handle
/// it directly.
///
/// Single-item inputs return a one-element Vec.
fn split_yaml_step_sequence(yaml: &str) -> Vec<String> {
    let mut chunks: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut depth_was_zero = false;
    for line in yaml.lines() {
        if (line.starts_with("- ") || line == "-") && depth_was_zero {
            // New sequence item — flush.
            if !current.trim().is_empty() {
                chunks.push(strip_trailing_blank_lines(&current));
            }
            current.clear();
            current.push_str(line);
            current.push('\n');
            depth_was_zero = false;
        } else if line.starts_with("- ") || line == "-" {
            // First item — open the accumulator.
            current.push_str(line);
            current.push('\n');
            depth_was_zero = false;
        } else if line.trim().is_empty() {
            current.push_str(line);
            current.push('\n');
            depth_was_zero = true;
        } else {
            current.push_str(line);
            current.push('\n');
            depth_was_zero = false;
        }
    }
    if !current.trim().is_empty() {
        chunks.push(strip_trailing_blank_lines(&current));
    }
    chunks
}

/// Strip trailing blank-only lines from `s` but preserve a single
/// terminating newline if the final non-blank line was newline-terminated.
fn strip_trailing_blank_lines(s: &str) -> String {
    let trimmed: String = s.trim_end_matches([' ', '\t']).to_string();
    // Collapse runs of trailing newlines down to one.
    let mut end = trimmed.len();
    while end > 0 && trimmed.as_bytes()[end - 1] == b'\n' {
        end -= 1;
    }
    let mut out = trimmed[..end].to_string();
    if trimmed.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// Build the agent prompt body — either inlined imports or a
/// runtime-import marker. Mirrors `compile_shared`'s logic.
fn build_agent_content(
    front_matter: &FrontMatter,
    input_path: &Path,
    markdown_body: &str,
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
    Ok(format!("{{{{#runtime-import {}}}}}", marker_path))
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
