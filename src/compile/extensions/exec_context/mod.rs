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
    /// `prompt_supplement()` (which receives no `CompileContext`) can
    /// suppress the prompt fragment on agents whose triggers no
    /// contributor cares about. Today that means "is `on.pr`
    /// configured" — future trigger contributors will OR in their
    /// own checks here.
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
        // `CompileContext`) and this aggregate for `prompt_supplement`
        // (which does not).
        let pr_active = match config.pr.as_ref().and_then(|p| p.enabled) {
            Some(true) => true,
            Some(false) => false,
            None => front_matter.pr_trigger().is_some(),
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

    fn prompt_supplement(&self) -> Option<String> {
        if !self.config.is_enabled() || !self.any_contributor_active {
            return None;
        }
        // Concatenate every contributor's prompt fragment under a
        // single "Execution context" header. We do not gate on
        // `should_activate` here because `should_activate` needs a
        // `CompileContext` that the trait method does not receive —
        // the fragments themselves describe how the agent should
        // detect whether their context is present at runtime (status
        // files / missing directories).
        let mut body = String::from("\n---\n\n## Execution context\n");
        for c in self.contributors() {
            let frag = c.prompt_fragment();
            if !frag.trim().is_empty() {
                body.push_str(&frag);
            }
        }
        Some(body)
    }

    fn required_bash_commands(&self) -> Vec<String> {
        if !self.config.is_enabled() {
            return vec![];
        }
        // Union of every contributor's required commands. The agent
        // bash allow-list needs these to read the staged context. We
        // do not gate on activation here because the bash allow-list
        // is a compile-time *capability* surface: it must be present
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

    fn validate(&self, _ctx: &CompileContext) -> anyhow::Result<Vec<String>> {
        // Reject ADO macro / template / runtime expressions in
        // `execution-context.pr.scope` entries. The scope values are
        // splatted into a bash array literal that ADO interpolates
        // BEFORE bash sees it — an entry like `$(System.AccessToken)`
        // would otherwise be replaced with the live token at runtime
        // and could be exfiltrated by an attacker who manages to land
        // a crafted scope value in the front matter.
        if let Some(pr) = self.config.pr.as_ref() {
            for entry in &pr.scope {
                if crate::validate::contains_ado_expression(entry) {
                    anyhow::bail!(
                        "Front matter 'execution-context.pr.scope[]' entry contains an \
                         ADO expression ('${{{{', '$(', or '$[') which is not allowed. \
                         Use literal pathspecs only. Found: '{}'",
                        entry,
                    );
                }
                if crate::validate::contains_pipeline_command(entry) {
                    anyhow::bail!(
                        "Front matter 'execution-context.pr.scope[]' entry contains an \
                         ADO pipeline command ('##vso[' or '##[') which is not allowed. \
                         Found: '{}'",
                        entry,
                    );
                }
                if entry.contains('\n') || entry.contains('\r') {
                    anyhow::bail!(
                        "Front matter 'execution-context.pr.scope[]' entry contains a \
                         newline which is not allowed. Found: '{}'",
                        entry,
                    );
                }
            }
        }
        Ok(vec![])
    }
}
