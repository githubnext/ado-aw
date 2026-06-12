//! Internal `ContextContributor` trait + `Contributor` enum.
//!
//! The execution-context extension is itself a `CompilerExtension`
//! (always-on, registered in `collect_extensions()`). Internally it
//! delegates to a small set of per-trigger **context contributors**,
//! each responsible for materialising one slice of `aw-context/`.
//!
//! v1 ships one contributor: `PrContextContributor`. Future
//! contributors (pipeline-completion, schedule, manual) slot in via
//! the same trait + enum without touching callers.
//!
//! ## Why a private trait instead of reusing `CompilerExtension`
//!
//! `CompilerExtension` is the public boundary between the compiler
//! and runtimes/tools. Context contributors are private implementation
//! detail of one extension; they share the same `CompileContext` input
//! but emit a narrower output (a single prepare step + a single prompt
//! supplement + a few env vars). Keeping them behind a small private
//! trait avoids accidentally exposing them as user-facing extensions
//! and lets us evolve the contract freely.

use crate::compile::extensions::CompileContext;

/// A unit of per-trigger execution-context generation.
///
/// Each contributor decides — based on `CompileContext` (front matter,
/// triggers, target) — whether it activates. Activated contributors
/// each emit exactly one prepare bash step (wrapped in an ADO
/// `condition:` so non-matching trigger types skip with zero cost)
/// and declare the bash commands the agent needs to inspect the
/// staged context. Prompt-fragment injection is the contributor's
/// own responsibility — emit `cat >> "/tmp/awf-tools/agent-prompt.md"`
/// inside `prepare_step` for whatever (success / failure) fragment the
/// runtime decides on.
pub(super) trait ContextContributor {
    /// Display name for diagnostics (e.g. `"pr"`). Defaults to
    /// `"unknown"`; implementors with a meaningful identifier should
    /// override. Currently no caller reads this — kept as a low-cost
    /// hook so a future log-line / audit step has a stable channel.
    #[allow(dead_code)]
    fn name(&self) -> &str {
        "unknown"
    }

    /// Whether this contributor activates for the given compile context.
    fn should_activate(&self, ctx: &CompileContext) -> bool;

    /// Generate the prepare step as a typed
    /// [`crate::compile::ir::step::Step`].
    fn prepare_step_typed(
        &self,
        ctx: &CompileContext,
    ) -> anyhow::Result<Option<crate::compile::ir::step::Step>>;

    /// Agent env vars this contributor exposes. Defaults to none —
    /// the ado-aw env-var channel rejects ADO `$(...)` expressions, so
    /// all per-trigger metadata currently flows through files. Kept
    /// on the trait so a future contributor that only needs literal
    /// values can opt in without changing the wiring.
    #[allow(dead_code)]
    fn agent_env_vars(&self) -> Vec<(String, String)> {
        Vec::new()
    }

    /// Bash commands the agent must have on its allow-list to inspect
    /// the staged context (e.g. `git diff`, `git show`). Aggregated by
    /// `ExecContextExtension::required_bash_commands` and forwarded
    /// through `src/engine.rs::args` to the agent's `shell(...)`
    /// allow-list.
    fn required_bash_commands(&self) -> Vec<String>;
}

/// Static-dispatch enum over all known contributors.
///
/// Mirrors the `Extension` enum pattern in `extensions/mod.rs`. v1
/// ships `Pr`; adding a future variant requires only a new arm here
/// and a registration in `ExecContextExtension::contributors()`.
pub(super) enum Contributor {
    Pr(super::pr::PrContextContributor),
}

impl ContextContributor for Contributor {
    fn name(&self) -> &str {
        match self {
            Contributor::Pr(c) => c.name(),
        }
    }
    fn should_activate(&self, ctx: &CompileContext) -> bool {
        match self {
            Contributor::Pr(c) => c.should_activate(ctx),
        }
    }
    fn prepare_step_typed(
        &self,
        ctx: &CompileContext,
    ) -> anyhow::Result<Option<crate::compile::ir::step::Step>> {
        match self {
            Contributor::Pr(c) => c.prepare_step_typed(ctx),
        }
    }
    fn agent_env_vars(&self) -> Vec<(String, String)> {
        match self {
            Contributor::Pr(c) => c.agent_env_vars(),
        }
    }
    fn required_bash_commands(&self) -> Vec<String> {
        match self {
            Contributor::Pr(c) => c.required_bash_commands(),
        }
    }
}
