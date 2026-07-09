//! Single always-on extension that delivers and runs ado-script bundles
//! (gate.js, import.js) inside the ADO pipeline.
//!
//! Two features, each emitted into the job that actually consumes the
//! bundle:
//!
//! - **Gate evaluator (`gate.js`)** — runs in the **Setup job** when
//!   `filters:` lowers to non-empty checks.
//! - **Runtime-import resolver (`import.js`)** — runs in the **Agent
//!   job** when `inlined-imports: false`.
//!
//! ## Why per-job emission
//!
//! ADO jobs use isolated VMs — `/tmp` is **not** shared. The
//! `ado-script.zip` bundle is therefore downloaded once per consuming
//! job. When both features are active, install + download steps appear
//! in **both** Setup and Agent. That's correct architecture given ADO's
//! topology, not waste.

use anyhow::Result;

use super::{CompileContext, CompilerExtension, Declarations, ExtensionPhase};
use crate::compile::agentic_pipeline::{download_package_step, nuget_authenticate_step};
use crate::compile::filter_ir::{
    GateContext, Severity, build_gate_step_typed, lower_pipeline_filters, lower_pr_filters,
    validate_pipeline_filters, validate_pr_filters,
};
use crate::compile::ir::condition::{Condition, Expr};
use crate::compile::ir::env::EnvValue;
use crate::compile::ir::ids::StepId;
use crate::compile::ir::output::OutputDecl;
use crate::compile::ir::step::{BashStep, Step};
use crate::compile::ir::tasks::use_node::UseNode;
use crate::compile::types::{PipelineFilters, PrFilters, SupplyChainConfig};

pub(crate) const GATE_EVAL_PATH: &str = "/tmp/ado-aw-scripts/ado-script/gate.js";
pub(crate) const IMPORT_EVAL_PATH: &str = "/tmp/ado-aw-scripts/ado-script/import.js";
/// Path to the exec-context-pr bundle inside the unpacked `ado-script.zip`.
/// Consumed by `src/compile/extensions/exec_context/pr.rs` to invoke
/// the bundle from the PR contributor's prepare step.
pub(crate) const EXEC_CONTEXT_PR_PATH: &str = "/tmp/ado-aw-scripts/ado-script/exec-context-pr.js";
/// Path to the exec-context-manual bundle (Stage 1 of the
/// exec-context contributor build-out — see plan.md). Consumed by
/// `src/compile/extensions/exec_context/manual.rs` to invoke the
/// bundle from the Manual contributor's prepare step.
pub(crate) const EXEC_CONTEXT_MANUAL_PATH: &str =
    "/tmp/ado-aw-scripts/ado-script/exec-context-manual.js";
/// Path to the exec-context-pipeline bundle (Stage 2 of the
/// exec-context contributor build-out — see plan.md). Consumed by
/// `src/compile/extensions/exec_context/pipeline.rs` to invoke the
/// bundle from the Pipeline contributor's prepare step.
pub(crate) const EXEC_CONTEXT_PIPELINE_PATH: &str =
    "/tmp/ado-aw-scripts/ado-script/exec-context-pipeline.js";
/// Path to the exec-context-ci-push bundle (Stage 3 of the
/// exec-context contributor build-out — see plan.md). Consumed by
/// `src/compile/extensions/exec_context/ci_push.rs`.
pub(crate) const EXEC_CONTEXT_CI_PUSH_PATH: &str =
    "/tmp/ado-aw-scripts/ado-script/exec-context-ci-push.js";
/// Path to the exec-context-workitem bundle (Stage 4 of the
/// exec-context contributor build-out — see plan.md). Consumed by
/// `src/compile/extensions/exec_context/workitem.rs`. Stages
/// per-WI directories with description / acceptance / repro
/// content; crosses an untrusted-prose boundary (WI bodies are
/// user-authored — see `docs/execution-context.md`).
pub(crate) const EXEC_CONTEXT_WORKITEM_PATH: &str =
    "/tmp/ado-aw-scripts/ado-script/exec-context-workitem.js";
/// Path to the exec-context-schedule bundle (Stage 5 of the
/// exec-context contributor build-out — see plan.md).
pub(crate) const EXEC_CONTEXT_SCHEDULE_PATH: &str =
    "/tmp/ado-aw-scripts/ado-script/exec-context-schedule.js";
/// Path to the exec-context-pr-checks bundle (Stage 6 of the
/// exec-context contributor build-out — see plan.md). Extension of
/// the PR contributor that stages build-validation check info under
/// `aw-context/pr/checks/`.
pub(crate) const EXEC_CONTEXT_PR_CHECKS_PATH: &str =
    "/tmp/ado-aw-scripts/ado-script/exec-context-pr-checks.js";
/// Path to the exec-context-repo bundle (Stage 7 of the build-out —
/// see plan.md). Pure git, no REST.
pub(crate) const EXEC_CONTEXT_REPO_PATH: &str =
    "/tmp/ado-aw-scripts/ado-script/exec-context-repo.js";
/// Path to the synthetic-PR-context bundle inside the unpacked
/// `ado-script.zip`. Runs in the Setup job before `prGate`; consumed
/// by [`AdoScriptExtension::declarations`].
pub(crate) const EXEC_CONTEXT_PR_SYNTH_PATH: &str =
    "/tmp/ado-aw-scripts/ado-script/exec-context-pr-synth.js";
/// Path to the safe-outputs approval-summary bundle inside the unpacked
/// `ado-script.zip`. Runs at the end of the Agent job to render the proposed
/// safe outputs to a sanitized markdown summary tab.
pub(crate) const APPROVAL_SUMMARY_PATH: &str =
    "/tmp/ado-aw-scripts/ado-script/approval-summary.js";
/// Path to the conclusion bundle inside the unpacked `ado-script.zip`. Runs in
/// the always-on Conclusion job (see [`crate::compile::agentic_pipeline`]) to
/// file pipeline-failure work items and diagnostic signals. Referenced both by
/// that job's shell body and by `Bundle::Conclusion.path()` so the two copies
/// cannot diverge.
pub(crate) const CONCLUSION_PATH: &str = "/tmp/ado-aw-scripts/ado-script/conclusion.js";
/// Path to the github-app-token bundle inside the unpacked `ado-script.zip`.
/// Runs immediately before the Copilot invocation in the Agent and Detection
/// jobs (issue #1316) to mint a GitHub App installation token and expose it as
/// a masked same-job `GITHUB_APP_TOKEN` variable.
pub(crate) const GITHUB_APP_TOKEN_PATH: &str =
    "/tmp/ado-aw-scripts/ado-script/github-app-token.js";
/// Path to the prepare-pr-base bundle inside the unpacked `ado-script.zip`.
/// Runs in the Agent job before the Copilot invocation (issue #1413) when
/// `create-pull-request` is configured, to fetch/deepen the target branch so
/// the host-side SafeOutputs MCP server can compute a diff base on
/// shallow-default agent pools.
pub(crate) const PREPARE_PR_BASE_PATH: &str =
    "/tmp/ado-aw-scripts/ado-script/prepare-pr-base.js";
const RELEASE_BASE_URL: &str = "https://github.com/githubnext/ado-aw/releases/download";

/// Single always-on extension that owns all `ado-script` bundle wiring.
pub struct AdoScriptExtension {
    pub pr_filters: Option<PrFilters>,
    pub pipeline_filters: Option<PipelineFilters>,
    pub inlined_imports: bool,
    /// Whether the PR-context contributor will activate. When true,
    /// the Agent-job install/download must fire even if
    /// `runtime_imports_active()` is false (i.e. the user has
    /// `inlined-imports: true` but a PR trigger configured), so that
    /// `exec-context-pr.js` is present for the `pr.rs` invocation.
    ///
    /// Populated at construction by `collect_extensions` using the
    /// shared `exec_context_pr_active` predicate so this stays in
    /// lock-step with `ExecContextExtension`'s own activation gate.
    pub exec_context_pr_active: bool,
    /// Whether the Manual-context contributor (Stage 1 of the
    /// exec-context contributor build-out — see plan.md) will
    /// activate. When true, the Agent-job install/download must
    /// fire so that `exec-context-manual.js` is present.
    ///
    /// Populated at construction by `collect_extensions` using the
    /// shared `manual_contributor_will_activate` predicate so this
    /// stays in lock-step with the contributor's `should_activate`.
    pub exec_context_manual_active: bool,
    /// Whether the Pipeline-context contributor (Stage 2 of the
    /// exec-context contributor build-out — see plan.md) will
    /// activate. When true, the Agent-job install/download must
    /// fire so that `exec-context-pipeline.js` is present.
    ///
    /// Populated at construction by `collect_extensions` using the
    /// shared `pipeline_contributor_will_activate` predicate so this
    /// stays in lock-step with the contributor's `should_activate`.
    pub exec_context_pipeline_active: bool,
    /// Whether the CI-push-context contributor (Stage 3 of the
    /// exec-context contributor build-out — see plan.md) will
    /// activate. Default-off opt-in feature; when true the
    /// install/download must fire so that
    /// `exec-context-ci-push.js` is present.
    pub exec_context_ci_push_active: bool,
    /// Whether the Workitem-context contributor (Stage 4 of the
    /// exec-context contributor build-out — see plan.md) will
    /// activate. Activates whenever the PR contributor activates
    /// unless explicitly disabled. **Crosses an untrusted-prose
    /// boundary** — see workitem.rs.
    pub exec_context_workitem_active: bool,
    /// Whether the Schedule-context contributor (Stage 5 of the
    /// exec-context contributor build-out — see plan.md) will
    /// activate. Opt-in (default OFF).
    pub exec_context_schedule_active: bool,
    /// Whether the PR-checks extension (Stage 6 of the build-out —
    /// see plan.md) will activate. Opt-in (default OFF) AND
    /// requires the PR contributor to activate.
    pub exec_context_pr_checks_active: bool,
    /// Whether the Repo-context contributor (Stage 7 of the
    /// build-out — see plan.md) will activate. Always-on capability,
    /// default OFF (opt-in).
    pub exec_context_repo_active: bool,
    /// Whether the safe-outputs approval-summary step will run at the
    /// end of the Agent job. True whenever the workflow enables any
    /// safe-output tool. When true the Agent-job install/download must
    /// fire so that `approval-summary.js` is present for the
    /// end-of-job render step (emitted by `build_agent_job`).
    pub safe_outputs_summary_active: bool,
    /// Whether GitHub App-backed Copilot auth is configured
    /// (`engine.github-app-token`, issue #1316). When true the Agent-job
    /// install/download must fire so that `github-app-token.js` is present for
    /// the mint (and revoke) steps that `build_agent_job` emits immediately
    /// around the Copilot run. Mirrors `safe_outputs_summary_active`: the
    /// consuming steps are emitted by `build_agent_job`, not this extension, so
    /// the flag drives the shared bundle download — the builder never has to
    /// inspect emitted steps to decide whether to download.
    pub github_app_token_active: bool,
    /// Whether `create-pull-request` is configured (issue #1413). When true the
    /// Agent-job install/download must fire so that `prepare-pr-base.js` is
    /// present for the base-ref prepare step that `build_agent_job` emits before
    /// the Copilot run. Mirrors `github_app_token_active`: the consuming step is
    /// emitted by `build_agent_job`, not this extension, so the flag drives the
    /// shared bundle download.
    pub prepare_pr_base_active: bool,
    /// PR trigger config required to build `PR_SYNTH_SPEC`. `Some(_)`
    /// is the single source of truth for "synthetic-from-ci path is
    /// active for this agent" — `is_some()` replaces what used to be a
    /// separate `synthetic_pr_active: bool` field, eliminating the
    /// invariant that the two had to be set together. Drives:
    ///
    ///  - Setup-job install/download fire (even with no `filters:`).
    ///  - Setup-job `synthPr` step emission (before any gate step).
    ///  - Downstream env coalescing (handled in `compile-coalesce-env`).
    ///
    /// Cloned from the front-matter because the extension outlives the
    /// borrow of `FrontMatter` in `collect_extensions`.
    pub pr_trigger_for_synth: Option<crate::compile::types::PrTriggerConfig>,
    /// Internal supply-chain configuration. When the `feed` mirror is
    /// configured, the `ado-script.zip` bundle is pulled from the internal
    /// Azure DevOps Artifacts feed instead of GitHub Releases.
    pub supply_chain: Option<crate::compile::types::SupplyChainConfig>,
}

impl AdoScriptExtension {
    /// Whether the synthetic-from-ci path is active for this agent.
    /// Set when `on.pr.mode == Synthetic` (the default), in which case
    /// `pr_trigger_for_synth` is populated. The compile-time
    /// invariant "if active, the spec must be available" is encoded in
    /// the field type, so this is just a thin accessor.
    pub fn synthetic_pr_active(&self) -> bool {
        self.pr_trigger_for_synth.is_some()
    }
}

impl AdoScriptExtension {
    /// Compute the lowered PR and pipeline checks once. Returns
    /// `(pr_checks, pipeline_checks)`; either may be empty, in which
    /// case the corresponding gate step is not emitted.
    ///
    /// Lowering is cheap but the gate-emitting helpers used to invoke
    /// `lower_*_filters()` twice (once to test emptiness, once to
    /// materialize). This helper folds both passes into a single
    /// computation that callers reuse.
    fn lowered_checks(
        &self,
    ) -> (
        Vec<crate::compile::filter_ir::FilterCheck>,
        Vec<crate::compile::filter_ir::FilterCheck>,
    ) {
        let pr_checks = self
            .pr_filters
            .as_ref()
            .map(lower_pr_filters)
            .unwrap_or_default();
        let pipeline_checks = self
            .pipeline_filters
            .as_ref()
            .map(lower_pipeline_filters)
            .unwrap_or_default();
        (pr_checks, pipeline_checks)
    }

    fn runtime_imports_active(&self) -> bool {
        !self.inlined_imports
    }

    /// Build the typed Agent-job condition clauses contributed by
    /// `ado-script`. Returned by [`CompilerExtension::declarations`]
    /// in [`Declarations::agent_conditions`] so the canonical-jobs
    /// builder can fold them into the Agent job's
    /// `condition:` without hard-coding knowledge of this extension's
    /// step IDs (`synthPr`, `prGate`, `pipelineGate`) or its
    /// signals.
    ///
    /// Semantics (preserved from the previous
    /// `agentic_pipeline::build_agentic_condition`):
    ///
    /// - When [`Self::synthetic_pr_active`], honour the Setup-job
    ///   `synthPr.AW_SYNTHETIC_PR_SKIP=true` self-skip signal.
    /// - When PR filters lower to a non-empty check set, REQUIRE the
    ///   `prGate.SHOULD_RUN=true` output for any build that is a real
    ///   PR OR a synth-promoted build; otherwise (non-PR, non-synth)
    ///   bypass the gate.
    /// - When pipeline filters lower to a non-empty check set,
    ///   REQUIRE the `pipelineGate.SHOULD_RUN=true` output for
    ///   `ResourceTrigger` builds; otherwise bypass.
    /// - User filter `expression:` escape hatches are emitted as
    ///   `Condition::Custom` atoms (the injection-vector check runs
    ///   at codegen time inside the `Custom` arm).
    ///
    /// The leading `succeeded()` clause is NOT included here — it is
    /// prepended by [`crate::compile::agentic_pipeline`] when it
    /// folds any non-empty contribution set into a single
    /// `Condition::And`. This keeps emission order identical to the
    /// pre-lift output (`succeeded()` first, then this extension's
    /// clauses in declaration order).
    fn build_agent_conditions(&self) -> Result<Vec<Condition>> {
        use crate::compile::ir::output::OutputRef;

        let (pr_checks, pipeline_checks) = self.lowered_checks();
        let has_pr_filters = !pr_checks.is_empty();
        let has_pipeline_filters = !pipeline_checks.is_empty();
        let synthetic_pr_active = self.synthetic_pr_active();
        let pr_expression = self
            .pr_filters
            .as_ref()
            .and_then(|f| f.expression.as_deref());
        let pipeline_expression = self
            .pipeline_filters
            .as_ref()
            .and_then(|f| f.expression.as_deref());

        let mut parts: Vec<Condition> = Vec::new();

        // Typed step-output refs. The producer step IDs (`synthPr`,
        // `prGate`, `pipelineGate`) and their declared outputs are
        // graph-validated at `build_graph` time, so a future rename
        // becomes a compile error instead of a silently broken runtime
        // condition. The `?` propagates an invalid-StepId error as a
        // compile-time bug (the strings are static).
        if synthetic_pr_active {
            let synth = StepId::new("synthPr")?;
            // ne(dependencies.Setup.outputs['synthPr.AW_SYNTHETIC_PR_SKIP'], 'true')
            parts.push(Condition::Ne(
                Expr::StepOutput(OutputRef::new(synth, "AW_SYNTHETIC_PR_SKIP")),
                Expr::Literal("true".into()),
            ));
        }

        if has_pr_filters {
            let pr_gate = StepId::new("prGate")?;
            let gate_passed = Condition::Eq(
                Expr::StepOutput(OutputRef::new(pr_gate, "SHOULD_RUN")),
                Expr::Literal("true".into()),
            );
            if synthetic_pr_active {
                let synth = StepId::new("synthPr")?;
                // or(
                //   and(
                //     ne(variables['Build.Reason'], 'PullRequest'),
                //     ne(dependencies.Setup.outputs['synthPr.AW_SYNTHETIC_PR'], 'true')
                //   ),
                //   eq(dependencies.Setup.outputs['prGate.SHOULD_RUN'], 'true')
                // )
                parts.push(Condition::Or(vec![
                    Condition::And(vec![
                        Condition::Ne(
                            Expr::Variable("Build.Reason".into()),
                            Expr::Literal("PullRequest".into()),
                        ),
                        Condition::Ne(
                            Expr::StepOutput(OutputRef::new(synth, "AW_SYNTHETIC_PR")),
                            Expr::Literal("true".into()),
                        ),
                    ]),
                    gate_passed,
                ]));
            } else {
                parts.push(Condition::Or(vec![
                    Condition::Ne(
                        Expr::Variable("Build.Reason".into()),
                        Expr::Literal("PullRequest".into()),
                    ),
                    gate_passed,
                ]));
            }
        }

        if has_pipeline_filters {
            let pipeline_gate = StepId::new("pipelineGate")?;
            parts.push(Condition::Or(vec![
                Condition::Ne(
                    Expr::Variable("Build.Reason".into()),
                    Expr::Literal("ResourceTrigger".into()),
                ),
                Condition::Eq(
                    Expr::StepOutput(OutputRef::new(pipeline_gate, "SHOULD_RUN")),
                    Expr::Literal("true".into()),
                ),
            ]));
        }

        if let Some(e) = pr_expression {
            parts.push(Condition::Custom(e.to_string()));
        }
        if let Some(e) = pipeline_expression {
            parts.push(Condition::Custom(e.to_string()));
        }

        Ok(parts)
    }
}

/// Returns the two-step bundle as typed `Step`s: a
/// `Step::Task(UseNode@1)` plus a `Step::Bash` for the curl, sha256,
/// and unzip pipeline. When an internal feed is configured the bundle is
/// pulled from the Azure DevOps Artifacts feed (NuGet) instead of GitHub
/// Releases; the `.nupkg` is unzipped and `ado-script.zip` relocated, then
/// verified and unpacked exactly as in the GitHub path.
///
/// Bundles are unpacked into `/tmp/ado-aw-scripts/` so the consumer
/// references `/tmp/ado-aw-scripts/ado-script/<bundle>.js`. Shared by the
/// Agent/Setup jobs (via the extension's declarations) and the Conclusion
/// job (via [`crate::compile::agentic_pipeline`]) so the supply-chain
/// mirror and unzip layout stay consistent across every consumer.
pub(crate) fn install_and_download_steps_typed(
    supply_chain: Option<&SupplyChainConfig>,
) -> Vec<Step> {
    let version = env!("CARGO_PKG_VERSION");
    let install = {
        let mut t = UseNode::new("22.x").into_step();
        t.timeout = Some(std::time::Duration::from_secs(300));
        t.condition = Some(Condition::Succeeded);
        t
    };

    if let Some(feed) = supply_chain.and_then(|sc| sc.feed.as_ref()) {
        let connection = supply_chain.and_then(|sc| sc.feed_connection());
        let mut auth = nuget_authenticate_step(connection);
        auth.condition = Some(Condition::Succeeded);
        let download_pkg = {
            let mut t = download_package_step(
                format!("Download ado-aw scripts (v{version})"),
                feed.name.as_str(),
                "ado-script",
                version,
                "/tmp/ado-aw-scripts/_pkg",
            );
            t.timeout = Some(std::time::Duration::from_secs(300));
            t.condition = Some(Condition::Succeeded);
            t
        };
        // Locate ado-script.zip + checksums.txt within the package staging
        // dir (handling both extracted-tree and raw-.nupkg delivery),
        // verify, then unzip the bundle into /tmp/ado-aw-scripts/.
        let script = "\
             set -eo pipefail\n\
             mkdir -p /tmp/ado-aw-scripts\n\
             STAGING=/tmp/ado-aw-scripts/_pkg\n\
             if [ -z \"$(find \"$STAGING\" -name 'ado-script.zip' -print -quit)\" ]; then\n  \
               NUPKG=\"$(find \"$STAGING\" -name '*.nupkg' -print -quit)\"\n  \
               if [ -n \"$NUPKG\" ]; then\n    \
                 unzip -o \"$NUPKG\" -d \"$STAGING\" >/dev/null\n  \
               fi\n\
             fi\n\
             ZIP=\"$(find \"$STAGING\" -name 'ado-script.zip' -print -quit)\"\n\
             CHK=\"$(find \"$STAGING\" -name 'checksums.txt' -print -quit)\"\n\
             if [ -z \"$ZIP\" ] || [ -z \"$CHK\" ]; then\n  \
               echo \"##vso[task.complete result=Failed]ado-script.zip or checksums.txt not found in package\"\n  \
               exit 1\n\
             fi\n\
             cp \"$ZIP\" /tmp/ado-aw-scripts/ado-script.zip\n\
             cp \"$CHK\" /tmp/ado-aw-scripts/checksums.txt\n\
             cd /tmp/ado-aw-scripts && grep \"ado-script.zip\" checksums.txt | sha256sum -c -\n\
             unzip -o /tmp/ado-aw-scripts/ado-script.zip -d /tmp/ado-aw-scripts/\n"
            .to_string();
        let mut b = BashStep::new(format!("Stage ado-aw scripts (v{version})"), script)
            .with_condition(Condition::Succeeded);
        b.timeout = Some(std::time::Duration::from_secs(300));
        return vec![
            Step::Task(install),
            Step::Task(auth),
            Step::Task(download_pkg),
            Step::Bash(b),
        ];
    }

    let download = {
        let script = format!(
            "set -eo pipefail\n\
             mkdir -p /tmp/ado-aw-scripts\n\
             curl -fsSL \"{RELEASE_BASE_URL}/v{version}/checksums.txt\" -o /tmp/ado-aw-scripts/checksums.txt\n\
             curl -fsSL \"{RELEASE_BASE_URL}/v{version}/ado-script.zip\" -o /tmp/ado-aw-scripts/ado-script.zip\n\
             cd /tmp/ado-aw-scripts && grep \"ado-script.zip\" checksums.txt | sha256sum -c -\n\
             unzip -o /tmp/ado-aw-scripts/ado-script.zip -d /tmp/ado-aw-scripts/\n"
        );
        let mut b = BashStep::new(format!("Download ado-aw scripts (v{version})"), script)
            .with_condition(Condition::Succeeded);
        b.timeout = Some(std::time::Duration::from_secs(300));
        b
    };
    vec![Step::Task(install), Step::Bash(download)]
}

/// Path-anchor ADO variables exposed to the agent prompt via the runtime
/// import resolver. The compiler owns this allowlist; `import.js`
/// substitutes only the `$(name)` tokens it is handed — it never reads
/// these from the environment (see
/// `scripts/ado-script/src/import/index.ts`). Each entry is both the
/// substitution key and the ADO macro name, so the resolver emits
/// `--var "<name>=$(<name>)"` and ADO expands the macro at runtime before
/// node runs. Keep this list minimal and non-secret — anything added here
/// is interpolated verbatim into the (potentially untrusted) agent prompt.
///
/// NOTE: each var is emitted as `--var "<name>=$(<name>)"`. If ADO ever
/// expanded a value containing a literal `"` (e.g. a user-influenced
/// variable added to this list), it would break bash argument parsing in
/// the resolver step — the same pre-existing exposure as the adjacent
/// `--base "$(Build.SourcesDirectory)"`. The current entries are
/// ADO-controlled path anchors that cannot contain `"`, so this is safe;
/// re-quote (or shell-escape) before adding any user-influenced variable.
const PROMPT_ADO_VARS: &[&str] = &["Build.SourcesDirectory", "Build.Repository.Name"];

/// The resolver step that expands runtime import markers in the agent prompt.
fn resolver_step_typed() -> Step {
    // `--var "<name>=$(<name>)"` for each allowlisted path-anchor var. ADO
    // substitutes the `$(...)` macro into the bash arg at runtime, so
    // import.js receives the concrete value and can replace `$(<name>)`
    // occurrences in the prompt — giving the same result whether imports
    // are inlined at compile time or resolved here at runtime.
    let var_flags: String = PROMPT_ADO_VARS
        .iter()
        .map(|name| format!(" --var \"{name}=$({name})\""))
        .collect();
    let script = format!(
        "set -eo pipefail\n\
         node '{IMPORT_EVAL_PATH}' /tmp/awf-tools/agent-prompt.md --base \"$(Build.SourcesDirectory)\"{var_flags}\n"
    );
    Step::Bash(
        BashStep::new("Resolve runtime imports (agent prompt)", script)
            .with_condition(Condition::Succeeded),
    )
}

/// The GitHub App token-mint step (issue #1316). Runs immediately before the
/// Copilot invocation in the Agent and Detection jobs to mint a GitHub App
/// installation access token via the `github-app-token` ado-script bundle and
/// expose it as a masked, same-job `GITHUB_APP_TOKEN` variable, which
/// `copilot_env` sources `GITHUB_TOKEN` from.
///
/// ## Why non-secret inputs are argv flags, not env vars
///
/// ADO injects **every pipeline variable** into a step's process env, so an
/// env-sourced knob can be silently shadowed by a same-named pipeline variable
/// — in the worst case redirecting the minted token to an attacker-chosen
/// variable name. Argv comes only from this compiler-authored script, so it
/// cannot be shadowed. All non-secret inputs (`--app-id`, `--owner`,
/// `--output-var`, `--repositories`, `--api-url`) are therefore passed as
/// single-quoted argv flags. Values are single-quoted (with `'\''` escaping) so
/// a value containing shell metacharacters cannot break out or trigger command
/// substitution.
///
/// The **private key** stays in the masked env (`GH_APP_PRIVATE_KEY:
/// $(secret)`): a secret must never appear on a command line (it would show in
/// process listings and the rendered step script), and ADO only maps a secret
/// variable into env on explicit reference. Its variable name is the
/// `private-key` override or the compiler-owned default `GITHUB_APP_PRIVATE_KEY`.
///
/// The step runs OUTSIDE the AWF sandbox and reaches `api.github.com` (or
/// `--api-url`) over the build agent pool's normal network — no AWF allowlist
/// entry is required.
pub fn github_app_token_step_typed(
    cfg: &crate::compile::types::GithubAppTokenConfig,
) -> Result<Step> {
    cfg.validate()?;
    // The App ID is a non-secret literal (numeric App ID or alphanumeric client
    // ID); single-quote it so any character is passed through as one argv token.
    let mut args: Vec<String> = vec![
        format!("--app-id {}", sh_single_quote(&cfg.app_id)),
        format!("--owner {}", sh_single_quote(&cfg.owner)),
        // Pin the output variable name (a compiler constant) as argv so no
        // pipeline variable can redirect the minted token.
        format!(
            "--output-var {}",
            sh_single_quote(crate::engine::GITHUB_APP_TOKEN_VAR)
        ),
    ];
    if !cfg.repositories.is_empty() {
        args.push(format!(
            "--repositories {}",
            sh_single_quote(&cfg.repositories.join(" "))
        ));
    }
    if let Some(api_url) = &cfg.api_url {
        args.push(format!("--api-url {}", sh_single_quote(api_url)));
    }
    let script = format!(
        "set -eo pipefail\nnode '{GITHUB_APP_TOKEN_PATH}' {}\n",
        args.join(" ")
    );
    let step = BashStep::new("Mint GitHub App token (Copilot engine auth)", script)
        .with_condition(Condition::Succeeded)
        // Only the secret rides in env — masked, never on the command line. Its
        // variable name is the `private-key` override or the default
        // `GITHUB_APP_PRIVATE_KEY` (see `GithubAppTokenConfig::private_key_var`).
        .with_env(
            "GH_APP_PRIVATE_KEY",
            EnvValue::secret(cfg.private_key_var().to_string()),
        );
    Ok(Step::Bash(step))
}

/// Single-quote a value for safe embedding in a bash command line, escaping any
/// embedded single quotes as `'\''`. Single quotes suppress all shell
/// expansion (`$(...)`, backticks, `${...}`, word-splitting), so a value with
/// metacharacters is passed through verbatim as one argv token.
fn sh_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// The create-pull-request base-ref prepare step (issue #1413). Runs in the
/// Agent job before the Copilot invocation when `create-pull-request` is
/// configured, invoking the `prepare-pr-base` ado-script bundle to fetch and
/// progressively deepen the target branch (`refs/remotes/origin/<target>`) in
/// every allowed create-PR repo dir. The host-side SafeOutputs MCP server
/// (`src/mcp.rs`) then resolves a diff base even on shallow-default agent pools —
/// no forced full-history `checkout: self`, no lock hand-edit.
///
/// `repos` is the set of `(dir, target_branch)` pairs the MCP server may
/// generate a patch from — one per allowed repo, in the SAME dir form
/// `resolve_git_dir_for_patch` resolves them: the resolved `working_directory`
/// (for `self`) and `working_directory/<alias>` for each `checkout:` alias, each
/// paired with THAT repo's resolved create-pull-request target branch (which may
/// differ per repo in a multi-checkout setup — see
/// `CreatePrConfig::resolve_target_branch`). Emitting them as repeated
/// `--repo-dir <dir> --target-branch <branch>` pairs lets the bundle deepen each
/// dir's own target, so a PR to ANY allowed repo works on shallow pools.
///
/// Each `--target-branch` is a single-quoted argv flag (a plain literal branch,
/// shadow-proof); each `--repo-dir` is a double-quoted ADO-macro path (matching
/// the `import.js --base "$(...)"` convention — single quotes would trip
/// shellcheck SC2016). The ADO bearer is projected via `apply_bundle_auth` (the
/// bundle uses it for the authenticated git fetch, and `SYSTEM_ACCESSTOKEN` is
/// the one predefined var ADO does not auto-inject). The step runs OUTSIDE the
/// AWF sandbox on the build agent's normal network, so it needs no AWF allowlist
/// entry.
pub fn prepare_pr_base_step_typed(repos: &[(String, String)]) -> Step {
    // Each pair emits `--repo-dir "<dir>" --target-branch '<branch>'`. Repo dirs
    // are compiler-generated ADO path macros (`$(Build.SourcesDirectory)` [+
    // `/<validated-alias>`]) — ADO substitutes the macro before bash runs and
    // aliases are validated, so they use DOUBLE quotes (single quotes would trip
    // shellcheck SC2016). The target branch is a plain literal, single-quoted
    // (shadow-proof).
    let repo_flags: String = repos
        .iter()
        .map(|(dir, target)| {
            format!(" --repo-dir \"{}\" --target-branch {}", dir, sh_single_quote(target))
        })
        .collect();
    let script = format!(
        "set -eo pipefail\nnode '{PREPARE_PR_BASE_PATH}'{}\n",
        repo_flags
    );
    let step = crate::compile::ado_bundle::apply_bundle_auth(
        BashStep::new(
            "Prepare create-pull-request base ref (fetch/deepen)",
            script,
        )
        .with_condition(Condition::Succeeded),
        crate::compile::ado_bundle::Bundle::PreparePrBase,
        crate::compile::ado_bundle::TokenSource::SystemAccessToken,
    );
    Step::Bash(step)
}

/// The GitHub App token **revocation** step (issue #1316). Runs after the
/// Copilot invocation in the Agent and Detection jobs (unless
/// `skip-token-revocation` is set) to delete the minted installation token
/// (`DELETE /installation/token`) so it does not remain valid for its full
/// ~1h lifetime — matching `actions/create-github-app-token`'s default.
///
/// Best-effort: it runs with `condition: always()` (so it fires even when the
/// Copilot run failed) and `continueOnError` (revocation failure never fails
/// the build). The minted token is read from the masked same-job
/// `GITHUB_APP_TOKEN` variable (via the `GH_APP_TOKEN` secret env); the API base
/// URL, if any, is an argv flag (see `github_app_token_step_typed` for why
/// non-secret inputs are argv, not env).
pub fn github_app_token_revoke_step_typed(
    cfg: &crate::compile::types::GithubAppTokenConfig,
) -> Result<Step> {
    // Validate for symmetry with the mint step, so neither function silently
    // assumes the other ran first (e.g. if a future caller emits revoke alone).
    cfg.validate()?;
    // No `set -eo pipefail` here (unlike the mint step) is intentional: this is
    // a best-effort cleanup. The bundle's `revoke` mode always exits 0 (it
    // downgrades every failure to a warning), and the step is `continueOnError`,
    // so aborting the shell early on a non-zero would neither help nor change
    // the outcome — it would only risk turning a benign revoke hiccup into a
    // timeline error.
    let api_url_arg = match &cfg.api_url {
        Some(api_url) => format!(" --api-url {}", sh_single_quote(api_url)),
        None => String::new(),
    };
    let script = format!("node '{GITHUB_APP_TOKEN_PATH}' revoke{api_url_arg}\n");
    let step = BashStep::new("Revoke GitHub App token", script)
        .with_condition(Condition::Always)
        .with_continue_on_error(true)
        .with_env(
            "GH_APP_TOKEN",
            EnvValue::secret(crate::engine::GITHUB_APP_TOKEN_VAR),
        );
    Ok(Step::Bash(step))
}

/// The synthetic-PR-context step that runs in the Setup job before
/// `prGate`. Declares the PR outputs so downstream consumers can
/// reference them via [`crate::compile::ir::output::OutputRef`].
/// The graph's auto-`isOutput=true` promotion kicks in for any
/// output that picks up a cross-step reader.
///
/// The `id` is the canonical step name `synthPr` — same as the
/// legacy emitter, and the value every consumer must use in its
/// `OutputRef`.
pub fn synthetic_pr_step_typed(spec_b64: &str) -> Result<BashStep> {
    let script = format!(
        "set -euo pipefail\n\
         node '{EXEC_CONTEXT_PR_SYNTH_PATH}'\n"
    );
    let condition = Condition::And(vec![
        Condition::Succeeded,
        Condition::Ne(
            Expr::Variable("Build.Reason".to_string()),
            Expr::Literal("PullRequest".to_string()),
        ),
    ]);
    let mut step = BashStep::new("Resolve synthetic PR context", script)
        .with_id(StepId::new("synthPr")?)
        .with_condition(condition);
    for name in SYNTH_PR_OUTPUT_NAMES {
        step = step.with_output(OutputDecl::new(*name));
    }
    // ADO auto-injects the predefined context vars this bundle reads
    // (SYSTEM_TEAMPROJECT, BUILD_REPOSITORY_ID, BUILD_REASON,
    // BUILD_REPOSITORY_PROVIDER, BUILD_SOURCEBRANCH, SYSTEM_PULLREQUEST_*),
    // so only the non-auto-injected SYSTEM_ACCESSTOKEN bearer and the
    // compiler-computed PR_SYNTH_SPEC are projected here.
    let step = crate::compile::ado_bundle::apply_bundle_auth(
        step,
        crate::compile::ado_bundle::Bundle::ExecContextPrSynth,
        crate::compile::ado_bundle::TokenSource::SystemAccessToken,
    )
    .with_env("PR_SYNTH_SPEC", EnvValue::literal(spec_b64));
    Ok(step)
}

/// Outputs declared by the `synthPr` step. Consumers in the same
/// job (e.g. `prGate`) reference these via `OutputRef::new(StepId::new("synthPr")?, NAME)`;
/// cross-job consumers (e.g. the Agent-job `exec-context-pr`
/// contributor) use the same OutputRef and the lowering pass
/// resolves the correct ADO reference syntax based on consumer
/// location.
///
/// The list reflects every `setOutput` the runtime
/// `exec-context-pr-synth.js` bundle emits (see that file's "Variables
/// emitted" docblock).
pub const SYNTH_PR_OUTPUT_NAMES: &[&str] = &[
    // Unified `AW_PR_*` namespace introduced in PR #972 — the
    // runtime bundle emits these via both `setOutput` (cross-job
    // OutputRef consumers) and `setVar` (same-job `$(name)` macro
    // consumers). The Agent-job-level `variables:` hoist consumes
    // these via cross-job OutputRef.
    "AW_PR_ID",
    "AW_PR_TARGETBRANCH",
    "AW_PR_SOURCEBRANCH",
    "AW_PR_IS_DRAFT",
    // Always-emitted control flags.
    "AW_SYNTHETIC_PR",
    "AW_SYNTHETIC_PR_SKIP",
];

/// Subset of [`SYNTH_PR_OUTPUT_NAMES`] hoisted into the Agent-job
/// `variables:` block by
/// [`crate::compile::agentic_pipeline::agent_job_variables_hoist`].
///
/// Every name listed here MUST also be in [`SYNTH_PR_OUTPUT_NAMES`]
/// (enforced by `synth_pr_hoist_subset_of_outputs` unit test) so
/// graph validation will not reject the cross-job `OutputRef` the
/// hoist emits.
///
/// `AW_SYNTHETIC_PR_SKIP` is intentionally excluded: it is consumed
/// only by the Agent-job `condition:` (a typed `Condition::Ne` over
/// the cross-job `OutputRef`), not by step `env:` — hoisting it
/// would add a pipeline variable no consumer ever reads.
pub const SYNTH_PR_AGENT_HOIST_NAMES: &[&str] = &[
    "AW_PR_ID",
    "AW_PR_TARGETBRANCH",
    "AW_PR_SOURCEBRANCH",
    "AW_PR_IS_DRAFT",
    "AW_SYNTHETIC_PR",
];

impl CompilerExtension for AdoScriptExtension {
    fn name(&self) -> &str {
        "ado-script"
    }

    fn phase(&self) -> ExtensionPhase {
        // System phase: ado-script's UseNode@1 install + bundle download +
        // resolver step must complete BEFORE any user-facing Runtime
        // extension (e.g. NodeExtension) runs. Otherwise our Node 22
        // install would prepend onto PATH after the user's pinned Node,
        // silently overriding the user's choice for the rest of the
        // Agent job. By running first, our install lives only during the
        // brief window before the user's Runtime install, and the
        // resolver step inside that window picks up our Node 22.
        ExtensionPhase::System
    }

    /// Typed-IR view. The marquee port: every step ado-script
    /// contributes is rebuilt as a typed `Step`, with explicit
    /// [`StepId`] / [`OutputDecl`] on the `synthPr` producer and
    /// typed [`crate::compile::ir::env::EnvValue::StepOutput`]
    /// references on the gate consumer. This is the commit that
    /// locks declarative synth-PR propagation — the lowering pass
    /// (not the extension) now decides whether each consumer sees
    /// the same-job macro form `$(synthPr.X)` or the cross-job
    /// `dependencies.Setup.outputs['synthPr.X']` form.
    ///
    /// Setup-job steps land in [`Declarations::setup_steps`]; Agent-
    /// job steps in [`Declarations::agent_prepare_steps`].
    fn declarations(&self, _ctx: &CompileContext) -> Result<Declarations> {
        let mut warnings = Vec::new();
        if let Some(f) = &self.pr_filters {
            for diag in validate_pr_filters(f) {
                match diag.severity {
                    Severity::Error => anyhow::bail!("{}", diag),
                    Severity::Warning | Severity::Info => {
                        warnings.push(format!("{}", diag));
                    }
                }
            }
        }
        if let Some(f) = &self.pipeline_filters {
            for diag in validate_pipeline_filters(f) {
                match diag.severity {
                    Severity::Error => anyhow::bail!("{}", diag),
                    Severity::Warning | Severity::Info => {
                        warnings.push(format!("{}", diag));
                    }
                }
            }
        }

        let (pr_checks, pipeline_checks) = self.lowered_checks();

        // ─── Setup job ─────────────────────────────────────────
        let mut setup_steps: Vec<Step> = Vec::new();
        if !pr_checks.is_empty() || !pipeline_checks.is_empty() || self.synthetic_pr_active() {
            setup_steps.extend(install_and_download_steps_typed(self.supply_chain.as_ref()));
            if let Some(pr) = self.pr_trigger_for_synth.as_ref() {
                let spec_b64 = crate::compile::filter_ir::build_pr_synth_spec(pr)?;
                setup_steps.push(Step::Bash(synthetic_pr_step_typed(&spec_b64)?));
            }
            if !pr_checks.is_empty() {
                setup_steps.push(Step::Bash(build_gate_step_typed(
                    GateContext::PullRequest,
                    &pr_checks,
                    GATE_EVAL_PATH,
                    self.synthetic_pr_active(),
                )?));
            }
            if !pipeline_checks.is_empty() {
                setup_steps.push(Step::Bash(build_gate_step_typed(
                    GateContext::PipelineCompletion,
                    &pipeline_checks,
                    GATE_EVAL_PATH,
                    // Pipeline-completion gates never observe synthetic
                    // PR semantics; macro-concat applies to PR gates only.
                    false,
                )?));
            }
        }

        // ─── Agent job ─────────────────────────────────────────
        let mut agent_prepare_steps: Vec<Step> = Vec::new();
        let import_active = self.runtime_imports_active();
        if import_active
            || self.exec_context_pr_active
            || self.exec_context_manual_active
            || self.exec_context_pipeline_active
            || self.exec_context_ci_push_active
            || self.exec_context_workitem_active
            || self.exec_context_schedule_active
            || self.exec_context_pr_checks_active
            || self.exec_context_repo_active
            || self.safe_outputs_summary_active
            || self.github_app_token_active
            || self.prepare_pr_base_active
        {
            agent_prepare_steps.extend(install_and_download_steps_typed(self.supply_chain.as_ref()));
            if import_active {
                agent_prepare_steps.push(resolver_step_typed());
            }
        }

        // ─── Agent-job condition contribution ──────────────────
        // Synth-PR / PR-filter / pipeline-filter / user-expression
        // clauses that gate the Agent job. The canonical-jobs builder
        // folds these (plus the contributions of any other extension
        // that gates the Agent job) into a single `Condition::And`
        // with a leading `succeeded()`.
        let agent_conditions = self.build_agent_conditions()?;

        Ok(Declarations {
            setup_steps,
            agent_prepare_steps,
            warnings,
            agent_conditions,
            ..Declarations::default()
        })
    }
}

/// Resolve `{{#runtime-import path}}` markers in `body` at compile time.
///
/// Used by `compile_shared` when `inlined-imports: true` so author-written
/// markers inside the agent's markdown body still work in inlined mode.
///
/// Path resolution: only **relative** paths are accepted. They are
/// resolved against `base_dir` (the source `.md` file's directory).
/// Absolute paths and `..` segments are rejected because compile-time
/// resolution against an untrusted branch (e.g. `ado-aw compile` on a
/// hostile PR) would otherwise embed arbitrary host files
/// (`{{#runtime-import /home/runner/.ssh/id_rsa}}`,
/// `{{#runtime-import ../../../../etc/passwd}}`) verbatim into the
/// compiled YAML.
///
/// Required markers fail with an error; optional `?`-form markers
/// silently drop if the referenced file is missing.
pub fn resolve_imports_inline(body: &str, base_dir: &std::path::Path) -> Result<String> {
    const MARKER_PREFIX: &str = "{{#runtime-import";

    let mut result = String::with_capacity(body.len());
    let mut cursor = 0usize;

    while let Some(rel_start) = body[cursor..].find(MARKER_PREFIX) {
        let start = cursor + rel_start;
        result.push_str(&body[cursor..start]);

        let marker_body_start = start + MARKER_PREFIX.len();
        let rel_end = body[marker_body_start..].find("}}").ok_or_else(|| {
            anyhow::anyhow!(
                "runtime-import: unterminated marker starting at byte {}",
                start
            )
        })?;
        let marker_end = marker_body_start + rel_end;
        let marker_body = body[marker_body_start..marker_end].trim();

        let (optional, path_str) = if let Some(rest) = marker_body.strip_prefix('?') {
            (true, rest.trim())
        } else {
            (false, marker_body)
        };

        anyhow::ensure!(
            !path_str.is_empty(),
            "runtime-import: missing path in marker '{}'",
            &body[start..marker_end + 2]
        );
        anyhow::ensure!(
            !path_str.chars().any(char::is_whitespace),
            "runtime-import: invalid path '{}': whitespace is not allowed",
            path_str
        );
        // Reject `}` in paths so the compile-time resolver stays in
        // strict parity with the runtime regex
        // (`scripts/ado-script/src/import/index.ts` — `[^\s}]+`). The
        // runtime regex stops the path capture at any `}`; the
        // compile-time resolver, by contrast, terminates only at the
        // closing `}}` and would otherwise happily accept a path like
        // `foo}bar.md`. Allowing `}` here would silently produce
        // different behaviour on the two paths (compile-time: file
        // looked up as `foo}bar.md`; runtime: marker survives
        // unexpanded). Reject up front so the failure mode is one
        // clear compile-time error in both modes.
        anyhow::ensure!(
            !path_str.contains('}'),
            "runtime-import: invalid path '{}': '}}' is not allowed (incompatible with the runtime resolver's path regex)",
            path_str
        );
        // Reject any path whose segments contain `..`. A malicious agent
        // body could otherwise reach files outside `base_dir` and embed
        // them verbatim into the compiled YAML — e.g.
        // `{{#runtime-import ../../../../etc/passwd}}` if `ado-aw compile`
        // is run on an untrusted PR branch. This guard applies to both
        // relative and absolute paths because `..` segments make any
        // path-confinement check unsound.
        anyhow::ensure!(
            !path_str
                .split(['/', '\\'])
                .any(|component| component == ".."),
            "runtime-import: invalid path '{}': '..' path components are not allowed",
            path_str
        );
        // Reject absolute paths at compile time. An untrusted PR branch
        // could otherwise embed arbitrary host files into the compiled
        // YAML (e.g. `{{#runtime-import /home/runner/.ssh/id_rsa}}`,
        // `{{#runtime-import C:\Users\…\secrets.txt}}`). Only relative
        // imports rooted in `base_dir` (the source `.md` file's
        // directory, which is part of the same repo) are safe.
        //
        // `Path::is_absolute` is platform-dependent: on Linux it
        // doesn't recognize `C:\foo` as absolute, and on Windows it
        // doesn't recognize a POSIX-style `/foo` UNC path. To make the
        // guard equally strict on every host where `ado-aw compile`
        // runs, also explicitly detect:
        //   - POSIX absolute (`/foo`)
        //   - Windows drive-letter absolute (`C:\foo`, `C:/foo`, any letter)
        //   - UNC (`\\server\share`)
        let is_drive_letter_absolute = {
            let mut chars = path_str.chars();
            matches!(
                (chars.next(), chars.next(), chars.next()),
                (Some(c), Some(':'), Some(sep))
                    if c.is_ascii_alphabetic() && (sep == '/' || sep == '\\')
            )
        };
        let is_absolute = std::path::Path::new(path_str).is_absolute()
            || path_str.starts_with('/')
            || path_str.starts_with("\\\\")
            || is_drive_letter_absolute;
        anyhow::ensure!(
            !is_absolute,
            "runtime-import: invalid path '{}': absolute paths are not allowed (use a relative path rooted at the agent's directory)",
            path_str
        );

        let abs = base_dir.join(path_str);

        match std::fs::read_to_string(&abs) {
            Ok(contents) => result.push_str(&contents),
            Err(_) if optional => {}
            Err(e) => {
                return Err(anyhow::anyhow!(
                    "runtime-import: file not found: {} ({})",
                    path_str,
                    e
                ));
            }
        }

        cursor = marker_end + 2;
    }

    result.push_str(&body[cursor..]);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::extensions::CompileContext;
    use crate::compile::types::*;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // ── extension behaviour ────────────────────────────────────────────

    /// Every name in `SYNTH_PR_AGENT_HOIST_NAMES` must also be declared
    /// in `SYNTH_PR_OUTPUT_NAMES`, otherwise `agent_job_variables_hoist`
    /// would emit a cross-job `OutputRef` to an output the producer
    /// never declares — graph validation would reject the pipeline.
    #[test]
    fn synth_pr_hoist_subset_of_outputs() {
        for hoisted in SYNTH_PR_AGENT_HOIST_NAMES {
            assert!(
                SYNTH_PR_OUTPUT_NAMES.contains(hoisted),
                "{hoisted} is in SYNTH_PR_AGENT_HOIST_NAMES but not in SYNTH_PR_OUTPUT_NAMES"
            );
        }
    }

    fn ext_with(
        pr: Option<PrFilters>,
        pipeline: Option<PipelineFilters>,
        inlined: bool,
    ) -> AdoScriptExtension {
        AdoScriptExtension {
            pr_filters: pr,
            pipeline_filters: pipeline,
            inlined_imports: inlined,
            exec_context_pr_active: false,
            exec_context_manual_active: false,
            exec_context_pipeline_active: false,
            exec_context_ci_push_active: false,
            exec_context_workitem_active: false,
            exec_context_schedule_active: false,
            exec_context_pr_checks_active: false,
            exec_context_repo_active: false,
            safe_outputs_summary_active: false,
            github_app_token_active: false,
            prepare_pr_base_active: false,
            pr_trigger_for_synth: None,
            supply_chain: None,
        }
    }

    #[test]
    fn name_and_phase() {
        let ext = ext_with(None, None, true);
        assert_eq!(ext.name(), "ado-script");
        // System phase ensures UseNode@1 install + bundle download +
        // resolver run BEFORE user-facing Runtime extensions (e.g. the
        // Node runtime), so the user's pinned Node version wins on PATH
        // for the rest of the Agent job.
        assert_eq!(ext.phase(), ExtensionPhase::System);
    }

    #[test]
    fn declarations_setup_steps_emits_install_download_and_gate_when_gate_active() {
        let filters = PrFilters {
            labels: Some(LabelFilter {
                any_of: vec!["run-agent".into()],
                ..Default::default()
            }),
            ..Default::default()
        };
        let ext = ext_with(Some(filters), None, true);
        let fm: FrontMatter = serde_yaml::from_str("name: t\ndescription: t").unwrap();
        let ctx = CompileContext::for_test(&fm);
        let steps = ext.declarations(&ctx).unwrap().setup_steps;
        assert_eq!(steps.len(), 3, "install + download + gate");
        match &steps[0] {
            Step::Task(t) => {
                assert_eq!(t.task, "UseNode@1");
                assert_eq!(t.display_name, "Install Node.js 22.x");
                assert!(!t.display_name.contains("for gate evaluator"));
            }
            other => panic!("expected NodeTool task, got {other:?}"),
        }
        match &steps[1] {
            Step::Bash(b) => {
                assert!(b.display_name.contains("Download ado-aw scripts"));
                assert!(b.script.contains("sha256sum -c -"));
            }
            other => panic!("expected download bash step, got {other:?}"),
        }
        match &steps[2] {
            Step::Bash(b) => assert!(
                b.script
                    .contains("node '/tmp/ado-aw-scripts/ado-script/gate.js'")
            ),
            other => panic!("expected gate bash step, got {other:?}"),
        }
    }

    #[test]
    fn declarations_setup_steps_emits_synth_step_when_synthetic_pr_active_without_gate() {
        use crate::compile::types::{BranchFilter, PrTriggerConfig};
        let ext = AdoScriptExtension {
            pr_filters: None,
            pipeline_filters: None,
            inlined_imports: true,
            exec_context_pr_active: false,
            exec_context_manual_active: false,
            exec_context_pipeline_active: false,
            exec_context_ci_push_active: false,
            exec_context_workitem_active: false,
            exec_context_schedule_active: false,
            exec_context_pr_checks_active: false,
            exec_context_repo_active: false,
            safe_outputs_summary_active: false,
            github_app_token_active: false,
            prepare_pr_base_active: false,
            pr_trigger_for_synth: Some(PrTriggerConfig {
                branches: Some(BranchFilter {
                    include: vec!["main".into()],
                    exclude: vec![],
                }),
                paths: None,
                filters: None,
                ..Default::default()
            }),
            supply_chain: None,
        };
        let fm: FrontMatter = serde_yaml::from_str("name: t\ndescription: t").unwrap();
        let ctx = CompileContext::for_test(&fm);
        let steps = ext.declarations(&ctx).unwrap().setup_steps;
        assert_eq!(steps.len(), 3, "install + download + synthPr");
        assert!(matches!(&steps[0], Step::Task(t) if t.task == "UseNode@1"));
        assert!(
            matches!(&steps[1], Step::Bash(b) if b.display_name.contains("Download ado-aw scripts"))
        );
        let Step::Bash(synth) = &steps[2] else {
            panic!("expected synthPr bash step, got {:?}", steps[2]);
        };
        assert_eq!(synth.id.as_ref().map(|i| i.as_str()), Some("synthPr"));
        assert!(synth.script.contains("exec-context-pr-synth.js"));
        assert!(synth.env.contains_key("PR_SYNTH_SPEC"));
        // The typed synth path exposes the unified AW_PR outputs; it
        // does not pass the legacy SYSTEM_PULLREQUEST_* env vars
        // directly.
        assert!(
            !synth.env.contains_key("SYSTEM_PULLREQUEST_PULLREQUESTID"),
            "typed synthPr reads unified AW_PR values and no longer passes SYSTEM_PULLREQUEST_PULLREQUESTID directly"
        );
    }

    #[test]
    fn declarations_setup_steps_emits_synth_step_before_gate_when_both_active() {
        use crate::compile::types::{BranchFilter, PrTriggerConfig};
        let filters = PrFilters {
            labels: Some(LabelFilter {
                any_of: vec!["run-agent".into()],
                ..Default::default()
            }),
            ..Default::default()
        };
        let ext = AdoScriptExtension {
            pr_filters: Some(filters),
            pipeline_filters: None,
            inlined_imports: true,
            exec_context_pr_active: false,
            exec_context_manual_active: false,
            exec_context_pipeline_active: false,
            exec_context_ci_push_active: false,
            exec_context_workitem_active: false,
            exec_context_schedule_active: false,
            exec_context_pr_checks_active: false,
            exec_context_repo_active: false,
            safe_outputs_summary_active: false,
            github_app_token_active: false,
            prepare_pr_base_active: false,
            pr_trigger_for_synth: Some(PrTriggerConfig {
                branches: Some(BranchFilter {
                    include: vec!["main".into()],
                    exclude: vec![],
                }),
                paths: None,
                filters: None,
                ..Default::default()
            }),
            supply_chain: None,
        };
        let fm: FrontMatter = serde_yaml::from_str("name: t\ndescription: t").unwrap();
        let ctx = CompileContext::for_test(&fm);
        let steps = ext.declarations(&ctx).unwrap().setup_steps;
        assert_eq!(steps.len(), 4, "install + download + synthPr + prGate");
        assert!(
            matches!(&steps[2], Step::Bash(b) if b.id.as_ref().map(|i| i.as_str()) == Some("synthPr"))
        );
        assert!(
            matches!(&steps[3], Step::Bash(b) if b.id.as_ref().map(|i| i.as_str()) == Some("prGate"))
        );
    }

    #[test]
    fn gate_and_import_eval_paths_consistent_with_download_step() {
        let extract_dir = "/tmp/ado-aw-scripts/";
        assert!(
            GATE_EVAL_PATH.starts_with(extract_dir),
            "GATE_EVAL_PATH must be under the unzip -d destination"
        );
        assert!(
            IMPORT_EVAL_PATH.starts_with(extract_dir),
            "IMPORT_EVAL_PATH must be under the unzip -d destination"
        );
        let zip_prefix = "ado-script/";
        assert!(
            GATE_EVAL_PATH
                .strip_prefix(extract_dir)
                .expect("gate path should include extract dir")
                .starts_with(zip_prefix),
            "GATE_EVAL_PATH suffix must match zip internal path prefix used in release.yml"
        );
        assert!(
            IMPORT_EVAL_PATH
                .strip_prefix(extract_dir)
                .expect("import path should include extract dir")
                .starts_with(zip_prefix),
            "IMPORT_EVAL_PATH suffix must match zip internal path prefix used in release.yml"
        );
        let steps = install_and_download_steps_typed(None);
        match &steps[1] {
            Step::Bash(download) => assert!(
                download.script.contains("-d /tmp/ado-aw-scripts/"),
                "download step must unzip to /tmp/ado-aw-scripts/"
            ),
            other => panic!("expected download bash step, got {other:?}"),
        }
    }

    #[test]
    fn github_app_token_step_renders_node_invocation_and_env() {
        use crate::compile::types::GithubAppTokenConfig;
        let cfg = GithubAppTokenConfig {
            app_id: "1234567".to_string(),
            private_key: Some("GH_APP_KEY".to_string()),
            owner: "octo-org".to_string(),
            repositories: vec!["octo-repo".to_string(), "other-repo".to_string()],
            api_url: None,
            skip_token_revocation: false,
        };
        let Step::Bash(step) = github_app_token_step_typed(&cfg).unwrap() else {
            panic!("expected a bash step");
        };
        assert_eq!(step.display_name, "Mint GitHub App token (Copilot engine auth)");
        assert!(
            step.script
                .contains("node '/tmp/ado-aw-scripts/ado-script/github-app-token.js'"),
            "script must invoke the bundle:\n{}",
            step.script
        );
        // Non-secret inputs are single-quoted argv flags (shadow-proof). The
        // app-id is a literal.
        assert!(
            step.script.contains("--app-id '1234567'"),
            "app-id must be a single-quoted literal arg:\n{}",
            step.script
        );
        assert!(
            step.script.contains("--owner 'octo-org'"),
            "owner must be a single-quoted argv flag:\n{}",
            step.script
        );
        assert!(
            step.script.contains("--repositories 'octo-repo other-repo'"),
            "repositories must be a single-quoted argv flag:\n{}",
            step.script
        );
        // The output variable name is pinned as argv so no pipeline variable
        // can redirect the minted token.
        assert!(
            step.script.contains("--output-var 'GITHUB_APP_TOKEN'"),
            "output-var must be pinned as an argv flag:\n{}",
            step.script
        );
        // No api-url configured ⇒ no --api-url flag (bundle uses its default).
        assert!(!step.script.contains("--api-url"));
        // Only the private key rides in env — as a masked secret, never argv.
        // Here the `private-key` override names GH_APP_KEY.
        assert!(matches!(
            step.env.get("GH_APP_PRIVATE_KEY"),
            Some(EnvValue::Secret(v)) if v == "GH_APP_KEY"
        ));
        // No non-secret GH_APP_* env keys (they are argv now).
        assert!(!step.env.contains_key("GH_APP_ID"));
        assert!(!step.env.contains_key("GH_APP_OWNER"));
        assert!(!step.env.contains_key("GH_APP_REPOSITORIES"));
        assert!(!step.env.contains_key("GH_APP_OUTPUT_VAR"));
        assert!(!step.env.contains_key("GH_APP_API_URL"));
    }

    #[test]
    fn github_app_token_step_defaults_private_key_var() {
        use crate::compile::types::GithubAppTokenConfig;
        // private-key omitted ⇒ the masked secret env uses the compiler default.
        let cfg = GithubAppTokenConfig {
            app_id: "1234567".to_string(),
            private_key: None,
            owner: "octo-org".to_string(),
            repositories: vec![],
            api_url: None,
            skip_token_revocation: false,
        };
        let Step::Bash(step) = github_app_token_step_typed(&cfg).unwrap() else {
            panic!("expected a bash step");
        };
        assert!(matches!(
            step.env.get("GH_APP_PRIVATE_KEY"),
            Some(EnvValue::Secret(v)) if v == "GITHUB_APP_PRIVATE_KEY"
        ));
    }

    #[test]
    fn github_app_token_step_accepts_client_id_app_id() {
        use crate::compile::types::GithubAppTokenConfig;
        // An alphanumeric client ID is a valid literal app-id (regression guard
        // against the old digits-only "is it a variable?" heuristic).
        let cfg = GithubAppTokenConfig {
            app_id: "Iv23liABCdef".to_string(),
            private_key: None,
            owner: "octo-org".to_string(),
            repositories: vec![],
            api_url: None,
            skip_token_revocation: false,
        };
        let Step::Bash(step) = github_app_token_step_typed(&cfg).unwrap() else {
            panic!("expected a bash step");
        };
        assert!(
            step.script.contains("--app-id 'Iv23liABCdef'"),
            "client-id app-id must be a single-quoted literal, not a macro:\n{}",
            step.script
        );
    }

    #[test]
    fn github_app_token_step_renders_literal_app_id_and_api_url() {
        use crate::compile::types::GithubAppTokenConfig;
        let cfg = GithubAppTokenConfig {
            app_id: "1234567".to_string(),
            private_key: Some("GH_APP_KEY".to_string()),
            owner: "octo-org".to_string(),
            repositories: vec![],
            api_url: Some("https://ghe.example.com/api/v3".to_string()),
            skip_token_revocation: false,
        };
        let Step::Bash(step) = github_app_token_step_typed(&cfg).unwrap() else {
            panic!("expected a bash step");
        };
        // A numeric app-id is emitted verbatim (single-quoted), not as a macro.
        assert!(
            step.script.contains("--app-id '1234567'"),
            "literal app-id must be a single-quoted verbatim arg:\n{}",
            step.script
        );
        assert!(!step.script.contains("--app-id '$("));
        assert!(
            step.script
                .contains("--api-url 'https://ghe.example.com/api/v3'"),
            "api-url must be a single-quoted argv flag:\n{}",
            step.script
        );
    }

    #[test]
    fn github_app_token_step_omits_repositories_when_empty() {
        use crate::compile::types::GithubAppTokenConfig;
        let cfg = GithubAppTokenConfig {
            app_id: "1234567".to_string(),
            private_key: Some("GH_APP_KEY".to_string()),
            owner: "octo-org".to_string(),
            repositories: vec![],
            api_url: None,
            skip_token_revocation: false,
        };
        let Step::Bash(step) = github_app_token_step_typed(&cfg).unwrap() else {
            panic!("expected a bash step");
        };
        assert!(!step.script.contains("--repositories"));
    }

    #[test]
    fn github_app_token_step_rejects_invalid_config() {
        use crate::compile::types::GithubAppTokenConfig;
        let cfg = GithubAppTokenConfig {
            app_id: "bad var".to_string(),
            private_key: Some("GH_APP_KEY".to_string()),
            owner: "octo-org".to_string(),
            repositories: vec![],
            api_url: None,
            skip_token_revocation: false,
        };
        assert!(github_app_token_step_typed(&cfg).is_err());
    }

    #[test]
    fn github_app_token_revoke_step_renders_best_effort_delete() {
        use crate::compile::ir::condition::Condition;
        use crate::compile::types::GithubAppTokenConfig;
        let cfg = GithubAppTokenConfig {
            app_id: "1234567".to_string(),
            private_key: Some("GH_APP_KEY".to_string()),
            owner: "octo-org".to_string(),
            repositories: vec![],
            api_url: Some("https://ghe.example.com/api/v3".to_string()),
            skip_token_revocation: false,
        };
        let Step::Bash(step) = github_app_token_revoke_step_typed(&cfg).unwrap() else {
            panic!("expected a bash step");
        };
        assert!(
            step.script
                .contains("node '/tmp/ado-aw-scripts/ado-script/github-app-token.js' revoke"),
            "revoke step must invoke the bundle in revoke mode:\n{}",
            step.script
        );
        // The minted token is passed as a masked secret; never fails the build.
        assert!(matches!(
            step.env.get("GH_APP_TOKEN"),
            Some(EnvValue::Secret(v)) if v == "GITHUB_APP_TOKEN"
        ));
        // api-url is an argv flag (non-secret), not an env var.
        assert!(
            step.script.contains("revoke --api-url 'https://ghe.example.com/api/v3'"),
            "revoke must pass api-url as an argv flag:\n{}",
            step.script
        );
        assert!(!step.env.contains_key("GH_APP_API_URL"));
        assert_eq!(step.condition, Some(Condition::Always));
        assert!(step.continue_on_error);
    }

    #[test]
    fn github_app_token_revoke_step_rejects_invalid_config() {
        // Symmetry with the mint step: the revoke builder validates too, so a
        // caller can't emit a revoke step from an invalid config.
        use crate::compile::types::GithubAppTokenConfig;
        let cfg = GithubAppTokenConfig {
            app_id: "1234567".to_string(),
            private_key: Some("GH_APP_KEY".to_string()),
            owner: "octo-org".to_string(),
            repositories: vec![],
            api_url: Some("http://insecure.example.com/api/v3".to_string()),
            skip_token_revocation: false,
        };
        assert!(github_app_token_revoke_step_typed(&cfg).is_err());
    }

    #[test]
    fn github_app_token_eval_path_consistent_with_download_dir() {
        assert!(GITHUB_APP_TOKEN_PATH.starts_with("/tmp/ado-aw-scripts/ado-script/"));
    }

    #[test]
    fn prepare_pr_base_step_emits_bundle_invocation_with_target_and_bearer() {
        let repos = vec![(
            "$(Build.SourcesDirectory)".to_string(),
            "main".to_string(),
        )];
        let Step::Bash(step) = prepare_pr_base_step_typed(&repos) else {
            panic!("expected a bash step");
        };
        assert_eq!(
            step.display_name,
            "Prepare create-pull-request base ref (fetch/deepen)"
        );
        assert!(
            step.script
                .contains("node '/tmp/ado-aw-scripts/ado-script/prepare-pr-base.js'"),
            "script must invoke the bundle:\n{}",
            step.script
        );
        // The repo dir (== MCP server bounding_directory) is a double-quoted argv
        // flag (ADO-macro path convention); its target is a single-quoted literal.
        assert!(
            step.script
                .contains("--repo-dir \"$(Build.SourcesDirectory)\" --target-branch 'main'"),
            "must emit the dir/target pair (double-quoted dir, single-quoted target):\n{}",
            step.script
        );
        // The ADO bearer is projected as a masked secret (bundle uses it for the
        // authenticated git fetch); SYSTEM_ACCESSTOKEN is not auto-injected.
        assert!(matches!(
            step.env.get("SYSTEM_ACCESSTOKEN"),
            Some(EnvValue::Secret(v)) if v == "System.AccessToken"
        ));
    }

    #[test]
    fn prepare_pr_base_step_quotes_a_non_default_target() {
        let repos = vec![(
            "$(Build.SourcesDirectory)".to_string(),
            "release/2.x".to_string(),
        )];
        let Step::Bash(step) = prepare_pr_base_step_typed(&repos) else {
            panic!("expected a bash step");
        };
        assert!(
            step.script.contains("--target-branch 'release/2.x'"),
            "non-default target must be passed through single-quoted:\n{}",
            step.script
        );
    }

    #[test]
    fn prepare_pr_base_step_emits_a_pair_per_repo_with_per_repo_targets() {
        // Multi-repo meta setup: self + two aliases, each with its OWN target
        // branch — one `--repo-dir "<dir>" --target-branch '<branch>'` pair each.
        let repos = vec![
            ("$(Build.SourcesDirectory)".to_string(), "main".to_string()),
            (
                "$(Build.SourcesDirectory)/tools".to_string(),
                "release".to_string(),
            ),
            (
                "$(Build.SourcesDirectory)/docs".to_string(),
                "gh-pages".to_string(),
            ),
        ];
        let Step::Bash(step) = prepare_pr_base_step_typed(&repos) else {
            panic!("expected a bash step");
        };
        assert_eq!(
            step.script.matches("--repo-dir ").count(),
            3,
            "one --repo-dir per repo:\n{}",
            step.script
        );
        assert!(
            step.script
                .contains("--repo-dir \"$(Build.SourcesDirectory)\" --target-branch 'main'")
        );
        assert!(
            step.script
                .contains("--repo-dir \"$(Build.SourcesDirectory)/tools\" --target-branch 'release'")
        );
        assert!(
            step.script.contains(
                "--repo-dir \"$(Build.SourcesDirectory)/docs\" --target-branch 'gh-pages'"
            )
        );
    }

    #[test]
    fn prepare_pr_base_path_consistent_with_download_dir() {
        assert!(PREPARE_PR_BASE_PATH.starts_with("/tmp/ado-aw-scripts/ado-script/"));
        assert!(PREPARE_PR_BASE_PATH.ends_with("prepare-pr-base.js"));
    }

    #[test]
    fn declarations_agent_prepare_download_fires_when_only_prepare_pr_base_active() {
        let mut ext = ext_with(None, None, true);
        ext.prepare_pr_base_active = true;
        let fm: FrontMatter = serde_yaml::from_str("name: t\ndescription: t").unwrap();
        let ctx = CompileContext::for_test(&fm);
        let steps = ext.declarations(&ctx).unwrap().agent_prepare_steps;
        // Install + download fire (so prepare-pr-base.js is staged), but no
        // runtime-import resolver (inlined_imports: true).
        assert_eq!(steps.len(), 2, "install + download only");
        assert!(matches!(&steps[0], Step::Task(t) if t.task == "UseNode@1"));
        assert!(
            matches!(&steps[1], Step::Bash(b) if b.display_name.contains("Download ado-aw scripts"))
        );
    }

    #[test]
    fn declarations_agent_prepare_steps_emits_install_download_and_resolver_when_runtime_imports_active()
     {
        let ext = ext_with(None, None, false);
        let fm: FrontMatter = serde_yaml::from_str("name: t\ndescription: t").unwrap();
        let ctx = CompileContext::for_test(&fm);
        let steps = ext.declarations(&ctx).unwrap().agent_prepare_steps;
        assert_eq!(steps.len(), 3, "install + download + resolver");
        assert!(matches!(&steps[0], Step::Task(t) if t.task == "UseNode@1"));
        assert!(
            matches!(&steps[1], Step::Bash(b) if b.display_name.contains("Download ado-aw scripts"))
        );
        let Step::Bash(resolver) = &steps[2] else {
            panic!("expected resolver bash step, got {:?}", steps[2]);
        };
        assert!(
            resolver
                .script
                .contains("node '/tmp/ado-aw-scripts/ado-script/import.js'")
        );
        assert_eq!(
            resolver.display_name,
            "Resolve runtime imports (agent prompt)"
        );
        // The resolver receives `--base "$(Build.SourcesDirectory)"` so
        // the compiler-emitted trigger-repo-relative marker path
        // resolves correctly. Absolute paths in author markers are
        // rejected by import.js — see its absolute-path guard.
        assert!(
            resolver
                .script
                .contains("--base \"$(Build.SourcesDirectory)\""),
            "resolver step must pass --base so trigger-repo-relative markers resolve correctly"
        );
        assert!(
            !resolver.script.contains("ADO_AW_IMPORT_BASE"),
            "resolver step must not export ADO_AW_IMPORT_BASE — base is passed via --base, not env"
        );
        // Each path-anchor var is passed as `--var "<name>=$(<name>)"` so
        // ADO expands the macro at runtime and import.js substitutes the
        // concrete value into the prompt (consistent with inlined mode).
        assert!(
            resolver
                .script
                .contains("--var \"Build.SourcesDirectory=$(Build.SourcesDirectory)\""),
            "resolver step must pass Build.SourcesDirectory as a --var, got: {}",
            resolver.script
        );
        assert!(
            resolver
                .script
                .contains("--var \"Build.Repository.Name=$(Build.Repository.Name)\""),
            "resolver step must pass Build.Repository.Name as a --var, got: {}",
            resolver.script
        );
    }

    #[test]
    fn validate_catches_min_gt_max_changes() {
        let filters = PrFilters {
            min_changes: Some(100),
            max_changes: Some(5),
            ..Default::default()
        };
        let ext = ext_with(Some(filters), None, true);
        let fm: FrontMatter = serde_yaml::from_str("name: t\ndescription: t").unwrap();
        let ctx = CompileContext::for_test(&fm);
        let err = ext.declarations(&ctx).unwrap_err();
        assert!(
            err.to_string().contains("min-changes"),
            "expected min-changes / max-changes error, got: {err}"
        );
    }

    #[test]
    fn required_hosts_empty_when_gate_active() {
        // ado-script never widens the agent's AWF allowlist regardless of
        // configuration. The bundle is downloaded at the pipeline-host level
        // (curl in a bash step before AWF starts), so the agent never reaches
        // github.com because of ado-script. Tested here with gate active — the
        // most counterintuitive configuration — since the gate evaluator also
        // runs outside the AWF agent sandbox (Setup job).
        let filters = PrFilters {
            labels: Some(LabelFilter {
                any_of: vec!["run-agent".into()],
                ..Default::default()
            }),
            ..Default::default()
        };
        let ext = ext_with(Some(filters), None, true);
        let fm: FrontMatter = serde_yaml::from_str("name: t\ndescription: t").unwrap();
        let ctx = CompileContext::for_test(&fm);
        assert!(ext.declarations(&ctx).unwrap().network_hosts.is_empty());
    }

    // ── agent_conditions contribution (issue #987) ──────────────────────
    //
    // These tests pin the shape of `Declarations::agent_conditions` per
    // trigger configuration. The agentic-pipeline builder folds these
    // into the Agent job's `condition:` with a leading `Succeeded`
    // (covered by `agent_conditions_fold_*` tests below); per-extension
    // contribution shape is asserted here so a future regression in
    // `build_agent_conditions` fails close to its source rather than
    // showing up as a fixture drift two layers away.

    fn ext_with_synth(
        pr: Option<PrFilters>,
        pipeline: Option<PipelineFilters>,
    ) -> AdoScriptExtension {
        use crate::compile::types::{BranchFilter, PrTriggerConfig};
        AdoScriptExtension {
            pr_filters: pr,
            pipeline_filters: pipeline,
            inlined_imports: true,
            exec_context_pr_active: false,
            exec_context_manual_active: false,
            exec_context_pipeline_active: false,
            exec_context_ci_push_active: false,
            exec_context_workitem_active: false,
            exec_context_schedule_active: false,
            exec_context_pr_checks_active: false,
            exec_context_repo_active: false,
            safe_outputs_summary_active: false,
            github_app_token_active: false,
            prepare_pr_base_active: false,
            pr_trigger_for_synth: Some(PrTriggerConfig {
                branches: Some(BranchFilter {
                    include: vec!["main".into()],
                    exclude: vec![],
                }),
                paths: None,
                filters: None,
                ..Default::default()
            }),
            supply_chain: None,
        }
    }

    fn label_pr_filters(label: &str) -> PrFilters {
        PrFilters {
            labels: Some(LabelFilter {
                any_of: vec![label.into()],
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn label_pipeline_filters() -> PipelineFilters {
        // `branch` is a fact-bearing field that lowers to a non-empty
        // check, which is what we need to trigger the pipeline-filter
        // gate clause without depending on a PR-only field.
        PipelineFilters {
            branch: Some(PatternFilter {
                pattern: "main".into(),
            }),
            ..Default::default()
        }
    }

    #[test]
    fn agent_conditions_empty_when_no_filters_and_no_synth() {
        let ext = ext_with(None, None, true);
        let fm: FrontMatter = serde_yaml::from_str("name: t\ndescription: t").unwrap();
        let ctx = CompileContext::for_test(&fm);
        let decl = ext.declarations(&ctx).unwrap();
        assert!(
            decl.agent_conditions.is_empty(),
            "no filters / no synth → no Agent-job clauses, got {:?}",
            decl.agent_conditions
        );
    }

    #[test]
    fn agent_conditions_emits_synth_skip_clause_when_synthetic_pr_active() {
        // Just synthetic-PR, no filters: a single Ne clause that
        // self-skips the Agent job when the synthPr step set
        // AW_SYNTHETIC_PR_SKIP=true.
        let ext = ext_with_synth(None, None);
        let fm: FrontMatter = serde_yaml::from_str("name: t\ndescription: t").unwrap();
        let ctx = CompileContext::for_test(&fm);
        let clauses = ext.declarations(&ctx).unwrap().agent_conditions;
        assert_eq!(clauses.len(), 1, "single synth-skip clause: {clauses:?}");
        match &clauses[0] {
            Condition::Ne(Expr::StepOutput(out), Expr::Literal(lit)) => {
                assert_eq!(out.step.as_str(), "synthPr");
                assert_eq!(out.name, "AW_SYNTHETIC_PR_SKIP");
                assert_eq!(lit, "true");
            }
            other => panic!("expected Ne(StepOutput, Literal), got {other:?}"),
        }
    }

    #[test]
    fn agent_conditions_emits_pr_gate_clause_when_pr_filters_present_no_synth() {
        // PR filters without synth-from-CI: an Or(...) gating real
        // PullRequest builds on prGate.SHOULD_RUN.
        let ext = ext_with(Some(label_pr_filters("run-agent")), None, true);
        let fm: FrontMatter = serde_yaml::from_str("name: t\ndescription: t").unwrap();
        let ctx = CompileContext::for_test(&fm);
        let clauses = ext.declarations(&ctx).unwrap().agent_conditions;
        assert_eq!(
            clauses.len(),
            1,
            "single PR-gate Or clause (no synth-skip): {clauses:?}"
        );
        let Condition::Or(parts) = &clauses[0] else {
            panic!("expected Or, got {:?}", clauses[0]);
        };
        assert_eq!(
            parts.len(),
            2,
            "Or(Build.Reason!=PR, prGate.SHOULD_RUN=true)"
        );
        match &parts[0] {
            Condition::Ne(Expr::Variable(name), Expr::Literal(lit)) => {
                assert_eq!(name, "Build.Reason");
                assert_eq!(lit, "PullRequest");
            }
            other => panic!("expected Ne(Build.Reason, PullRequest), got {other:?}"),
        }
        match &parts[1] {
            Condition::Eq(Expr::StepOutput(out), Expr::Literal(lit)) => {
                assert_eq!(out.step.as_str(), "prGate");
                assert_eq!(out.name, "SHOULD_RUN");
                assert_eq!(lit, "true");
            }
            other => panic!("expected Eq(prGate.SHOULD_RUN, true), got {other:?}"),
        }
    }

    #[test]
    fn agent_conditions_pr_gate_carves_out_synth_when_synthetic_active() {
        // PR filters AND synth-from-CI: the inner `Ne != PullRequest`
        // becomes And(Ne != PullRequest, Ne synthPr.AW_SYNTHETIC_PR != true)
        // so synth-promoted CI builds also have to clear prGate.
        let ext = ext_with_synth(Some(label_pr_filters("run-agent")), None);
        let fm: FrontMatter = serde_yaml::from_str("name: t\ndescription: t").unwrap();
        let ctx = CompileContext::for_test(&fm);
        let clauses = ext.declarations(&ctx).unwrap().agent_conditions;
        assert_eq!(
            clauses.len(),
            2,
            "synth-skip + PR-gate (with synth carve-out): {clauses:?}"
        );
        // Clause 0: synth-skip (already covered above; just sanity check).
        assert!(matches!(
            &clauses[0],
            Condition::Ne(Expr::StepOutput(out), _)
                if out.step.as_str() == "synthPr" && out.name == "AW_SYNTHETIC_PR_SKIP"
        ));
        // Clause 1: Or(And(Ne PullRequest, Ne synthPr.AW_SYNTHETIC_PR), Eq prGate.SHOULD_RUN).
        let Condition::Or(parts) = &clauses[1] else {
            panic!("expected Or, got {:?}", clauses[1]);
        };
        assert_eq!(parts.len(), 2);
        let Condition::And(and_parts) = &parts[0] else {
            panic!("expected And inside Or, got {:?}", parts[0]);
        };
        assert_eq!(and_parts.len(), 2);
        // and_parts[0]: Ne(Build.Reason, "PullRequest") — real PullRequest
        // builds must still clear the PR gate even when synth is active.
        assert!(matches!(
            &and_parts[0],
            Condition::Ne(Expr::Variable(name), Expr::Literal(lit))
                if name == "Build.Reason" && lit == "PullRequest"
        ));
        // and_parts[1]: Ne(synthPr.AW_SYNTHETIC_PR, "true") — synth-promoted
        // CI builds are also routed through the gate.
        assert!(matches!(
            &and_parts[1],
            Condition::Ne(Expr::StepOutput(out), Expr::Literal(lit))
                if out.step.as_str() == "synthPr" && out.name == "AW_SYNTHETIC_PR" && lit == "true"
        ));
        // parts[1]: Eq(prGate.SHOULD_RUN, "true") — gate short-circuit for
        // builds that cleared the PR filter.
        assert!(matches!(
            &parts[1],
            Condition::Eq(Expr::StepOutput(out), Expr::Literal(lit))
                if out.step.as_str() == "prGate" && out.name == "SHOULD_RUN" && lit == "true"
        ));
    }

    #[test]
    fn agent_conditions_emits_pipeline_gate_clause_when_pipeline_filters_present() {
        let ext = ext_with(None, Some(label_pipeline_filters()), true);
        let fm: FrontMatter = serde_yaml::from_str("name: t\ndescription: t").unwrap();
        let ctx = CompileContext::for_test(&fm);
        let clauses = ext.declarations(&ctx).unwrap().agent_conditions;
        assert_eq!(clauses.len(), 1, "single pipeline-gate Or clause");
        let Condition::Or(parts) = &clauses[0] else {
            panic!("expected Or, got {:?}", clauses[0]);
        };
        assert_eq!(parts.len(), 2);
        assert!(matches!(
            &parts[0],
            Condition::Ne(Expr::Variable(name), Expr::Literal(lit))
                if name == "Build.Reason" && lit == "ResourceTrigger"
        ));
        assert!(matches!(
            &parts[1],
            Condition::Eq(Expr::StepOutput(out), Expr::Literal(lit))
                if out.step.as_str() == "pipelineGate" && out.name == "SHOULD_RUN" && lit == "true"
        ));
    }

    #[test]
    fn agent_conditions_appends_custom_expressions_after_typed_clauses() {
        // User `expression:` escape hatches arrive after typed clauses
        // in declaration order: pr_expression first, then
        // pipeline_expression.
        let pr = PrFilters {
            labels: Some(LabelFilter {
                any_of: vec!["run-agent".into()],
                ..Default::default()
            }),
            expression: Some("eq(variables['MyVar'], 'pr-yes')".into()),
            ..Default::default()
        };
        let pipeline = PipelineFilters {
            branch: Some(PatternFilter {
                pattern: "main".into(),
            }),
            expression: Some("eq(variables['MyVar'], 'pipe-yes')".into()),
            ..Default::default()
        };
        let ext = ext_with(Some(pr), Some(pipeline), true);
        let fm: FrontMatter = serde_yaml::from_str("name: t\ndescription: t").unwrap();
        let ctx = CompileContext::for_test(&fm);
        let clauses = ext.declarations(&ctx).unwrap().agent_conditions;
        // [pr-gate Or, pipeline-gate Or, Custom pr-expr, Custom pipeline-expr]
        assert_eq!(clauses.len(), 4, "got: {clauses:?}");
        assert!(matches!(&clauses[0], Condition::Or(_)));
        assert!(matches!(&clauses[1], Condition::Or(_)));
        assert!(
            matches!(&clauses[2], Condition::Custom(s) if s == "eq(variables['MyVar'], 'pr-yes')")
        );
        assert!(
            matches!(&clauses[3], Condition::Custom(s) if s == "eq(variables['MyVar'], 'pipe-yes')")
        );
    }

    // ── resolve_imports_inline ─────────────────────────────────────────

    static NEXT_ID: AtomicUsize = AtomicUsize::new(0);

    struct TestWorkspace {
        path: PathBuf,
    }

    impl TestWorkspace {
        fn new() -> Self {
            let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            let path = std::env::current_dir()
                .expect("current dir")
                .join("target")
                .join("ado-script-tests")
                .join(format!("{}-{}", std::process::id(), id));
            if path.exists() {
                let _ = fs::remove_dir_all(&path);
            }
            fs::create_dir_all(&path).expect("create workspace");
            Self { path }
        }

        fn write(&self, relative: &str, contents: &str) -> PathBuf {
            let path = self.path.join(relative);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("create parent");
            }
            fs::write(&path, contents).expect("write fixture file");
            path
        }
    }

    impl Drop for TestWorkspace {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn required_marker_resolves_to_file_contents() {
        let workspace = TestWorkspace::new();
        workspace.write("snippet.md", "hello from import\n");

        let result = resolve_imports_inline(
            "before\n{{#runtime-import snippet.md}}\nafter\n",
            &workspace.path,
        )
        .unwrap();

        assert_eq!(result, "before\nhello from import\n\nafter\n");
    }

    #[test]
    fn required_marker_missing_file_returns_error() {
        let workspace = TestWorkspace::new();
        let err =
            resolve_imports_inline("{{#runtime-import missing.md}}", &workspace.path).unwrap_err();
        assert!(
            err.to_string()
                .contains("runtime-import: file not found: missing.md")
        );
    }

    #[test]
    fn optional_marker_missing_file_replaces_with_empty_string() {
        let workspace = TestWorkspace::new();
        let result =
            resolve_imports_inline("pre{{#runtime-import? missing.md}}post", &workspace.path)
                .unwrap();
        assert_eq!(result, "prepost");
    }

    /// Relative paths under `base_dir` resolve correctly. Absolute paths
    /// are explicitly rejected — see `rejects_absolute_path_at_compile_time`.
    #[test]
    fn supports_relative_path_resolution() {
        let workspace = TestWorkspace::new();
        let nested_base = workspace.path.join("nested");
        fs::create_dir_all(&nested_base).unwrap();
        workspace.write("nested/relative.md", "relative-body");

        let relative =
            resolve_imports_inline("{{#runtime-import relative.md}}", &nested_base).unwrap();

        assert_eq!(relative, "relative-body");
    }

    /// Compile-time absolute-path rejection. The compile machine has
    /// privileged filesystem access (e.g. CI runners hold `.ssh/id_rsa`,
    /// hosted-pool service-connection material, dotfiles under the
    /// runner's home dir). An untrusted PR branch's markdown body must
    /// NOT be able to embed those files via
    /// `{{#runtime-import /home/runner/.ssh/id_rsa}}`. Only relative
    /// imports rooted under the agent's `.md` file's directory — which
    /// is itself inside the repo — are safe in adversarial scenarios.
    #[test]
    fn rejects_absolute_posix_path_at_compile_time() {
        let workspace = TestWorkspace::new();
        let err =
            resolve_imports_inline("{{#runtime-import /etc/passwd}}", &workspace.path).unwrap_err();
        assert!(
            err.to_string().contains("absolute paths are not allowed"),
            "expected absolute-path rejection, got: {err}"
        );
    }

    #[test]
    fn rejects_absolute_windows_drive_path_at_compile_time() {
        let workspace = TestWorkspace::new();
        let err = resolve_imports_inline(
            r"{{#runtime-import C:\Users\runner\secret.txt}}",
            &workspace.path,
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("absolute paths are not allowed"),
            "expected absolute-path rejection, got: {err}"
        );
    }

    #[test]
    fn rejects_unc_path_at_compile_time() {
        let workspace = TestWorkspace::new();
        let err = resolve_imports_inline(
            r"{{#runtime-import \\server\share\file.md}}",
            &workspace.path,
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("absolute paths are not allowed"),
            "expected absolute-path rejection, got: {err}"
        );
    }

    #[test]
    fn resolves_multiple_markers_in_one_body() {
        let workspace = TestWorkspace::new();
        workspace.write("one.md", "ONE");
        workspace.write("two.md", "TWO");

        let result = resolve_imports_inline(
            "A {{#runtime-import one.md}} B {{#runtime-import two.md}} C",
            &workspace.path,
        )
        .unwrap();

        assert_eq!(result, "A ONE B TWO C");
    }

    /// `}` rejection keeps the compile-time resolver in strict parity
    /// with the runtime regex (`[^\s}]+`). Without this guard, a path
    /// like `foo}bar.md` would be accepted at compile time but cause
    /// the runtime resolver to either truncate it or leave the marker
    /// unexpanded — silent divergence. Reject up front.
    #[test]
    fn rejects_path_containing_closing_brace() {
        let workspace = TestWorkspace::new();
        let err =
            resolve_imports_inline("{{#runtime-import foo}bar.md}}", &workspace.path).unwrap_err();
        assert!(
            err.to_string().contains("is not allowed"),
            "expected `}}` rejection, got: {err}"
        );
    }

    /// Path traversal: `..` segments would let a malicious agent body
    /// reach files outside `base_dir` (e.g. `../../../../etc/passwd` when
    /// `ado-aw compile` runs over an untrusted PR branch). Reject at
    /// resolution time regardless of whether the file actually exists.
    #[test]
    fn rejects_relative_path_with_dotdot_segment() {
        let workspace = TestWorkspace::new();
        let err = resolve_imports_inline("{{#runtime-import ../escape.md}}", &workspace.path)
            .unwrap_err();
        assert!(
            err.to_string()
                .contains("'..' path components are not allowed"),
            "expected '..' rejection, got: {err}"
        );
    }

    #[test]
    fn rejects_path_with_embedded_dotdot_segment() {
        let workspace = TestWorkspace::new();
        let err =
            resolve_imports_inline("{{#runtime-import sub/../../escape.md}}", &workspace.path)
                .unwrap_err();
        assert!(
            err.to_string()
                .contains("'..' path components are not allowed"),
            "expected '..' rejection, got: {err}"
        );
    }

    #[test]
    fn rejects_absolute_path_with_dotdot_segment() {
        let workspace = TestWorkspace::new();
        // The `..`-segment guard fires before the absolute-path guard,
        // so an absolute path with embedded `..` is reported as a
        // traversal violation. Either rejection is acceptable for this
        // input shape.
        let err = resolve_imports_inline(
            "{{#runtime-import /tmp/agents/../../etc/passwd}}",
            &workspace.path,
        )
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("'..' path components are not allowed"),
            "expected '..' rejection, got: {err}"
        );
    }

    #[test]
    fn rejects_backslash_dotdot_segment_on_windows_style_paths() {
        let workspace = TestWorkspace::new();
        let err =
            resolve_imports_inline(r"{{#runtime-import sub\..\..\escape.md}}", &workspace.path)
                .unwrap_err();
        assert!(
            err.to_string()
                .contains("'..' path components are not allowed"),
            "expected '..' rejection, got: {err}"
        );
    }

    /// `..filename.md` and `name..md` are not path-traversal — they're
    /// literal filenames where `..` is part of the name, not a segment.
    /// Make sure the segment-aware check doesn't false-positive on these.
    #[test]
    fn allows_literal_double_dot_in_filename() {
        let workspace = TestWorkspace::new();
        workspace.write("..hidden.md", "DOTHIDDEN");
        workspace.write("name..md", "DOUBLE");

        let a = resolve_imports_inline("{{#runtime-import ..hidden.md}}", &workspace.path).unwrap();
        let b = resolve_imports_inline("{{#runtime-import name..md}}", &workspace.path).unwrap();

        assert_eq!(a, "DOTHIDDEN");
        assert_eq!(b, "DOUBLE");
    }

    // ── Typed-IR declarations (port-ado-script) ─────────────────────

    /// `declarations()` returns empty step lists when neither
    /// runtime-import nor exec-context-pr nor any gate / synth path
    /// is active.
    #[test]
    fn declarations_empty_when_nothing_active() {
        let ext = ext_with(None, None, true);
        let fm: FrontMatter = serde_yaml::from_str("name: t\ndescription: t").unwrap();
        let ctx = CompileContext::for_test(&fm);
        let decl = ext.declarations(&ctx).unwrap();
        assert!(decl.setup_steps.is_empty());
        assert!(decl.agent_prepare_steps.is_empty());
    }

    /// `declarations()` setup_steps must surface a typed
    /// `Step::Task(UseNode@1)` followed by `Step::Bash` (download)
    /// followed by the typed gate `Step::Bash` when a PR gate is
    /// active. No `Step::RawYaml`.
    #[test]
    fn declarations_setup_steps_typed_with_gate_active() {
        use crate::compile::types::LabelFilter;
        let filters = PrFilters {
            labels: Some(LabelFilter {
                any_of: vec!["run-agent".into()],
                ..Default::default()
            }),
            ..Default::default()
        };
        let ext = ext_with(Some(filters), None, true);
        let fm: FrontMatter = serde_yaml::from_str("name: t\ndescription: t").unwrap();
        let ctx = CompileContext::for_test(&fm);
        let decl = ext.declarations(&ctx).unwrap();
        assert_eq!(decl.setup_steps.len(), 3, "install + download + prGate");

        match &decl.setup_steps[0] {
            Step::Task(t) => assert_eq!(t.task, "UseNode@1"),
            other => panic!("expected Task(UseNode@1), got {other:?}"),
        }
        match &decl.setup_steps[1] {
            Step::Bash(b) => assert!(b.display_name.starts_with("Download ado-aw scripts")),
            other => panic!("expected Bash(download), got {other:?}"),
        }
        match &decl.setup_steps[2] {
            Step::Bash(b) => {
                assert_eq!(b.id.as_ref().map(|i| i.as_str()), Some("prGate"));
                assert_eq!(b.display_name, "Evaluate PR filters");
                assert!(b.env.contains_key("GATE_SPEC"));
                assert!(b.env.contains_key("SYSTEM_ACCESSTOKEN"));
            }
            other => panic!("expected Bash(prGate) with id, got {other:?}"),
        }
    }

    /// When the synth path is active, the typed `synthPr` step lands
    /// before any gate step and carries the five `AW_SYNTHETIC_PR*`
    /// outputs as typed `OutputDecl`s.
    #[test]
    fn declarations_setup_steps_typed_with_synthetic_pr_active() {
        use crate::compile::types::{BranchFilter, PrTriggerConfig};
        let ext = AdoScriptExtension {
            pr_filters: None,
            pipeline_filters: None,
            inlined_imports: true,
            exec_context_pr_active: false,
            exec_context_manual_active: false,
            exec_context_pipeline_active: false,
            exec_context_ci_push_active: false,
            exec_context_workitem_active: false,
            exec_context_schedule_active: false,
            exec_context_pr_checks_active: false,
            exec_context_repo_active: false,
            safe_outputs_summary_active: false,
            github_app_token_active: false,
            prepare_pr_base_active: false,
            pr_trigger_for_synth: Some(PrTriggerConfig {
                branches: Some(BranchFilter {
                    include: vec!["main".into()],
                    exclude: vec![],
                }),
                paths: None,
                filters: None,
                ..Default::default()
            }),
            supply_chain: None,
        };
        let fm: FrontMatter = serde_yaml::from_str("name: t\ndescription: t").unwrap();
        let ctx = CompileContext::for_test(&fm);
        let decl = ext.declarations(&ctx).unwrap();
        assert_eq!(decl.setup_steps.len(), 3, "install + download + synthPr");

        match &decl.setup_steps[2] {
            Step::Bash(b) => {
                assert_eq!(b.id.as_ref().map(|i| i.as_str()), Some("synthPr"));
                assert_eq!(b.display_name, "Resolve synthetic PR context");
                // Outputs declared, in canonical order. The unified
                // `AW_PR_*` namespace (PR #972) is the primary
                // surface; the legacy `AW_SYNTHETIC_PR_*` identifier
                // names remain declared for back-compat with the
                // typed gate-step emitter until those references
                // migrate (see `SYNTH_PR_OUTPUT_NAMES`).
                let names: Vec<&str> = b.outputs.iter().map(|o| o.name.as_str()).collect();
                assert_eq!(
                    names,
                    vec![
                        "AW_PR_ID",
                        "AW_PR_TARGETBRANCH",
                        "AW_PR_SOURCEBRANCH",
                        "AW_PR_IS_DRAFT",
                        "AW_SYNTHETIC_PR",
                        "AW_SYNTHETIC_PR_SKIP",
                    ]
                );
                // Condition is a typed And(Succeeded, Ne(BuildReason, "PullRequest")).
                match b.condition.as_ref().expect("condition required") {
                    crate::compile::ir::condition::Condition::And(parts) => {
                        assert_eq!(parts.len(), 2);
                        assert!(matches!(
                            parts[0],
                            crate::compile::ir::condition::Condition::Succeeded
                        ));
                        assert!(matches!(
                            parts[1],
                            crate::compile::ir::condition::Condition::Ne(_, _)
                        ));
                    }
                    other => panic!("expected Condition::And, got {other:?}"),
                }
            }
            other => panic!("expected Bash(synthPr) with id, got {other:?}"),
        }
    }

    /// **Marquee regression test**: the typed gate step's PR-related
    /// env values read the unified `AW_PR_*` Setup-job-level
    /// variables that the `synthPr` step's `setVar` calls register
    /// in the regular variable namespace. Same-job consumer; reads
    /// must use the `$(name)` macro form (NOT `$[ variables['…'] ]`
    /// — runtime expressions are not evaluated inside step `env:`
    /// values, see PR #956).
    #[test]
    fn typed_gate_pr_id_lowers_to_macro_concat_in_same_job() {
        use crate::compile::filter_ir::{
            Fact, FilterCheck, GateContext, Predicate, build_gate_step_typed,
        };
        use crate::compile::ir::graph::build_graph;
        use crate::compile::ir::ids::JobId;
        use crate::compile::ir::job::{Job, Pool};
        use crate::compile::ir::lower::{LoweringContext, lower_step};
        use crate::compile::ir::{Pipeline, PipelineBody, PipelineShape, Resources, Triggers};

        // Three checks together cover the three identifiers that
        // read from the synth-emitted `AW_PR_*` variables:
        //   - LabelSetMatch (PrLabels → PrMetadata) → ADO_PR_ID
        //   - SourceBranch fact → ADO_SOURCE_BRANCH
        //   - TargetBranch fact → ADO_TARGET_BRANCH
        let checks = vec![
            FilterCheck {
                name: "labels",
                predicate: Predicate::LabelSetMatch {
                    any_of: vec!["run-agent".to_string()],
                    all_of: vec![],
                    none_of: vec![],
                },
                build_tag_suffix: "label-mismatch",
            },
            FilterCheck {
                name: "source-branch",
                predicate: Predicate::GlobMatch {
                    fact: Fact::SourceBranch,
                    pattern: "refs/heads/*".to_string(),
                },
                build_tag_suffix: "source-branch-mismatch",
            },
            FilterCheck {
                name: "target-branch",
                predicate: Predicate::GlobMatch {
                    fact: Fact::TargetBranch,
                    pattern: "refs/heads/main".to_string(),
                },
                build_tag_suffix: "target-branch-mismatch",
            },
        ];
        let synth = synthetic_pr_step_typed("AAAA").unwrap();
        let gate = build_gate_step_typed(
            GateContext::PullRequest,
            &checks,
            GATE_EVAL_PATH,
            true, // synthetic_pr_active
        )
        .unwrap();

        let mut setup_job = Job::new(
            JobId::new("Setup").unwrap(),
            "Setup",
            Pool::VmImage("u".into()),
        );
        setup_job.push_step(Step::Bash(synth));
        setup_job.push_step(Step::Bash(gate));

        let p = Pipeline {
            name: "t".into(),
            parameters: Vec::new(),
            resources: Resources::default(),
            triggers: Triggers::default(),
            variables: Vec::new(),
            body: PipelineBody::Jobs(vec![setup_job]),
            shape: PipelineShape::Standalone,
        };

        // Walk the IR; lower the gate step; assert its env block reads
        // the unified AW_PR_* setVar variables via plain $(name) macros.
        let g = build_graph(&p).unwrap();
        let setup_id = JobId::new("Setup").unwrap();
        let ctx = LoweringContext {
            graph: &g,
            stage: None,
            job: &setup_id,
        };
        let jobs = match &p.body {
            PipelineBody::Jobs(j) => j,
            _ => unreachable!(),
        };
        let gate_step = &jobs[0].steps[1];
        let lowered = lower_step(gate_step, &ctx).unwrap();
        let env_yaml = serde_yaml::to_string(&lowered).unwrap();
        assert!(
            env_yaml.contains("ADO_PR_ID: $(AW_PR_ID)"),
            "ADO_PR_ID must read unified AW_PR_ID var via $() macro; got:\n{env_yaml}"
        );
        assert!(
            env_yaml.contains("ADO_SOURCE_BRANCH: $(AW_PR_SOURCEBRANCH)"),
            "ADO_SOURCE_BRANCH must read AW_PR_SOURCEBRANCH var; got:\n{env_yaml}"
        );
        assert!(
            env_yaml.contains("ADO_TARGET_BRANCH: $(AW_PR_TARGETBRANCH)"),
            "ADO_TARGET_BRANCH must read AW_PR_TARGETBRANCH var; got:\n{env_yaml}"
        );
        // AW_SYNTHETIC_PR uses the same setVar form, NOT
        // $(synthPr.AW_SYNTHETIC_PR) — both work at runtime but the
        // legacy emitter pinned the setVar wire form.
        assert!(
            env_yaml.contains("AW_SYNTHETIC_PR: $(AW_SYNTHETIC_PR)"),
            "AW_SYNTHETIC_PR must use same-job setVar macro; got:\n{env_yaml}"
        );
        assert!(
            !env_yaml.contains("variables['synthPr."),
            "must not emit runtime-expression form for same-job consumer; got:\n{env_yaml}"
        );
    }
}
