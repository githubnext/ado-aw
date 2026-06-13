//! Typed ADO condition AST.
//!
//! Replaces the hand-built condition strings that today live in
//! `generate_agentic_depends_on` (`src/compile/common.rs:2388-2530`)
//! and `compile_gate_step_external` (`src/compile/filter_ir.rs:1147+`).
//!
//! ## Layout
//!
//! - [`Condition`] / [`Expr`] — the AST.
//! - [`codegen`] — lowering to the literal ADO condition string,
//!   including the [`Condition::Custom`] injection check and the
//!   per-consumer-location step-output resolution via
//!   [`super::output::lower_outputref`].

use super::output::OutputRef;

/// A typed ADO condition expression.
///
/// All ADO `condition:` strings are eventually reducible to one of
/// these forms. The `Custom` escape hatch is intentionally
/// last-resort; the codegen pass runs it through
/// [`crate::validate::contains_pipeline_command`] +
/// [`crate::validate::contains_newline`] to reject the two injection
/// vectors that matter inside a condition scalar (raw ADO logging
/// commands and embedded newlines that would break the YAML scalar
/// shape). `Custom` does **not** reject general ADO expressions like
/// `$(Build.Reason)` — those are exactly what the escape hatch is for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Condition {
    /// `succeeded()` — the default ADO step / job / stage condition.
    Succeeded,
    /// `always()` — run regardless of upstream success / failure.
    Always,
    /// `failed()` — run only when an upstream step / job / stage
    /// failed.
    Failed,
    /// `succeededOrFailed()` — run after upstream completion, no
    /// matter the result. Distinct from `Always` in that
    /// cancellations short-circuit it.
    SucceededOrFailed,
    /// Logical AND. Flattened during lowering, so callers do not need
    /// to flatten themselves.
    And(Vec<Condition>),
    /// Logical OR. Flattened during lowering.
    Or(Vec<Condition>),
    /// Logical NOT.
    Not(Box<Condition>),
    /// Equality between two [`Expr`]s.
    Eq(Expr, Expr),
    /// Inequality between two [`Expr`]s.
    Ne(Expr, Expr),
    /// Escape hatch for conditions the AST does not yet model. The
    /// codegen pass rejects values containing pipeline-command
    /// markers (`##vso[`, `##[`) or newlines; ADO expressions and
    /// macros are allowed since avoiding them defeats the purpose of
    /// the escape hatch.
    Custom(String),
}

/// A typed sub-expression appearing inside a [`Condition`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expr {
    /// String literal (will be emitted single-quoted in ADO syntax).
    Literal(String),
    /// Reference to a pipeline variable: `variables['<name>']`.
    Variable(String),
    /// Reference to a step output. Lowered to the same family of
    /// reference syntaxes as [`super::env::EnvValue::StepOutput`].
    StepOutput(OutputRef),
}

impl Condition {
    /// Construct an `And` from an iterator of conditions.
    pub fn and<I: IntoIterator<Item = Condition>>(parts: I) -> Self {
        Condition::And(parts.into_iter().collect())
    }

    /// Construct an `Or` from an iterator of conditions.
    pub fn or<I: IntoIterator<Item = Condition>>(parts: I) -> Self {
        Condition::Or(parts.into_iter().collect())
    }

    /// Construct a `Not`.
    pub fn not(inner: Condition) -> Self {
        Condition::Not(Box::new(inner))
    }
}

pub mod codegen {
    //! Lower [`Condition`] / [`Expr`] to ADO condition strings.
    //!
    //! Used by [`super::super::lower`] from inside its per-step
    //! recursion; lives here so the AST and its codegen stay colocated.

    use anyhow::{Result, bail};

    use super::{Condition, Expr};
    use crate::compile::ir::graph::Graph;
    use crate::compile::ir::ids::{JobId, StageId};
    use crate::compile::ir::output::{ConsumerLocation, ProducerLocation, lower_outputref};

    /// Per-consumer location + graph access for codegen.
    ///
    /// Mirrors `lower::LoweringContext` but lives here so the codegen
    /// helpers don't have to pull in everything `lower` needs. Built
    /// once per consumer at the call site.
    pub struct CondCodegenCtx<'a> {
        pub graph: &'a Graph,
        pub stage: Option<&'a StageId>,
        pub job: &'a JobId,
    }

    impl<'a> CondCodegenCtx<'a> {
        pub fn consumer(&self) -> ConsumerLocation<'a> {
            ConsumerLocation {
                stage: self.stage,
                job: self.job,
            }
        }
    }

    /// Lower a [`Condition`] to its ADO condition string.
    ///
    /// Flattens nested `And`/`Or` for compact output and runs the
    /// `Custom` injection check.
    pub fn lower_condition(ctx: &CondCodegenCtx<'_>, c: &Condition) -> Result<String> {
        Ok(match c {
            Condition::Succeeded => "succeeded()".to_string(),
            Condition::Always => "always()".to_string(),
            Condition::Failed => "failed()".to_string(),
            Condition::SucceededOrFailed => "succeededOrFailed()".to_string(),
            Condition::And(parts) => {
                let flat = flatten_and(parts);
                let lowered = flat
                    .iter()
                    .map(|p| lower_condition(ctx, p))
                    .collect::<Result<Vec<_>>>()?;
                format!("and({})", lowered.join(", "))
            }
            Condition::Or(parts) => {
                let flat = flatten_or(parts);
                let lowered = flat
                    .iter()
                    .map(|p| lower_condition(ctx, p))
                    .collect::<Result<Vec<_>>>()?;
                format!("or({})", lowered.join(", "))
            }
            Condition::Not(inner) => format!("not({})", lower_condition(ctx, inner)?),
            Condition::Eq(a, b) => format!("eq({}, {})", lower_expr(ctx, a)?, lower_expr(ctx, b)?),
            Condition::Ne(a, b) => format!("ne({}, {})", lower_expr(ctx, a)?, lower_expr(ctx, b)?),
            Condition::Custom(raw) => {
                validate_custom_condition(raw)?;
                raw.clone()
            }
        })
    }

    /// Reject the two injection vectors that matter inside a
    /// condition scalar:
    ///
    /// - ADO pipeline commands (`##vso[`, `##[`) — would be acted on
    ///   at runtime if echoed by an executor.
    /// - Embedded newlines — would break the YAML scalar shape (a
    ///   scalar with embedded `\n` can flip from inline to block
    ///   style, and the resulting YAML may not parse the way we want).
    ///
    /// Does **not** reject ADO expressions (`$(...)`, `$[...]`,
    /// `${{...}}`); the whole point of `Custom` is to embed ADO
    /// syntax the AST does not yet model.
    fn validate_custom_condition(raw: &str) -> Result<()> {
        if crate::validate::contains_pipeline_command(raw) {
            bail!(
                "Condition::Custom: pipeline-command marker ('##vso[' or '##[') in condition body \
                 is rejected for safety. Got: {raw:?}"
            );
        }
        if crate::validate::contains_newline(raw) {
            bail!(
                "Condition::Custom: embedded newline in condition body is rejected (would break YAML scalar shape). \
                 Got: {raw:?}"
            );
        }
        Ok(())
    }

    /// Lower an [`Expr`] to its ADO atom string. `Expr::StepOutput`
    /// uses the consumer's location from `ctx` to pick the right
    /// reference syntax.
    pub fn lower_expr(ctx: &CondCodegenCtx<'_>, e: &Expr) -> Result<String> {
        Ok(match e {
            Expr::Literal(v) => format!("'{}'", v.replace('\'', "''")),
            Expr::Variable(name) => format!("variables['{name}']"),
            Expr::StepOutput(r) => {
                let producer_loc = ctx
                    .graph
                    .step_locations
                    .get(&r.step)
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "ir::condition: Expr::StepOutput references unknown step '{}' \
                             (graph::build_graph should have caught this)",
                            r.step
                        )
                    })?;
                let producer = ProducerLocation {
                    stage: producer_loc.stage.as_ref(),
                    job: &producer_loc.job,
                };
                lower_outputref(ctx.consumer(), producer, r)?
            }
        })
    }

    fn flatten_and(parts: &[Condition]) -> Vec<&Condition> {
        let mut out = Vec::with_capacity(parts.len());
        for p in parts {
            if let Condition::And(children) = p {
                out.extend(flatten_and(children));
            } else {
                out.push(p);
            }
        }
        out
    }

    fn flatten_or(parts: &[Condition]) -> Vec<&Condition> {
        let mut out = Vec::with_capacity(parts.len());
        for p in parts {
            if let Condition::Or(children) = p {
                out.extend(flatten_or(children));
            } else {
                out.push(p);
            }
        }
        out
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::compile::ir::ids::JobId;

        fn ctx_for<'a>(graph: &'a Graph, job: &'a JobId) -> CondCodegenCtx<'a> {
            CondCodegenCtx {
                graph,
                stage: None,
                job,
            }
        }

        #[test]
        fn lowers_each_terminal_variant() {
            let g = Graph::default();
            let job = JobId::new("J").unwrap();
            let ctx = ctx_for(&g, &job);
            assert_eq!(lower_condition(&ctx, &Condition::Succeeded).unwrap(), "succeeded()");
            assert_eq!(lower_condition(&ctx, &Condition::Always).unwrap(), "always()");
            assert_eq!(lower_condition(&ctx, &Condition::Failed).unwrap(), "failed()");
            assert_eq!(
                lower_condition(&ctx, &Condition::SucceededOrFailed).unwrap(),
                "succeededOrFailed()"
            );
        }

        #[test]
        fn flattens_nested_and_or() {
            let g = Graph::default();
            let job = JobId::new("J").unwrap();
            let ctx = ctx_for(&g, &job);
            let c = Condition::and([
                Condition::Succeeded,
                Condition::and([Condition::Always, Condition::Failed]),
            ]);
            assert_eq!(
                lower_condition(&ctx, &c).unwrap(),
                "and(succeeded(), always(), failed())"
            );
            let c = Condition::or([
                Condition::or([Condition::Succeeded, Condition::Failed]),
                Condition::Always,
            ]);
            assert_eq!(
                lower_condition(&ctx, &c).unwrap(),
                "or(succeeded(), failed(), always())"
            );
        }

        #[test]
        fn lowers_eq_ne_with_literal_and_variable() {
            let g = Graph::default();
            let job = JobId::new("J").unwrap();
            let ctx = ctx_for(&g, &job);
            let c = Condition::Eq(
                Expr::Variable("Build.Reason".into()),
                Expr::Literal("PullRequest".into()),
            );
            assert_eq!(
                lower_condition(&ctx, &c).unwrap(),
                "eq(variables['Build.Reason'], 'PullRequest')"
            );
            let c = Condition::Ne(
                Expr::Variable("Build.Reason".into()),
                Expr::Literal("PullRequest".into()),
            );
            assert_eq!(
                lower_condition(&ctx, &c).unwrap(),
                "ne(variables['Build.Reason'], 'PullRequest')"
            );
        }

        #[test]
        fn lowers_not_and_nested_combinations() {
            let g = Graph::default();
            let job = JobId::new("J").unwrap();
            let ctx = ctx_for(&g, &job);
            let c = Condition::not(Condition::Eq(
                Expr::Variable("X".into()),
                Expr::Literal("y".into()),
            ));
            assert_eq!(
                lower_condition(&ctx, &c).unwrap(),
                "not(eq(variables['X'], 'y'))"
            );
        }

        #[test]
        fn literal_expr_quotes_apostrophe_safely() {
            let g = Graph::default();
            let job = JobId::new("J").unwrap();
            let ctx = ctx_for(&g, &job);
            let e = Expr::Literal("it's fine".into());
            assert_eq!(lower_expr(&ctx, &e).unwrap(), "'it''s fine'");
        }

        #[test]
        fn custom_passes_ado_expressions_through() {
            let g = Graph::default();
            let job = JobId::new("J").unwrap();
            let ctx = ctx_for(&g, &job);
            let c = Condition::Custom(
                "eq(dependencies.Setup.outputs['x.y'], 'true')".to_string(),
            );
            assert_eq!(
                lower_condition(&ctx, &c).unwrap(),
                "eq(dependencies.Setup.outputs['x.y'], 'true')"
            );
            let c = Condition::Custom("eq(variables['X'], '${{ parameters.y }}')".to_string());
            assert!(lower_condition(&ctx, &c).is_ok());
        }

        #[test]
        fn custom_rejects_pipeline_command_injection() {
            let g = Graph::default();
            let job = JobId::new("J").unwrap();
            let ctx = ctx_for(&g, &job);
            let c = Condition::Custom("##vso[task.setvariable variable=X]y".to_string());
            let err = lower_condition(&ctx, &c).unwrap_err();
            assert!(format!("{err:#}").contains("pipeline-command marker"));
        }

        #[test]
        fn custom_rejects_embedded_newline() {
            let g = Graph::default();
            let job = JobId::new("J").unwrap();
            let ctx = ctx_for(&g, &job);
            let c = Condition::Custom("eq(a, b)\nor(c, d)".to_string());
            let err = lower_condition(&ctx, &c).unwrap_err();
            assert!(format!("{err:#}").contains("embedded newline"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::ir::ids::StepId;

    #[test]
    fn and_constructor_collects_iterator() {
        let c = Condition::and([Condition::Succeeded, Condition::Always]);
        match c {
            Condition::And(parts) => assert_eq!(parts.len(), 2),
            _ => panic!(),
        }
    }

    #[test]
    fn expr_step_output_carries_typed_producer() {
        let step = StepId::new("synthPr").unwrap();
        let e = Expr::StepOutput(OutputRef::new(step.clone(), "AW_SYNTHETIC_PR_SKIP"));
        match e {
            Expr::StepOutput(r) => {
                assert_eq!(r.step, step);
                assert_eq!(r.name, "AW_SYNTHETIC_PR_SKIP");
            }
            _ => panic!(),
        }
    }

    #[test]
    fn not_boxes_inner() {
        let c = Condition::not(Condition::Succeeded);
        assert!(matches!(c, Condition::Not(_)));
    }
}
