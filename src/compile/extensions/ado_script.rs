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
use crate::compile::filter_ir::{
    GateContext, Severity, build_gate_step_typed, lower_pipeline_filters, lower_pr_filters,
    validate_pipeline_filters, validate_pr_filters,
};
use crate::compile::ir::condition::{Condition, Expr};
use crate::compile::ir::env::EnvValue;
use crate::compile::ir::ids::StepId;
use crate::compile::ir::output::OutputDecl;
use crate::compile::ir::step::{BashStep, Step, TaskStep};
use crate::compile::types::{PipelineFilters, PrFilters};

const GATE_EVAL_PATH: &str = "/tmp/ado-aw-scripts/ado-script/gate.js";
pub(crate) const IMPORT_EVAL_PATH: &str = "/tmp/ado-aw-scripts/ado-script/import.js";
/// Path to the exec-context-pr bundle inside the unpacked `ado-script.zip`.
/// Consumed by `src/compile/extensions/exec_context/pr.rs` to invoke
/// the bundle from the PR contributor's prepare step.
pub(crate) const EXEC_CONTEXT_PR_PATH: &str = "/tmp/ado-aw-scripts/ado-script/exec-context-pr.js";
/// Path to the synthetic-PR-context bundle inside the unpacked
/// `ado-script.zip`. Runs in the Setup job before `prGate`; consumed
/// by [`AdoScriptExtension::declarations`].
pub(crate) const EXEC_CONTEXT_PR_SYNTH_PATH: &str =
    "/tmp/ado-aw-scripts/ado-script/exec-context-pr-synth.js";
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
/// `Step::Task(NodeTool@0)` plus a `Step::Bash` for the curl + sha256
/// + unzip pipeline.
fn install_and_download_steps_typed() -> Vec<Step> {
    let version = env!("CARGO_PKG_VERSION");
    let install = {
        let mut t =
            TaskStep::new("NodeTool@0", "Install Node.js 20.x").with_input("versionSpec", "20.x");
        t.timeout = Some(std::time::Duration::from_secs(300));
        t.condition = Some(Condition::Succeeded);
        t
    };
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

/// The resolver step that expands runtime import markers in the agent prompt.
fn resolver_step_typed() -> Step {
    let script = format!(
        "set -eo pipefail\n\
         node '{IMPORT_EVAL_PATH}' /tmp/awf-tools/agent-prompt.md --base \"$(Build.SourcesDirectory)\"\n"
    );
    Step::Bash(
        BashStep::new("Resolve runtime imports (agent prompt)", script)
            .with_condition(Condition::Succeeded),
    )
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
    let envs: &[(&str, EnvValue)] = &[
        (
            "SYSTEM_ACCESSTOKEN",
            EnvValue::ado_macro("System.AccessToken")?,
        ),
        (
            "ADO_COLLECTION_URI",
            EnvValue::ado_macro("System.CollectionUri")?,
        ),
        ("ADO_PROJECT", EnvValue::ado_macro("System.TeamProject")?),
        ("ADO_REPO_ID", EnvValue::ado_macro("Build.Repository.ID")?),
        ("BUILD_REASON", EnvValue::ado_macro("Build.Reason")?),
        (
            "BUILD_REPOSITORY_PROVIDER",
            EnvValue::ado_macro("Build.Repository.Provider")?,
        ),
        (
            "BUILD_SOURCEBRANCH",
            EnvValue::ado_macro("Build.SourceBranch")?,
        ),
        ("PR_SYNTH_SPEC", EnvValue::literal(spec_b64)),
    ];
    for (k, v) in envs {
        step = step.with_env(*k, v.clone());
    }
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
        // System phase: ado-script's NodeTool@0 install + bundle download +
        // resolver step must complete BEFORE any user-facing Runtime
        // extension (e.g. NodeExtension) runs. Otherwise our Node 20
        // install would prepend onto PATH after the user's pinned Node,
        // silently overriding the user's choice for the rest of the
        // Agent job. By running first, our install lives only during the
        // brief window before the user's Runtime install, and the
        // resolver step inside that window picks up our Node 20.
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
            setup_steps.extend(install_and_download_steps_typed());
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
        if import_active || self.exec_context_pr_active {
            agent_prepare_steps.extend(install_and_download_steps_typed());
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
            pr_trigger_for_synth: None,
        }
    }

    #[test]
    fn name_and_phase() {
        let ext = ext_with(None, None, true);
        assert_eq!(ext.name(), "ado-script");
        // System phase ensures NodeTool@0 install + bundle download +
        // resolver run BEFORE user-facing Runtime extensions (e.g. the
        // Node runtime), so the user's pinned Node version wins on PATH
        // for the rest of the Agent job.
        assert_eq!(ext.phase(), ExtensionPhase::System);
    }

    #[test]
    fn declarations_setup_steps_empty_without_gate() {
        let ext = ext_with(None, None, true);
        let fm: FrontMatter = serde_yaml::from_str("name: t\ndescription: t").unwrap();
        let ctx = CompileContext::for_test(&fm);
        assert!(ext.declarations(&ctx).unwrap().setup_steps.is_empty());
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
                assert_eq!(t.task, "NodeTool@0");
                assert_eq!(t.display_name, "Install Node.js 20.x");
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
            pr_trigger_for_synth: Some(PrTriggerConfig {
                branches: Some(BranchFilter {
                    include: vec!["main".into()],
                    exclude: vec![],
                }),
                paths: None,
                filters: None,
                ..Default::default()
            }),
        };
        let fm: FrontMatter = serde_yaml::from_str("name: t\ndescription: t").unwrap();
        let ctx = CompileContext::for_test(&fm);
        let steps = ext.declarations(&ctx).unwrap().setup_steps;
        assert_eq!(steps.len(), 3, "install + download + synthPr");
        assert!(matches!(&steps[0], Step::Task(t) if t.task == "NodeTool@0"));
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
            pr_trigger_for_synth: Some(PrTriggerConfig {
                branches: Some(BranchFilter {
                    include: vec!["main".into()],
                    exclude: vec![],
                }),
                paths: None,
                filters: None,
                ..Default::default()
            }),
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
        let steps = install_and_download_steps_typed();
        match &steps[1] {
            Step::Bash(download) => assert!(
                download.script.contains("-d /tmp/ado-aw-scripts/"),
                "download step must unzip to /tmp/ado-aw-scripts/"
            ),
            other => panic!("expected download bash step, got {other:?}"),
        }
    }

    #[test]
    fn declarations_agent_prepare_steps_empty_when_inlined_imports_true() {
        let ext = ext_with(None, None, true);
        let fm: FrontMatter = serde_yaml::from_str("name: t\ndescription: t").unwrap();
        let ctx = CompileContext::for_test(&fm);
        assert!(
            ext.declarations(&ctx)
                .unwrap()
                .agent_prepare_steps
                .is_empty()
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
        assert!(matches!(&steps[0], Step::Task(t) if t.task == "NodeTool@0"));
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
        assert!(ext.declarations(&ctx).is_err());
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
            pr_trigger_for_synth: Some(PrTriggerConfig {
                branches: Some(BranchFilter {
                    include: vec!["main".into()],
                    exclude: vec![],
                }),
                paths: None,
                filters: None,
                ..Default::default()
            }),
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
        assert_eq!(parts.len(), 2, "Or(Build.Reason!=PR, prGate.SHOULD_RUN=true)");
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
        assert!(matches!(
            &and_parts[1],
            Condition::Ne(Expr::StepOutput(out), Expr::Literal(lit))
                if out.step.as_str() == "synthPr" && out.name == "AW_SYNTHETIC_PR" && lit == "true"
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
    /// is active. Mirrors `setup_steps_empty_without_gate` /
    /// `prepare_steps_empty_when_inlined_imports_true` for the typed
    /// path.
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
    /// `Step::Task(NodeTool@0)` followed by `Step::Bash` (download)
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
            Step::Task(t) => assert_eq!(t.task, "NodeTool@0"),
            other => panic!("expected Task(NodeTool@0), got {other:?}"),
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
            pr_trigger_for_synth: Some(PrTriggerConfig {
                branches: Some(BranchFilter {
                    include: vec!["main".into()],
                    exclude: vec![],
                }),
                paths: None,
                filters: None,
                ..Default::default()
            }),
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

    /// `declarations()` agent_prepare_steps surfaces typed install +
    /// download + resolver when runtime imports are active.
    #[test]
    fn declarations_agent_prepare_steps_typed_with_runtime_imports() {
        let ext = ext_with(None, None, false);
        let fm: FrontMatter = serde_yaml::from_str("name: t\ndescription: t").unwrap();
        let ctx = CompileContext::for_test(&fm);
        let decl = ext.declarations(&ctx).unwrap();
        assert_eq!(decl.agent_prepare_steps.len(), 3);
        match &decl.agent_prepare_steps[0] {
            Step::Task(t) => assert_eq!(t.task, "NodeTool@0"),
            other => panic!("expected Task, got {other:?}"),
        }
        match &decl.agent_prepare_steps[2] {
            Step::Bash(b) => assert_eq!(b.display_name, "Resolve runtime imports (agent prompt)"),
            other => panic!("expected Bash(resolver), got {other:?}"),
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
