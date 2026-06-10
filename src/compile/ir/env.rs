//! Typed environment-variable values for steps.
//!
//! Replaces the hand-built strings that today live in
//! `src/compile/extensions/exec_context/pr.rs` and friends. The
//! lowering pass (introduced in the `ir-output-lowering` commit) turns
//! each [`EnvValue`] into the literal ADO scalar that gets emitted into
//! the step's `env:` block.
//!
//! ## Variants
//!
//! - [`EnvValue::Literal`] — a plain string (e.g. `"true"`).
//! - [`EnvValue::AdoMacro`] — an ADO predefined-variable macro like
//!   `$(Build.Reason)`. Only macros in [`ALLOWED_ADO_MACROS`] are
//!   accepted at construction so a future typo is caught at compile
//!   time, not at pipeline-runtime where it would silently expand to
//!   the literal text `$(Bad.Var)`.
//! - [`EnvValue::PipelineVar`] — a user-defined pipeline variable
//!   reference (`$(MY_VAR)`). Less constrained than `AdoMacro`
//!   because the universe of user vars is open.
//! - [`EnvValue::Secret`] — same lowering as `PipelineVar` but
//!   flagged for audit (e.g. so the upcoming `ir::validate` pass can
//!   reject leaking a secret into a non-secret context).
//! - [`EnvValue::StepOutput`] — a reference to an output declared by
//!   another step. The lowering pass picks the correct ADO syntax
//!   (same-job macro / cross-job / cross-stage).
//! - [`EnvValue::Coalesce`] — the typed form of
//!   `$[ coalesce(a, b, …, '') ]`. Lowers to a single ADO runtime
//!   expression. Nested `Coalesce` is flattened during lowering.

use super::output::OutputRef;

/// A typed value that ends up on the right-hand side of a YAML
/// `env:` mapping entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvValue {
    /// Plain string literal.
    Literal(String),
    /// ADO predefined-variable macro. Must be a member of
    /// [`ALLOWED_ADO_MACROS`].
    AdoMacro(&'static str),
    /// User-defined pipeline variable reference (`$(NAME)`).
    PipelineVar(String),
    /// Secret pipeline variable reference (`$(NAME)`); same wire
    /// shape as `PipelineVar` but tagged for the validate pass.
    Secret(String),
    /// Output of another step. The lowering pass selects the correct
    /// ADO reference syntax based on the consumer's location relative
    /// to the producer.
    StepOutput(OutputRef),
    /// Coalesce expression: lowers to `$[ coalesce(<a>, <b>, …, '') ]`.
    /// Nested `Coalesce` is flattened so the final form has at most
    /// one outer `$[ coalesce(...) ]` wrapper.
    Coalesce(Vec<EnvValue>),
}

/// Allowlist of ADO predefined-variable macros that may appear in
/// [`EnvValue::AdoMacro`]. Sourced from the canonical list at
/// <https://learn.microsoft.com/en-us/azure/devops/pipelines/build/variables>.
///
/// The list is intentionally a closed enum-via-data: any value not on
/// it must use [`EnvValue::PipelineVar`] instead, which makes the
/// "is this a real ADO predefined variable" check explicit.
///
/// Extend this list (with a rationale comment) when a new
/// predefined-variable use site appears.
pub const ALLOWED_ADO_MACROS: &[&str] = &[
    // Build context — used everywhere the agent needs to know where /
    // why / on what code the build is running.
    "Build.Reason",
    "Build.BuildId",
    "Build.SourceBranch",
    "Build.SourceVersion",
    "Build.SourcesDirectory",
    "Build.Repository.ID",
    "Build.Repository.Name",
    "Build.Repository.Provider",
    "Build.DefinitionName",
    // Pipeline / system context — Setup-job synthetic-PR resolver, AWF
    // launch, and most safe-output executors need at least one of
    // these.
    "Pipeline.Workspace",
    "Agent.TempDirectory",
    "System.AccessToken",
    "System.CollectionUri",
    "System.TeamProject",
    "System.DefinitionId",
    // PR-build identifiers — coalesced with synthPr.* outputs on the
    // synthetic-from-CI path.
    "System.PullRequest.PullRequestId",
    "System.PullRequest.SourceBranch",
    "System.PullRequest.TargetBranch",
];

impl EnvValue {
    /// Construct an [`EnvValue::Literal`].
    pub fn literal(s: impl Into<String>) -> Self {
        EnvValue::Literal(s.into())
    }

    /// Construct an [`EnvValue::AdoMacro`], validating `name` against
    /// [`ALLOWED_ADO_MACROS`].
    ///
    /// Returns `Err` for unknown macros so a typo can't silently
    /// produce the literal text `$(Bad.Var)` at runtime.
    pub fn ado_macro(name: &'static str) -> anyhow::Result<Self> {
        if !ALLOWED_ADO_MACROS.contains(&name) {
            anyhow::bail!(
                "EnvValue::ado_macro('{name}'): not in ALLOWED_ADO_MACROS — \
                 use EnvValue::PipelineVar for user-defined variables, or add \
                 the macro to the allowlist with a rationale"
            );
        }
        Ok(EnvValue::AdoMacro(name))
    }

    /// Construct an [`EnvValue::PipelineVar`].
    pub fn pipeline_var(name: impl Into<String>) -> Self {
        EnvValue::PipelineVar(name.into())
    }

    /// Construct an [`EnvValue::Secret`].
    pub fn secret(name: impl Into<String>) -> Self {
        EnvValue::Secret(name.into())
    }

    /// Construct an [`EnvValue::StepOutput`].
    pub fn step_output(r: OutputRef) -> Self {
        EnvValue::StepOutput(r)
    }

    /// Construct an [`EnvValue::Coalesce`]. The lowering pass
    /// flattens nested `Coalesce` and appends `''` for safety, so
    /// callers do not have to.
    pub fn coalesce(values: Vec<EnvValue>) -> Self {
        EnvValue::Coalesce(values)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::ir::ids::StepId;

    #[test]
    fn ado_macro_accepts_allowlisted() {
        assert!(matches!(
            EnvValue::ado_macro("Build.Reason").unwrap(),
            EnvValue::AdoMacro("Build.Reason")
        ));
    }

    #[test]
    fn ado_macro_rejects_unknown() {
        let err = EnvValue::ado_macro("Not.A.Real.Var").unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("not in ALLOWED_ADO_MACROS"));
    }

    #[test]
    fn coalesce_carries_typed_children() {
        let step = StepId::new("synthPr").unwrap();
        let v = EnvValue::coalesce(vec![
            EnvValue::ado_macro("System.PullRequest.PullRequestId").unwrap(),
            EnvValue::step_output(OutputRef::new(step, "AW_SYNTHETIC_PR_ID")),
        ]);
        match v {
            EnvValue::Coalesce(parts) => assert_eq!(parts.len(), 2),
            _ => panic!("expected Coalesce"),
        }
    }
}
