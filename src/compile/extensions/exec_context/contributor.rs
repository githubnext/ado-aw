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
/// `condition:` so non-matching trigger types skip with zero cost),
/// plus a prompt-supplement fragment and env var declarations.
pub(super) trait ContextContributor {
    /// Display name for diagnostics (e.g. `"pr"`).
    #[allow(dead_code)]
    fn name(&self) -> &str;

    /// Whether this contributor activates for the given compile context.
    fn should_activate(&self, ctx: &CompileContext) -> bool;

    /// Generate the prepare-step YAML (a single `- bash:` block or
    /// equivalent). Must include its own ADO `condition:` so the step
    /// no-ops on non-matching trigger types. Empty string = no step.
    fn prepare_step(&self, ctx: &CompileContext) -> String;

    /// Markdown fragment to append to the agent prompt (under the
    /// "Execution context" supplement section). Empty = no fragment.
    fn prompt_fragment(&self) -> String;

    /// Agent env vars this contributor exposes. Currently unused
    /// (the ado-aw env-var channel rejects ADO `$(...)` expressions,
    /// so all per-trigger metadata flows through files), but kept on
    /// the trait so a future contributor can opt in if it only needs
    /// literal values.
    #[allow(dead_code)]
    fn agent_env_vars(&self) -> Vec<(String, String)>;

    /// Bash commands the agent must have on its allow-list to read
    /// the staged context (e.g. `cat`, `ls`). The agent itself does
    /// NOT need `git`, `mkdir`, etc. — those run in the precompute
    /// step which is outside the agent sandbox.
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
    fn prepare_step(&self, ctx: &CompileContext) -> String {
        match self {
            Contributor::Pr(c) => c.prepare_step(ctx),
        }
    }
    fn prompt_fragment(&self) -> String {
        match self {
            Contributor::Pr(c) => c.prompt_fragment(),
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
