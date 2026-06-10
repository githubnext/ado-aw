//! Declared step outputs and references to them.
//!
//! A step that wants its output visible to other steps records the
//! output in [`BashStep::outputs`](super::step::BashStep::outputs)
//! using [`OutputDecl`]. Consumers reference the value via
//! [`OutputRef`].
//!
//! The actual lowering of an [`OutputRef`] to one of the three ADO
//! reference syntaxes (same-job macro, cross-job, cross-stage) lives
//! in the `ir-output-lowering` commit; this module just defines the
//! types.

use super::ids::StepId;

/// A named output exported by a step.
///
/// The compiler auto-emits `isOutput=true` on the underlying
/// `##vso[task.setvariable]` line iff at least one cross-step
/// consumer references this name via [`OutputRef`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputDecl {
    /// The output variable name (the `variable=` value in
    /// `##vso[task.setvariable variable=NAME;isOutput=true]`).
    pub name: String,
    /// Whether the producing step also marks the variable as a secret
    /// (`issecret=true`). Independent of cross-step visibility.
    pub is_secret: bool,
}

impl OutputDecl {
    /// Construct a plain (non-secret) output declaration.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            is_secret: false,
        }
    }

    /// Construct a secret output declaration.
    pub fn secret(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            is_secret: true,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outputdecl_new_defaults_to_non_secret() {
        let d = OutputDecl::new("AW_SYNTHETIC_PR");
        assert_eq!(d.name, "AW_SYNTHETIC_PR");
        assert!(!d.is_secret);
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
}
