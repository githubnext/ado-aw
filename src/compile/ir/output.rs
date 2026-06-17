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
/// ADO requires `isOutput=true` on the underlying
/// `##vso[task.setvariable]` line for an output to be visible to
/// **any** cross-step consumer — same-job (`$(stepName.X)`),
/// cross-job (`dependencies.<job>.outputs[...]`), or cross-stage
/// (`stageDependencies.<stage>.<job>.outputs[...]`). The graph pass
/// (see [`super::graph`]) detects which declared outputs have at
/// least one cross-step reader and sets
/// [`OutputDecl::auto_is_output`] to `true` on those decls.
///
/// **`auto_is_output` is an informational signal, not an emit-time
/// rewrite.** The IR does **not** introspect or rewrite the producer's
/// bash body — extension authors are responsible for ensuring the
/// emitted `##vso[task.setvariable variable=NAME …]` line includes
/// `isOutput=true` whenever the output is consumed cross-step.
/// Producers that emit outputs out of band (e.g. by invoking a JS
/// bundle that calls the ADO REST API or shells the directive
/// itself) are responsible for the same guarantee.
///
/// Forgetting `isOutput=true` is a silent-failure mode at runtime
/// (all cross-step consumers read empty values). See the synthPr
/// regression history (memory: `azure devops`, PR #956, PR #975)
/// for the empirical cost of getting this wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputDecl {
    /// The output variable name (the `variable=` value in
    /// `##vso[task.setvariable variable=NAME;isOutput=true]`).
    pub name: String,
    /// Whether the producing step also marks the variable as a secret
    /// (`issecret=true`). Independent of cross-step visibility.
    pub is_secret: bool,
    /// Set by the graph pass (see
    /// [`super::graph::Graph::outputs_needing_is_output`]) to `true`
    /// when at least one cross-step consumer references this output.
    /// Informational signal only — the IR does not introspect or
    /// rewrite the producer's step body, so extension authors must
    /// ensure `isOutput=true` is present in the emitted
    /// `##vso[task.setvariable]` directive (or in the equivalent
    /// out-of-band emit path).
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
///
/// **Uniqueness:** the [`StepId`] is the *only* key used to resolve
/// the producer's location — there is no job-qualified form here.
/// Step IDs are therefore required to be pipeline-wide unique; see
/// [`crate::compile::ir::ids`] for the contract and
/// [`crate::compile::ir::graph::build_graph`] for the enforcement.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct OutputRef {
    /// The producer step's id. **Must be pipeline-wide unique** —
    /// see type-level doc on uniqueness.
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
///
/// # Errors
///
/// Returns `Err` if the cross-stage branch is taken but the producer
/// has no stage. Graph validation in [`super::graph::build_graph`]
/// rejects mixed staged / un-staged references before lowering, so
/// callers reached through [`super::graph::resolve`] → `lower` never
/// trip this — but the error path keeps the function honest if it
/// is invoked outside the validated flow.
pub fn lower_outputref(
    consumer: ConsumerLocation<'_>,
    producer: ProducerLocation<'_>,
    r: &OutputRef,
) -> anyhow::Result<String> {
    // Same job?
    if consumer.job == producer.job
        && consumer.stage.map(|s| s.as_str()) == producer.stage.map(|s| s.as_str())
    {
        return Ok(format!("$({step}.{name})", step = r.step, name = r.name));
    }
    // Different stage?
    if consumer.stage.map(|s| s.as_str()) != producer.stage.map(|s| s.as_str()) {
        // Cross-stage refs are only valid when both sides are inside
        // stages. Graph validation rejects mixed staged/un-staged
        // before reaching here; the error path covers callers that
        // bypass the validation pass.
        let prod_stage = producer.stage.ok_or_else(|| {
            anyhow::anyhow!(
                "ir::output::lower_outputref: cross-stage reference to step '{}' \
                 has no producer stage (graph validation should have rejected this; \
                 producer job={}, consumer stage={:?}, consumer job={})",
                r.step,
                producer.job,
                consumer.stage.map(|s| s.as_str()),
                consumer.job,
            )
        })?;
        return Ok(format!(
            "stageDependencies.{stage}.{job}.outputs['{step}.{name}']",
            stage = prod_stage,
            job = producer.job,
            step = r.step,
            name = r.name,
        ));
    }
    // Same stage (or both stage-less), different jobs.
    Ok(format!(
        "dependencies.{job}.outputs['{step}.{name}']",
        job = producer.job,
        step = r.step,
        name = r.name,
    ))
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
        assert_eq!(d.name, "MCP_GATEWAY_API_KEY");
        assert!(d.is_secret);
        assert!(!d.auto_is_output);
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
            lower_outputref(consumer, producer, &r).unwrap(),
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
            lower_outputref(consumer, producer, &r).unwrap(),
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
            lower_outputref(consumer, producer, &r).unwrap(),
            "stageDependencies.StageA.Setup.outputs['synthPr.AW_SYNTHETIC_PR']"
        );
    }

    #[test]
    fn cross_job_no_stage_uses_dependencies_form() {
        // Both consumer and producer are stage-less (top-level
        // PipelineBody::Jobs). Same syntax as same-stage cross-job.
        let pj = job_id("Setup");
        let cj = job_id("Agent");
        let producer = ProducerLocation {
            stage: None,
            job: &pj,
        };
        let consumer = ConsumerLocation {
            stage: None,
            job: &cj,
        };
        let r = OutputRef::new(StepId::new("synthPr").unwrap(), "X");
        assert_eq!(
            lower_outputref(consumer, producer, &r).unwrap(),
            "dependencies.Setup.outputs['synthPr.X']"
        );
    }

    #[test]
    fn errors_when_cross_stage_producer_has_no_stage() {
        // Mixed staged/un-staged is invalid; graph validation
        // normally rejects this before lowering, but the function
        // surfaces it as a typed error rather than panicking.
        let producer_job = job_id("Setup");
        let producer = ProducerLocation {
            stage: None,
            job: &producer_job,
        };
        let consumer_job = job_id("Agent");
        let consumer_stage = stage_id("StageB");
        let consumer = ConsumerLocation {
            stage: Some(&consumer_stage),
            job: &consumer_job,
        };
        let r = OutputRef::new(StepId::new("synthPr").unwrap(), "AW_SYNTHETIC_PR");
        let err = lower_outputref(consumer, producer, &r).unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("cross-stage reference"),
            "expected cross-stage error, got: {msg}"
        );
    }
}
