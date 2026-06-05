//! Execution-context compiler extension.
//!
//! Always-on extension that owns the `aw-context/` precompute pipeline:
//! a fan-out over per-trigger [`ContextContributor`](contributor::ContextContributor)s
//! that materialise context (changed-files, diffs, snapshots, metadata)
//! on disk + supplement the agent prompt so the agent can read it
//! without rolling its own git plumbing.
//!
//! ## Why an extension, not a one-off PR-context flag
//!
//! See `docs/execution-context.md` and issue #860. The short version:
//! PR is the first (critical) contributor, but pipeline-completion,
//! schedule, and manual triggers all have context worth staging too.
//! Having one owner — gated by trigger — keeps the trust boundary
//! tight and the agent-visible layout uniform across trigger types.
//!
//! ## Prompt injection
//!
//! From v6.2 onward, contributors append their prompt fragments
//! **directly from their own prepare steps** to
//! `/tmp/awf-tools/agent-prompt.md` (created earlier by the "Prepare
//! agent prompt" step in `base.yml`). The extension does NOT implement
//! `prompt_supplement` — there is no static, always-injected prompt
//! header. Each contributor chooses at runtime, inside its prepare-step
//! bash, what (if anything) to append.
//!
//! ## Trust boundary
//!
//! Per-contributor prepare steps MAY pass `SYSTEM_ACCESSTOKEN` into
//! their own `env:` block (e.g. for the PR contributor's bearer
//! injection). This token is never propagated into the agent
//! container's env and never persisted to `.git/config`. See
//! `pr.rs` for the in-step bearer wrapper.

mod contributor;
mod pr;

use crate::compile::extensions::{CompileContext, CompilerExtension, ExtensionPhase};
use crate::compile::types::ExecutionContextConfig;

use contributor::{ContextContributor, Contributor};
use pr::PrContextContributor;

/// Always-on execution-context extension.
///
/// Owns the `aw-context/` precompute pipeline. Registered
/// unconditionally in
/// [`collect_extensions`](crate::compile::extensions::collect_extensions);
/// individual contributors gate themselves via
/// [`ContextContributor::should_activate`].
pub struct ExecContextExtension {
    config: ExecutionContextConfig,
    /// Whether the front matter configures any trigger that a context
    /// contributor activates on. Captured at construction time so
    /// `required_bash_commands()` (which receives no `CompileContext`)
    /// can suppress the contributor's bash allow-list contributions on
    /// agents whose triggers no contributor cares about. Today that
    /// means "is `on.pr` configured" — future trigger contributors
    /// will OR in their own checks here.
    any_contributor_active: bool,
}

impl ExecContextExtension {
    /// Build the extension from the resolved front-matter config.
    /// Pass `ExecutionContextConfig::default()` when the front matter
    /// omits the block entirely — defaults are "on for the triggers
    /// configured in `on:`".
    pub fn new(
        config: ExecutionContextConfig,
        front_matter: &crate::compile::types::FrontMatter,
    ) -> Self {
        // Pre-compute whether *any* contributor will activate, mirroring
        // each contributor's `should_activate` logic. The duplication is
        // intentional: keeping `should_activate` as the runtime
        // source-of-truth for `prepare_steps` (which DOES have a
        // `CompileContext`) and this aggregate for
        // `required_bash_commands` (which does not).
        //
        // MAINTENANCE: keep this aggregate in lock-step with each
        // contributor's `should_activate`. When adding a new
        // contributor, OR-in its activation predicate here so its
        // `required_bash_commands` are not silently suppressed.
        //
        // For the PR contributor specifically: `on.pr` is REQUIRED.
        // An explicit `pr.enabled: true` on a non-PR-triggered agent
        // does NOT activate (the prepare step would be dead code
        // because of the runtime `Build.Reason == 'PullRequest'` gate,
        // and silently widening the agent's bash allow-list with the
        // 7 git commands for a step that can never run is a footgun).
        let pr_trigger_configured = front_matter.pr_trigger().is_some();
        let pr_active = if !pr_trigger_configured {
            false
        } else {
            match config.pr.as_ref().and_then(|p| p.enabled) {
                Some(false) => false,
                Some(true) | None => true,
            }
        };
        Self {
            config,
            any_contributor_active: pr_active,
        }
    }

    /// Return the contributors that *might* contribute, in v1 order.
    /// Activation is decided per-contributor via
    /// [`ContextContributor::should_activate`].
    fn contributors(&self) -> Vec<Contributor> {
        // Default-empty PR config when omitted: keeps the existing
        // "on by default when on.pr is configured" behaviour without
        // the user having to write `execution-context.pr: {}`.
        let pr_cfg = self.config.pr.clone().unwrap_or_default();
        vec![Contributor::Pr(PrContextContributor::new(pr_cfg))]
    }
}

impl CompilerExtension for ExecContextExtension {
    fn name(&self) -> &str {
        "Execution Context"
    }

    fn phase(&self) -> ExtensionPhase {
        // Tool phase: runs after Runtime so any runtime-installed git
        // (none today, but defensive) is on PATH; before user `steps:`
        // so they can read `aw-context/`.
        ExtensionPhase::Tool
    }

    fn prepare_steps(&self, ctx: &CompileContext) -> Vec<String> {
        // Master switch off → no steps, no `aw-context/`.
        if !self.config.is_enabled() {
            return vec![];
        }
        self.contributors()
            .into_iter()
            .filter(|c| c.should_activate(ctx))
            .map(|c| c.prepare_step(ctx))
            .filter(|s| !s.is_empty())
            .collect()
    }

    fn required_bash_commands(&self) -> Vec<String> {
        // No bash contributions when the extension is off or when no
        // contributor will activate (avoids quietly widening the agent
        // bash allow-list on agents with no PR trigger configured).
        if !self.config.is_enabled() || !self.any_contributor_active {
            return vec![];
        }
        // Union of every contributor's required commands. The agent
        // bash allow-list needs these to inspect the staged context
        // (e.g. `git diff $BASE..$HEAD`). We do not gate per-contributor
        // on `should_activate` here because the bash allow-list is a
        // compile-time *capability* surface: it must be present
        // whenever the contributor *might* activate at runtime
        // (manual queue of a PR-triggered pipeline, etc.).
        let mut out: Vec<String> = self
            .contributors()
            .into_iter()
            .flat_map(|c| c.required_bash_commands())
            .collect();
        out.sort();
        out.dedup();
        out
    }
}
