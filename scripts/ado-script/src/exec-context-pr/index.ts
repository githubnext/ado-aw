/**
 * exec-context-pr — Stage PR signals for the agent on PR-triggered
 * Azure DevOps builds.
 *
 * Invoked from the Agent job's prepare phase by `pr.rs::prepare_step`
 * (in the Rust compiler). Reads PR identifiers and the workspace
 * checkout from ADO env vars, resolves the merge-base, and stages:
 *
 *   - aw-context/pr/base.sha — target merge-base SHA
 *   - aw-context/pr/head.sha — PR head SHA
 *   - aw-context/pr/error.txt — present only on failure
 *
 * It also appends a tailored success-or-failure fragment to the agent
 * prompt at `/tmp/awf-tools/agent-prompt.md`.
 *
 * Trust boundary:
 *   - The bearer (`SYSTEM_ACCESSTOKEN`) is passed in via env from the
 *     wrapping prepare-step's `env:` block; it is NOT visible to the
 *     agent step.
 *   - The bearer is then passed to the spawned `git` child process via
 *     `GIT_CONFIG_COUNT` / `GIT_CONFIG_KEY_0` / `GIT_CONFIG_VALUE_0`
 *     env vars (see `git.ts::bearerEnv`). It never appears in argv and
 *     is never written to `.git/config`.
 *   - This is a strict improvement over the v6.2 bash implementation
 *     where the bearer lived in the wrapping shell's env (shared with
 *     `fail()`, regex validation, etc.); here it is confined to the
 *     git subprocess's env exclusively.
 */
import { mkdirSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";

import { bearerEnv } from "./git.js";
import { resolveMergeBase } from "./merge-base.js";
import { appendToAgentPrompt, failureFragment, successFragment } from "./prompt.js";
import { isIdentifierError, validateIdentifiers } from "./validate.js";

const DEFAULT_AGENT_PROMPT_PATH = "/tmp/awf-tools/agent-prompt.md";

/**
 * Resolve the agent prompt file path. Production: hard-coded
 * `/tmp/awf-tools/agent-prompt.md` (created by base.yml's
 * "Prepare agent prompt" step). Tests may override via the
 * `AW_AGENT_PROMPT_FILE` env var.
 *
 * SECURITY NOTE: `AW_AGENT_PROMPT_FILE` is a *test-only* seam. The
 * compiled step's `env:` block (see `pr.rs::prepare_step`) only maps
 * `SYSTEM_ACCESSTOKEN`, but Node still inherits the full pipeline
 * environment, so a pipeline variable named `AW_AGENT_PROMPT_FILE`
 * would silently redirect where the prompt fragment is appended.
 * This requires pipeline-variable write access (already a high-trust
 * capability) and only changes where the *contributor's own* prompt
 * fragment lands — it cannot expand the agent's read surface. If we
 * ever need to harden this further, the right move is to read the
 * default path from a const here and stop honouring the env var
 * outside of unit-test mode (e.g. gate on `process.env.NODE_ENV`).
 */
function agentPromptPath(env: NodeJS.ProcessEnv): string {
  return env.AW_AGENT_PROMPT_FILE && env.AW_AGENT_PROMPT_FILE.length > 0
    ? env.AW_AGENT_PROMPT_FILE
    : DEFAULT_AGENT_PROMPT_PATH;
}

function awContextDir(env: NodeJS.ProcessEnv): string {
  const root = env.BUILD_SOURCESDIRECTORY && env.BUILD_SOURCESDIRECTORY.length > 0
    ? env.BUILD_SOURCESDIRECTORY
    : process.cwd();
  return join(root, "aw-context");
}

function awPrDir(env: NodeJS.ProcessEnv): string {
  return join(awContextDir(env), "pr");
}

function writeFailure(
  prDir: string,
  promptPath: string,
  reason: string,
  partial: { prId?: string; project?: string; repo?: string },
): void {
  writeFileSync(join(prDir, "error.txt"), reason, "utf8");
  appendToAgentPrompt(promptPath, failureFragment(reason, partial));
  // Match the bash version's stdout posture: log the failure but exit
  // 0 so the rest of the pipeline can proceed (the agent will see the
  // failure prompt and surface it).
  process.stdout.write(`[aw-context] pr context preparation failed: ${reason}\n`);
}

export function main(env: NodeJS.ProcessEnv = process.env): number {
  const prDir = awPrDir(env);
  const promptPath = agentPromptPath(env);

  // Hard-fail on infra-level errors (read-only workspace, missing
  // parent dir, etc.). Without this, a failed `mkdirSync` would
  // throw, and the wrapping bash step would propagate exit 1. This
  // matches the v6.2 bash `mkdir -p "$AW_PR_DIR" || { ...; exit 1; }`
  // hard-fail.
  try {
    mkdirSync(prDir, { recursive: true });
  } catch (err) {
    process.stderr.write(
      `[aw-context] fatal: could not create ${prDir} (check BUILD_SOURCESDIRECTORY permissions): ${(err as Error).message}\n`,
    );
    return 1;
  }

  // Clean any stale artefacts from a prior run (re-runs in local
  // dev, agent retries, etc.). `force: true` makes the call a no-op
  // when the file doesn't exist.
  for (const f of ["error.txt", "base.sha", "head.sha"]) {
    rmSync(join(prDir, f), { force: true });
  }

  const idsOrErr = validateIdentifiers(env);
  if (isIdentifierError(idsOrErr)) {
    writeFailure(prDir, promptPath, idsOrErr.reason, {
      prId: env.SYSTEM_PULLREQUEST_PULLREQUESTID,
      project: env.SYSTEM_TEAMPROJECT,
      repo: env.BUILD_REPOSITORY_NAME,
    });
    return 0;
  }
  const ids = idsOrErr;

  const fetchEnv = bearerEnv(env.SYSTEM_ACCESSTOKEN);
  const mb = resolveMergeBase(ids.targetShort, fetchEnv);
  if (!mb.ok) {
    writeFailure(prDir, promptPath, mb.reason, { prId: ids.prId, project: ids.project, repo: ids.repo });
    return 0;
  }

  writeFileSync(join(prDir, "base.sha"), mb.baseSha, "utf8");
  writeFileSync(join(prDir, "head.sha"), mb.headSha, "utf8");

  appendToAgentPrompt(promptPath, successFragment(ids));

  process.stdout.write(
    `[aw-context] pr context staged: base=${mb.baseSha} head=${mb.headSha} pr=${ids.prId} project=${ids.project} repo=${ids.repo}\n`,
  );
  return 0;
}

// Top-level invocation. `process.exit` is called here (not in `main`)
// so tests can call `main(env)` and inspect the return value without
// terminating the test process.
const exitCode = main();
process.exit(exitCode);
