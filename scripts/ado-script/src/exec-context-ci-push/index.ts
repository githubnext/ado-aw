/**
 * exec-context-ci-push — Stage "since last green build on this branch"
 * diff context for non-PR push builds (Stage 3 of the exec-context
 * contributor build-out — see plan.md).
 *
 * Invoked from the Agent job's prepare phase by `ci_push.rs::prepare_step`
 * (in the Rust compiler). Steps:
 *   1. Validate identifiers (definition id, current SHA, source branch).
 *   2. Call `listLastSuccessfulBuildOnBranch(project, defId, branch, currentId)`
 *      to find the previous green build's SHA.
 *   3. `git fetch --depth=...` progressively until both `current` and
 *      `previous` SHAs are reachable in the workspace's clone.
 *   4. Compute `git merge-base previous current` → `base.sha`.
 *   5. Stage `current-sha`, `previous-sha`, `base.sha`, `commits.txt`,
 *      `changed-files.txt` under `aw-context/ci-push/`.
 *   6. Append a `## CI-push context` fragment to the agent prompt.
 *
 * On any failure (no previous green build, depth-budget exhausted,
 * REST error, etc.) the bundle stages `error.txt` and appends a
 * failure-fragment that tells the agent NOT to claim "diff is empty"
 * when the diff couldn't actually be resolved.
 *
 * Trust boundary:
 *   - SYSTEM_ACCESSTOKEN is the bearer for both the REST lookup AND
 *     the `git fetch` deepening (passed via `bearerEnv` from
 *     shared/git.ts → spawned git child via GIT_CONFIG_* env vars).
 *   - Bearer never reaches argv, never written to `.git/config`,
 *     never visible to the agent process.
 *   - All staged artefacts are git output / build infrastructure
 *     metadata — no user-controlled HTML or free-text fields.
 */
import { mkdirSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";

import { listLastSuccessfulBuildOnBranch } from "../shared/build.js";
import { bearerEnv, gitOk, runGit } from "../shared/git.js";
import { appendToAgentPrompt } from "../shared/prompt.js";
import { sanitizeForPrompt } from "../shared/validate.js";

const DEFAULT_AGENT_PROMPT_PATH = "/tmp/awf-tools/agent-prompt.md";
const SHA40_RE = /^[0-9a-f]{40}$/i;

function agentPromptPath(env: NodeJS.ProcessEnv): string {
  return env.AW_AGENT_PROMPT_FILE && env.AW_AGENT_PROMPT_FILE.length > 0
    ? env.AW_AGENT_PROMPT_FILE
    : DEFAULT_AGENT_PROMPT_PATH;
}

function awCiPushDir(env: NodeJS.ProcessEnv): string {
  const root =
    env.BUILD_SOURCESDIRECTORY && env.BUILD_SOURCESDIRECTORY.length > 0
      ? env.BUILD_SOURCESDIRECTORY
      : process.cwd();
  return join(root, "aw-context", "ci-push");
}

export type IdentifiersOk = {
  ok: true;
  project: string;
  definitionId: number;
  buildId: number;
  currentSha: string;
  branchRef: string;
};
export type IdentifiersErr = { ok: false; reason: string };
export type Identifiers = IdentifiersOk | IdentifiersErr;

export function validateIdentifiers(env: NodeJS.ProcessEnv): Identifiers {
  const project = env.SYSTEM_TEAMPROJECT ?? "";
  const definitionIdRaw = env.SYSTEM_DEFINITIONID ?? "";
  const buildIdRaw = env.BUILD_BUILDID ?? "";
  const currentSha = env.BUILD_SOURCEVERSION ?? "";
  const branchRef = env.BUILD_SOURCEBRANCH ?? "";

  if (project.length === 0) {
    return { ok: false, reason: "SYSTEM_TEAMPROJECT is empty" };
  }
  if (!/^[0-9]+$/.test(definitionIdRaw)) {
    return {
      ok: false,
      reason: `SYSTEM_DEFINITIONID='${sanitizeForPrompt(definitionIdRaw)}' is not a positive integer`,
    };
  }
  if (!/^[0-9]+$/.test(buildIdRaw)) {
    return {
      ok: false,
      reason: `BUILD_BUILDID='${sanitizeForPrompt(buildIdRaw)}' is not a positive integer`,
    };
  }
  if (!SHA40_RE.test(currentSha)) {
    return {
      ok: false,
      reason: `BUILD_SOURCEVERSION='${sanitizeForPrompt(currentSha)}' is not a 40-char hex SHA`,
    };
  }
  if (branchRef.length === 0) {
    return { ok: false, reason: "BUILD_SOURCEBRANCH is empty" };
  }
  return {
    ok: true,
    project,
    definitionId: Number(definitionIdRaw),
    buildId: Number(buildIdRaw),
    currentSha,
    branchRef,
  };
}

/** Try fetching `sha` from origin at progressively larger depths until
 * `git cat-file -e sha` succeeds (i.e. the commit is reachable in the
 * local object DB). Returns true on success, false on depth-budget
 * exhaustion. Mirrors the deepening pattern in shared/merge-base.ts. */
function ensureShaReachable(
  sha: string,
  bearerEnvVars: Record<string, string>,
): boolean {
  // Fast path: the workspace might already have the SHA (e.g. when
  // fetchDepth is generous in the pipeline).
  if (gitOk(["cat-file", "-e", sha]) !== null) return true;

  const depths = ["200", "500", "2000"];
  for (const depth of depths) {
    const r = runGit(
      ["fetch", "--no-tags", `--depth=${depth}`, "origin", sha],
      bearerEnvVars,
    );
    if (r.status === 0 && gitOk(["cat-file", "-e", sha]) !== null) {
      return true;
    }
  }
  // Last-ditch attempt: --unshallow (no-op if already unshallow).
  const r = runGit(["fetch", "--no-tags", "--unshallow", "origin"], bearerEnvVars);
  if (r.status === 0 && gitOk(["cat-file", "-e", sha]) !== null) {
    return true;
  }
  return false;
}

export function successFragment(args: {
  currentSha: string;
  previousSha: string;
  baseSha: string;
  branchRef: string;
  commitsCount: number;
  changedFilesCount: number;
}): string {
  const {
    currentSha,
    previousSha,
    baseSha,
    branchRef,
    commitsCount,
    changedFilesCount,
  } = args;
  return [
    "",
    "## CI-push context",
    "",
    `This build is on branch \`${sanitizeForPrompt(branchRef)}\` at \`${currentSha}\`.`,
    `The previous successful build of this pipeline on this branch was at \`${previousSha}\`.`,
    `${commitsCount} new commit(s) introduced ${changedFilesCount} change(s) since then.`,
    "",
    "For git inspection (offline; objects already in workspace):",
    "",
    "  PREV=$(cat aw-context/ci-push/previous-sha)",
    "  CURR=$(cat aw-context/ci-push/current-sha)",
    "  cat aw-context/ci-push/commits.txt           # one-line commit summaries",
    "  cat aw-context/ci-push/changed-files.txt     # name + status",
    "  git diff $PREV..$CURR -- <path>              # full per-file diff",
    "  git log $PREV..$CURR                         # full commit messages",
    "",
    `merge-base resolved to \`${baseSha}\` (used internally — usually equal to PREV).`,
    "",
  ].join("\n");
}

export function failureFragment(reason: string): string {
  return [
    "",
    "## CI-push context",
    "",
    `CI-push context preparation failed.`,
    `Reason: ${sanitizeForPrompt(reason, 200)}`,
    "",
    "Local `git diff` against a previous-green base is unavailable.",
    "Do NOT claim the diff is empty or that no changes landed.",
    "Surface the failure (e.g. via `report_incomplete`) or fall back to",
    "inspecting the current commit's standalone diff.",
    "",
  ].join("\n");
}

function writeFailure(dir: string, promptPath: string, reason: string): void {
  writeFileSync(join(dir, "error.txt"), reason, "utf8");
  appendToAgentPrompt(promptPath, failureFragment(reason));
  process.stdout.write(
    `[aw-context] ci-push context preparation failed: ${reason}\n`,
  );
}

export async function main(
  env: NodeJS.ProcessEnv = process.env,
): Promise<number> {
  const dir = awCiPushDir(env);
  const promptPath = agentPromptPath(env);

  try {
    mkdirSync(dir, { recursive: true });
  } catch (err) {
    process.stderr.write(
      `[aw-context] fatal: could not create ${dir} (check BUILD_SOURCESDIRECTORY permissions): ${(err as Error).message}\n`,
    );
    return 1;
  }

  for (const f of [
    "error.txt",
    "current-sha",
    "previous-sha",
    "base.sha",
    "commits.txt",
    "changed-files.txt",
  ]) {
    rmSync(join(dir, f), { force: true });
  }

  const idsOrErr = validateIdentifiers(env);
  if (!idsOrErr.ok) {
    writeFailure(dir, promptPath, idsOrErr.reason);
    return 0;
  }
  const ids = idsOrErr;

  let previousBuild;
  try {
    previousBuild = await listLastSuccessfulBuildOnBranch(
      ids.project,
      ids.definitionId,
      ids.branchRef,
      ids.buildId,
    );
  } catch (err) {
    writeFailure(
      dir,
      promptPath,
      `failed to query last successful build for definition ${ids.definitionId} on '${ids.branchRef}': ${(err as Error).message}`,
    );
    return 0;
  }

  if (previousBuild === null || !previousBuild.sourceVersion) {
    writeFailure(
      dir,
      promptPath,
      `no previous successful build of definition ${ids.definitionId} found on '${ids.branchRef}' (first build, or all previous builds failed/were pruned)`,
    );
    return 0;
  }
  const previousSha = previousBuild.sourceVersion;
  if (!SHA40_RE.test(previousSha)) {
    writeFailure(
      dir,
      promptPath,
      `previous build's sourceVersion='${sanitizeForPrompt(previousSha)}' is not a 40-char hex SHA`,
    );
    return 0;
  }

  const bearerEnvVars = bearerEnv(env.SYSTEM_ACCESSTOKEN);
  if (!ensureShaReachable(previousSha, bearerEnvVars)) {
    writeFailure(
      dir,
      promptPath,
      `could not fetch previous SHA ${previousSha} after progressive deepening; depth-budget exhausted`,
    );
    return 0;
  }
  if (!ensureShaReachable(ids.currentSha, bearerEnvVars)) {
    writeFailure(
      dir,
      promptPath,
      `could not fetch current SHA ${ids.currentSha} after progressive deepening`,
    );
    return 0;
  }

  const baseSha = gitOk(["merge-base", previousSha, ids.currentSha]);
  if (!baseSha || !SHA40_RE.test(baseSha)) {
    writeFailure(
      dir,
      promptPath,
      `git merge-base ${previousSha} ${ids.currentSha} did not return a 40-char hex SHA`,
    );
    return 0;
  }

  const commitsResult = runGit([
    "log",
    "--oneline",
    `${previousSha}..${ids.currentSha}`,
  ]);
  const changedResult = runGit([
    "diff",
    "--name-status",
    `${previousSha}..${ids.currentSha}`,
  ]);
  const commits = commitsResult.status === 0 ? commitsResult.stdout : "";
  const changed = changedResult.status === 0 ? changedResult.stdout : "";
  const commitsCount = commits.split("\n").filter((l) => l.length > 0).length;
  const changedFilesCount = changed.split("\n").filter((l) => l.length > 0).length;

  writeFileSync(join(dir, "current-sha"), ids.currentSha, "utf8");
  writeFileSync(join(dir, "previous-sha"), previousSha, "utf8");
  writeFileSync(join(dir, "base.sha"), baseSha, "utf8");
  writeFileSync(join(dir, "commits.txt"), commits, "utf8");
  writeFileSync(join(dir, "changed-files.txt"), changed, "utf8");

  appendToAgentPrompt(
    promptPath,
    successFragment({
      currentSha: ids.currentSha,
      previousSha,
      baseSha,
      branchRef: ids.branchRef,
      commitsCount,
      changedFilesCount,
    }),
  );

  process.stdout.write(
    `[aw-context] ci-push context staged: current=${ids.currentSha} previous=${previousSha} commits=${commitsCount} files=${changedFilesCount}\n`,
  );
  return 0;
}

// `spawnSync` import is unused at file-level — kept for future
// expansion. Tree-shaken by ncc.
void spawnSync;

if (
  typeof process !== "undefined" &&
  process.argv[1] &&
  process.argv[1] === fileURLToPath(import.meta.url)
) {
  main()
    .then((rc) => process.exit(rc))
    .catch((err) => {
      process.stderr.write(
        `[aw-context] ci-push fatal: ${(err as Error).message}\n`,
      );
      process.exit(1);
    });
}
