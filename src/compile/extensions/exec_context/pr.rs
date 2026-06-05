//! PR-context contributor (v6.2).
//!
//! Materialises a small, focused set of PR signals for PR-triggered builds
//! and appends a tailored prompt fragment directly to the agent prompt file.
//!
//! ## Artefacts
//!
//! On success (merge-base resolved):
//!
//! - `aw-context/pr/base.sha` — target merge-base SHA
//! - `aw-context/pr/head.sha` — PR head SHA
//!
//! On failure (validation or merge-base resolution failed):
//!
//! - `aw-context/pr/error.txt` — one-line failure reason
//!
//! No `status.txt`, no `metadata.txt`, no `changed-files*.txt`, no
//! `diff.patch`, no `head-files/`/`base-files/`. The agent runs `git diff`
//! itself against `$BASE..$HEAD` (the workspace's `.git/objects/` are
//! already populated by the precompute fetch).
//!
//! ## Prompt injection
//!
//! The PR contributor does NOT use the `prompt_supplement` trait method.
//! Instead, the precompute step appends the success-or-failure prompt
//! fragment directly to `/tmp/awf-tools/agent-prompt.md` (which is
//! created earlier by the "Prepare agent prompt" step in `base.yml`,
//! ahead of the `{{ prepare_steps }}` marker). This is the same
//! mechanism gh-aw uses for its built-in PR prompt section, adapted for
//! ado-aw's per-extension prepare-step model.
//!
//! Short identifiers (`PR_ID`, `PROJECT`, `REPO`) are interpolated into
//! the prompt heredoc via unquoted `<<EOF` so the agent sees literal
//! values ("This is PR #4242 in project 'OneBranch' / repository
//! 'awesome-repo'.") and example ADO MCP tool calls with the right
//! arguments pre-filled.
//!
//! Long opaque SHAs stay as files (`base.sha`, `head.sha`) because the
//! agent reuses them across many shell commands and transcription risk
//! on a 40-char hex string is non-trivial.
//!
//! ## Trust boundary
//!
//! - `SYSTEM_ACCESSTOKEN` is mapped only into THIS step's `env:` block,
//!   never the agent step's env.
//! - The bearer is injected via `GIT_CONFIG_COUNT` / `GIT_CONFIG_KEY_0` /
//!   `GIT_CONFIG_VALUE_0` env vars (NOT via `git -c http.extraheader=...`
//!   on argv), so the token never appears in process listings.
//! - The token is never written to `.git/config`; `persistCredentials`
//!   is never `true`; no checkout override is emitted.
//! - The step is gated by `condition: eq(variables['Build.Reason'],
//!   'PullRequest')` so it never runs on non-PR builds.

use crate::compile::extensions::CompileContext;
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
        // The PR contributor is meaningful ONLY when `on.pr` is
        // configured: the prepare step is gated at runtime by
        // `Build.Reason == 'PullRequest'`, and `required_bash_commands`
        // (which extends the agent's bash allow-list with 7 git
        // commands) is a compile-time artifact. Without `on.pr`, the
        // step would be dead code AND we would silently widen the
        // agent's bash surface for no runtime benefit.
        //
        // So `on.pr` is REQUIRED. `pr.enabled` is then an opt-out
        // switch (defaults to true, set false to suppress). An
        // explicit `pr.enabled: true` without `on.pr` is treated
        // the same as the default (i.e. inactive); we do not honour
        // it as an unconditional activate-anyway.
        if ctx.front_matter.pr_trigger().is_none() {
            return false;
        }
        match self.config.explicit_enabled() {
            Some(false) => false,
            Some(true) | None => true,
        }
    }

    fn prepare_step(&self, _ctx: &CompileContext) -> String {
        // The bash below intentionally uses `set -uo pipefail` (no `-e`):
        // we want to capture failures into `pr/error.txt` and the failure
        // prompt branch rather than abort the step. The agent-prompt
        // file gets either the success or failure fragment, never both.
        //
        // The prompt heredoc uses UNQUOTED `<<EOF` (without single
        // quotes around the delimiter) so `${PR_ID}` / `${PROJECT}` /
        // `${REPO}` expand into literal values that the agent sees in
        // the prompt text. References that should stay literal in the
        // prompt (e.g. `$BASE`, `$HEAD` for the agent to use later)
        // are escaped with `\$`.
        r#"- bash: |
    set -uo pipefail

    AW_CONTEXT_DIR="${BUILD_SOURCESDIRECTORY:-$PWD}/aw-context"
    AW_PR_DIR="$AW_CONTEXT_DIR/pr"
    AGENT_PROMPT="/tmp/awf-tools/agent-prompt.md"

    # Hard-fail on infra-level errors (read-only workspace, missing
    # parent dir, etc.) BEFORE the soft `fail()` machinery is even
    # defined. Without this, `set -uo pipefail` (no `-e`) would
    # silently swallow a failed `mkdir`, then `fail()` would itself
    # fail to write `error.txt`, and the step would exit 0 with
    # nothing staged AND no failure signal in the agent prompt.
    # That's strictly worse than a hard build break, which loudly
    # tells the operator that the pipeline configuration is broken.
    mkdir -p "$AW_PR_DIR" || { echo "[aw-context] fatal: could not create $AW_PR_DIR (check BUILD_SOURCESDIRECTORY permissions)"; exit 1; }
    rm -f "$AW_PR_DIR/error.txt" "$AW_PR_DIR/base.sha" "$AW_PR_DIR/head.sha" 2>/dev/null || true

    PR_ID="${SYSTEM_PULLREQUEST_PULLREQUESTID:-}"
    PR_TARGET_BRANCH="${SYSTEM_PULLREQUEST_TARGETBRANCH:-}"
    PROJECT="${SYSTEM_TEAMPROJECT:-}"
    REPO="${BUILD_REPOSITORY_NAME:-}"

    fail() {
      local _reason="$1"
      echo "$_reason" > "$AW_PR_DIR/error.txt"
      {
        printf '\n'
        printf '## PR context\n\n'
        printf 'PR #%s in project %s / repository %s -- context preparation failed.\n' \
          "${PR_ID:-<unknown>}" "${PROJECT:-<unknown>}" "${REPO:-<unknown>}"
        printf 'Reason: %s\n\n' "$_reason"
        # shellcheck disable=SC2016
        printf 'Local `git diff` is unavailable (the PR merge-base could not be resolved\n'
        printf 'within the depth budget, or PR identifier validation failed). You may\n'
        printf 'still call Azure DevOps MCP using the identifiers above\n'
        # shellcheck disable=SC2016
        printf '(e.g. `repo_get_pull_request_by_id`), OR surface the failure and stop.\n'
        printf 'Do NOT produce an empty review or pretend the PR has no changes.\n'
      } >> "$AGENT_PROMPT"
      echo "[aw-context] pr context preparation failed: $_reason"
      exit 0
    }

    # Strict allowlist validation of identifiers before interpolating
    # them into the agent prompt. These come from ADO predefined vars
    # (infra-set, not PR-author-controlled) but defence-in-depth is
    # cheap and prevents future regressions if ADO ever changes its
    # variable population.
    if [[ ! "$PR_ID" =~ ^[0-9]+$ ]]; then
      fail "PR identifier validation failed (PR_ID='$PR_ID' is not a positive integer)."
    fi
    if [[ ! "$PROJECT" =~ ^[A-Za-z0-9._\ -]+$ ]]; then
      fail "PR identifier validation failed (PROJECT='$PROJECT' contains disallowed characters)."
    fi
    if [[ ! "$REPO" =~ ^[A-Za-z0-9._-]+$ ]]; then
      fail "PR identifier validation failed (REPO='$REPO' contains disallowed characters)."
    fi
    if [ -z "$PR_TARGET_BRANCH" ]; then
      fail "System.PullRequest.TargetBranch is empty; cannot resolve merge-base."
    fi
    # Defence-in-depth: PR_TARGET_BRANCH comes from ADO infra
    # (System.PullRequest.TargetBranch) but we interpolate it into a
    # git refspec ("+refs/heads/...:refs/remotes/origin/..."), so
    # validate it with the same posture as the other identifiers.
    # Allowed: refs/heads/-prefixed branches with `[A-Za-z0-9._/-]`
    # name characters (the same character set git itself accepts for
    # branch names in `refs/heads/<name>`).
    if [[ ! "$PR_TARGET_BRANCH" =~ ^[A-Za-z0-9._/-]+$ ]]; then
      fail "PR identifier validation failed (PR_TARGET_BRANCH='$PR_TARGET_BRANCH' contains disallowed characters)."
    fi

    PR_TARGET_SHORT="${PR_TARGET_BRANCH#refs/heads/}"

    # Bearer header is injected via GIT_CONFIG_* env vars (not via
    # `git -c` on argv) so the token does NOT appear in process
    # listings.
    if [ -n "${SYSTEM_ACCESSTOKEN:-}" ]; then
      git_fetch() {
        GIT_CONFIG_COUNT=1 \
        GIT_CONFIG_KEY_0="http.extraheader" \
        GIT_CONFIG_VALUE_0="Authorization: bearer ${SYSTEM_ACCESSTOKEN}" \
          git fetch "$@"
      }
    else
      git_fetch() { git fetch "$@"; }
    fi

    fetch_target_at_depth() {
      local _depth_arg="$1"
      git_fetch --no-tags "$_depth_arg" origin \
        "+refs/heads/${PR_TARGET_SHORT}:refs/remotes/origin/${PR_TARGET_SHORT}" \
        >/dev/null 2>&1
    }

    HEAD_SHA="$(git rev-parse HEAD 2>/dev/null || true)"
    # `wc -w` itself returns 0 on empty input ("0"), so a `|| echo 0`
    # fallback is unreachable. Default to "0" via parameter expansion
    # when the upstream command produced no output at all.
    PARENTS="$(git rev-list --parents -n 1 HEAD 2>/dev/null | wc -w)"
    PARENTS="${PARENTS:-0}"

    BASE_SHA=""
    HEAD_TIP_SHA=""
    if [ "${PARENTS:-0}" -ge 3 ]; then
      # ADO synthetic merge commit: HEAD^1 is the target tip at PR
      # preparation time, HEAD^2 is the PR head. Compute the true
      # common ancestor (`merge-base HEAD^1 HEAD^2`) so `BASE_SHA`
      # has the SAME semantics as the progressive-deepening path.
      # If we used HEAD^1 directly, `git diff $BASE..$HEAD` would
      # silently produce a narrower "vs current target tip" change set
      # in the synthetic-merge case and a broader "since branch point"
      # change set in the deepening case — agents would see different
      # diffs depending on ADO's checkout mode.
      MERGE_P1="$(git rev-parse 'HEAD^1' 2>/dev/null || true)"
      MERGE_P2="$(git rev-parse 'HEAD^2' 2>/dev/null || true)"
      HEAD_TIP_SHA="$MERGE_P2"
      if [ -n "$MERGE_P1" ] && [ -n "$MERGE_P2" ]; then
        BASE_SHA="$(git merge-base "$MERGE_P1" "$MERGE_P2" 2>/dev/null || true)"
        # Fall back to the target tip if merge-base cannot resolve
        # within the workspace's shallow history (rare on a synthetic
        # merge commit since both parents are present, but be safe).
        if [ -z "$BASE_SHA" ]; then
          BASE_SHA="$MERGE_P1"
        fi
      fi
    else
      HEAD_TIP_SHA="$HEAD_SHA"
      # Progressive deepening: stop ONLY when merge-base actually
      # resolves against the deepened target ref.
      for _depth_arg in --depth=200 --depth=500 --depth=2000 --unshallow; do
        fetch_target_at_depth "$_depth_arg" || continue
        BASE_SHA="$(git merge-base "origin/${PR_TARGET_SHORT}" HEAD 2>/dev/null || true)"
        if [ -n "$BASE_SHA" ]; then
          break
        fi
      done
    fi

    if [ -z "$BASE_SHA" ] || [ -z "$HEAD_TIP_SHA" ]; then
      fail "Could not resolve base/head SHAs after progressive deepening of '$PR_TARGET_BRANCH' (HEAD=$HEAD_SHA, parents=$PARENTS)."
    fi

    printf '%s' "$BASE_SHA"     > "$AW_PR_DIR/base.sha"
    printf '%s' "$HEAD_TIP_SHA" > "$AW_PR_DIR/head.sha"

    # Success prompt: use printf calls (not a heredoc) because YAML
    # block-scalar indentation interacts badly with bash heredoc
    # terminator-at-column-0 requirements. Format-string substitution
    # (%s) keeps ${PR_ID}/${PROJECT}/${REPO} interpolation safe even
    # if they contained characters that would be unsafe in a `cat`
    # argument; the strict identifier regex above already restricts
    # them to alphanumerics, '.', '_', '-' (and space, for project).
    {
      printf '\n'
      printf '## PR context\n\n'
      printf "This is PR #%s in project '%s' / repository '%s'.\n\n" "$PR_ID" "$PROJECT" "$REPO"
      printf 'For git inspection (offline; objects are already in the workspace):\n\n'
      # shellcheck disable=SC2016
      printf '  BASE=$(cat aw-context/pr/base.sha)\n'
      # shellcheck disable=SC2016
      printf '  HEAD=$(cat aw-context/pr/head.sha)\n'
      # shellcheck disable=SC2016
      printf '  git diff --stat $BASE..$HEAD          # size budget first\n'
      # shellcheck disable=SC2016
      printf '  git diff --name-status $BASE..$HEAD   # changed files\n'
      # shellcheck disable=SC2016
      printf '  git diff $BASE..$HEAD                 # full patch\n'
      # shellcheck disable=SC2016
      printf '  git diff $BASE..$HEAD -- <path>       # per-file\n'
      # shellcheck disable=SC2016
      printf '  git show $HEAD:<path>                  # file at PR head\n'
      # shellcheck disable=SC2016
      printf '  git log  $BASE..$HEAD                 # PR commits\n\n'
      # shellcheck disable=SC2016
      printf 'For Azure DevOps MCP (if the `azure-devops` tool is configured),\n'
      printf 'the PR identifiers are pre-filled in these example calls:\n\n'
      printf "  repo_get_pull_request_by_id(project='%s', repositoryId='%s', pullRequestId=%s)\n" \
        "$PROJECT" "$REPO" "$PR_ID"
      printf "  repo_list_pull_request_threads(project='%s', repositoryId='%s', pullRequestId=%s)\n" \
        "$PROJECT" "$REPO" "$PR_ID"
      printf "  repo_create_pull_request_thread(project='%s', repositoryId='%s', pullRequestId=%s, comments=[...], status='active')\n" \
        "$PROJECT" "$REPO" "$PR_ID"
    } >> "$AGENT_PROMPT"

    echo "[aw-context] pr context staged: base=$BASE_SHA head=$HEAD_TIP_SHA pr=$PR_ID project=$PROJECT repo=$REPO"
  env:
    SYSTEM_ACCESSTOKEN: $(System.AccessToken)
  displayName: "Stage PR execution context (aw-context/pr/*)"
  condition: eq(variables['Build.Reason'], 'PullRequest')"#
            .to_string()
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
