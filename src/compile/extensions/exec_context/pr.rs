//! PR-context contributor (v7).
//!
//! Activates on PR-triggered builds and stages a small focused set of
//! PR signals + appends a tailored prompt fragment to the agent prompt
//! file. The actual logic lives in the `exec-context-pr.js`
//! `ado-script` bundle — this Rust module's job is to emit a slim YAML
//! step that invokes it with the right env vars and condition gate.
//!
//! ## Artefacts (staged by the bundle on success)
//!
//! - `aw-context/pr/base.sha` — target merge-base SHA
//! - `aw-context/pr/head.sha` — PR head SHA
//!
//! On failure (validation or merge-base resolution failed):
//!
//! - `aw-context/pr/error.txt` — one-line failure reason
//!
//! ## Prompt injection
//!
//! `exec-context-pr.js` appends a success-or-failure prompt fragment
//! directly to `/tmp/awf-tools/agent-prompt.md` (created earlier by
//! the "Prepare agent prompt" step in `base.yml`). Short identifiers
//! (`PR_ID`, `PROJECT`, `REPO`) are interpolated into the prompt
//! literally so the agent sees natural English ("This is PR #4242 in
//! project 'OneBranch' / repository 'awesome-repo'.") + example ADO
//! MCP tool calls with the right arguments pre-filled.
//!
//! ## Trust boundary
//!
//! - `SYSTEM_ACCESSTOKEN` is mapped only into THIS step's `env:` block,
//!   never the agent step's env. Within this step, Node inherits the
//!   variable on its `process.env` (unavoidable — the ADO `env:` block
//!   exports to the step process), but it is never logged, never
//!   passed in argv, and never written to `.git/config`.
//! - The wrapping `GIT_CONFIG_*` env vars that actually carry the
//!   bearer into `git`'s `http.extraheader` config (see
//!   `scripts/ado-script/src/exec-context-pr/git.ts::bearerEnv`) are
//!   only ever set in the *spawned `git` child's* environment — not
//!   in Node's global `process.env`. This is a strict improvement
//!   over the v6.2 bash implementation, where the bearer also lived
//!   in the wrapping shell's env (shared with the `fail()` function,
//!   regex validation, etc.) on top of the same Node-step exposure.
//! - The token is never written to `.git/config`; `persistCredentials`
//!   is never `true`; no checkout override is emitted.
//! - The step is gated by `condition: eq(variables['Build.Reason'],
//!   'PullRequest')` so it never runs on non-PR builds.
//!
//! ## Wiring
//!
//! The bundle's install + download is owned by `AdoScriptExtension`'s
//! Agent-job `prepare_steps`. It fires whenever EITHER the
//! runtime-import resolver (`import.js`) OR the PR contributor
//! (this module) is active. See
//! `src/compile/extensions/ado_script.rs::prepare_steps` for the gate.
//!
//! `AdoScriptExtension` runs at `ExtensionPhase::System` and
//! `ExecContextExtension` runs at `ExtensionPhase::Tool`, so the
//! install/download always lands before the bundle invocation in the
//! emitted YAML.

use crate::compile::extensions::CompileContext;
use crate::compile::extensions::ado_script::EXEC_CONTEXT_PR_PATH;
use crate::compile::types::PrContextConfig;

use super::contributor::ContextContributor;

/// PR-context contributor. Activates when `on.pr` is configured
/// (unless explicitly disabled via `execution-context.pr.enabled: false`).
pub(super) struct PrContextContributor {
    config: PrContextConfig,
    /// Whether `on.pr.mode == Synthetic` for this agent. Drives
    /// emission of the coalesced `SYSTEM_PULLREQUEST_*` env vars so the
    /// bundle reads either real PR identifiers (true PR builds) or the
    /// `synthPr` Setup-job outputs (CI builds promoted via synth).
    synthetic_pr_active: bool,
}

impl PrContextContributor {
    pub(super) fn new(config: PrContextConfig, synthetic_pr_active: bool) -> Self {
        Self {
            config,
            synthetic_pr_active,
        }
    }
}

impl ContextContributor for PrContextContributor {
    fn name(&self) -> &str {
        "pr"
    }

    fn should_activate(&self, ctx: &CompileContext) -> bool {
        // MAINTENANCE: this MUST stay in lock-step with
        // `super::pr_contributor_will_activate` (the shared helper used
        // by `collect_extensions` to populate
        // `AdoScriptExtension::exec_context_pr_active`). The divergence-
        // trap tests in `super::tests` exercise the helper path; this
        // method is the runtime-context-aware version that
        // `prepare_steps` calls.
        if ctx.front_matter.pr_trigger().is_none() {
            return false;
        }
        match self.config.explicit_enabled() {
            Some(false) => false,
            Some(true) | None => true,
        }
    }

    fn prepare_step(&self, _ctx: &CompileContext) -> String {
        // Slim node-invocation wrapper. The actual logic (identifier
        // validation, fetch/merge-base, prompt fragment generation)
        // lives in the `exec-context-pr.js` bundle.
        //
        // `set -euo pipefail` is intentional here: the bundle exits 0
        // on every soft failure (validation, merge-base) and reserves
        // non-zero exits for true infra failures (e.g. could not
        // create the output directory) — those SHOULD propagate as a
        // hard pipeline failure.
        //
        // `SYSTEM_ACCESSTOKEN` is mapped only into this step's `env:`
        // block. Node receives it on `process.env` and passes it to
        // the spawned `git` subprocess via `GIT_CONFIG_*` env vars
        // (never argv). It is NEVER visible to the agent step.
        //
        // When `mode: synthetic` is on, the PR-identifier env vars
        // are emitted using `$[ coalesce(...) ]` so the bundle picks
        // up either the real `System.PullRequest.*` (on a true PR
        // build) OR the synthPr Setup-job output (on a CI build
        // promoted via exec-context-pr-synth.js). The step's
        // condition is also broadened to accept synth-promoted builds.
        //
        // Cross-job reference is correct here: this step runs in the
        // **Agent** job (which depends on Setup), so
        // `dependencies.Setup.outputs['synthPr.X']` resolves at runtime.
        // (Same-job references would need `variables['synthPr.X']`
        // instead — used by the gate step inside the Setup job itself.)
        // Runtime expressions `$[ ... ]` are documented as valid in
        // step-level `env:` blocks; see
        // <https://learn.microsoft.com/en-us/azure/devops/pipelines/process/variables>.
        // `System.PullRequest.TargetBranch` is `refs/heads/<name>` (full
        // ref form), matching the `targetRefName` shape returned by the
        // ADO REST API and stored in `AW_SYNTHETIC_PR_TARGETBRANCH`, so
        // the coalesce yields a consistent value either way.
        // `$[ ... ]` runtime expressions are wrapped in YAML double
        // quotes because their values contain single quotes (e.g.
        // `variables['System.PullRequest.PullRequestId']`). ADO accepts
        // them unquoted in practice, but double-quoting matches the
        // form shown in ADO docs and is strictly conformant to the
        // YAML spec (which reserves `'` as a scalar indicator).
        let (pr_id_macro, target_branch_macro, condition) = if self.synthetic_pr_active {
            (
                "\"$[ coalesce(variables['System.PullRequest.PullRequestId'], dependencies.Setup.outputs['synthPr.AW_SYNTHETIC_PR_ID']) ]\"",
                "\"$[ coalesce(variables['System.PullRequest.TargetBranch'], dependencies.Setup.outputs['synthPr.AW_SYNTHETIC_PR_TARGETBRANCH']) ]\"",
                "or(eq(variables['Build.Reason'], 'PullRequest'), eq(dependencies.Setup.outputs['synthPr.AW_SYNTHETIC_PR'], 'true'))",
            )
        } else {
            (
                "$(System.PullRequest.PullRequestId)",
                "$(System.PullRequest.TargetBranch)",
                "eq(variables['Build.Reason'], 'PullRequest')",
            )
        };
        format!(
            r#"- bash: |
    set -euo pipefail
    node '{EXEC_CONTEXT_PR_PATH}'
  env:
    SYSTEM_ACCESSTOKEN: $(System.AccessToken)
    SYSTEM_PULLREQUEST_PULLREQUESTID: {pr_id_macro}
    SYSTEM_PULLREQUEST_TARGETBRANCH: {target_branch_macro}
    SYSTEM_TEAMPROJECT: $(System.TeamProject)
    BUILD_REPOSITORY_NAME: $(Build.Repository.Name)
    BUILD_SOURCESDIRECTORY: $(Build.SourcesDirectory)
  displayName: "Stage PR execution context (aw-context/pr/*)"
  condition: {condition}"#
        )
    }

    fn agent_env_vars(&self) -> Vec<(String, String)> {
        vec![]
    }

    fn required_bash_commands(&self) -> Vec<String> {
        // Read-only git commands the agent needs to inspect the PR diff
        // locally. Added unconditionally when this contributor activates
        // (matches the runtime-extension pattern in
        // `src/runtimes/*/extension.rs::required_bash_commands`).
        vec![
            "git".to_string(),
            "git diff".to_string(),
            "git log".to_string(),
            "git show".to_string(),
            "git status".to_string(),
            "git rev-parse".to_string(),
            "git symbolic-ref".to_string(),
        ]
    }
}

#[cfg(test)]
mod tests {
    //! Direct unit tests for `PrContextContributor::prepare_step` —
    //! pins both the `mode: synthetic` (default) and `mode: policy`
    //! emitted YAML shapes for the env-coalesce macros and the
    //! step-level `condition:`. Catches accidental regressions of the
    //! coalesce wiring without round-tripping through a full snapshot
    //! fixture.
    use super::*;
    use crate::compile::extensions::CompileContext;
    use crate::compile::types::{FrontMatter, PrContextConfig};

    fn parse_fm(src: &str) -> FrontMatter {
        let (fm, _) = crate::compile::common::parse_markdown(src).unwrap();
        fm
    }

    fn pr_fm() -> FrontMatter {
        parse_fm(
            "---\nname: test\ndescription: test\non:\n  pr:\n    branches:\n      include: [main]\n---\n",
        )
    }

    #[test]
    fn prepare_step_synth_active_emits_coalesced_env_and_broadened_condition() {
        let contributor = PrContextContributor::new(PrContextConfig::default(), true);
        let fm = pr_fm();
        let ctx = CompileContext::for_test(&fm);
        let step = contributor.prepare_step(&ctx);

        // Env: PR id + target branch are coalesced via cross-job runtime
        // expressions wrapped in YAML double quotes (Agent job depends
        // on Setup, so `dependencies.Setup.outputs[...]` is the correct
        // form here — distinct from the gate step which is same-job).
        assert!(
            step.contains(
                "SYSTEM_PULLREQUEST_PULLREQUESTID: \"$[ coalesce(variables['System.PullRequest.PullRequestId'], dependencies.Setup.outputs['synthPr.AW_SYNTHETIC_PR_ID']) ]\""
            ),
            "synth-active prepare step must coalesce PR id with synthPr fallback: {step}"
        );
        assert!(
            step.contains(
                "SYSTEM_PULLREQUEST_TARGETBRANCH: \"$[ coalesce(variables['System.PullRequest.TargetBranch'], dependencies.Setup.outputs['synthPr.AW_SYNTHETIC_PR_TARGETBRANCH']) ]\""
            ),
            "synth-active prepare step must coalesce target branch with synthPr fallback: {step}"
        );

        // Condition: broadened to accept real PR builds OR synth-promoted
        // CI builds.
        assert!(
            step.contains(
                "condition: or(eq(variables['Build.Reason'], 'PullRequest'), eq(dependencies.Setup.outputs['synthPr.AW_SYNTHETIC_PR'], 'true'))"
            ),
            "synth-active prepare step must broaden the condition to accept synth-promoted builds: {step}"
        );
    }

    #[test]
    fn prepare_step_synth_inactive_emits_plain_macros_and_narrow_condition() {
        let contributor = PrContextContributor::new(PrContextConfig::default(), false);
        let fm = pr_fm();
        let ctx = CompileContext::for_test(&fm);
        let step = contributor.prepare_step(&ctx);

        // Env: plain `$(...)` macros for the real System.PullRequest.*
        // predefined variables — no coalesce, no quoting.
        assert!(
            step.contains("SYSTEM_PULLREQUEST_PULLREQUESTID: $(System.PullRequest.PullRequestId)"),
            "synth-inactive prepare step must use the plain ADO macro form: {step}"
        );
        assert!(
            step.contains("SYSTEM_PULLREQUEST_TARGETBRANCH: $(System.PullRequest.TargetBranch)"),
            "synth-inactive prepare step must use the plain ADO macro form: {step}"
        );

        // Condition: narrow to real PR builds only.
        assert!(
            step.contains("condition: eq(variables['Build.Reason'], 'PullRequest')"),
            "synth-inactive prepare step must keep the narrow PR-build condition: {step}"
        );

        // Defensive: the synth-mode signature MUST NOT appear when the
        // synth path is inactive.
        assert!(
            !step.contains("synthPr.AW_SYNTHETIC_PR"),
            "synth-inactive prepare step must not reference any synthPr Setup-job output: {step}"
        );
        assert!(
            !step.contains("coalesce("),
            "synth-inactive prepare step must not emit a coalesce expression: {step}"
        );
    }
}
