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
use super::job::{Job, Pool};
use super::output::{ConsumerLocation, OutputRef, ProducerLocation, lower_outputref};
use super::stage::Stage;
use super::step::{
    BashStep, CheckoutRepo, CheckoutStep, DownloadStep, PublishStep, Step, SubmodulesOpt, TaskStep,
};
use super::{Pipeline, PipelineBody, PipelineShape};

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
    root.insert(s("name"), s(&p.name));

    // For the `ir-yaml-emit`/`ir-output-lowering` commits we only
    // model the canonical standalone shape end-to-end. OneEs /
    // JobTemplate / StageTemplate wrap the same body in different
    // outer scaffolding; their wrapping is added in the
    // target-compiler commits.
    match &p.shape {
        PipelineShape::Standalone => {}
        PipelineShape::OneEs { .. }
        | PipelineShape::JobTemplate { .. }
        | PipelineShape::StageTemplate { .. } => {
            unimplemented!(
                "PipelineShape wrapping is introduced by the compile-target-* commits"
            );
        }
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

fn lower_stage(stage: &Stage, graph: &Graph) -> Result<Value> {
    let mut m = Mapping::new();
    m.insert(s("stage"), s(stage.id.as_str()));
    m.insert(s("displayName"), s(&stage.display_name));
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
    let mut jobs = Vec::with_capacity(stage.jobs.len());
    for job in &stage.jobs {
        jobs.push(lower_job(job, Some(&stage.id), graph)?);
    }
    m.insert(s("jobs"), Value::Sequence(jobs));
    Ok(Value::Mapping(m))
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
    if !job.depends_on.is_empty() {
        let deps: Vec<Value> = job.depends_on.iter().map(|d| s(d.as_str())).collect();
        m.insert(s("dependsOn"), Value::Sequence(deps));
    }
    if let Some(cond) = &job.condition {
        m.insert(s("condition"), s(&lower_condition(&ctx.cond_ctx(), cond)?));
    }
    if let Some(t) = job.timeout {
        m.insert(s("timeoutInMinutes"), Value::from(minutes_ceil(t)));
    }
    m.insert(s("pool"), lower_pool(&job.pool));
    let mut steps = Vec::with_capacity(job.steps.len());
    for step in &job.steps {
        steps.push(lower_step(step, &ctx)?);
    }
    m.insert(s("steps"), Value::Sequence(steps));
    Ok(Value::Mapping(m))
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

fn lower_step(step: &Step, ctx: &LoweringContext<'_>) -> Result<Value> {
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
    let mut m = Mapping::new();
    m.insert(s("bash"), s(&b.script));
    if let Some(id) = &b.id {
        m.insert(s("name"), s(id.as_str()));
    }
    m.insert(s("displayName"), s(&b.display_name));
    if let Some(cond) = &b.condition {
        m.insert(s("condition"), s(&lower_condition(&ctx.cond_ctx(), cond)?));
    }
    if let Some(t) = b.timeout {
        m.insert(s("timeoutInMinutes"), Value::from(minutes_ceil(t)));
    }
    if b.continue_on_error {
        m.insert(s("continueOnError"), Value::Bool(true));
    }
    if let Some(wd) = &b.working_directory {
        m.insert(s("workingDirectory"), s(wd));
    }
    if !b.env.is_empty() {
        let mut env_map = Mapping::new();
        for (k, v) in &b.env {
            env_map.insert(s(k), s(&lower_env_value(ctx, v)?));
        }
        m.insert(s("env"), Value::Mapping(env_map));
    }
    Ok(Value::Mapping(m))
}

fn lower_task(t: &TaskStep, ctx: &LoweringContext<'_>) -> Result<Value> {
    let mut m = Mapping::new();
    m.insert(s("task"), s(&t.task));
    if let Some(id) = &t.id {
        m.insert(s("name"), s(id.as_str()));
    }
    m.insert(s("displayName"), s(&t.display_name));
    if let Some(cond) = &t.condition {
        m.insert(s("condition"), s(&lower_condition(&ctx.cond_ctx(), cond)?));
    }
    if let Some(timeout) = t.timeout {
        m.insert(s("timeoutInMinutes"), Value::from(minutes_ceil(timeout)));
    }
    if t.continue_on_error {
        m.insert(s("continueOnError"), Value::Bool(true));
    }
    if !t.inputs.is_empty() {
        let mut inputs = Mapping::new();
        for (k, v) in &t.inputs {
            inputs.insert(s(k), s(v));
        }
        m.insert(s("inputs"), Value::Mapping(inputs));
    }
    if !t.env.is_empty() {
        let mut env_map = Mapping::new();
        for (k, v) in &t.env {
            env_map.insert(s(k), s(&lower_env_value(ctx, v)?));
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
}

