//! Typed ADO condition AST.
//!
//! Replaces the hand-built condition strings that today live in
//! `generate_agentic_depends_on` (`src/compile/common.rs:2388-2530`)
//! and `compile_gate_step_external` (`src/compile/filter_ir.rs:1147+`).
//!
//! Only the **types** are defined in the `ir-types` commit; the
//! lowering of [`Condition`] / [`Expr`] to the literal ADO condition
//! string lives in the `ir-condition-codegen` commit.

use super::output::OutputRef;

/// A typed ADO condition expression.
///
/// All ADO `condition:` strings are eventually reducible to one of
/// these forms. The `Custom` escape hatch is intentionally
/// last-resort; the IR validate pass runs it through the same
/// pipeline-command-injection check that the rest of the compiler
/// applies to user-supplied expressions.
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
    /// validate pass rejects values that contain pipeline-command
    /// injection markers.
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
