//! Lower the typed IR ([`super::Pipeline`]) to a
//! [`serde_yaml::Value`] tree.
//!
//! ## Scope of this commit (`ir-yaml-emit`)
//!
//! - Covers every IR variant that does **not** need cross-step
//!   resolution: literal env values, plain conditions, all step
//!   kinds, jobs, stages, the pipeline-level fields modelled so far.
//! - The variants that need cross-step resolution
//!   ([`super::env::EnvValue::StepOutput`],
//!   [`super::env::EnvValue::Coalesce`], and
//!   [`super::condition::Expr::StepOutput`]) panic via
//!   [`unimplemented!`] with a pointer to the commit that fills them
//!   in (`ir-output-lowering` and `ir-condition-codegen`). The unit
//!   tests in this commit never exercise those variants — they land
//!   in their own commits where the lowering algorithm is the unit
//!   under test.
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

use super::condition::{Condition, Expr};
use super::env::EnvValue;
use super::job::{Job, Pool};
use super::stage::Stage;
use super::step::{
    BashStep, CheckoutRepo, CheckoutStep, DownloadStep, PublishStep, Step, SubmodulesOpt, TaskStep,
};
use super::{Pipeline, PipelineBody, PipelineShape};

/// Lower a [`Pipeline`] to a [`serde_yaml::Value`].
pub fn lower(p: &Pipeline) -> Result<Value> {
    let mut root = Mapping::new();
    root.insert(s("name"), s(&p.name));

    // For the `ir-yaml-emit` commit we only model the canonical
    // standalone shape end-to-end. OneEs / JobTemplate / StageTemplate
    // wrap the same body in different outer scaffolding; their
    // wrapping is added in the target-compiler commits.
    match &p.shape {
        PipelineShape::Standalone => {}
        PipelineShape::OneEs { .. }
        | PipelineShape::JobTemplate { .. }
        | PipelineShape::StageTemplate { .. } => {
            // Pre-existing skeleton; populated by compile-target-1es /
            // compile-target-job / compile-target-stage.
            unimplemented!(
                "PipelineShape wrapping is introduced by the compile-target-* commits"
            );
        }
    }

    match &p.body {
        PipelineBody::Jobs(jobs) => {
            let mut seq = Vec::with_capacity(jobs.len());
            for job in jobs {
                seq.push(lower_job(job)?);
            }
            root.insert(s("jobs"), Value::Sequence(seq));
        }
        PipelineBody::Stages(stages) => {
            let mut seq = Vec::with_capacity(stages.len());
            for stage in stages {
                seq.push(lower_stage(stage)?);
            }
            root.insert(s("stages"), Value::Sequence(seq));
        }
    }

    Ok(Value::Mapping(root))
}

fn lower_stage(stage: &Stage) -> Result<Value> {
    let mut m = Mapping::new();
    m.insert(s("stage"), s(stage.id.as_str()));
    m.insert(s("displayName"), s(&stage.display_name));
    if !stage.depends_on.is_empty() {
        let deps: Vec<Value> = stage.depends_on.iter().map(|d| s(d.as_str())).collect();
        m.insert(s("dependsOn"), Value::Sequence(deps));
    }
    if let Some(cond) = &stage.condition {
        m.insert(s("condition"), s(&lower_condition(cond)?));
    }
    let mut jobs = Vec::with_capacity(stage.jobs.len());
    for job in &stage.jobs {
        jobs.push(lower_job(job)?);
    }
    m.insert(s("jobs"), Value::Sequence(jobs));
    Ok(Value::Mapping(m))
}

fn lower_job(job: &Job) -> Result<Value> {
    let mut m = Mapping::new();
    m.insert(s("job"), s(job.id.as_str()));
    m.insert(s("displayName"), s(&job.display_name));
    if !job.depends_on.is_empty() {
        let deps: Vec<Value> = job.depends_on.iter().map(|d| s(d.as_str())).collect();
        m.insert(s("dependsOn"), Value::Sequence(deps));
    }
    if let Some(cond) = &job.condition {
        m.insert(s("condition"), s(&lower_condition(cond)?));
    }
    if let Some(t) = job.timeout {
        m.insert(s("timeoutInMinutes"), Value::from(minutes_ceil(t)));
    }
    m.insert(s("pool"), lower_pool(&job.pool));
    let mut steps = Vec::with_capacity(job.steps.len());
    for step in &job.steps {
        steps.push(lower_step(step)?);
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

fn lower_step(step: &Step) -> Result<Value> {
    match step {
        Step::Bash(b) => lower_bash(b),
        Step::Task(t) => lower_task(t),
        Step::Checkout(c) => Ok(lower_checkout(c)),
        Step::Download(d) => Ok(lower_download(d)),
        Step::Publish(p) => Ok(lower_publish(p)),
    }
}

fn lower_bash(b: &BashStep) -> Result<Value> {
    let mut m = Mapping::new();
    m.insert(s("bash"), s(&b.script));
    if let Some(id) = &b.id {
        m.insert(s("name"), s(id.as_str()));
    }
    m.insert(s("displayName"), s(&b.display_name));
    if let Some(cond) = &b.condition {
        m.insert(s("condition"), s(&lower_condition(cond)?));
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
            env_map.insert(s(k), s(&lower_env_value(v)?));
        }
        m.insert(s("env"), Value::Mapping(env_map));
    }
    Ok(Value::Mapping(m))
}

fn lower_task(t: &TaskStep) -> Result<Value> {
    let mut m = Mapping::new();
    m.insert(s("task"), s(&t.task));
    if let Some(id) = &t.id {
        m.insert(s("name"), s(id.as_str()));
    }
    m.insert(s("displayName"), s(&t.display_name));
    if let Some(cond) = &t.condition {
        m.insert(s("condition"), s(&lower_condition(cond)?));
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
            env_map.insert(s(k), s(&lower_env_value(v)?));
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

fn lower_download(d: &DownloadStep) -> Value {
    let mut m = Mapping::new();
    m.insert(s("download"), s(&d.source));
    m.insert(s("artifact"), s(&d.artifact));
    if let Some(cond) = &d.condition {
        // `lower_condition` returns Result, but we know inputs to this
        // helper cannot contain step-output references — DownloadStep's
        // condition is restricted to the static subset by construction.
        // Use expect with a load-bearing message so a future regression
        // surfaces loudly rather than silently.
        m.insert(
            s("condition"),
            s(lower_condition(cond).expect("DownloadStep.condition: simple variants only")),
        );
    }
    Value::Mapping(m)
}

fn lower_publish(p: &PublishStep) -> Value {
    let mut m = Mapping::new();
    m.insert(s("publish"), s(&p.path));
    m.insert(s("artifact"), s(&p.artifact));
    if let Some(cond) = &p.condition {
        m.insert(
            s("condition"),
            s(lower_publish_condition(cond)),
        );
    }
    Value::Mapping(m)
}

fn lower_publish_condition(cond: &Condition) -> String {
    lower_condition(cond).expect("PublishStep.condition: simple variants only")
}

/// Lower an [`EnvValue`] to its ADO scalar form.
///
/// The variants that need cross-step resolution
/// ([`EnvValue::StepOutput`], [`EnvValue::Coalesce`]) are introduced
/// in the `ir-output-lowering` commit.
fn lower_env_value(v: &EnvValue) -> Result<String> {
    match v {
        EnvValue::Literal(s) => Ok(s.clone()),
        EnvValue::AdoMacro(name) => Ok(format!("$({name})")),
        EnvValue::PipelineVar(name) => Ok(format!("$({name})")),
        EnvValue::Secret(name) => Ok(format!("$({name})")),
        EnvValue::StepOutput(_) => Err(anyhow::anyhow!(
            "ir::lower: EnvValue::StepOutput lowering is introduced by the ir-output-lowering commit"
        )),
        EnvValue::Coalesce(_) => Err(anyhow::anyhow!(
            "ir::lower: EnvValue::Coalesce lowering is introduced by the ir-output-lowering commit"
        )),
    }
    .context("lower_env_value")
}

/// Lower a [`Condition`] to its ADO condition string.
///
/// Only the static subset (no [`Expr::StepOutput`]) is handled here.
/// Full codegen — including `Custom` injection checks and pretty
/// indentation for And/Or chains — is the `ir-condition-codegen`
/// commit.
fn lower_condition(c: &Condition) -> Result<String> {
    Ok(match c {
        Condition::Succeeded => "succeeded()".to_string(),
        Condition::Always => "always()".to_string(),
        Condition::Failed => "failed()".to_string(),
        Condition::SucceededOrFailed => "succeededOrFailed()".to_string(),
        Condition::And(parts) => {
            let lowered = parts
                .iter()
                .map(lower_condition)
                .collect::<Result<Vec<_>>>()?;
            format!("and({})", lowered.join(", "))
        }
        Condition::Or(parts) => {
            let lowered = parts
                .iter()
                .map(lower_condition)
                .collect::<Result<Vec<_>>>()?;
            format!("or({})", lowered.join(", "))
        }
        Condition::Not(inner) => format!("not({})", lower_condition(inner)?),
        Condition::Eq(a, b) => format!("eq({}, {})", lower_expr(a)?, lower_expr(b)?),
        Condition::Ne(a, b) => format!("ne({}, {})", lower_expr(a)?, lower_expr(b)?),
        Condition::Custom(raw) => raw.clone(),
    })
}

fn lower_expr(e: &Expr) -> Result<String> {
    Ok(match e {
        Expr::Literal(v) => format!("'{}'", v.replace('\'', "''")),
        Expr::Variable(name) => format!("variables['{name}']"),
        Expr::StepOutput(_) => anyhow::bail!(
            "ir::lower: Expr::StepOutput lowering is introduced by the ir-output-lowering commit"
        ),
    })
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
    use crate::compile::ir::ids::{JobId, StepId};

    #[test]
    fn lower_condition_static_variants() {
        assert_eq!(lower_condition(&Condition::Succeeded).unwrap(), "succeeded()");
        assert_eq!(
            lower_condition(&Condition::and([
                Condition::Succeeded,
                Condition::Ne(Expr::Variable("Build.Reason".into()), Expr::Literal("PullRequest".into())),
            ]))
            .unwrap(),
            "and(succeeded(), ne(variables['Build.Reason'], 'PullRequest'))"
        );
    }

    #[test]
    fn lower_env_value_simple_variants() {
        assert_eq!(lower_env_value(&EnvValue::literal("x")).unwrap(), "x");
        assert_eq!(
            lower_env_value(&EnvValue::ado_macro("Build.Reason").unwrap()).unwrap(),
            "$(Build.Reason)"
        );
        assert_eq!(
            lower_env_value(&EnvValue::pipeline_var("MY_VAR")).unwrap(),
            "$(MY_VAR)"
        );
        assert_eq!(
            lower_env_value(&EnvValue::secret("MCP_API_KEY")).unwrap(),
            "$(MCP_API_KEY)"
        );
    }

    #[test]
    fn lower_env_value_step_output_errors_until_next_commit() {
        use crate::compile::ir::output::OutputRef;
        let r = OutputRef::new(StepId::new("synthPr").unwrap(), "X");
        let err = lower_env_value(&EnvValue::step_output(r)).unwrap_err();
        assert!(format!("{err:#}").contains("ir-output-lowering"));
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

        let v = lower_job(&job).unwrap();
        let m = match v {
            Value::Mapping(m) => m,
            _ => panic!(),
        };
        let keys: Vec<&str> = m
            .keys()
            .filter_map(|k| k.as_str())
            .collect();
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
}
