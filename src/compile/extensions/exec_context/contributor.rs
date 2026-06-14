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

    /// Bash commands the agent must have on its allow-list to inspect
    /// the staged context (e.g. `git diff`, `git show`). Aggregated by
    /// `ExecContextExtension` and forwarded
    /// through `src/engine.rs::args` to the agent's `shell(...)`
    /// allow-list.
    fn bash_commands(&self) -> Vec<String>;
}

/// Static-dispatch enum over all known contributors.
pub(super) enum Contributor {
    Pr(super::pr::PrContextContributor),
    Manual(super::manual::ManualContextContributor),
    Pipeline(super::pipeline::PipelineContextContributor),
    CiPush(super::ci_push::CiPushContextContributor),
    Workitem(super::workitem::WorkitemContextContributor),
}

impl ContextContributor for Contributor {
    fn name(&self) -> &str {
        match self {
            Contributor::Pr(c) => c.name(),
            Contributor::Manual(c) => c.name(),
            Contributor::Pipeline(c) => c.name(),
            Contributor::CiPush(c) => c.name(),
            Contributor::Workitem(c) => c.name(),
        }
    }
    fn should_activate(&self, ctx: &CompileContext) -> bool {
        match self {
            Contributor::Pr(c) => c.should_activate(ctx),
            Contributor::Manual(c) => c.should_activate(ctx),
            Contributor::Pipeline(c) => c.should_activate(ctx),
            Contributor::CiPush(c) => c.should_activate(ctx),
            Contributor::Workitem(c) => c.should_activate(ctx),
        }
    }
    fn prepare_step_typed(
        &self,
        ctx: &CompileContext,
    ) -> anyhow::Result<Option<crate::compile::ir::step::Step>> {
        match self {
            Contributor::Pr(c) => c.prepare_step_typed(ctx),
            Contributor::Manual(c) => c.prepare_step_typed(ctx),
            Contributor::Pipeline(c) => c.prepare_step_typed(ctx),
            Contributor::CiPush(c) => c.prepare_step_typed(ctx),
            Contributor::Workitem(c) => c.prepare_step_typed(ctx),
        }
    }
    fn bash_commands(&self) -> Vec<String> {
        match self {
            Contributor::Pr(c) => c.bash_commands(),
            Contributor::Manual(c) => c.bash_commands(),
            Contributor::Pipeline(c) => c.bash_commands(),
            Contributor::CiPush(c) => c.bash_commands(),
            Contributor::Workitem(c) => c.bash_commands(),
        }
    }
}
