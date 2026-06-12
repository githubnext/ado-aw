//! Lower the typed IR ([`super::Pipeline`]) to a
//! [`serde_yaml::Value`] tree.
//!
//! ## Lowering context
//!
//! `EnvValue::StepOutput`, `EnvValue::Coalesce`, and
//! `Expr::StepOutput` need the consumer's location plus the producer's
//! location to pick the correct ADO reference syntax. The
//! [`LoweringContext`] carries the graph (see [`super::graph`]) and
//! the current consumer's stage / job so the recursive lowering
//! helpers stay pure.
//!
//! ## Shape contract
//!
//! Mapping keys are inserted in the order they appear in the
//! generated `serde_yaml::Mapping`, which `serde_yaml::to_string`
//! preserves. The canonical ordering is: identity keys first
//! (`job`, `displayName`, etc.), then static configuration
//! (`dependsOn`, `condition`, `pool`, `timeoutInMinutes`), then
//! payload (`steps` / `jobs` / `stages`). This matches the layout
//! reviewers are used to seeing in committed lock files.

use anyhow::{Context, Result};
use serde_yaml::{Mapping, Value};
use std::time::Duration;

use super::condition::codegen::{CondCodegenCtx, lower_condition};
use super::env::EnvValue;
use super::graph::Graph;
use super::ids::{JobId, StageId};
use super::job::{Job, Pool, TemplateDependsOnWrap};
use super::output::{ConsumerLocation, OutputRef, ProducerLocation, lower_outputref};
use super::stage::{Stage, StageExternalParamsWrap};
use super::step::{
    BashStep, CheckoutRepo, CheckoutStep, DownloadStep, PublishStep, Step, SubmodulesOpt, TaskStep,
};
use super::{
    CiTrigger, Parameter, ParameterDefault, ParameterKind, Pipeline, PipelineBody, PipelineResource,
    PipelineShape, PipelineVar, PrTrigger, RepositoryResource, Resources, Schedule,
};

/// Per-step lowering context carried through the recursive helpers.
///
/// Built once per step at `lower_job` time. Holds the graph (for
/// producer lookup) and the consumer's location (for syntax
/// selection).
pub struct LoweringContext<'a> {
    pub graph: &'a Graph,
    pub stage: Option<&'a StageId>,
    pub job: &'a JobId,
}

impl<'a> LoweringContext<'a> {
    fn consumer(&self) -> ConsumerLocation<'a> {
        ConsumerLocation {
            stage: self.stage,
            job: self.job,
        }
    }

    /// Build a [`CondCodegenCtx`] sharing the same producer-lookup
    /// and consumer-location data. Cheap (only borrows).
    fn cond_ctx(&self) -> CondCodegenCtx<'a> {
        CondCodegenCtx {
            graph: self.graph,
            stage: self.stage,
            job: self.job,
        }
    }
}

/// Lower a [`Pipeline`] to a [`serde_yaml::Value`].
///
/// Builds the dependency graph internally so callers don't have to
/// thread it through; if the graph fails validation, the error is
/// returned immediately. Use [`lower_with_graph`] when you have an
/// already-built graph.
pub fn lower(p: &Pipeline) -> Result<Value> {
    let graph = super::graph::build_graph(p).context("ir::lower: graph build failed")?;
    super::graph::detect_cycles(&graph).context("ir::lower: cycle detection failed")?;
    lower_with_graph(p, &graph)
}

/// Lower a [`Pipeline`] with an externally-provided [`Graph`]. The
/// graph **must** be one previously returned by
/// [`super::graph::build_graph`] for `p` (or equivalent); we trust
/// the producer locations recorded there.
pub fn lower_with_graph(p: &Pipeline, graph: &Graph) -> Result<Value> {
    let mut root = Mapping::new();

    // PipelineShape determines the top-level wrapping. The two
    // template shapes (`target: job` / `target: stage`) suppress
    // `name:`, `resources:`, and triggers — the parent pipeline owns
    // those concerns.
    let is_template = matches!(
        &p.shape,
        PipelineShape::JobTemplate { .. } | PipelineShape::StageTemplate { .. }
    );

    match &p.shape {
        PipelineShape::Standalone => {
            root.insert(s("name"), s(&p.name));
        }
        PipelineShape::JobTemplate { .. } | PipelineShape::StageTemplate { .. } => {
            // No top-level `name:` — the parent pipeline supplies one.
        }
        PipelineShape::OneEs { .. } => {
            unimplemented!(
                "PipelineShape::OneEs wrapping is introduced by the compile-target-1es commit"
            );
        }
    }

    // Top-level blocks, in the order the canonical lock files emit them:
    //   parameters → resources → schedules → pr → trigger → variables →
    //   (jobs|stages)
    //
    // Template shapes (`target: job` / `target: stage`) skip
    // `resources:` and triggers — the parent pipeline owns those.
    //
    // Each helper inserts its block only when its source data is
    // non-empty / configured, so an unused field produces no YAML key.
    if !p.parameters.is_empty() {
        root.insert(s("parameters"), lower_parameters(&p.parameters));
    }
    if !is_template {
        if let Some(resources) = lower_resources(&p.resources) {
            root.insert(s("resources"), resources);
        }
        if !p.triggers.schedules.is_empty() {
            root.insert(s("schedules"), lower_schedules(&p.triggers.schedules));
        }
        if let Some(pr) = lower_pr_trigger(p.triggers.pr.as_ref()) {
            root.insert(s("pr"), pr);
        }
        if let Some(ci) = lower_ci_trigger(p.triggers.ci.as_ref()) {
            root.insert(s("trigger"), ci);
        }
    }
    if !p.variables.is_empty() {
        root.insert(s("variables"), lower_variables(&p.variables));
    }

    match &p.body {
        PipelineBody::Jobs(jobs) => {
            let mut seq = Vec::with_capacity(jobs.len());
            for job in jobs {
                seq.push(lower_job(job, None, graph)?);
            }
            root.insert(s("jobs"), Value::Sequence(seq));
        }
        PipelineBody::Stages(stages) => {
            let mut seq = Vec::with_capacity(stages.len());
            for stage in stages {
                seq.push(lower_stage(stage, graph)?);
            }
            root.insert(s("stages"), Value::Sequence(seq));
        }
    }

    Ok(Value::Mapping(root))
}

/// Lower a `parameters:` block. Each entry becomes a mapping
/// `{ name, displayName?, type, default }` matching ADO's runtime-
/// parameter schema. `displayName:` is omitted for parameters with
/// `display_name == None` (used by auto-injected template parameters
/// `dependsOn` / `condition`). Defaults to the parameter's declared
/// default (no synthesised defaults for parameters with
/// `ParameterDefault::None`).
fn lower_parameters(params: &[Parameter]) -> Value {
    let mut seq = Vec::with_capacity(params.len());
    for p in params {
        let mut m = Mapping::new();
        m.insert(s("name"), s(&p.name));
        if let Some(dn) = &p.display_name {
            m.insert(s("displayName"), s(dn));
        }
        m.insert(
            s("type"),
            s(match p.kind {
                ParameterKind::Boolean => "boolean",
                ParameterKind::String => "string",
                ParameterKind::Number => "number",
                ParameterKind::Object => "object",
            }),
        );
        match &p.default {
            ParameterDefault::Bool(b) => {
                m.insert(s("default"), Value::Bool(*b));
            }
            ParameterDefault::String(v) => {
                m.insert(s("default"), s(v));
            }
            ParameterDefault::Number(n) => {
                m.insert(s("default"), Value::from(*n));
            }
            ParameterDefault::Sequence(items) => {
                m.insert(s("default"), Value::Sequence(items.clone()));
            }
            ParameterDefault::None => {}
        }
        if !p.values.is_empty() {
            m.insert(s("values"), Value::Sequence(p.values.clone()));
        }
        seq.push(Value::Mapping(m));
    }
    Value::Sequence(seq)
}

/// Lower a `resources:` block to a mapping with optional
/// `repositories:` / `pipelines:` keys. Returns `None` when both
/// lists are empty so the caller can elide the entire `resources:`
/// key.
fn lower_resources(r: &Resources) -> Option<Value> {
    if r.repositories.is_empty() && r.pipelines.is_empty() {
        return None;
    }
    let mut m = Mapping::new();
    if !r.repositories.is_empty() {
        let mut seq = Vec::with_capacity(r.repositories.len());
        for repo in &r.repositories {
            seq.push(lower_repository_resource(repo));
        }
        m.insert(s("repositories"), Value::Sequence(seq));
    }
    if !r.pipelines.is_empty() {
        let mut seq = Vec::with_capacity(r.pipelines.len());
        for pr in &r.pipelines {
            seq.push(lower_pipeline_resource(pr));
        }
        m.insert(s("pipelines"), Value::Sequence(seq));
    }
    Some(Value::Mapping(m))
}

fn lower_repository_resource(r: &RepositoryResource) -> Value {
    let mut m = Mapping::new();
    match r {
        RepositoryResource::SelfRepo { clean, submodules } => {
            m.insert(s("repository"), s("self"));
            m.insert(s("clean"), Value::Bool(*clean));
            m.insert(s("submodules"), Value::Bool(*submodules));
        }
        RepositoryResource::Named {
            identifier,
            kind,
            name,
            r#ref,
        } => {
            m.insert(s("repository"), s(identifier));
            m.insert(s("type"), s(kind));
            m.insert(s("name"), s(name));
            if let Some(r) = r#ref {
                m.insert(s("ref"), s(r));
            }
        }
    }
    Value::Mapping(m)
}

fn lower_pipeline_resource(p: &PipelineResource) -> Value {
    let mut m = Mapping::new();
    m.insert(s("pipeline"), s(&p.identifier));
    m.insert(s("source"), s(&p.source));
    if let Some(project) = &p.project {
        m.insert(s("project"), s(project));
    }
    if p.branches.is_empty() {
        // `trigger: true` means "trigger on any branch"
        m.insert(s("trigger"), Value::Bool(p.trigger));
    } else {
        let mut trigger_m = Mapping::new();
        let mut branches_m = Mapping::new();
        let include: Vec<Value> = p.branches.iter().map(s).collect();
        branches_m.insert(s("include"), Value::Sequence(include));
        trigger_m.insert(s("branches"), Value::Mapping(branches_m));
        m.insert(s("trigger"), Value::Mapping(trigger_m));
    }
    Value::Mapping(m)
}

fn lower_schedules(schedules: &[Schedule]) -> Value {
    let mut seq = Vec::with_capacity(schedules.len());
    for sch in schedules {
        let mut m = Mapping::new();
        m.insert(s("cron"), s(&sch.cron));
        m.insert(s("displayName"), s(&sch.display_name));
        if !sch.branches_include.is_empty() {
            let mut branches_m = Mapping::new();
            let include: Vec<Value> = sch.branches_include.iter().map(s).collect();
            branches_m.insert(s("include"), Value::Sequence(include));
            m.insert(s("branches"), Value::Mapping(branches_m));
        }
        if sch.always {
            m.insert(s("always"), Value::Bool(true));
        }
        seq.push(Value::Mapping(m));
    }
    Value::Sequence(seq)
}

/// Lower a `pr:` trigger. Returns `None` when no trigger is
/// configured (caller elides the key entirely — that's the "ADO
/// default" behaviour). Returns `Some(scalar "none")` for the
/// disabled form. Returns `Some(mapping)` for a configured PR
/// trigger with branch / path filters.
fn lower_pr_trigger(pr: Option<&PrTrigger>) -> Option<Value> {
    let pr = pr?;
    if pr.disabled {
        return Some(s("none"));
    }
    let mut m = Mapping::new();
    if !pr.branches_include.is_empty() || !pr.branches_exclude.is_empty() {
        let mut branches_m = Mapping::new();
        if !pr.branches_include.is_empty() {
            let include: Vec<Value> = pr.branches_include.iter().map(s).collect();
            branches_m.insert(s("include"), Value::Sequence(include));
        }
        if !pr.branches_exclude.is_empty() {
            let exclude: Vec<Value> = pr.branches_exclude.iter().map(s).collect();
            branches_m.insert(s("exclude"), Value::Sequence(exclude));
        }
        m.insert(s("branches"), Value::Mapping(branches_m));
    }
    if !pr.paths_include.is_empty() || !pr.paths_exclude.is_empty() {
        let mut paths_m = Mapping::new();
        if !pr.paths_include.is_empty() {
            let include: Vec<Value> = pr.paths_include.iter().map(s).collect();
            paths_m.insert(s("include"), Value::Sequence(include));
        }
        if !pr.paths_exclude.is_empty() {
            let exclude: Vec<Value> = pr.paths_exclude.iter().map(s).collect();
            paths_m.insert(s("exclude"), Value::Sequence(exclude));
        }
        m.insert(s("paths"), Value::Mapping(paths_m));
    }
    Some(Value::Mapping(m))
}

/// Lower a `trigger:` (CI) field. Returns `None` for "ADO default"
/// (no key emitted). Returns `Some(scalar "none")` for the disabled
/// form, which is the only non-default shape standalone uses today.
fn lower_ci_trigger(ci: Option<&CiTrigger>) -> Option<Value> {
    let ci = ci?;
    if ci.disabled {
        Some(s("none"))
    } else {
        // A fully-typed `trigger:` block (branches/paths) would land
        // here. Standalone agents today either use the ADO default
        // (no key) or `trigger: none`; the mapping shape can be
        // added when an emitter actually needs it.
        None
    }
}

fn lower_variables(vars: &[PipelineVar]) -> Value {
    let mut seq = Vec::with_capacity(vars.len());
    for v in vars {
        let mut m = Mapping::new();
        m.insert(s("name"), s(&v.name));
        m.insert(s("value"), s(&v.value));
        if v.is_secret {
            m.insert(s("isSecret"), Value::Bool(true));
        }
        seq.push(Value::Mapping(m));
    }
    Value::Sequence(seq)
}

fn lower_stage(stage: &Stage, graph: &Graph) -> Result<Value> {
    let mut m = Mapping::new();
    m.insert(s("stage"), s(stage.id.as_str()));
    m.insert(s("displayName"), s(&stage.display_name));
    if let Some(wrap) = &stage.external_params_wrap {
        // External-param wrap rule: when set, the stage carries no
        // typed `depends_on` / `condition` of its own (the caller
        // owns these via the template parameters). Surfacing both
        // simultaneously would produce two `dependsOn:` keys in the
        // emitted YAML.
        if !stage.depends_on.is_empty() || stage.condition.is_some() {
            return Err(anyhow::anyhow!(
                "ir::lower: stage '{}' has both external_params_wrap and typed depends_on/condition — these are mutually exclusive",
                stage.id
            ));
        }
        lower_stage_external_wrap(&mut m, wrap);
    } else {
        if !stage.depends_on.is_empty() {
            let deps: Vec<Value> = stage.depends_on.iter().map(|d| s(d.as_str())).collect();
            m.insert(s("dependsOn"), Value::Sequence(deps));
        }
        if let Some(cond) = &stage.condition {
            let ctx = LoweringContext {
                graph,
                stage: Some(&stage.id),
                // Stage-level conditions can reference cross-stage outputs;
                // there is no "consumer job" in that context. Use the
                // first job's id as a placeholder — the lowering only
                // distinguishes job identity for SAME-stage references,
                // and a cross-stage ref always picks the
                // `stageDependencies.*` syntax regardless of consumer job.
                job: stage
                    .jobs
                    .first()
                    .map(|j| &j.id)
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "ir::lower: stage '{}' has a condition but no jobs",
                            stage.id
                        )
                    })?,
            };
            m.insert(s("condition"), s(&lower_condition(&ctx.cond_ctx(), cond)?));
        }
    }
    let mut jobs = Vec::with_capacity(stage.jobs.len());
    for job in &stage.jobs {
        jobs.push(lower_job(job, Some(&stage.id), graph)?);
    }
    m.insert(s("jobs"), Value::Sequence(jobs));
    Ok(Value::Mapping(m))
}

/// Emit the `${{ if ne(length(parameters.<X>), 0) }}: dependsOn:` and
/// `${{ if ne(parameters.<Y>, '') }}: condition:` keys for a stage
/// whose external ordering is supplied at the template-invocation
/// site. The emitted YAML matches `src/data/stage-base.yml` (the
/// template the `target: stage` compiler used before the IR
/// migration).
fn lower_stage_external_wrap(m: &mut Mapping, wrap: &StageExternalParamsWrap) {
    // dependsOn branch (ne-only — no caller-omitted default emission)
    let mut dep_body = Mapping::new();
    dep_body.insert(
        s("dependsOn"),
        s(format!("${{{{ parameters.{} }}}}", wrap.depends_on_param)),
    );
    m.insert(
        s(format!(
            "${{{{ if ne(length(parameters.{}), 0) }}}}",
            wrap.depends_on_param
        )),
        Value::Mapping(dep_body),
    );
    // condition branch (ne-only)
    let mut cond_body = Mapping::new();
    cond_body.insert(
        s("condition"),
        s(format!("${{{{ parameters.{} }}}}", wrap.condition_param)),
    );
    m.insert(
        s(format!(
            "${{{{ if ne(parameters.{}, '') }}}}",
            wrap.condition_param
        )),
        Value::Mapping(cond_body),
    );
}

fn lower_job(job: &Job, stage: Option<&StageId>, graph: &Graph) -> Result<Value> {
    let ctx = LoweringContext {
        graph,
        stage,
        job: &job.id,
    };
    let mut m = Mapping::new();
    m.insert(s("job"), s(job.id.as_str()));
    m.insert(s("displayName"), s(&job.display_name));
    if let Some(wrap) = &job.template_dependson_wrap {
        lower_job_template_wrap(&mut m, job, wrap, &ctx)?;
    } else {
        if !job.depends_on.is_empty() {
            // Single-dep emits as a scalar `dependsOn: <name>` (matching
            // base.yml). Multi-dep emits as a sequence.
            if job.depends_on.len() == 1 {
                m.insert(s("dependsOn"), s(job.depends_on[0].as_str()));
            } else {
                let deps: Vec<Value> = job.depends_on.iter().map(|d| s(d.as_str())).collect();
                m.insert(s("dependsOn"), Value::Sequence(deps));
            }
        }
        if let Some(cond) = &job.condition {
            m.insert(s("condition"), s(&lower_condition(&ctx.cond_ctx(), cond)?));
        }
    }
    if let Some(t) = job.timeout {
        m.insert(s("timeoutInMinutes"), Value::from(minutes_ceil(t)));
    }
    if !job.variables.is_empty() {
        let mut vars = Mapping::new();
        for v in &job.variables {
            vars.insert(s(&v.name), s(&lower_env_value(&ctx, &v.value)?));
        }
        m.insert(s("variables"), Value::Mapping(vars));
    }
    m.insert(s("pool"), lower_pool(&job.pool));
    let mut steps = Vec::with_capacity(job.steps.len());
    for step in &job.steps {
        steps.push(lower_step(step, &ctx)?);
    }
    m.insert(s("steps"), Value::Sequence(steps));
    Ok(Value::Mapping(m))
}

/// Emit dual-branch `${{ if eq/ne(length(parameters.<X>), 0) }}` for
/// `dependsOn:` and `${{ if eq/ne(parameters.<Y>, '') }}` for
/// `condition:` to merge external template-parameter values with the
/// job's internal `depends_on` / `condition`.
///
/// Layout matches `common::generate_agentic_depends_on` for the
/// `target: job` branch (see `src/data/job-base.yml`):
///
/// - When internal `depends_on` is non-empty:
///   - `eq` branch → `dependsOn: <single>` (scalar) or
///     `dependsOn: [<list>]` (sequence).
///   - `ne` branch → list starting with internal deps, then
///     `${{ each d in parameters.<X> }}: - ${{ d }}`.
/// - When internal `depends_on` is empty:
///   - `ne`-only branch → `dependsOn: ${{ parameters.<X> }}`.
///
/// Condition mirrors: when internal is set, the eq-branch is the
/// internal body verbatim and the ne-branch appends
/// `${{ parameters.<Y> }}` into the same `and(…)`. When internal is
/// unset, only the ne-branch emits `condition: ${{ parameters.<Y> }}`.
fn lower_job_template_wrap(
    m: &mut Mapping,
    job: &Job,
    wrap: &TemplateDependsOnWrap,
    ctx: &LoweringContext<'_>,
) -> Result<()> {
    // ─── dependsOn ────────────────────────────────────────────────
    if !job.depends_on.is_empty() {
        // eq branch — internal only
        let mut eq_body = Mapping::new();
        if job.depends_on.len() == 1 {
            eq_body.insert(s("dependsOn"), s(job.depends_on[0].as_str()));
        } else {
            let deps: Vec<Value> = job.depends_on.iter().map(|d| s(d.as_str())).collect();
            eq_body.insert(s("dependsOn"), Value::Sequence(deps));
        }
        m.insert(
            s(format!(
                "${{{{ if eq(length(parameters.{}), 0) }}}}",
                wrap.depends_on_param
            )),
            Value::Mapping(eq_body),
        );
        // ne branch — list with internal deps then each external d
        let mut ne_body = Mapping::new();
        let mut seq: Vec<Value> =
            job.depends_on.iter().map(|d| s(d.as_str())).collect();
        // The `${{ each d in parameters.X }}: - ${{ d }}` pattern is a
        // template-expression nested mapping. We encode it as a
        // mapping whose key is the `${{ each ... }}` expression and
        // value is a one-element sequence `[${{ d }}]`.
        let mut each_inner = Mapping::new();
        each_inner.insert(
            s(format!(
                "${{{{ each d in parameters.{} }}}}",
                wrap.depends_on_param
            )),
            Value::Sequence(vec![s("${{ d }}")]),
        );
        seq.push(Value::Mapping(each_inner));
        ne_body.insert(s("dependsOn"), Value::Sequence(seq));
        m.insert(
            s(format!(
                "${{{{ if ne(length(parameters.{}), 0) }}}}",
                wrap.depends_on_param
            )),
            Value::Mapping(ne_body),
        );
    } else {
        // ne-only branch — caller value used as the entire dependsOn.
        let mut ne_body = Mapping::new();
        ne_body.insert(
            s("dependsOn"),
            s(format!("${{{{ parameters.{} }}}}", wrap.depends_on_param)),
        );
        m.insert(
            s(format!(
                "${{{{ if ne(length(parameters.{}), 0) }}}}",
                wrap.depends_on_param
            )),
            Value::Mapping(ne_body),
        );
    }

    // ─── condition ────────────────────────────────────────────────
    if let Some(internal_cond) = &job.condition {
        let internal_str = lower_condition(&ctx.cond_ctx(), internal_cond)?;
        // eq branch — internal condition verbatim.
        let mut eq_body = Mapping::new();
        eq_body.insert(s("condition"), s(&internal_str));
        m.insert(
            s(format!(
                "${{{{ if eq(parameters.{}, '') }}}}",
                wrap.condition_param
            )),
            Value::Mapping(eq_body),
        );
        // ne branch — internal condition with caller condition
        // appended into the same `and(…)` body. We extract the body
        // of the internal `and(...)` if present, otherwise wrap it.
        let merged = merge_condition_with_template_param(
            &internal_str,
            &wrap.condition_param,
        );
        let mut ne_body = Mapping::new();
        ne_body.insert(s("condition"), s(&merged));
        m.insert(
            s(format!(
                "${{{{ if ne(parameters.{}, '') }}}}",
                wrap.condition_param
            )),
            Value::Mapping(ne_body),
        );
    } else {
        // ne-only branch — caller value used as the entire condition.
        let mut ne_body = Mapping::new();
        ne_body.insert(
            s("condition"),
            s(format!("${{{{ parameters.{} }}}}", wrap.condition_param)),
        );
        m.insert(
            s(format!(
                "${{{{ if ne(parameters.{}, '') }}}}",
                wrap.condition_param
            )),
            Value::Mapping(ne_body),
        );
    }
    Ok(())
}

/// Append a `${{ parameters.<name> }}` clause into an existing ADO
/// condition string. When the input is already an `and(<args>)`
/// expression, the parameter is appended as an additional arg
/// (`and(<args>, ${{ parameters.<name> }})`). Otherwise the input is
/// wrapped: `and(<expr>, ${{ parameters.<name> }})`.
///
/// Mirrors the merge logic in
/// `common::generate_agentic_depends_on`'s condition body.
fn merge_condition_with_template_param(internal: &str, param_name: &str) -> String {
    let trimmed = internal.trim();
    let template_ref = format!("${{{{ parameters.{} }}}}", param_name);
    if let Some(rest) = trimmed.strip_prefix("and(")
        && let Some(inner) = rest.strip_suffix(')')
    {
        format!("and({}, {})", inner, template_ref)
    } else {
        format!("and({}, {})", trimmed, template_ref)
    }
}

fn lower_pool(pool: &Pool) -> Value {
    let mut m = Mapping::new();
    match pool {
        Pool::VmImage(img) => {
            m.insert(s("vmImage"), s(img));
        }
        Pool::Named { name, image, os } => {
            m.insert(s("name"), s(name));
            if let Some(img) = image {
                m.insert(s("image"), s(img));
            }
            if let Some(os) = os {
                m.insert(s("os"), s(os));
            }
        }
    }
    Value::Mapping(m)
}

pub(crate) fn lower_step(step: &Step, ctx: &LoweringContext<'_>) -> Result<Value> {
    match step {
        Step::Bash(b) => lower_bash(b, ctx),
        Step::Task(t) => lower_task(t, ctx),
        Step::Checkout(c) => Ok(lower_checkout(c)),
        Step::Download(d) => lower_download(d, ctx),
        Step::Publish(p) => lower_publish(p, ctx),
        Step::RawYaml(raw) => lower_raw_yaml(raw),
    }
}

/// Parse a `Step::RawYaml(...)` body into a `serde_yaml::Value`.
///
/// The body must be a single YAML mapping; we accept it with or
/// without a leading `- ` because some legacy emitters include it
/// (they're emitting a step inside an enclosing sequence). When the
/// `- ` is present, every subsequent line is also de-indented by two
/// columns so the mapping parses as a top-level document.
fn lower_raw_yaml(raw: &str) -> Result<Value> {
    let trimmed = raw.trim_start();
    let body = if let Some(rest) = trimmed.strip_prefix("- ") {
        // Strip 2 leading spaces from every line after the first so
        // the continuation lines aren't read as part of the first
        // line's scalar value.
        let mut out = String::with_capacity(rest.len());
        for (i, line) in rest.split_inclusive('\n').enumerate() {
            if i == 0 {
                out.push_str(line);
            } else {
                out.push_str(line.strip_prefix("  ").unwrap_or(line));
            }
        }
        out
    } else {
        trimmed.to_string()
    };
    let value: Value = serde_yaml::from_str(&body)
        .context("ir::lower: Step::RawYaml body is not a valid YAML mapping")?;
    Ok(value)
}

fn lower_bash(b: &BashStep, ctx: &LoweringContext<'_>) -> Result<Value> {
    // Field order matches the legacy YAML emitter for byte-equality:
    // bash → name → displayName → workingDirectory → timeoutInMinutes →
    // condition → continueOnError → env.
    let mut m = Mapping::new();
    m.insert(s("bash"), s(&b.script));
    if let Some(id) = &b.id {
        m.insert(s("name"), s(id.as_str()));
    }
    m.insert(s("displayName"), s(&b.display_name));
    if let Some(wd) = &b.working_directory {
        m.insert(s("workingDirectory"), s(wd));
    }
    if let Some(t) = b.timeout {
        m.insert(s("timeoutInMinutes"), Value::from(minutes_ceil(t)));
    }
    if let Some(cond) = &b.condition {
        m.insert(s("condition"), s(&lower_condition(&ctx.cond_ctx(), cond)?));
    }
    if b.continue_on_error {
        m.insert(s("continueOnError"), Value::Bool(true));
    }
    if !b.env.is_empty() {
        let mut env_map = Mapping::new();
        for (k, v) in &b.env {
            // RawYamlScalar bypasses string lowering — its inner value
            // is inserted into the env mapping directly so serde_yaml's
            // emitter sees the original scalar type (e.g. number vs
            // quoted string).
            let value = match v {
                EnvValue::RawYamlScalar(raw) => raw.clone(),
                other => s(&lower_env_value(ctx, other)?),
            };
            env_map.insert(s(k), value);
        }
        m.insert(s("env"), Value::Mapping(env_map));
    }
    Ok(Value::Mapping(m))
}

fn lower_task(t: &TaskStep, ctx: &LoweringContext<'_>) -> Result<Value> {
    // Field order matches the legacy YAML emitter for byte-equality with
    // committed lock files: task → name → inputs → displayName →
    // timeoutInMinutes → condition → continueOnError → env.
    let mut m = Mapping::new();
    m.insert(s("task"), s(&t.task));
    if let Some(id) = &t.id {
        m.insert(s("name"), s(id.as_str()));
    }
    if !t.inputs.is_empty() {
        let mut inputs = Mapping::new();
        for (k, v) in &t.inputs {
            inputs.insert(s(k), s(v));
        }
        m.insert(s("inputs"), Value::Mapping(inputs));
    }
    m.insert(s("displayName"), s(&t.display_name));
    if let Some(timeout) = t.timeout {
        m.insert(s("timeoutInMinutes"), Value::from(minutes_ceil(timeout)));
    }
    if let Some(cond) = &t.condition {
        m.insert(s("condition"), s(&lower_condition(&ctx.cond_ctx(), cond)?));
    }
    if t.continue_on_error {
        m.insert(s("continueOnError"), Value::Bool(true));
    }
    if !t.env.is_empty() {
        let mut env_map = Mapping::new();
        for (k, v) in &t.env {
            let value = match v {
                EnvValue::RawYamlScalar(raw) => raw.clone(),
                other => s(&lower_env_value(ctx, other)?),
            };
            env_map.insert(s(k), value);
        }
        m.insert(s("env"), Value::Mapping(env_map));
    }
    Ok(Value::Mapping(m))
}

fn lower_checkout(c: &CheckoutStep) -> Value {
    let mut m = Mapping::new();
    match &c.repository {
        CheckoutRepo::Self_ => {
            m.insert(s("checkout"), s("self"));
        }
        CheckoutRepo::Named(name) => {
            m.insert(s("checkout"), s(name));
        }
    }
    if let Some(clean) = c.clean {
        m.insert(s("clean"), Value::Bool(clean));
    }
    if let Some(sub) = &c.submodules {
        let v = match sub {
            SubmodulesOpt::True => s("true"),
            SubmodulesOpt::False => s("false"),
            SubmodulesOpt::Recursive => s("recursive"),
        };
        m.insert(s("submodules"), v);
    }
    if let Some(fd) = c.fetch_depth {
        m.insert(s("fetchDepth"), Value::from(fd));
    }
    if let Some(pc) = c.persist_credentials {
        m.insert(s("persistCredentials"), Value::Bool(pc));
    }
    Value::Mapping(m)
}

fn lower_download(d: &DownloadStep, ctx: &LoweringContext<'_>) -> Result<Value> {
    let mut m = Mapping::new();
    m.insert(s("download"), s(&d.source));
    m.insert(s("artifact"), s(&d.artifact));
    if let Some(cond) = &d.condition {
        m.insert(s("condition"), s(&lower_condition(&ctx.cond_ctx(), cond)?));
    }
    Ok(Value::Mapping(m))
}

fn lower_publish(p: &PublishStep, ctx: &LoweringContext<'_>) -> Result<Value> {
    let mut m = Mapping::new();
    m.insert(s("publish"), s(&p.path));
    m.insert(s("artifact"), s(&p.artifact));
    if let Some(cond) = &p.condition {
        m.insert(s("condition"), s(&lower_condition(&ctx.cond_ctx(), cond)?));
    }
    Ok(Value::Mapping(m))
}

/// Lower an [`EnvValue`] to its ADO scalar form. `StepOutput` and
/// `Coalesce` variants use the consumer location from `ctx` to pick
/// the right reference syntax via [`lower_outputref`].
fn lower_env_value(ctx: &LoweringContext<'_>, v: &EnvValue) -> Result<String> {
    match v {
        EnvValue::Literal(s) => Ok(s.clone()),
        EnvValue::AdoMacro(name) => Ok(format!("$({name})")),
        EnvValue::PipelineVar(name) => Ok(format!("$({name})")),
        EnvValue::Secret(name) => Ok(format!("$({name})")),
        EnvValue::StepOutput(r) => Ok(lower_outputref_for(ctx, r)?),
        EnvValue::Coalesce(children) => {
            let mut parts: Vec<String> = Vec::with_capacity(children.len() + 1);
            for c in children {
                // Inside Coalesce, AdoMacro / PipelineVar / Secret /
                // StepOutput lower to ADO **expression** atoms (not
                // macro-form $()). Variables: `variables['Name']`;
                // step outputs: same reference syntax as outside,
                // but without the `$()` wrap because we're already
                // inside `$[ … ]`.
                parts.push(lower_env_value_as_expr_atom(ctx, c)?);
            }
            parts.push("''".to_string());
            Ok(format!("$[ coalesce({}) ]", parts.join(", ")))
        }
        EnvValue::Concat(children) => {
            // Macro-form concatenation: lower each child in macro
            // context (NOT expression-atom) and join verbatim. This
            // keeps the resulting scalar a plain ADO macro string so
            // same-job consumers see the macro form `$(stepName.X)`,
            // which is the only form that resolves correctly inside
            // the producing job. See `EnvValue::Concat` doc-comment
            // for the bug history.
            let mut out = String::new();
            for c in children {
                out.push_str(&lower_env_value(ctx, c)?);
            }
            Ok(out)
        }
        EnvValue::RawYamlScalar(raw) => {
            // String fallback for callers that still go through
            // `lower_env_value`; the env-mapping insertion path in
            // `lower_bash` / `lower_task` short-circuits this variant
            // to preserve typed scalar identity.
            Ok(yaml_value_to_scalar_string(raw))
        }
    }
}

fn yaml_value_to_scalar_string(v: &serde_yaml::Value) -> String {
    match v {
        serde_yaml::Value::String(s) => s.clone(),
        serde_yaml::Value::Number(n) => n.to_string(),
        serde_yaml::Value::Bool(b) => b.to_string(),
        serde_yaml::Value::Null => String::new(),
        other => serde_yaml::to_string(other).unwrap_or_default().trim().to_string(),
    }
}

/// Sub-expression form for atoms inside `$[ coalesce(...) ]`.
///
/// Inside an ADO runtime expression, predefined variables use
/// `variables['Name']`, not `$(Name)`. Step output references inside
/// expressions use the *unwrapped* `dependencies.X` /
/// `stageDependencies.X` / `variables['stepName.X']` form.
fn lower_env_value_as_expr_atom(ctx: &LoweringContext<'_>, v: &EnvValue) -> Result<String> {
    match v {
        EnvValue::Literal(s) => Ok(format!("'{}'", s.replace('\'', "''"))),
        EnvValue::AdoMacro(name) => Ok(format!("variables['{name}']")),
        EnvValue::PipelineVar(name) => Ok(format!("variables['{name}']")),
        EnvValue::Secret(name) => Ok(format!("variables['{name}']")),
        EnvValue::StepOutput(r) => Ok(lower_outputref_for_expr(ctx, r)?),
        EnvValue::Coalesce(children) => {
            // Flatten nested Coalesce: their children appear inline
            // in the enclosing one's argument list. This matches the
            // documented behaviour in `EnvValue` doc-comments.
            let mut parts: Vec<String> = Vec::with_capacity(children.len());
            for c in children {
                parts.push(lower_env_value_as_expr_atom(ctx, c)?);
            }
            // Don't wrap in `$[ … ]` again — we are already inside one.
            Ok(format!("coalesce({})", parts.join(", ")))
        }
        EnvValue::Concat(_) => {
            // `Concat` is a macro-form construct (no `$[ … ]` wrap).
            // It does not have a natural lowering inside an
            // expression-atom context — the macro syntax `$(…)` is
            // not an ADO expression atom. If a future caller wants
            // concat semantics inside an expression, they should
            // express it with string concatenation operators that
            // ADO expressions support. For now, this is an authoring
            // error.
            anyhow::bail!(
                "ir::lower: EnvValue::Concat is not valid inside a Coalesce \
                 (or other expression-atom context); use Concat at the top \
                 level of an env value only"
            )
        }
        EnvValue::RawYamlScalar(raw) => {
            // Inside an ADO expression, render the raw scalar as a
            // single-quoted literal (numbers / booleans → literal
            // text without quotes).
            match raw {
                serde_yaml::Value::String(s) => Ok(format!("'{}'", s.replace('\'', "''"))),
                serde_yaml::Value::Number(n) => Ok(n.to_string()),
                serde_yaml::Value::Bool(b) => Ok(b.to_string()),
                other => Ok(yaml_value_to_scalar_string(other)),
            }
        }
    }
}

/// Lower an OutputRef in macro form (suitable for direct env-value
/// substitution): the result is the **whole** ADO scalar.
fn lower_outputref_for(ctx: &LoweringContext<'_>, r: &OutputRef) -> Result<String> {
    let producer_loc = ctx.graph.step_locations.get(&r.step).ok_or_else(|| {
        anyhow::anyhow!(
            "ir::lower: OutputRef references unknown step '{}' \
             (graph::build_graph should have caught this)",
            r.step
        )
    })?;
    let producer = ProducerLocation {
        stage: producer_loc.stage.as_ref(),
        job: &producer_loc.job,
    };
    Ok(lower_outputref(ctx.consumer(), producer, r))
}

/// Lower an OutputRef in **expression-atom** form (no `$(...)` wrap).
fn lower_outputref_for_expr(ctx: &LoweringContext<'_>, r: &OutputRef) -> Result<String> {
    let producer_loc = ctx.graph.step_locations.get(&r.step).ok_or_else(|| {
        anyhow::anyhow!(
            "ir::lower: OutputRef references unknown step '{}' \
             (graph::build_graph should have caught this)",
            r.step
        )
    })?;
    let producer = ProducerLocation {
        stage: producer_loc.stage.as_ref(),
        job: &producer_loc.job,
    };
    // Reuse the same lowering and strip the `$()` wrap for same-job
    // macro form, since we're inside `$[ … ]` already.
    let lowered = lower_outputref(ctx.consumer(), producer, r);
    if let Some(rest) = lowered.strip_prefix("$(").and_then(|s| s.strip_suffix(')')) {
        // Same-job macro: `$(step.name)` → expression form
        // `variables['step.name']`. ADO runtime expressions cannot
        // see step outputs from the producing job via `variables[…]`
        // either; this is the same limitation as `compile_gate_step_external`
        // documents in src/compile/filter_ir.rs. Coalesce inputs
        // should therefore not target same-job outputs — the caller
        // chooses Coalesce only for cross-job/cross-stage cases.
        Ok(format!("variables['{rest}']"))
    } else {
        Ok(lowered)
    }
}

fn minutes_ceil(d: Duration) -> u64 {
    let secs = d.as_secs();
    secs.div_ceil(60)
}

fn s(v: impl Into<String>) -> Value {
    Value::String(v.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::ir::condition::Condition;
    use crate::compile::ir::ids::{JobId, StepId};
    use crate::compile::ir::output::OutputDecl;
    use crate::compile::ir::step::BashStep;
    use crate::compile::ir::{PipelineBody, PipelineShape, Resources, Triggers};

    fn ctx_for<'a>(graph: &'a Graph, job: &'a JobId) -> LoweringContext<'a> {
        LoweringContext {
            graph,
            stage: None,
            job,
        }
    }

    #[test]
    fn lower_condition_static_variants() {
        // Quick sanity that lower.rs threads the condition codegen
        // through. Full coverage lives in `condition::codegen::tests`.
        let g = Graph::default();
        let job = JobId::new("J").unwrap();
        let ctx = ctx_for(&g, &job);
        assert_eq!(
            lower_condition(&ctx.cond_ctx(), &Condition::Succeeded).unwrap(),
            "succeeded()"
        );
    }

    #[test]
    fn lower_env_value_simple_variants() {
        let g = Graph::default();
        let job = JobId::new("J").unwrap();
        let ctx = ctx_for(&g, &job);
        assert_eq!(lower_env_value(&ctx, &EnvValue::literal("x")).unwrap(), "x");
        assert_eq!(
            lower_env_value(&ctx, &EnvValue::ado_macro("Build.Reason").unwrap()).unwrap(),
            "$(Build.Reason)"
        );
        assert_eq!(
            lower_env_value(&ctx, &EnvValue::pipeline_var("MY_VAR")).unwrap(),
            "$(MY_VAR)"
        );
        assert_eq!(
            lower_env_value(&ctx, &EnvValue::secret("MCP_API_KEY")).unwrap(),
            "$(MCP_API_KEY)"
        );
    }

    #[test]
    fn lower_env_value_coalesce_produces_canonical_form() {
        // Build a pipeline with synthPr producer in Setup and a
        // consumer in Agent so the producer location resolves through
        // the graph correctly.
        let synth = StepId::new("synthPr").unwrap();
        let producer = Step::Bash(
            BashStep::new("Setup", "echo s")
                .with_id(synth.clone())
                .with_output(OutputDecl::new("AW_SYNTHETIC_PR_ID")),
        );
        let mut setup = Job::new(JobId::new("Setup").unwrap(), "Setup", Pool::VmImage("u".into()));
        setup.push_step(producer);
        let agent_job = Job::new(JobId::new("Agent").unwrap(), "Agent", Pool::VmImage("u".into()));
        let p = Pipeline {
            name: "t".into(),
            parameters: Vec::new(),
            resources: Resources::default(),
            triggers: Triggers::default(),
            variables: Vec::new(),
            body: PipelineBody::Jobs(vec![setup, agent_job]),
            shape: PipelineShape::Standalone,
        };
        let g = super::super::graph::build_graph(&p).unwrap();

        let agent_id = JobId::new("Agent").unwrap();
        let ctx = LoweringContext {
            graph: &g,
            stage: None,
            job: &agent_id,
        };

        let v = EnvValue::coalesce(vec![
            EnvValue::ado_macro("System.PullRequest.PullRequestId").unwrap(),
            EnvValue::step_output(OutputRef::new(synth, "AW_SYNTHETIC_PR_ID")),
        ]);
        assert_eq!(
            lower_env_value(&ctx, &v).unwrap(),
            "$[ coalesce(variables['System.PullRequest.PullRequestId'], dependencies.Setup.outputs['synthPr.AW_SYNTHETIC_PR_ID'], '') ]"
        );
    }

    /// `EnvValue::Concat` lowers to the macro-form concatenation of
    /// each child's lowered scalar — no `$[ … ]` wrap, no separator.
    /// For a same-job consumer the StepOutput child resolves to the
    /// macro form `$(stepName.X)`, so the final string is the
    /// `$(System.PullRequest.X)$(synthPr.X)` exclusive-OR concat
    /// used by the gate step today.
    #[test]
    fn lower_env_value_concat_produces_macro_form_for_same_job() {
        let synth = StepId::new("synthPr").unwrap();
        let producer = Step::Bash(
            BashStep::new("synth", "echo s")
                .with_id(synth.clone())
                .with_output(OutputDecl::new("AW_SYNTHETIC_PR_ID")),
        );
        let consumer = Step::Bash(BashStep::new("gate", "node gate.js"));
        let mut setup = Job::new(JobId::new("Setup").unwrap(), "Setup", Pool::VmImage("u".into()));
        setup.push_step(producer);
        setup.push_step(consumer);
        let p = Pipeline {
            name: "t".into(),
            parameters: Vec::new(),
            resources: Resources::default(),
            triggers: Triggers::default(),
            variables: Vec::new(),
            body: PipelineBody::Jobs(vec![setup]),
            shape: PipelineShape::Standalone,
        };
        let g = super::super::graph::build_graph(&p).unwrap();

        let setup_id = JobId::new("Setup").unwrap();
        let ctx = LoweringContext {
            graph: &g,
            stage: None,
            job: &setup_id,
        };

        let v = EnvValue::concat(vec![
            EnvValue::ado_macro("System.PullRequest.PullRequestId").unwrap(),
            EnvValue::step_output(OutputRef::new(synth, "AW_SYNTHETIC_PR_ID")),
        ]);
        assert_eq!(
            lower_env_value(&ctx, &v).unwrap(),
            "$(System.PullRequest.PullRequestId)$(synthPr.AW_SYNTHETIC_PR_ID)"
        );
    }

    /// `EnvValue::Concat` is not valid inside a Coalesce — the macro
    /// form `$(…)` is not an ADO expression atom.
    #[test]
    fn lower_env_value_concat_inside_coalesce_errors() {
        let synth = StepId::new("synthPr").unwrap();
        let producer = Step::Bash(
            BashStep::new("synth", "echo s")
                .with_id(synth.clone())
                .with_output(OutputDecl::new("X")),
        );
        let mut setup = Job::new(JobId::new("Setup").unwrap(), "Setup", Pool::VmImage("u".into()));
        setup.push_step(producer);
        let p = Pipeline {
            name: "t".into(),
            parameters: Vec::new(),
            resources: Resources::default(),
            triggers: Triggers::default(),
            variables: Vec::new(),
            body: PipelineBody::Jobs(vec![setup]),
            shape: PipelineShape::Standalone,
        };
        let g = super::super::graph::build_graph(&p).unwrap();

        let setup_id = JobId::new("Setup").unwrap();
        let ctx = LoweringContext {
            graph: &g,
            stage: None,
            job: &setup_id,
        };

        let v = EnvValue::coalesce(vec![EnvValue::concat(vec![
            EnvValue::literal("a"),
            EnvValue::literal("b"),
        ])]);
        let err = lower_env_value(&ctx, &v).unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("Concat is not valid inside a Coalesce"),
            "expected Concat-in-Coalesce error, got: {msg}"
        );
    }

    #[test]
    fn lower_job_emits_canonical_key_order() {
        let mut job = Job::new(
            JobId::new("Agent").unwrap(),
            "Agent",
            Pool::VmImage("ubuntu-22.04".into()),
        );
        job.depends_on.push(JobId::new("Setup").unwrap());
        job.condition = Some(Condition::Succeeded);
        job.push_step(Step::Bash(BashStep::new("ado-aw", "echo hi")));

        let g = Graph::default();
        let v = lower_job(&job, None, &g).unwrap();
        let m = match v {
            Value::Mapping(m) => m,
            _ => panic!(),
        };
        let keys: Vec<&str> = m.keys().filter_map(|k| k.as_str()).collect();
        assert_eq!(
            keys,
            vec!["job", "displayName", "dependsOn", "condition", "pool", "steps"]
        );
    }

    #[test]
    fn minutes_ceil_rounds_up_partial_minutes() {
        assert_eq!(minutes_ceil(Duration::from_secs(0)), 0);
        assert_eq!(minutes_ceil(Duration::from_secs(1)), 1);
        assert_eq!(minutes_ceil(Duration::from_secs(60)), 1);
        assert_eq!(minutes_ceil(Duration::from_secs(61)), 2);
    }

    #[test]
    fn raw_yaml_step_round_trips_into_steps_sequence() {
        // The RawYaml migration bridge must carry pre-formatted step
        // YAML through the canonical normalisation: parse the body
        // into a serde_yaml::Value, re-emit it as part of the
        // surrounding sequence.
        let raw = "bash: |\n  echo legacy\ndisplayName: Legacy step\n";
        let mut job = Job::new(
            JobId::new("Agent").unwrap(),
            "Agent",
            Pool::VmImage("ubuntu-22.04".into()),
        );
        job.push_step(Step::RawYaml(raw.to_string()));
        let p = Pipeline {
            name: "t".into(),
            parameters: Vec::new(),
            resources: Resources::default(),
            triggers: Triggers::default(),
            variables: Vec::new(),
            body: PipelineBody::Jobs(vec![job]),
            shape: PipelineShape::Standalone,
        };
        let v = super::lower(&p).unwrap();
        let step = &v["jobs"][0]["steps"][0];
        assert_eq!(step["bash"].as_str(), Some("echo legacy\n"));
        assert_eq!(step["displayName"].as_str(), Some("Legacy step"));
    }

    #[test]
    fn raw_yaml_step_accepts_leading_dash() {
        // Some legacy emitters include the leading `- ` because they
        // were emitting into an enclosing sequence; the lowering must
        // strip it.
        let raw = "- bash: echo dash\n  displayName: With dash\n";
        let mut job = Job::new(
            JobId::new("Agent").unwrap(),
            "Agent",
            Pool::VmImage("ubuntu-22.04".into()),
        );
        job.push_step(Step::RawYaml(raw.to_string()));
        let p = Pipeline {
            name: "t".into(),
            parameters: Vec::new(),
            resources: Resources::default(),
            triggers: Triggers::default(),
            variables: Vec::new(),
            body: PipelineBody::Jobs(vec![job]),
            shape: PipelineShape::Standalone,
        };
        let v = super::lower(&p).unwrap();
        let step = &v["jobs"][0]["steps"][0];
        assert_eq!(step["bash"].as_str(), Some("echo dash"));
    }

    #[test]
    fn raw_yaml_step_rejects_invalid_body() {
        let mut job = Job::new(
            JobId::new("Agent").unwrap(),
            "Agent",
            Pool::VmImage("ubuntu-22.04".into()),
        );
        job.push_step(Step::RawYaml("not: [valid yaml".to_string()));
        let p = Pipeline {
            name: "t".into(),
            parameters: Vec::new(),
            resources: Resources::default(),
            triggers: Triggers::default(),
            variables: Vec::new(),
            body: PipelineBody::Jobs(vec![job]),
            shape: PipelineShape::Standalone,
        };
        let err = super::lower(&p).unwrap_err();
        assert!(format!("{err:#}").contains("Step::RawYaml"));
    }

    // ── Phase 0: top-level pipeline lowering tests ─────────────────

    /// `parameters:` with a Boolean default round-trips through emit
    /// to the canonical ADO runtime-parameter shape.
    #[test]
    fn lower_parameters_emits_typed_runtime_parameter() {
        let p = Pipeline {
            name: "P".into(),
            parameters: vec![Parameter {
                name: "clearMemory".into(),
                display_name: Some("Clear agent memory".into()),
                kind: ParameterKind::Boolean,
                default: ParameterDefault::Bool(false),
                values: Vec::new(),
            }],
            resources: Resources::default(),
            triggers: Triggers::default(),
            variables: Vec::new(),
            body: PipelineBody::Jobs(Vec::new()),
            shape: PipelineShape::Standalone,
        };
        let g = Graph::default();
        let v = lower_with_graph(&p, &g).unwrap();
        let yaml = serde_yaml::to_string(&v).unwrap();
        assert!(
            yaml.contains("name: clearMemory"),
            "parameters entry must include name; got: {yaml}"
        );
        assert!(yaml.contains("type: boolean"));
        assert!(yaml.contains("default: false"));
        assert!(yaml.contains("displayName: Clear agent memory"));
    }

    /// `resources.repositories` always emits the canonical `self`
    /// entry with `clean: true` and `submodules: true`.
    #[test]
    fn lower_resources_emits_self_repository_with_clean_and_submodules() {
        let p = Pipeline {
            name: "P".into(),
            parameters: Vec::new(),
            resources: Resources {
                repositories: vec![RepositoryResource::SelfRepo {
                    clean: true,
                    submodules: true,
                }],
                pipelines: Vec::new(),
            },
            triggers: Triggers::default(),
            variables: Vec::new(),
            body: PipelineBody::Jobs(Vec::new()),
            shape: PipelineShape::Standalone,
        };
        let g = Graph::default();
        let v = lower_with_graph(&p, &g).unwrap();
        let yaml = serde_yaml::to_string(&v).unwrap();
        assert!(yaml.contains("repository: self"));
        assert!(yaml.contains("clean: true"));
        assert!(yaml.contains("submodules: true"));
    }

    /// `resources` with both repositories and pipelines emits both
    /// sub-keys in canonical order.
    #[test]
    fn lower_resources_emits_pipelines_block_when_present() {
        let p = Pipeline {
            name: "P".into(),
            parameters: Vec::new(),
            resources: Resources {
                repositories: vec![RepositoryResource::SelfRepo {
                    clean: true,
                    submodules: true,
                }],
                pipelines: vec![PipelineResource {
                    identifier: "upstream_build".into(),
                    source: "Upstream Build".into(),
                    project: Some("OneBranch".into()),
                    branches: vec!["main".into(), "release/*".into()],
                    trigger: true,
                }],
            },
            triggers: Triggers::default(),
            variables: Vec::new(),
            body: PipelineBody::Jobs(Vec::new()),
            shape: PipelineShape::Standalone,
        };
        let g = Graph::default();
        let v = lower_with_graph(&p, &g).unwrap();
        let yaml = serde_yaml::to_string(&v).unwrap();
        assert!(yaml.contains("pipeline: upstream_build"));
        assert!(yaml.contains("source: Upstream Build"));
        assert!(yaml.contains("project: OneBranch"));
        // With non-empty branches, trigger becomes a mapping with
        // branches.include — not a bare `trigger: true`.
        assert!(yaml.contains("trigger:"));
        assert!(yaml.contains("include:"));
        assert!(yaml.contains("- main"));
        assert!(yaml.contains("- release/*"));
    }

    /// `schedules:` round-trips cron + displayName + branches.include
    /// + always:true to the canonical lock-file shape.
    #[test]
    fn lower_schedules_emits_canonical_block() {
        let p = Pipeline {
            name: "P".into(),
            parameters: Vec::new(),
            resources: Resources::default(),
            triggers: Triggers {
                schedules: vec![Schedule {
                    cron: "44 2 * * 1".into(),
                    display_name: "Scheduled run".into(),
                    branches_include: vec!["main".into()],
                    always: true,
                }],
                pr: None,
                ci: None,
            },
            variables: Vec::new(),
            body: PipelineBody::Jobs(Vec::new()),
            shape: PipelineShape::Standalone,
        };
        let g = Graph::default();
        let v = lower_with_graph(&p, &g).unwrap();
        let yaml = serde_yaml::to_string(&v).unwrap();
        assert!(yaml.contains("cron: 44 2 * * 1"));
        assert!(yaml.contains("displayName: Scheduled run"));
        assert!(yaml.contains("always: true"));
        assert!(yaml.contains("- main"));
    }

    /// `pr: none` and `trigger: none` round-trip as plain scalars.
    /// This is the shape every standalone fixture uses today.
    #[test]
    fn lower_pr_and_trigger_none_emits_bare_scalars() {
        let p = Pipeline {
            name: "P".into(),
            parameters: Vec::new(),
            resources: Resources::default(),
            triggers: Triggers {
                schedules: Vec::new(),
                pr: Some(PrTrigger {
                    branches_include: Vec::new(),
                    branches_exclude: Vec::new(),
                    paths_include: Vec::new(),
                    paths_exclude: Vec::new(),
                    disabled: true,
                }),
                ci: Some(CiTrigger { disabled: true }),
            },
            variables: Vec::new(),
            body: PipelineBody::Jobs(Vec::new()),
            shape: PipelineShape::Standalone,
        };
        let g = Graph::default();
        let v = lower_with_graph(&p, &g).unwrap();
        let yaml = serde_yaml::to_string(&v).unwrap();
        assert!(yaml.contains("pr: none"), "expected `pr: none`; got: {yaml}");
        assert!(
            yaml.contains("trigger: none"),
            "expected `trigger: none`; got: {yaml}"
        );
    }

    /// Configured `pr:` block with branch + path filters emits the
    /// nested mapping shape ADO expects.
    #[test]
    fn lower_pr_trigger_with_filters_emits_branches_and_paths_blocks() {
        let p = Pipeline {
            name: "P".into(),
            parameters: Vec::new(),
            resources: Resources::default(),
            triggers: Triggers {
                schedules: Vec::new(),
                pr: Some(PrTrigger {
                    branches_include: vec!["main".into()],
                    branches_exclude: vec!["dev/*".into()],
                    paths_include: vec!["src/**".into()],
                    paths_exclude: vec!["docs/**".into()],
                    disabled: false,
                }),
                ci: None,
            },
            variables: Vec::new(),
            body: PipelineBody::Jobs(Vec::new()),
            shape: PipelineShape::Standalone,
        };
        let g = Graph::default();
        let v = lower_with_graph(&p, &g).unwrap();
        let yaml = serde_yaml::to_string(&v).unwrap();
        // `pr:` mapping with branches + paths sub-mappings.
        assert!(yaml.contains("pr:"));
        assert!(yaml.contains("branches:"));
        assert!(yaml.contains("paths:"));
        assert!(yaml.contains("- main"));
        assert!(yaml.contains("- dev/*"));
        assert!(yaml.contains("src/**"));
        assert!(yaml.contains("docs/**"));
        // Defensive: must NOT collapse to `pr: none`.
        assert!(!yaml.contains("pr: none"));
    }

    /// When `Triggers` defaults are used (no schedules, no pr, no
    /// ci), `lower_with_graph` MUST emit no `pr:` / `trigger:` /
    /// `schedules:` keys at all (so ADO falls back to "trigger on
    /// any branch" defaults). The canonical lock files never use
    /// this shape, but it's the correct ADO default and the
    /// `compile-target-job` / `compile-target-stage` commits rely
    /// on it.
    #[test]
    fn lower_with_default_triggers_emits_no_trigger_keys() {
        let p = Pipeline {
            name: "P".into(),
            parameters: Vec::new(),
            resources: Resources::default(),
            triggers: Triggers::default(),
            variables: Vec::new(),
            body: PipelineBody::Jobs(Vec::new()),
            shape: PipelineShape::Standalone,
        };
        let g = Graph::default();
        let v = lower_with_graph(&p, &g).unwrap();
        let yaml = serde_yaml::to_string(&v).unwrap();
        assert!(!yaml.contains("pr:"));
        assert!(!yaml.contains("trigger:"));
        assert!(!yaml.contains("schedules:"));
        assert!(!yaml.contains("parameters:"));
        assert!(!yaml.contains("resources:"));
        assert!(!yaml.contains("variables:"));
    }

    /// `variables:` lowers to a sequence of name/value mappings;
    /// secrets carry the `isSecret: true` flag.
    #[test]
    fn lower_variables_emits_name_value_and_secret_flag() {
        let p = Pipeline {
            name: "P".into(),
            parameters: Vec::new(),
            resources: Resources::default(),
            triggers: Triggers::default(),
            variables: vec![
                PipelineVar {
                    name: "PLAIN_VAR".into(),
                    value: "hello".into(),
                    is_secret: false,
                },
                PipelineVar {
                    name: "SECRET_VAR".into(),
                    value: "$(SC_TOKEN)".into(),
                    is_secret: true,
                },
            ],
            body: PipelineBody::Jobs(Vec::new()),
            shape: PipelineShape::Standalone,
        };
        let g = Graph::default();
        let v = lower_with_graph(&p, &g).unwrap();
        let yaml = serde_yaml::to_string(&v).unwrap();
        assert!(yaml.contains("name: PLAIN_VAR"));
        assert!(yaml.contains("value: hello"));
        assert!(yaml.contains("name: SECRET_VAR"));
        assert!(yaml.contains("isSecret: true"));
    }

    // ─── Template shape wrapping ──────────────────────────────────

    /// `PipelineShape::StageTemplate` skips `name:`, `resources:`,
    /// and triggers; the body emits as a single `stages:` block.
    #[test]
    fn lower_stage_template_omits_name_resources_triggers() {
        use crate::compile::ir::stage::Stage;
        use crate::compile::ir::{
            CiTrigger, PrTrigger, RepositoryResource, Schedule, TemplateParams,
        };
        let stage = Stage::new(
            crate::compile::ir::ids::StageId::new("Main").unwrap(),
            "Main",
        );
        let p = Pipeline {
            // Even though name/resources/triggers are populated, the
            // template shape suppresses them.
            name: "should-not-appear".into(),
            parameters: Vec::new(),
            resources: Resources {
                repositories: vec![RepositoryResource::SelfRepo {
                    clean: true,
                    submodules: false,
                }],
                pipelines: Vec::new(),
            },
            triggers: Triggers {
                schedules: vec![Schedule {
                    cron: "0 0 * * *".into(),
                    display_name: "Daily".into(),
                    branches_include: vec!["main".into()],
                    always: true,
                }],
                pr: Some(PrTrigger {
                    branches_include: Vec::new(),
                    branches_exclude: Vec::new(),
                    paths_include: Vec::new(),
                    paths_exclude: Vec::new(),
                    disabled: true,
                }),
                ci: Some(CiTrigger { disabled: true }),
            },
            variables: Vec::new(),
            body: PipelineBody::Stages(vec![stage]),
            shape: PipelineShape::StageTemplate {
                external_params: TemplateParams::default(),
            },
        };
        let g = Graph::default();
        let yaml = serde_yaml::to_string(&lower_with_graph(&p, &g).unwrap()).unwrap();
        assert!(
            !yaml.contains("name:") || !yaml.contains("should-not-appear"),
            "template shape must not emit top-level `name:`, got: {yaml}"
        );
        assert!(!yaml.contains("resources:"), "template shape skips resources, got: {yaml}");
        assert!(!yaml.contains("schedules:"), "template shape skips schedules, got: {yaml}");
        assert!(!yaml.contains("pr:"), "template shape skips pr, got: {yaml}");
        assert!(!yaml.contains("trigger:"), "template shape skips trigger, got: {yaml}");
        assert!(yaml.contains("stages:"), "must emit `stages:`, got: {yaml}");
    }

    /// `PipelineShape::JobTemplate` skips the same fields and emits
    /// the body as a flat top-level `jobs:` list.
    #[test]
    fn lower_job_template_omits_name_resources_triggers() {
        use crate::compile::ir::TemplateParams;
        let job_ = Job::new(JobId::new("Agent").unwrap(), "Agent", Pool::VmImage("u".into()));
        let p = Pipeline {
            name: "x".into(),
            parameters: Vec::new(),
            resources: Resources::default(),
            triggers: Triggers::default(),
            variables: Vec::new(),
            body: PipelineBody::Jobs(vec![job_]),
            shape: PipelineShape::JobTemplate {
                external_params: TemplateParams::default(),
            },
        };
        let g = Graph::default();
        let yaml = serde_yaml::to_string(&lower_with_graph(&p, &g).unwrap()).unwrap();
        assert!(!yaml.starts_with("name:"), "must skip top-level name, got: {yaml}");
        assert!(yaml.contains("jobs:"), "must emit jobs:, got: {yaml}");
    }

    /// `Stage::external_params_wrap` emits the `${{ if ne(... }}:`
    /// keys for caller-supplied `dependsOn` / `condition`.
    #[test]
    fn lower_stage_emits_external_params_wrap_keys() {
        use crate::compile::ir::stage::{Stage, StageExternalParamsWrap};
        let mut stage = Stage::new(
            crate::compile::ir::ids::StageId::new("Main").unwrap(),
            "Main Stage",
        );
        stage.external_params_wrap = Some(StageExternalParamsWrap {
            depends_on_param: "dependsOn".into(),
            condition_param: "condition".into(),
        });
        let g = Graph::default();
        let v = lower_stage(&stage, &g).unwrap();
        let yaml = serde_yaml::to_string(&v).unwrap();
        assert!(
            yaml.contains("${{ if ne(length(parameters.dependsOn), 0) }}:"),
            "must emit dependsOn ne-branch key, got: {yaml}"
        );
        assert!(
            yaml.contains("dependsOn: ${{ parameters.dependsOn }}"),
            "must emit caller-deferred dependsOn value, got: {yaml}"
        );
        assert!(
            yaml.contains("${{ if ne(parameters.condition, '') }}:"),
            "must emit condition ne-branch key, got: {yaml}"
        );
        assert!(
            yaml.contains("condition: ${{ parameters.condition }}"),
            "must emit caller-deferred condition value, got: {yaml}"
        );
    }

    /// `Job::template_dependson_wrap` with internal `Setup` dep emits
    /// the dual-branch `${{ if eq(length(parameters.dependsOn), 0) }}`
    /// blocks merging internal + caller deps.
    #[test]
    fn lower_job_emits_template_wrap_dual_branch_with_internal_setup() {
        use crate::compile::ir::job::TemplateDependsOnWrap;
        let setup = JobId::new("Setup").unwrap();
        let mut agent = Job::new(JobId::new("Agent").unwrap(), "Agent", Pool::VmImage("u".into()));
        agent.depends_on = vec![setup.clone()];
        agent.template_dependson_wrap = Some(TemplateDependsOnWrap {
            depends_on_param: "dependsOn".into(),
            condition_param: "condition".into(),
        });
        let g = Graph::default();
        let v = lower_job(&agent, None, &g).unwrap();
        let yaml = serde_yaml::to_string(&v).unwrap();
        // eq-branch: scalar `dependsOn: Setup`
        assert!(
            yaml.contains("${{ if eq(length(parameters.dependsOn), 0) }}:"),
            "must emit eq-branch key, got: {yaml}"
        );
        assert!(
            yaml.contains("dependsOn: Setup"),
            "eq-branch must contain `dependsOn: Setup`, got: {yaml}"
        );
        // ne-branch: list with Setup then each external d
        assert!(
            yaml.contains("${{ if ne(length(parameters.dependsOn), 0) }}:"),
            "must emit ne-branch key, got: {yaml}"
        );
        assert!(
            yaml.contains("${{ each d in parameters.dependsOn }}:"),
            "ne-branch must contain `each d in parameters.dependsOn`, got: {yaml}"
        );
        assert!(
            yaml.contains("${{ d }}"),
            "ne-branch must contain `${{{{ d }}}}`, got: {yaml}"
        );
        // condition: no internal cond → ne-only branch with caller value
        assert!(
            yaml.contains("${{ if ne(parameters.condition, '') }}:"),
            "must emit condition ne-branch, got: {yaml}"
        );
        assert!(
            yaml.contains("condition: ${{ parameters.condition }}"),
            "must emit caller condition, got: {yaml}"
        );
    }

    /// `Job::template_dependson_wrap` with no internal depends_on
    /// emits only the `ne` branch with `dependsOn: ${{ parameters.X }}`.
    #[test]
    fn lower_job_template_wrap_no_internal_dep_emits_ne_only() {
        use crate::compile::ir::job::TemplateDependsOnWrap;
        let mut agent = Job::new(JobId::new("Agent").unwrap(), "Agent", Pool::VmImage("u".into()));
        agent.template_dependson_wrap = Some(TemplateDependsOnWrap {
            depends_on_param: "dependsOn".into(),
            condition_param: "condition".into(),
        });
        let g = Graph::default();
        let v = lower_job(&agent, None, &g).unwrap();
        let yaml = serde_yaml::to_string(&v).unwrap();
        assert!(
            !yaml.contains("${{ if eq(length(parameters.dependsOn), 0) }}:"),
            "must NOT emit eq-branch when no internal dep, got: {yaml}"
        );
        assert!(
            yaml.contains("${{ if ne(length(parameters.dependsOn), 0) }}:"),
            "must emit ne-branch key, got: {yaml}"
        );
        assert!(
            yaml.contains("dependsOn: ${{ parameters.dependsOn }}"),
            "must emit caller-deferred dependsOn value, got: {yaml}"
        );
    }

    /// `Job::template_dependson_wrap` with internal `and(...)` condition
    /// merges the caller's `${{ parameters.condition }}` into the same
    /// `and(...)` body.
    #[test]
    fn lower_job_template_wrap_merges_internal_and_condition_with_caller() {
        use crate::compile::ir::condition::Condition;
        use crate::compile::ir::job::TemplateDependsOnWrap;
        let mut agent = Job::new(JobId::new("Agent").unwrap(), "Agent", Pool::VmImage("u".into()));
        agent.condition = Some(Condition::And(vec![
            Condition::Succeeded,
            Condition::Custom("eq(variables['x'], 'y')".into()),
        ]));
        agent.template_dependson_wrap = Some(TemplateDependsOnWrap {
            depends_on_param: "dependsOn".into(),
            condition_param: "condition".into(),
        });
        let g = Graph::default();
        let v = lower_job(&agent, None, &g).unwrap();
        let yaml = serde_yaml::to_string(&v).unwrap();
        // eq-branch: internal verbatim
        assert!(
            yaml.contains("${{ if eq(parameters.condition, '') }}:"),
            "must emit condition eq-branch, got: {yaml}"
        );
        // ne-branch: internal + caller appended inside `and(...)`
        assert!(
            yaml.contains("${{ if ne(parameters.condition, '') }}:"),
            "must emit condition ne-branch, got: {yaml}"
        );
        assert!(
            yaml.contains("${{ parameters.condition }}"),
            "ne-branch must contain caller condition ref, got: {yaml}"
        );
        // The merged ne-branch must keep the internal succeeded() / x=y.
        assert!(
            yaml.contains("succeeded()") && yaml.contains("eq(variables['x'], 'y')"),
            "merged condition must keep internal parts, got: {yaml}"
        );
    }

    #[test]
    fn merge_condition_handles_and_wrapping() {
        assert_eq!(
            merge_condition_with_template_param("and(succeeded(), eq(a, b))", "condition"),
            "and(succeeded(), eq(a, b), ${{ parameters.condition }})"
        );
        assert_eq!(
            merge_condition_with_template_param("succeeded()", "condition"),
            "and(succeeded(), ${{ parameters.condition }})"
        );
    }
}

