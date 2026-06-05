//! PR-context contributor.
//!
//! Materialises `aw-context/pr/*` for PR-triggered builds. Handles the
//! four footguns documented in issue #860:
//!
//! 1. **Shallow checkout** — `checkout: self` does a depth-1 fetch by
//!    default. We progressively deepen the two refs we need
//!    (`System.PullRequest.SourceBranch` / `TargetBranch`) with
//!    `--depth=200 → 500 → 2000 → --unshallow` until `merge-base`
//!    resolves.
//! 2. **`origin/<target>` not fetched** — we explicitly fetch the
//!    two PR refs into `refs/remotes/origin/<short>` so they exist
//!    as local remote-tracking refs.
//! 3. **Persisted creds variability** — we don't depend on
//!    `persistCredentials: true`. The step injects
//!    `SYSTEM_ACCESSTOKEN` only into its own env block (never into the
//!    agent step) and wraps git fetches with
//!    `git -c http.extraheader=Authorization: bearer …`. The token
//!    is never written to `.git/config` and never reaches the agent
//!    sandbox.
//! 4. **Synthetic merge commit fragility** — we detect whether `HEAD`
//!    is a merge commit (`git rev-list --parents -n 1 HEAD` → two
//!    parents) and pick the right pair of SHAs to diff. If it isn't
//!    a merge commit, we fall back to `merge-base origin/<target> HEAD`.
//!
//! The step writes `aw-context/pr/status.txt` as the single source of
//! truth for whether the agent has usable context. Agents read this
//! file first and fall back to "no PR context" behaviour if the
//! status is anything other than `OK`.

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

    /// Resolve the effective scope as a single space-separated string
    /// suitable for splatting into a bash array literal. Each pathspec
    /// has already been sanitised by `PrContextConfig::sanitize_config_fields`.
    fn scope_for_bash(&self) -> String {
        self.config
            .scope
            .iter()
            .map(|s| format!("'{}'", s.replace('\'', "'\\''")))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

impl ContextContributor for PrContextContributor {
    fn name(&self) -> &str {
        "pr"
    }

    fn should_activate(&self, ctx: &CompileContext) -> bool {
        // Activates when on.pr is set, unless explicitly disabled via
        // execution-context.pr.enabled: false. Per-trigger gating at
        // compile time keeps the prepare step out of the generated
        // YAML entirely when it can't possibly apply — and the
        // step-level ADO condition handles the runtime case of a
        // user manually queuing a non-PR build of a PR-triggered
        // pipeline.
        let pr_trigger_configured = ctx.front_matter.pr_trigger().is_some();
        let explicit = self.config.explicit_enabled();
        match explicit {
            Some(true) => true,
            Some(false) => false,
            None => pr_trigger_configured,
        }
    }

    fn prepare_step(&self, _ctx: &CompileContext) -> String {
        let unified = self.config.unified_or_default();
        let max_diff_bytes = self.config.max_diff_bytes_or_default();
        let snapshots = self.config.snapshots_or_default();
        let scope_array_literal = self.scope_for_bash();

        // Snapshots are gated at compile time (not via a bash
        // conditional on a compile-time constant) to avoid
        // shellcheck SC2050 ("constant expression").
        let snapshot_block = if snapshots {
            r#"
    mkdir -p "$AW_PR_DIR/head-files" "$AW_PR_DIR/base-files"
    while IFS=$'\t' read -r STATUS REST; do
      case "$STATUS" in
        A|M|T|R*|C*)
          FILE="${REST##*$'\t'}"
          DEST_DIR="$AW_PR_DIR/head-files/$(dirname -- "$FILE")"
          mkdir -p "$DEST_DIR" 2>/dev/null || true
          git show "${HEAD_TIP_SHA}:${FILE}" \
            > "$AW_PR_DIR/head-files/${FILE}" 2>/dev/null || true
          ;;
        D)
          FILE="$REST"
          DEST_DIR="$AW_PR_DIR/base-files/$(dirname -- "$FILE")"
          mkdir -p "$DEST_DIR" 2>/dev/null || true
          git show "${BASE_SHA}:${FILE}" \
            > "$AW_PR_DIR/base-files/${FILE}" 2>/dev/null || true
          ;;
      esac
    done < "$AW_PR_DIR/changed-files-in-scope.txt""#
        } else {
            ""
        };

        // The prepare step is gated by an ADO `condition:` so it
        // no-ops on non-PR builds (e.g. manual queue of a PR-triggered
        // pipeline). Inside the step, `set -uo pipefail` is enabled
        // and every git fetch goes through the in-step `git_fetch`
        // wrapper. The wrapper injects the bearer header via Git's
        // `GIT_CONFIG_*` env vars (NOT via `git -c` on argv) so the
        // token never appears in a process listing.
        //
        // `set -e` is intentionally NOT used: we want to capture
        // failures into `pr/error.txt` + `pr/status.txt` rather than
        // abort the step. The agent reads `status.txt` first.
        format!(
            r#"- bash: |
    set -uo pipefail

    AW_CONTEXT_DIR="${{BUILD_SOURCESDIRECTORY:-$PWD}}/aw-context"
    AW_PR_DIR="$AW_CONTEXT_DIR/pr"

    mkdir -p "$AW_CONTEXT_DIR" "$AW_PR_DIR"
    : > "$AW_CONTEXT_DIR/status.txt"
    : > "$AW_CONTEXT_DIR/trigger.txt"
    : > "$AW_CONTEXT_DIR/metadata.txt"
    : > "$AW_PR_DIR/status.txt"
    : > "$AW_PR_DIR/metadata.txt"
    rm -f "$AW_CONTEXT_DIR/error.txt" "$AW_PR_DIR/error.txt" 2>/dev/null || true

    echo "pr"  > "$AW_CONTEXT_DIR/trigger.txt"
    {{
      echo "build_id=${{BUILD_BUILDID:-}}"
      echo "build_reason=${{BUILD_REASON:-}}"
      echo "repository=${{BUILD_REPOSITORY_NAME:-}}"
      echo "source_branch=${{BUILD_SOURCEBRANCH:-}}"
    }} > "$AW_CONTEXT_DIR/metadata.txt"

    PR_ID="${{SYSTEM_PULLREQUEST_PULLREQUESTID:-}}"
    PR_SOURCE_BRANCH="${{SYSTEM_PULLREQUEST_SOURCEBRANCH:-}}"
    PR_TARGET_BRANCH="${{SYSTEM_PULLREQUEST_TARGETBRANCH:-}}"

    if [ -z "$PR_ID" ] || [ -z "$PR_TARGET_BRANCH" ]; then
      echo "NO_PR_CONTEXT" > "$AW_PR_DIR/status.txt"
      echo "OK"            > "$AW_CONTEXT_DIR/status.txt"
      echo "System.PullRequest.* variables missing; build is not a PR." \
        > "$AW_PR_DIR/error.txt"
      echo "[aw-context] not a PR build; skipping PR context."
      exit 0
    fi

    PR_TARGET_SHORT="${{PR_TARGET_BRANCH#refs/heads/}}"
    PR_SOURCE_SHORT="${{PR_SOURCE_BRANCH#refs/heads/}}"

    # Bearer header is injected via GIT_CONFIG_* env vars (not via
    # `git -c` on argv) so the token does NOT appear in process
    # listings. The variables are scoped to each git_fetch call's
    # subshell via env-prefixing.
    if [ -n "${{SYSTEM_ACCESSTOKEN:-}}" ]; then
      git_fetch() {{
        GIT_CONFIG_COUNT=1 \
        GIT_CONFIG_KEY_0="http.extraheader" \
        GIT_CONFIG_VALUE_0="Authorization: bearer ${{SYSTEM_ACCESSTOKEN}}" \
          git fetch "$@"
      }}
    else
      git_fetch() {{ git fetch "$@"; }}
    fi

    # Fetch a single ref at one depth (no probing). Returns 0 on
    # successful fetch, non-zero on failure. Callers chain depths
    # themselves.
    fetch_ref_at_depth() {{
      local _short="$1"
      local _depth_arg="$2"
      git_fetch --no-tags "$_depth_arg" origin \
        "+refs/heads/${{_short}}:refs/remotes/origin/${{_short}}" \
        >/dev/null 2>&1
    }}

    # Fetch the source branch (best-effort; only needed for
    # informational head-tip resolution, not for the diff).
    if [ -n "$PR_SOURCE_SHORT" ]; then
      fetch_ref_at_depth "$PR_SOURCE_SHORT" "--depth=200" || \
        fetch_ref_at_depth "$PR_SOURCE_SHORT" "--depth=2000" || \
        fetch_ref_at_depth "$PR_SOURCE_SHORT" "--unshallow" || \
        echo "fetch failed for source branch: $PR_SOURCE_BRANCH" \
          >> "$AW_PR_DIR/error.txt"
    fi

    # Detect merge commit shape up front: if HEAD is a 2-parent merge
    # commit, base = HEAD^1 / head = HEAD^2 and we don't need the
    # target branch fetched for the diff. Otherwise, fetch the target
    # branch progressively until `merge-base` actually resolves.
    HEAD_SHA="$(git rev-parse HEAD 2>/dev/null || true)"
    PARENTS="$(git rev-list --parents -n 1 HEAD 2>/dev/null | wc -w || echo 0)"

    BASE_SHA=""
    HEAD_TIP_SHA=""
    if [ "${{PARENTS:-0}}" -ge 3 ]; then
      BASE_SHA="$(git rev-parse "HEAD^1" 2>/dev/null || true)"
      HEAD_TIP_SHA="$(git rev-parse "HEAD^2" 2>/dev/null || true)"
    else
      HEAD_TIP_SHA="$HEAD_SHA"
      # Progressive deepening: only stop when merge-base ACTUALLY
      # resolves against the deepened target ref. A successful fetch
      # at shallow depth is not enough on its own.
      for _depth_arg in --depth=200 --depth=500 --depth=2000 --unshallow; do
        fetch_ref_at_depth "$PR_TARGET_SHORT" "$_depth_arg" || continue
        BASE_SHA="$(git merge-base "origin/${{PR_TARGET_SHORT}}" HEAD 2>/dev/null || true)"
        if [ -n "$BASE_SHA" ]; then
          break
        fi
      done
    fi

    if [ -z "$BASE_SHA" ] || [ -z "$HEAD_TIP_SHA" ]; then
      echo "DIFF_RESOLUTION_FAILED" > "$AW_PR_DIR/status.txt"
      echo "OK"                    > "$AW_CONTEXT_DIR/status.txt"
      {{
        echo "Could not resolve base/head SHAs after progressive deepening."
        echo "HEAD_SHA=$HEAD_SHA"
        echo "PARENTS=$PARENTS"
        echo "PR_TARGET_BRANCH=$PR_TARGET_BRANCH"
      }} >> "$AW_PR_DIR/error.txt"
      exit 0
    fi

    {{
      echo "pr_id=$PR_ID"
      echo "source_branch=$PR_SOURCE_BRANCH"
      echo "target_branch=$PR_TARGET_BRANCH"
      echo "base_sha=$BASE_SHA"
      echo "head_sha=$HEAD_TIP_SHA"
    }} > "$AW_PR_DIR/metadata.txt"

    SCOPE=({scope})

    # Track failures of the core diff commands. We do NOT swallow
    # errors with `|| true` — any failure here means we cannot trust
    # the staged context, and we must signal that to the agent via
    # status.txt.
    CTX_OK=1
    if ! git diff --name-status "$BASE_SHA" "$HEAD_TIP_SHA" \
        > "$AW_PR_DIR/changed-files.txt" 2>>"$AW_PR_DIR/error.txt"; then
      CTX_OK=0
    fi

    if [ "${{#SCOPE[@]}}" -gt 0 ]; then
      if ! git diff --name-status "$BASE_SHA" "$HEAD_TIP_SHA" -- "${{SCOPE[@]}}" \
          > "$AW_PR_DIR/changed-files-in-scope.txt" 2>>"$AW_PR_DIR/error.txt"; then
        CTX_OK=0
      fi
    else
      cp "$AW_PR_DIR/changed-files.txt" "$AW_PR_DIR/changed-files-in-scope.txt"
    fi

    DIFF_TMP="$(mktemp)"
    DIFF_OK=1
    if [ "${{#SCOPE[@]}}" -gt 0 ]; then
      git diff --find-renames -U{unified} "$BASE_SHA" "$HEAD_TIP_SHA" \
        -- "${{SCOPE[@]}}" > "$DIFF_TMP" 2>>"$AW_PR_DIR/error.txt" \
        || DIFF_OK=0
    else
      git diff --find-renames -U{unified} "$BASE_SHA" "$HEAD_TIP_SHA" \
        > "$DIFF_TMP" 2>>"$AW_PR_DIR/error.txt" \
        || DIFF_OK=0
    fi
    if [ "$DIFF_OK" -eq 0 ]; then
      CTX_OK=0
    fi
    DIFF_SIZE="$(wc -c < "$DIFF_TMP" | tr -d ' ')"
    if [ "${{DIFF_SIZE:-0}}" -gt {max_diff_bytes} ]; then
      head -c {max_diff_bytes} "$DIFF_TMP" > "$AW_PR_DIR/diff.patch"
      printf '\n--- TRUNCATED at %d bytes; full diff suppressed ---\n' \
        {max_diff_bytes} >> "$AW_PR_DIR/diff.patch"
    else
      cp "$DIFF_TMP" "$AW_PR_DIR/diff.patch"
    fi
    rm -f "$DIFF_TMP"
{snapshot_block}

    if [ "$CTX_OK" -eq 1 ]; then
      echo "OK" > "$AW_PR_DIR/status.txt"
    else
      echo "CONTEXT_GENERATION_FAILED" > "$AW_PR_DIR/status.txt"
    fi
    echo "OK" > "$AW_CONTEXT_DIR/status.txt"
    echo "[aw-context] pr context staged: base=$BASE_SHA head=$HEAD_TIP_SHA diff_bytes=$DIFF_SIZE ctx_ok=$CTX_OK"
  env:
    SYSTEM_ACCESSTOKEN: $(System.AccessToken)
  displayName: "Stage PR execution context (aw-context/pr/*)"
  condition: eq(variables['Build.Reason'], 'PullRequest')"#,
            scope = scope_array_literal,
            unified = unified,
            max_diff_bytes = max_diff_bytes,
            snapshot_block = snapshot_block,
        )
    }

    fn prompt_fragment(&self) -> String {
        // Always appended. On non-PR runs the directory is absent and
        // the agent's `cat status.txt` call returns NO_PR_CONTEXT (or
        // the file is missing, which the agent must also treat as
        // "no PR context").
        //
        // The fragment deliberately does NOT mention env vars: the
        // ado-aw env-var injection channel rejects ADO `$(...)`
        // expressions, so all PR metadata flows through files. This
        // is a single source of truth and avoids per-channel drift.
        r#"
### PR context (when triggered by a pull request)

A pipeline step stages execution context for you under `aw-context/`,
relative to your working directory.

**Read `aw-context/pr/status.txt` first** — it's the single source of
truth for whether usable PR context is available:

- `OK` — `aw-context/pr/*` is fully populated. Prefer reading those files
  over running `git fetch` / `git diff` yourself.
- `NO_PR_CONTEXT` — this build is not a PR. The `aw-context/pr/` directory
  may exist but its contents are not meaningful. Skip PR-specific logic.
- `DIFF_RESOLUTION_FAILED` — the precompute step ran but could not resolve
  the base / head SHAs. See `aw-context/pr/error.txt` for the reason.
  Surface this in your output rather than silently producing an empty review.
- `CONTEXT_GENERATION_FAILED` — base / head SHAs resolved, but at least one
  of the `git diff` commands that populates `changed-files.txt`,
  `changed-files-in-scope.txt`, or `diff.patch` failed. The metadata file
  is still trustworthy, but the diff / file-list contents may be empty or
  partial. See `aw-context/pr/error.txt`.

If `aw-context/pr/status.txt` does not exist at all, treat it as
`NO_PR_CONTEXT` and skip PR-specific logic.

When `status.txt` is `OK`, you can rely on these files:

| File                                          | Contents                              |
|-----------------------------------------------|---------------------------------------|
| `aw-context/pr/metadata.txt`                  | `pr_id`, `source_branch`, `target_branch`, `base_sha`, `head_sha` |
| `aw-context/pr/changed-files.txt`             | Full `git diff --name-status` output  |
| `aw-context/pr/changed-files-in-scope.txt`    | Name-status restricted to the configured scope |
| `aw-context/pr/diff.patch`                    | Unified diff, scoped, capped (may end with a `--- TRUNCATED …` marker) |
| `aw-context/pr/head-files/<path>`             | Post-PR snapshots of added / modified files |
| `aw-context/pr/base-files/<path>`             | Pre-PR snapshots of deleted files     |
"#
        .to_string()
    }

    fn agent_env_vars(&self) -> Vec<(String, String)> {
        // None: the compiler's agent-env-var channel rejects ADO
        // `$(...)` expressions, and we'd otherwise need to bounce
        // everything through pipeline output variables. Files are
        // a single source of truth and avoid that complexity. The
        // agent reads metadata from `aw-context/pr/metadata.txt`.
        vec![]
    }

    fn required_bash_commands(&self) -> Vec<String> {
        // None: the agent reads `aw-context/*` via its normal
        // file-reading mechanism (e.g. the `edit` tool or native
        // copilot file reads), not via shell. We deliberately do
        // NOT inject `cat`/`ls` into the bash allow-list — that
        // would silently widen the agent's shell capability when
        // the user has restricted or disabled bash.
        vec![]
    }
}
