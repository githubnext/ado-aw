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
//! - [`EnvValue::Concat`] — the **macro-form** sibling of `Coalesce`:
//!   children are lowered individually and the results are joined
//!   without a separator (`<a><b>…`). Used today for the
//!   `$(System.PullRequest.X)$(synthPr.X)` exclusive-OR concat in
//!   the `prGate` step — both halves are macros that are
//!   mutually-empty at runtime, so concatenation yields the live
//!   value with **no runtime-expression wrap**. This matters for
//!   same-job consumers, where macro form is the only form that
//!   resolves correctly (see `src/compile/filter_ir.rs` for the
//!   underlying bug history).

use super::output::OutputRef;

/// A typed value that ends up on the right-hand side of a YAML
/// `env:` mapping entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvValue {
    /// Plain string literal — emitted **verbatim** into the YAML
    /// `env:` value position with no escaping or sanitisation.
    ///
    /// **Compiler-internal use only.** Construction sites must pass
    /// a hardcoded string, a constant, or a value derived from
    /// front-matter that has already been routed through
    /// `crate::validate::reject_pipeline_injection` (or a stronger
    /// equivalent). Never construct from raw user-supplied input —
    /// use [`EnvValue::PipelineVar`] (`$(NAME)` macro form) or
    /// [`EnvValue::Secret`] when the value should come from an ADO
    /// variable, or [`EnvValue::AdoMacro`] for predefined variables
    /// on the allowlist.
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
    ///
    /// Children that resolve to same-job step outputs are rejected at
    /// lower time — use [`EnvValue::Concat`] instead.
    Coalesce(Vec<EnvValue>),
    /// Macro-form concatenation: lowers each child individually and
    /// joins the results with no separator and no outer wrap.
    ///
    /// Use this when the result must remain a plain ADO scalar (not
    /// a `$[ … ]` runtime expression), e.g. when the consumer is in
    /// the same job as the producing step output and the macro form
    /// `$(stepName.X)` is the only form that resolves correctly.
    /// Typical pattern is two mutually-empty macros so concatenation
    /// yields the live value — the `prGate` step's
    /// `$(System.PullRequest.X)$(synthPr.X)` exclusive-OR.
    Concat(Vec<EnvValue>),
    /// Pre-built YAML scalar emitted verbatim into the value position.
    ///
    /// Used by [`crate::compile::agentic_pipeline`] when a legacy YAML
    /// env-block carries a non-string scalar (integer / boolean) that
    /// must round-trip unquoted (e.g. `GITHUB_READ_ONLY: 1` — not
    /// `'1'`). Bypasses the string-formatting lowering so
    /// serde_yaml's emitter sees the typed value directly.
    RawYamlScalar(serde_yaml::Value),
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
    // Requestor identity — surfaced by the `manual` execution-context
    // contributor (issue #860 follow-up; plan.md Stage 1) to give
    // manually-queued agents access to who queued them. Already
    // referenced as `$(Build.RequestedForEmail)` by the PR filter IR
    // (`src/compile/filter_ir.rs`); adding them to the typed allowlist
    // so the manual contributor can use `EnvValue::ado_macro(...)`
    // instead of stringly-typed `PipelineVar`.
    "Build.RequestedFor",
    "Build.RequestedForEmail",
    // Upstream-build identifiers populated when ADO triggers this
    // pipeline via a `resources.pipelines` completion trigger.
    // Surfaced by the `pipeline` execution-context contributor
    // (Stage 2 of the contributor build-out — see plan.md) so the
    // bundle can fetch upstream-build metadata via the Build REST API.
    // Note: these are always present on `Build.Reason == 'ResourceTrigger'`
    // builds and absent otherwise — the contributor gates on Build.Reason
    // so the macros never expand to empty in production paths.
    "Build.TriggeredBy.BuildId",
    "Build.TriggeredBy.DefinitionId",
    "Build.TriggeredBy.DefinitionName",
    "Build.TriggeredBy.ProjectID",
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
    /// Construct an [`EnvValue::Literal`]. **Compiler-internal use
    /// only** — see the variant's doc-comment for the contract on
    /// safe inputs (hardcoded strings, constants, or values
    /// pre-validated against `reject_pipeline_injection`).
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
    /// callers do not have to. Children that resolve to same-job step
    /// outputs are rejected at lower time — use [`EnvValue::Concat`]
    /// instead.
    pub fn coalesce(values: Vec<EnvValue>) -> Self {
        EnvValue::Coalesce(values)
    }

    /// Construct an [`EnvValue::Concat`] — macro-form concatenation
    /// of children. Unlike `Coalesce`, no outer wrap is added; the
    /// lowered children are joined verbatim.
    pub fn concat(values: Vec<EnvValue>) -> Self {
        EnvValue::Concat(values)
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
            EnvValue::step_output(OutputRef::new(step.clone(), "AW_SYNTHETIC_PR_ID")),
        ]);
        match v {
            EnvValue::Coalesce(parts) => {
                assert_eq!(parts.len(), 2);
                assert_eq!(
                    parts[0],
                    EnvValue::AdoMacro("System.PullRequest.PullRequestId"),
                    "first child must be the AdoMacro"
                );
                assert_eq!(
                    parts[1],
                    EnvValue::StepOutput(OutputRef::new(step, "AW_SYNTHETIC_PR_ID")),
                    "second child must be the StepOutput"
                );
            }
            _ => panic!("expected Coalesce"),
        }
    }

    #[test]
    fn concat_carries_typed_children() {
        let step = StepId::new("synthPr").unwrap();
        let v = EnvValue::concat(vec![
            EnvValue::ado_macro("System.PullRequest.PullRequestId").unwrap(),
            EnvValue::step_output(OutputRef::new(step, "AW_SYNTHETIC_PR_ID")),
        ]);
        match v {
            EnvValue::Concat(parts) => assert_eq!(parts.len(), 2),
            _ => panic!("expected Concat"),
        }
    }
}
