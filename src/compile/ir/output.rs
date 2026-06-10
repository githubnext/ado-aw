//! Declared step outputs and references to them.
//!
//! A step that wants its output visible to other steps records the
//! output in [`BashStep::outputs`](super::step::BashStep::outputs)
//! using [`OutputDecl`]. Consumers reference the value via
//! [`OutputRef`].
//!
//! ## Reference-syntax lowering ([`lower_outputref`])
//!
//! ADO has three distinct syntaxes for reading a step output and
//! the right one depends on **where the consumer lives** relative to
//! the producer:
//!
//! | consumer location vs. producer       | syntax                                                            |
//! |--------------------------------------|-------------------------------------------------------------------|
//! | same job                             | `$(stepName.X)`                                                   |
//! | sibling job in same stage / no stage | `dependencies.<job>.outputs['stepName.X']`                        |
//! | different stage                      | `stageDependencies.<stage>.<job>.outputs['stepName.X']`           |
//!
//! See `compile_gate_step_external`'s doc-comment in
//! `src/compile/filter_ir.rs` for the empirical justification of the
//! same-job macro form — runtime expressions in the producing job
//! cannot read `variables['stepName.X']`, they need the macro.

use super::ids::{JobId, StageId, StepId};

/// A named output exported by a step.
///
/// The compiler auto-emits `isOutput=true` on the underlying
/// `##vso[task.setvariable]` line iff at least one cross-step
/// consumer references this name via [`OutputRef`]. The graph pass
/// (see [`super::graph`]) populates
/// [`OutputDecl::auto_is_output`] so emitters can consult it; the
/// actual bash rewrite is performed at emit time by the producer's
/// extension.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputDecl {
    /// The output variable name (the `variable=` value in
    /// `##vso[task.setvariable variable=NAME;isOutput=true]`).
    pub name: String,
    /// Whether the producing step also marks the variable as a secret
    /// (`issecret=true`). Independent of cross-step visibility.
    pub is_secret: bool,
    /// Set by the graph pass to `true` when at least one cross-step
    /// consumer references this output. Producers should emit
    /// `isOutput=true` on the corresponding `##vso[task.setvariable]`
    /// line iff this flag is set. Defaults to `false` because newly
    /// constructed `OutputDecl`s have not yet been seen by the graph
    /// pass.
    pub auto_is_output: bool,
}

impl OutputDecl {
    /// Construct a plain (non-secret) output declaration.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            is_secret: false,
            auto_is_output: false,
        }
    }

    /// Construct a secret output declaration.
    pub fn secret(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            is_secret: true,
            auto_is_output: false,
        }
    }
}

/// A reference to a step's output, resolved by the IR lowering pass.
///
/// At build time the consumer just names the producer step and the
/// output it wants; at lower time the IR picks the correct ADO
/// reference syntax based on whether the consumer lives in the same
/// job / a sibling job in the same stage / a different stage.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct OutputRef {
    /// The producer step's id.
    pub step: StepId,
    /// The output variable name (must match an [`OutputDecl::name`]
    /// on the producer).
    pub name: String,
}

impl OutputRef {
    /// Construct an output reference.
    pub fn new(step: StepId, name: impl Into<String>) -> Self {
        Self {
            step,
            name: name.into(),
        }
    }
}

/// Where a consumer lives. Mirrors the relevant subset of
/// [`super::graph::StepLocation`]: only `stage` and `job` matter for
/// reference-syntax selection.
#[derive(Debug, Clone)]
pub struct ConsumerLocation<'a> {
    pub stage: Option<&'a StageId>,
    pub job: &'a JobId,
}

/// Where a producer lives. Same fields as [`ConsumerLocation`] but a
/// distinct type so call sites cannot mix them up.
#[derive(Debug, Clone)]
pub struct ProducerLocation<'a> {
    pub stage: Option<&'a StageId>,
    pub job: &'a JobId,
}

/// Lower an [`OutputRef`] to its ADO scalar form, picking the right
/// syntax based on consumer/producer location.
///
/// Mirrors the three-row table in this module's top-level
/// doc-comment.
pub fn lower_outputref(
    consumer: ConsumerLocation<'_>,
    producer: ProducerLocation<'_>,
    r: &OutputRef,
) -> String {
    // Same job?
    if consumer.job == producer.job && consumer.stage.map(|s| s.as_str()) == producer.stage.map(|s| s.as_str()) {
        return format!("$({step}.{name})", step = r.step, name = r.name);
    }
    // Different stage?
    if consumer.stage.map(|s| s.as_str()) != producer.stage.map(|s| s.as_str()) {
        // Cross-stage refs are only valid when both sides are inside
        // stages — graph validation already rejects mixed
        // staged/un-staged, so unwrap here is load-bearing.
        let prod_stage = producer
            .stage
            .expect("cross-stage ref must have producer stage (graph validation enforces)");
        return format!(
            "stageDependencies.{stage}.{job}.outputs['{step}.{name}']",
            stage = prod_stage,
            job = producer.job,
            step = r.step,
            name = r.name,
        );
    }
    // Same stage (or both stage-less), different jobs.
    format!(
        "dependencies.{job}.outputs['{step}.{name}']",
        job = producer.job,
        step = r.step,
        name = r.name,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outputdecl_new_defaults_to_non_secret_and_not_auto_is_output() {
        let d = OutputDecl::new("AW_SYNTHETIC_PR");
        assert_eq!(d.name, "AW_SYNTHETIC_PR");
        assert!(!d.is_secret);
        assert!(!d.auto_is_output);
    }

    #[test]
    fn outputdecl_secret_marks_secret() {
        let d = OutputDecl::secret("MCP_GATEWAY_API_KEY");
        assert!(d.is_secret);
    }

    #[test]
    fn outputref_carries_typed_producer() {
        let step = StepId::new("synthPr").unwrap();
        let r = OutputRef::new(step.clone(), "AW_SYNTHETIC_PR");
        assert_eq!(r.step, step);
        assert_eq!(r.name, "AW_SYNTHETIC_PR");
    }

    fn job_id(s: &str) -> JobId {
        JobId::new(s).unwrap()
    }
    fn stage_id(s: &str) -> StageId {
        StageId::new(s).unwrap()
    }

    #[test]
    fn lowers_same_job_to_macro_form() {
        let producer_job = job_id("Setup");
        let producer = ProducerLocation {
            stage: None,
            job: &producer_job,
        };
        let consumer_job = job_id("Setup");
        let consumer = ConsumerLocation {
            stage: None,
            job: &consumer_job,
        };
        let r = OutputRef::new(StepId::new("synthPr").unwrap(), "AW_SYNTHETIC_PR");
        assert_eq!(
            lower_outputref(consumer, producer, &r),
            "$(synthPr.AW_SYNTHETIC_PR)"
        );
    }

    #[test]
    fn lowers_cross_job_same_stage_to_dependencies_form() {
        let producer_job = job_id("Setup");
        let producer_stage = stage_id("S");
        let producer = ProducerLocation {
            stage: Some(&producer_stage),
            job: &producer_job,
        };
        let consumer_job = job_id("Agent");
        let consumer_stage = stage_id("S");
        let consumer = ConsumerLocation {
            stage: Some(&consumer_stage),
            job: &consumer_job,
        };
        let r = OutputRef::new(StepId::new("synthPr").unwrap(), "AW_SYNTHETIC_PR_SKIP");
        assert_eq!(
            lower_outputref(consumer, producer, &r),
            "dependencies.Setup.outputs['synthPr.AW_SYNTHETIC_PR_SKIP']"
        );
    }

    #[test]
    fn lowers_cross_stage_to_stage_dependencies_form() {
        let producer_job = job_id("Setup");
        let producer_stage = stage_id("StageA");
        let producer = ProducerLocation {
            stage: Some(&producer_stage),
            job: &producer_job,
        };
        let consumer_job = job_id("Agent");
        let consumer_stage = stage_id("StageB");
        let consumer = ConsumerLocation {
            stage: Some(&consumer_stage),
            job: &consumer_job,
        };
        let r = OutputRef::new(StepId::new("synthPr").unwrap(), "AW_SYNTHETIC_PR");
        assert_eq!(
            lower_outputref(consumer, producer, &r),
            "stageDependencies.StageA.Setup.outputs['synthPr.AW_SYNTHETIC_PR']"
        );
    }

    #[test]
    fn cross_job_no_stage_uses_dependencies_form() {
        // Both consumer and producer are stage-less (top-level
        // PipelineBody::Jobs). Same syntax as same-stage cross-job.
        let pj = job_id("Setup");
        let cj = job_id("Agent");
        let producer = ProducerLocation { stage: None, job: &pj };
        let consumer = ConsumerLocation { stage: None, job: &cj };
        let r = OutputRef::new(StepId::new("synthPr").unwrap(), "X");
        assert_eq!(
            lower_outputref(consumer, producer, &r),
            "dependencies.Setup.outputs['synthPr.X']"
        );
    }
}
