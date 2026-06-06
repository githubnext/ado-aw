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
use crate::compile::types::{ExecutionContextConfig, FrontMatter};

use contributor::{ContextContributor, Contributor};
use pr::PrContextContributor;

/// Returns `true` iff the PR-context contributor will activate for the
/// given front matter. Shared between `ExecContextExtension::new` (for
/// its own `any_contributor_active` precomputation) and
/// `collect_extensions` (which passes it to `AdoScriptExtension` so
/// the Agent-job install/download fires whenever the bundle is needed).
///
/// MAINTENANCE: this MUST match `PrContextContributor::should_activate`
/// (in `pr.rs`). The duplication is intentional — `should_activate`
/// takes a `CompileContext` that includes both front matter and target,
/// while this helper only needs the front matter (because `target` is
/// not relevant to PR activation today).
pub fn pr_contributor_will_activate(front_matter: &FrontMatter) -> bool {
    // Borrow the embedded config when present; fall back to a stack-
    // local default. Avoids the per-call clone — this helper is called
    // on every `collect_extensions` invocation, which is hot during
    // compile.
    let default_cfg = ExecutionContextConfig::default();
    let cfg = front_matter
        .execution_context
        .as_ref()
        .unwrap_or(&default_cfg);
    pr_contributor_will_activate_with_cfg(cfg, front_matter)
}

/// Variant that takes the resolved `ExecutionContextConfig` explicitly.
/// Used by [`ExecContextExtension::new`] so its internal
/// `any_contributor_active` precomputation tracks the config it was
/// handed, not just the config embedded in `front_matter` (which can
/// diverge in unit tests).
fn pr_contributor_will_activate_with_cfg(
    cfg: &ExecutionContextConfig,
    front_matter: &FrontMatter,
) -> bool {
    if front_matter.pr_trigger().is_none() {
        return false;
    }
    if !cfg.is_enabled() {
        return false;
    }
    let pr_enabled = cfg.pr.as_ref().and_then(|p| p.enabled);
    !matches!(pr_enabled, Some(false))
}

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
        // Use the shared activation predicate so this stays in
        // lock-step with `collect_extensions` (which passes the same
        // signal to `AdoScriptExtension`). Use the cfg-aware variant
        // so unit tests that construct a custom `config` (separate
        // from `front_matter.execution_context`) still see the right
        // activation answer.
        let any_contributor_active =
            pr_contributor_will_activate_with_cfg(&config, front_matter);
        Self {
            config,
            any_contributor_active,
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

#[cfg(test)]
mod tests {
    //! Divergence-trap tests for the `any_contributor_active`
    //! pre-computation. The pattern in [`ExecContextExtension::new`]
    //! duplicates each contributor's `should_activate` logic so the
    //! pre-computed flag can gate [`required_bash_commands`] (which
    //! receives no `CompileContext`). The risk is that a future
    //! contributor author wires up `should_activate` + the
    //! `contributors()` list but forgets to OR-in the aggregate
    //! check, silently suppressing the contributor's bash-allow-list
    //! contributions.
    //!
    //! These tests exercise the `new()` → `required_bash_commands()`
    //! path independently (no fixture-compile, no `prepare_steps`,
    //! no `CompileContext`) so a future divergence trips here at
    //! unit-test time rather than at E2E time.

    use super::*;
    use crate::compile::types::{
        ExecutionContextConfig, FrontMatter, PrContextConfig,
    };

    /// Parse a minimal markdown agent into a `FrontMatter`.
    fn parse_fm(src: &str) -> FrontMatter {
        let (fm, _) = crate::compile::common::parse_markdown(src).unwrap();
        fm
    }

    /// Minimal agent with `on.pr` configured (default `branches`).
    fn pr_triggered_front_matter() -> FrontMatter {
        parse_fm(
            "---\nname: test\ndescription: test\non:\n  pr:\n    branches:\n      include: [main]\n---\n",
        )
    }

    /// Minimal agent with no triggers configured.
    fn no_trigger_front_matter() -> FrontMatter {
        parse_fm("---\nname: test\ndescription: test\n---\n")
    }

    /// When `on.pr` is configured (default `pr.enabled`),
    /// `required_bash_commands` MUST yield the PR contributor's
    /// git commands. If a future contributor diverges this from
    /// `should_activate`, this assertion trips.
    #[test]
    fn required_bash_commands_matches_pr_contributor_active_default() {
        let ext =
            ExecContextExtension::new(ExecutionContextConfig::default(), &pr_triggered_front_matter());
        let cmds = ext.required_bash_commands();
        assert!(
            !cmds.is_empty(),
            "PR contributor is active (on.pr configured, default pr.enabled) \
             but required_bash_commands is empty — `any_contributor_active` \
             has diverged from `PrContextContributor::should_activate`."
        );
        assert!(
            cmds.iter().any(|c| c == "git diff"),
            "PR contributor's git commands missing from required_bash_commands: {cmds:?}"
        );
    }

    /// Same scenario, with `pr.enabled: true` explicit. Must still
    /// yield commands (matches `should_activate`).
    #[test]
    fn required_bash_commands_matches_pr_contributor_active_explicit_enabled() {
        let cfg = ExecutionContextConfig {
            enabled: None,
            pr: Some(PrContextConfig {
                enabled: Some(true),
            }),
        };
        let ext = ExecContextExtension::new(cfg, &pr_triggered_front_matter());
        assert!(
            !ext.required_bash_commands().is_empty(),
            "explicit pr.enabled: true + on.pr configured must yield bash commands"
        );
    }

    /// With `on.pr` configured but `pr.enabled: false`, the
    /// contributor is inactive — commands MUST be suppressed.
    /// Mirrors `should_activate`'s `Some(false)` arm.
    #[test]
    fn required_bash_commands_suppressed_when_pr_disabled() {
        let cfg = ExecutionContextConfig {
            enabled: None,
            pr: Some(PrContextConfig {
                enabled: Some(false),
            }),
        };
        let ext = ExecContextExtension::new(cfg, &pr_triggered_front_matter());
        assert!(
            ext.required_bash_commands().is_empty(),
            "pr.enabled: false must suppress required_bash_commands"
        );
    }

    /// No `on.pr` trigger configured → contributor inactive →
    /// no commands. Mirrors `should_activate`'s `on.pr` gate.
    #[test]
    fn required_bash_commands_suppressed_without_on_pr() {
        let ext =
            ExecContextExtension::new(ExecutionContextConfig::default(), &no_trigger_front_matter());
        assert!(
            ext.required_bash_commands().is_empty(),
            "without on.pr configured, required_bash_commands must be empty"
        );
    }

    /// Explicit `pr.enabled: true` without `on.pr` must still
    /// yield no commands (v6.2 footgun fix — bash allow-list is a
    /// compile-time artifact for a step that can never run).
    #[test]
    fn required_bash_commands_suppressed_when_enabled_without_on_pr() {
        let cfg = ExecutionContextConfig {
            enabled: None,
            pr: Some(PrContextConfig {
                enabled: Some(true),
            }),
        };
        let ext = ExecContextExtension::new(cfg, &no_trigger_front_matter());
        assert!(
            ext.required_bash_commands().is_empty(),
            "pr.enabled: true without on.pr must NOT widen the agent bash allow-list"
        );
    }

    /// Master switch off must suppress commands regardless of
    /// contributor state.
    #[test]
    fn required_bash_commands_suppressed_when_master_switch_off() {
        let cfg = ExecutionContextConfig {
            enabled: Some(false),
            pr: None,
        };
        let ext = ExecContextExtension::new(cfg, &pr_triggered_front_matter());
        assert!(
            ext.required_bash_commands().is_empty(),
            "execution-context.enabled: false must suppress required_bash_commands"
        );
    }
}
