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
use crate::compile::ir::condition::{Condition, Expr};
use crate::compile::ir::env::EnvValue;
use crate::compile::ir::ids::StepId;
use crate::compile::ir::output::OutputRef;
use crate::compile::ir::step::{BashStep, Step};
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
        // ## Synth-active vs synth-inactive env wiring
        //
        // **Synth-active** (`mode: synthetic`, the default): the
        // `synthPr` Setup-job step runs unconditionally and emits
        // `AW_PR_ID` / `AW_PR_TARGETBRANCH` / `AW_PR_SOURCEBRANCH`
        // under canonical names — on real PR builds they hold the
        // copied `SYSTEM_PULLREQUEST_*` values; on synth-promoted CI
        // builds they hold the discovered PR identifiers. The Agent
        // job hoists those outputs to job-level variables (see
        // `generate_agent_job_variables`). This step consumes them
        // via plain `$(name)` macros — no `$[ ... ]` in step `env:`
        // (which ADO doesn't evaluate; that bug bit
        // msazuresphere/4x4 build #612528).
        //
        // **Synth-inactive** (`mode: policy`): no `synthPr` step
        // emits the hoist; the step reads `$(System.PullRequest.*)`
        // macros directly and gates on `eq(Build.Reason,
        // 'PullRequest')` at step level.
        //
        // ## Synth-active gating — bash, not step `condition:`
        //
        // ADO step-level `condition:` fields CANNOT reference
        // `dependencies.<Job>.outputs[...]`. That syntax is only legal
        // in **job**-level `condition:` and in `variables:` mappings.
        // Attempting to use it in a step condition produces a pipeline-
        // validation error ("Unrecognized value: 'dependencies'") and
        // the build fails before the Agent job starts.
        //
        // We therefore gate in bash: the resolved `AW_PR_ID` is empty
        // iff this is neither a real PR build nor a synth-promoted CI
        // build, which is exactly when the bundle should skip. Same
        // gate logic, but in the only place ADO actually lets us put
        // it. The step still emits as `succeeded` in the ADO UI on
        // skips (with a single log line) rather than `skipped` — a
        // minor cosmetic cost for avoiding a cross-cutting template
        // / trait change.
        //
        // The synth-INACTIVE branch is unchanged: its
        // `condition: eq(variables['Build.Reason'], 'PullRequest')`
        // only reads `variables[...]`, which IS legal at step level.
        let (pr_id_macro, target_branch_macro, prelude, condition) = if self.synthetic_pr_active {
            (
                "$(AW_PR_ID)",
                "$(AW_PR_TARGETBRANCH)",
                // Bash gate. `$AW_PR_ID` reads the hoisted job-level
                // variable via the step-env `$(...)` macro below. It
                // is non-empty when the build is either a real PR or
                // synth-promoted; empty otherwise. Quoted for
                // shellcheck and `set -u` safety.
                "    if [ -z \"$AW_PR_ID\" ]; then\n      echo \"[aw-context] No PR identifier resolved (not a PR build and not synth-promoted); skipping exec-context-pr.\"\n      exit 0\n    fi\n",
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
    BUILD_SOURCESDIRECTORY: $(Build.SourcesDirectory)
  displayName: "Stage PR execution context (aw-context/pr/*)"
  condition: {condition}"#
        )
    }

    fn agent_env_vars(&self) -> Vec<(String, String)> {
        vec![]
    }

    fn prepare_step_typed(
        &self,
        _ctx: &CompileContext,
    ) -> anyhow::Result<Option<Step>> {
        // Typed-IR sibling of [`Self::prepare_step`]. The synth-active
        // path uses the typed [`EnvValue::Coalesce`] / [`EnvValue::StepOutput`]
        // pair instead of hand-written `$[ coalesce(...) ]` strings;
        // the lowering pass picks the cross-job
        // `dependencies.Setup.outputs[...]` form for the synthPr ref
        // (the consumer is in the Agent job, the producer in Setup).
        //
        // Coexists with `prepare_step` until production callers switch.
        let synth_id = StepId::new("synthPr")?;
        let (pr_id, target_branch, prelude, condition, synth_extras) = if self.synthetic_pr_active
        {
            let pr_id = EnvValue::coalesce(vec![
                EnvValue::ado_macro("System.PullRequest.PullRequestId")?,
                EnvValue::step_output(OutputRef::new(synth_id.clone(), "AW_SYNTHETIC_PR_ID")),
            ]);
            let target_branch = EnvValue::coalesce(vec![
                EnvValue::ado_macro("System.PullRequest.TargetBranch")?,
                EnvValue::step_output(OutputRef::new(
                    synth_id.clone(),
                    "AW_SYNTHETIC_PR_TARGETBRANCH",
                )),
            ]);
            // Same bash gate as the legacy emitter — the typed Step
            // models the same scalar bash body verbatim.
            let prelude = "    if [ \"$BUILD_REASON\" != \"PullRequest\" ] && [ \"$AW_SYNTHETIC_PR\" != \"true\" ]; then\n      echo \"[aw-context] Not a PR build and not synth-promoted; skipping exec-context-pr.\"\n      exit 0\n    fi\n";
            // BUILD_REASON + AW_SYNTHETIC_PR projected through env so
            // the bash gate has plain `$BUILD_REASON` / `$AW_SYNTHETIC_PR`
            // to read (cross-job refs are illegal in step `condition:`
            // but legal in step `env:` values).
            let synth_extras: Vec<(&'static str, EnvValue)> = vec![
                ("BUILD_REASON", EnvValue::ado_macro("Build.Reason")?),
                (
                    "AW_SYNTHETIC_PR",
                    // Single-child Coalesce lowers to
                    // `coalesce(<child>, '')` — same shape as the
                    // legacy emitter's hand-written
                    // `$[ coalesce(dependencies.Setup.outputs['synthPr.AW_SYNTHETIC_PR'], '') ]`.
                    EnvValue::coalesce(vec![EnvValue::step_output(OutputRef::new(
                        synth_id, "AW_SYNTHETIC_PR",
                    ))]),
                ),
            ];
            (
                pr_id,
                target_branch,
                prelude,
                Condition::Succeeded,
                synth_extras,
            )
        } else {
            (
                EnvValue::ado_macro("System.PullRequest.PullRequestId")?,
                EnvValue::ado_macro("System.PullRequest.TargetBranch")?,
                "",
                Condition::Eq(
                    Expr::Variable("Build.Reason".to_string()),
                    Expr::Literal("PullRequest".to_string()),
                ),
                vec![],
            )
        };
        let script = format!(
            "set -euo pipefail\n{prelude}node '{EXEC_CONTEXT_PR_PATH}'\n"
        );
        let mut step = BashStep::new(
            "Stage PR execution context (aw-context/pr/*)",
            script,
        )
        .with_condition(condition)
        .with_env(
            "SYSTEM_ACCESSTOKEN",
            EnvValue::ado_macro("System.AccessToken")?,
        )
        .with_env("SYSTEM_PULLREQUEST_PULLREQUESTID", pr_id)
        .with_env("SYSTEM_PULLREQUEST_TARGETBRANCH", target_branch)
        .with_env(
            "SYSTEM_TEAMPROJECT",
            EnvValue::ado_macro("System.TeamProject")?,
        )
        .with_env(
            "BUILD_REPOSITORY_NAME",
            EnvValue::ado_macro("Build.Repository.Name")?,
        )
        .with_env(
            "BUILD_SOURCESDIRECTORY",
            EnvValue::ado_macro("Build.SourcesDirectory")?,
        );
        for (k, v) in synth_extras {
            step = step.with_env(k, v);
        }
        Ok(Some(Step::Bash(step)))
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
    fn prepare_step_synth_active_uses_macros_for_hoisted_aw_pr_vars_and_bash_guard() {
        let contributor = PrContextContributor::new(PrContextConfig::default(), true);
        let fm = pr_fm();
        let ctx = CompileContext::for_test(&fm);
        let step = contributor.prepare_step(&ctx);

        // Env: PR id + target branch read the Agent-job-level hoisted
        // AW_PR_* variables (which `generate_agent_job_variables`
        // declares from `dependencies.Setup.outputs['synthPr.AW_PR_*']`).
        // Use plain `$(name)` macros — NOT `$[ ... ]` runtime expressions
        // (ADO doesn't evaluate `$[ ... ]` inside step `env:`; the
        // literal expression string gets passed verbatim and downstream
        // validation rejects it — see msazuresphere/4x4 build #612528).
        assert!(
            step.contains("SYSTEM_PULLREQUEST_PULLREQUESTID: $(AW_PR_ID)"),
            "synth-active prepare step must read the hoisted Agent-job-level AW_PR_ID via $() macro: {step}"
        );
        assert!(
            step.contains("SYSTEM_PULLREQUEST_TARGETBRANCH: $(AW_PR_TARGETBRANCH)"),
            "synth-active prepare step must read the hoisted Agent-job-level AW_PR_TARGETBRANCH via $() macro: {step}"
        );

        // Defensive: NO `$[ ... ]` runtime expressions in this step's
        // env block. They're only legal inside `variables:` mappings
        // and `condition:` fields — putting them in step env is the
        // exact bug class this refactor eliminates.
        let env_block_start = step
            .find("\n  env:\n")
            .expect("step must have an env block");
        let env_block_end = step[env_block_start..]
            .find("\n  displayName:")
            .map(|i| env_block_start + i)
            .unwrap_or(step.len());
        let env_block = &step[env_block_start..env_block_end];
        assert!(
            !env_block.contains("$["),
            "prepare step env block must not contain `$[ ` runtime expressions \
             (ADO doesn't evaluate them in step env — use job-level variables \
             hoist + $() macro instead): {env_block}"
        );

        // Bash guard: empty `$AW_PR_ID` means "not a PR build and not
        // synth-promoted". Single check replaces the previous
        // BUILD_REASON + AW_SYNTHETIC_PR pair (the merge now happens
        // inside `exec-context-pr-synth.js`).
        assert!(
            step.contains("if [ -z \"$AW_PR_ID\" ]; then"),
            "synth-active prepare step must include the bash gate on empty AW_PR_ID: {step}"
        );
        assert!(
            step.contains("[aw-context] No PR identifier resolved"),
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
             (only legal in job-level conditions / `variables:` mappings): {step}"
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
            !step.contains("AW_PR_ID"),
            "synth-inactive prepare step must not reference the synth-only AW_PR_ID hoist: {step}"
        );
        assert!(
            !step.contains("synthPr"),
            "synth-inactive prepare step must not reference any synthPr Setup-job output: {step}"
        );
    }

    // ── Typed-IR `prepare_step_typed` shape tests (port-exec-context) ──

    /// Synth-active: the typed prepare step's env block must carry
    /// typed `Coalesce(AdoMacro, StepOutput)` for `SYSTEM_PULLREQUEST_*`
    /// and a typed `Coalesce(StepOutput)` for `AW_SYNTHETIC_PR` —
    /// no [`Step::RawYaml`], no hand-written `$[ coalesce(...) ]`
    /// strings.
    #[test]
    fn prepare_step_typed_synth_active_carries_typed_coalesce_envs() {
        let contributor = PrContextContributor::new(PrContextConfig::default(), true);
        let fm = pr_fm();
        let ctx = CompileContext::for_test(&fm);
        let step = contributor
            .prepare_step_typed(&ctx)
            .expect("typed prepare_step succeeds")
            .expect("contributor activates");

        let bash = match &step {
            Step::Bash(b) => b,
            other => panic!("expected Step::Bash, got {other:?}"),
        };

        // Condition: succeeded() — cross-job dep refs are illegal at
        // step level, so the synth-active path gates in bash and
        // keeps the step condition trivial.
        assert!(
            matches!(bash.condition, Some(Condition::Succeeded)),
            "synth-active condition must be Succeeded; got {:?}",
            bash.condition
        );

        // PR id env: typed Coalesce[AdoMacro, StepOutput].
        let pr_id = bash
            .env
            .get("SYSTEM_PULLREQUEST_PULLREQUESTID")
            .expect("PR id env present");
        match pr_id {
            EnvValue::Coalesce(parts) => {
                assert_eq!(parts.len(), 2);
                assert!(matches!(
                    parts[0],
                    EnvValue::AdoMacro("System.PullRequest.PullRequestId")
                ));
                match &parts[1] {
                    EnvValue::StepOutput(r) => {
                        assert_eq!(r.step.as_str(), "synthPr");
                        assert_eq!(r.name, "AW_SYNTHETIC_PR_ID");
                    }
                    other => panic!("expected StepOutput, got {other:?}"),
                }
            }
            other => panic!("expected Coalesce, got {other:?}"),
        }

        // Target branch env: same shape with the target-branch macro
        // + the synth target-branch output.
        let target_branch = bash
            .env
            .get("SYSTEM_PULLREQUEST_TARGETBRANCH")
            .expect("target branch env present");
        match target_branch {
            EnvValue::Coalesce(parts) => {
                assert!(matches!(
                    parts[0],
                    EnvValue::AdoMacro("System.PullRequest.TargetBranch")
                ));
                match &parts[1] {
                    EnvValue::StepOutput(r) => {
                        assert_eq!(r.name, "AW_SYNTHETIC_PR_TARGETBRANCH");
                    }
                    other => panic!("expected StepOutput, got {other:?}"),
                }
            }
            other => panic!("expected Coalesce, got {other:?}"),
        }

        // AW_SYNTHETIC_PR projected with a single-child Coalesce —
        // lowering adds the trailing `''` automatically so the wire
        // form matches `coalesce(<ref>, '')`.
        let synth_flag = bash
            .env
            .get("AW_SYNTHETIC_PR")
            .expect("AW_SYNTHETIC_PR env present");
        match synth_flag {
            EnvValue::Coalesce(parts) => {
                assert_eq!(parts.len(), 1);
                match &parts[0] {
                    EnvValue::StepOutput(r) => {
                        assert_eq!(r.name, "AW_SYNTHETIC_PR");
                    }
                    other => panic!("expected StepOutput, got {other:?}"),
                }
            }
            other => panic!("expected Coalesce, got {other:?}"),
        }

        // BUILD_REASON projected through env as a typed AdoMacro.
        assert!(matches!(
            bash.env.get("BUILD_REASON"),
            Some(EnvValue::AdoMacro("Build.Reason"))
        ));

        // SYSTEM_ACCESSTOKEN must still be in the step's env (the
        // trust boundary that the bundle relies on).
        assert!(matches!(
            bash.env.get("SYSTEM_ACCESSTOKEN"),
            Some(EnvValue::AdoMacro("System.AccessToken"))
        ));
    }

    /// Synth-inactive: PR id / target branch are plain
    /// `EnvValue::AdoMacro` values, no Coalesce; condition is the
    /// typed `Eq(Variable("Build.Reason"), Literal("PullRequest"))`.
    #[test]
    fn prepare_step_typed_synth_inactive_uses_plain_macros_and_narrow_condition() {
        let contributor = PrContextContributor::new(PrContextConfig::default(), false);
        let fm = pr_fm();
        let ctx = CompileContext::for_test(&fm);
        let step = contributor
            .prepare_step_typed(&ctx)
            .expect("typed prepare_step succeeds")
            .expect("contributor activates");

        let bash = match &step {
            Step::Bash(b) => b,
            other => panic!("expected Step::Bash, got {other:?}"),
        };

        assert!(matches!(
            bash.env.get("SYSTEM_PULLREQUEST_PULLREQUESTID"),
            Some(EnvValue::AdoMacro("System.PullRequest.PullRequestId"))
        ));
        assert!(matches!(
            bash.env.get("SYSTEM_PULLREQUEST_TARGETBRANCH"),
            Some(EnvValue::AdoMacro("System.PullRequest.TargetBranch"))
        ));

        // No BUILD_REASON / AW_SYNTHETIC_PR env entries (the bash
        // guard isn't emitted on the synth-inactive path).
        assert!(!bash.env.contains_key("BUILD_REASON"));
        assert!(!bash.env.contains_key("AW_SYNTHETIC_PR"));

        match bash.condition.as_ref().expect("condition required") {
            Condition::Eq(Expr::Variable(name), Expr::Literal(lit)) => {
                assert_eq!(name, "Build.Reason");
                assert_eq!(lit, "PullRequest");
            }
            other => panic!(
                "expected Condition::Eq(Variable, Literal), got {other:?}"
            ),
        }
    }
}
