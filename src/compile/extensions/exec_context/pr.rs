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
}

impl PrContextContributor {
    pub(super) fn new(config: PrContextConfig) -> Self {
        Self { config }
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
        format!(
            r#"- bash: |
    set -euo pipefail
    node '{EXEC_CONTEXT_PR_PATH}'
  env:
    SYSTEM_ACCESSTOKEN: $(System.AccessToken)
    SYSTEM_PULLREQUEST_PULLREQUESTID: $(System.PullRequest.PullRequestId)
    SYSTEM_PULLREQUEST_TARGETBRANCH: $(System.PullRequest.TargetBranch)
    SYSTEM_TEAMPROJECT: $(System.TeamProject)
    BUILD_REPOSITORY_NAME: $(Build.Repository.Name)
    BUILD_SOURCESDIRECTORY: $(Build.SourcesDirectory)
  displayName: "Stage PR execution context (aw-context/pr/*)"
  condition: eq(variables['Build.Reason'], 'PullRequest')"#
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
