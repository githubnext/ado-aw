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
        // When `mode: synthetic` is on, PR-identifier env vars are
        // coalesced via `$[ ... ]` runtime expressions reading the
        // **Agent-job-level** variables that `generate_agent_job_variables`
        // hoists from `dependencies.Setup.outputs['synthPr.*']`. We
        // deliberately do NOT put `dependencies.Setup.outputs[...]`
        // directly in step-level `env:` here — that combination has
        // proven unreliable in practice (msazuresphere/4x4 build
        // #612290: the same reference resolved correctly at
        // job-condition scope but returned empty at step-env scope,
        // causing the bash gate to short-circuit on a synth-promoted
        // build and the agent to emit `noop`). The job-variable hoist
        // is the documented safe location for cross-job output
        // references: <https://learn.microsoft.com/en-us/azure/devops/pipelines/process/variables#use-outputs-in-the-same-pipeline>.
        //
        // `System.PullRequest.TargetBranch` is `refs/heads/<name>` (full
        // ref form), matching the `targetRefName` shape stashed in
        // `AW_SYNTHETIC_PR_TARGETBRANCH`, so the coalesce yields a
        // consistent value either way.
        //
        // ## Synth-active gating — bash, not step `condition:`
        //
        // ADO step-level `condition:` fields CANNOT reference
        // `dependencies.<Job>.outputs[...]`. That syntax is only legal
        // in **job**-level `condition:`, in `variables:` mappings,
        // and in step-level `env:` values (via `$[ ... ]`). Attempting
        // to use it in a step condition produces a pipeline-validation
        // error ("Unrecognized value: 'dependencies'") and the build
        // fails before the Agent job starts.
        //
        // We therefore project the synth flag through the same
        // step-level `env:` indirection used for the PR id / target
        // branch (reading the hoisted job-level variable, not the
        // raw cross-job dependency) and gate in the bash body. The
        // step still emits as `succeeded` in the ADO UI on
        // non-PR / non-synth builds (with a single skip log line)
        // rather than as `skipped` — a minor cosmetic cost for
        // avoiding a cross-cutting template / trait change.
        //
        // The synth-INACTIVE branch is unchanged: its
        // `condition: eq(variables['Build.Reason'], 'PullRequest')`
        // only reads `variables[...]`, which IS legal at step level.
        let (pr_id_macro, target_branch_macro, prelude, condition) = if self.synthetic_pr_active {
            (
                "\"$[ coalesce(variables['System.PullRequest.PullRequestId'], variables['AW_SYNTHETIC_PR_ID']) ]\"",
                "\"$[ coalesce(variables['System.PullRequest.TargetBranch'], variables['AW_SYNTHETIC_PR_TARGETBRANCH']) ]\"",
                // Bash gate. `$AW_SYNTHETIC_PR` reads the hoisted
                // job-level variable via the step-env `$(...)` macro
                // below, which is "true" on a synth-promoted build,
                // "" otherwise. Both var refs are quoted for
                // shellcheck; both args to `[` are literal-string
                // comparisons so `set -u` is safe on either side.
                "    if [ \"$BUILD_REASON\" != \"PullRequest\" ] && [ \"$AW_SYNTHETIC_PR\" != \"true\" ]; then\n      echo \"[aw-context] Not a PR build and not synth-promoted; skipping exec-context-pr.\"\n      exit 0\n    fi\n",
                "succeeded()",
            )
        } else {
            (
                "$(System.PullRequest.PullRequestId)",
                "$(System.PullRequest.TargetBranch)",
                "",
                "eq(variables['Build.Reason'], 'PullRequest')",
            )
        };
        // Synth-active path adds two env vars (BUILD_REASON + the
        // synth flag) the bash prelude reads. `AW_SYNTHETIC_PR` is
        // pulled via the `$(name)` macro from the Agent-job-level
        // `variables:` block (see `generate_agent_job_variables` in
        // `compile/common.rs`) — NOT directly from
        // `dependencies.Setup.outputs[...]`, see the doc-comment on
        // this function for why.
        let synth_env = if self.synthetic_pr_active {
            "\n    BUILD_REASON: $(Build.Reason)\n    AW_SYNTHETIC_PR: $(AW_SYNTHETIC_PR)"
        } else {
            ""
        };
        format!(
            r#"- bash: |
    set -euo pipefail
{prelude}    node '{EXEC_CONTEXT_PR_PATH}'
  env:
    SYSTEM_ACCESSTOKEN: $(System.AccessToken)
    SYSTEM_PULLREQUEST_PULLREQUESTID: {pr_id_macro}
    SYSTEM_PULLREQUEST_TARGETBRANCH: {target_branch_macro}
    SYSTEM_TEAMPROJECT: $(System.TeamProject)
    BUILD_REPOSITORY_NAME: $(Build.Repository.Name)
    BUILD_SOURCESDIRECTORY: $(Build.SourcesDirectory){synth_env}
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
    fn prepare_step_synth_active_emits_coalesced_env_and_bash_synth_guard() {
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
                "SYSTEM_PULLREQUEST_PULLREQUESTID: \"$[ coalesce(variables['System.PullRequest.PullRequestId'], variables['AW_SYNTHETIC_PR_ID']) ]\""
            ),
            "synth-active prepare step must coalesce PR id with the hoisted job-level AW_SYNTHETIC_PR_ID variable: {step}"
        );
        assert!(
            step.contains(
                "SYSTEM_PULLREQUEST_TARGETBRANCH: \"$[ coalesce(variables['System.PullRequest.TargetBranch'], variables['AW_SYNTHETIC_PR_TARGETBRANCH']) ]\""
            ),
            "synth-active prepare step must coalesce target branch with the hoisted job-level AW_SYNTHETIC_PR_TARGETBRANCH variable: {step}"
        );

        // Env: BUILD_REASON + synth flag projected through env so the
        // bash gate has plain `$BUILD_REASON` / `$AW_SYNTHETIC_PR` to
        // read. `AW_SYNTHETIC_PR` is read via the same-job macro from
        // the Agent-job-level `variables:` block (hoisted from
        // `dependencies.Setup.outputs['synthPr.AW_SYNTHETIC_PR']` in
        // `generate_agent_job_variables`), NOT the raw cross-job
        // dependency reference at step-env scope (that combination
        // proved unreliable in msazuresphere/4x4 build #612290).
        assert!(
            step.contains("BUILD_REASON: $(Build.Reason)"),
            "synth-active prepare step must project Build.Reason through env for the bash guard: {step}"
        );
        assert!(
            step.contains("AW_SYNTHETIC_PR: $(AW_SYNTHETIC_PR)"),
            "synth-active prepare step must pull AW_SYNTHETIC_PR from the Agent-job-level hoisted variable via the $(...) macro: {step}"
        );
        assert!(
            !step.contains("dependencies.Setup.outputs['synthPr."),
            "synth-active prepare step must NOT reference `dependencies.Setup.outputs[...]` directly at step-env scope — that combination is unreliable; use the hoisted Agent-job-level variables instead: {step}"
        );

        // Bash guard: literal `if` chain that exits 0 when neither
        // condition holds. Variables are double-quoted so shellcheck
        // is clean and `set -u` is safe.
        assert!(
            step.contains(
                "if [ \"$BUILD_REASON\" != \"PullRequest\" ] && [ \"$AW_SYNTHETIC_PR\" != \"true\" ]; then"
            ),
            "synth-active prepare step must include the bash gate (replaces the illegal step-level condition): {step}"
        );
        assert!(
            step.contains(
                "[aw-context] Not a PR build and not synth-promoted; skipping exec-context-pr."
            ),
            "synth-active prepare step must emit a single skip log line so the no-op is discoverable: {step}"
        );

        // Step condition: must be `succeeded()` (the only legal form
        // here — cross-job dep refs are illegal at step level).
        assert!(
            step.contains("condition: succeeded()"),
            "synth-active prepare step must use `condition: succeeded()` and gate in bash: {step}"
        );

        // Regression trap: the v6.x emission put a cross-job ref in
        // the step `condition:`. ADO rejects that with
        // "Unrecognized value: 'dependencies'" and the pipeline never
        // starts the Agent job. Must NEVER come back.
        assert!(
            !step.contains(
                "condition: or(eq(variables['Build.Reason'], 'PullRequest'), eq(dependencies.Setup.outputs"
            ),
            "synth-active prepare step must NOT use the illegal cross-job dep ref in step `condition:` \
             (only legal in job-level conditions / `variables:` mappings / step `env:` values): {step}"
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
