/**
 * exec-context-schedule — Stage "since last run of this pipeline on
 * this branch" diff context for scheduled builds (Stage 5 of the
 * exec-context contributor build-out — see plan.md).
 *
 * Mechanically very similar to `exec-context-ci-push`:
 *   - Reads the same identifiers (project, definition id, current
 *     SHA, source branch).
 *   - Calls the same `listLastSuccessfulBuildOnBranch` helper to
 *     find the previous green build's SHA.
 *   - Uses the same `git fetch` deepening to ensure SHAs are
 *     reachable, computes merge-base, stages the same five files.
 *
 * Why a separate bundle rather than sharing exec-context-ci-push?
 *   - Different runtime gate / stage path naming
 *     (aw-context/schedule/ vs aw-context/ci-push/) keeps
 *     agent-facing layouts intentional. An agent that opts into
 *     ci-push should not be silently affected by scheduled runs.
 *   - Allows future divergence (e.g. a future iteration could add
 *     `previous-run-time` + `window-hours` files unique to the
 *     schedule contributor without touching ci-push).
 */
import { mkdirSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

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

function awScheduleDir(env: NodeJS.ProcessEnv): string {
  const root =
    env.BUILD_SOURCESDIRECTORY && env.BUILD_SOURCESDIRECTORY.length > 0
      ? env.BUILD_SOURCESDIRECTORY
      : process.cwd();
  return join(root, "aw-context", "schedule");
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

function ensureShaReachable(
  sha: string,
  bearerEnvVars: Record<string, string>,
): boolean {
  if (gitOk(["cat-file", "-e", sha]) !== null) return true;
  for (const depth of ["200", "500", "2000"]) {
    const r = runGit(
      ["fetch", "--no-tags", `--depth=${depth}`, "origin", sha],
      bearerEnvVars,
    );
    if (r.status === 0 && gitOk(["cat-file", "-e", sha]) !== null) return true;
  }
  const r = runGit(["fetch", "--no-tags", "--unshallow", "origin"], bearerEnvVars);
  return r.status === 0 && gitOk(["cat-file", "-e", sha]) !== null;
}

export function successFragment(args: {
  currentSha: string;
  previousSha: string;
  branchRef: string;
  commitsCount: number;
  changedFilesCount: number;
  previousRunTime: string | undefined;
}): string {
  const { currentSha, previousSha, branchRef, commitsCount, changedFilesCount, previousRunTime } =
    args;
  return [
    "",
    "## Schedule context",
    "",
    `This scheduled build is on branch \`${sanitizeForPrompt(branchRef)}\` at \`${currentSha}\`.`,
    previousRunTime
      ? `The previous successful scheduled run was at \`${sanitizeForPrompt(previousRunTime)}\` ` +
        `(SHA \`${previousSha}\`).`
      : `The previous successful scheduled run was at SHA \`${previousSha}\`.`,
    `${commitsCount} new commit(s) introduced ${changedFilesCount} change(s) since then.`,
    "",
    "For git inspection (offline; objects already in workspace):",
    "",
    "  PREV=$(cat aw-context/schedule/previous-run-sha)",
    "  CURR=$(cat aw-context/schedule/current-sha)",
    "  cat aw-context/schedule/commits.txt",
    "  cat aw-context/schedule/changed-files.txt",
    "  git diff $PREV..$CURR",
    "  git log $PREV..$CURR",
    "",
  ].join("\n");
}

export function failureFragment(reason: string): string {
  return [
    "",
    "## Schedule context",
    "",
    "Schedule context preparation failed.",
    `Reason: ${sanitizeForPrompt(reason, 200)}`,
    "",
    "Local `git diff` against a previous-run base is unavailable.",
    "Do NOT claim the diff is empty or that no changes landed.",
    "",
  ].join("\n");
}

function writeFailure(dir: string, promptPath: string, reason: string): void {
  writeFileSync(join(dir, "error.txt"), reason, "utf8");
  appendToAgentPrompt(promptPath, failureFragment(reason));
  process.stdout.write(
    `[aw-context] schedule context preparation failed: ${reason}\n`,
  );
}

export async function main(env: NodeJS.ProcessEnv = process.env): Promise<number> {
  const dir = awScheduleDir(env);
  const promptPath = agentPromptPath(env);

  try {
    mkdirSync(dir, { recursive: true });
  } catch (err) {
    process.stderr.write(
      `[aw-context] fatal: could not create ${dir}: ${(err as Error).message}\n`,
    );
    return 1;
  }
  for (const f of [
    "error.txt",
    "current-sha",
    "previous-run-sha",
    "previous-run-time",
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
      `failed to query last successful build: ${(err as Error).message}`,
    );
    return 0;
  }
  if (previousBuild === null || !previousBuild.sourceVersion) {
    writeFailure(
      dir,
      promptPath,
      `no previous successful build of definition ${ids.definitionId} found on '${ids.branchRef}'`,
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
  const previousRunTime =
    previousBuild.finishTime instanceof Date
      ? previousBuild.finishTime.toISOString()
      : typeof previousBuild.finishTime === "string"
      ? previousBuild.finishTime
      : undefined;

  const bearerEnvVars = bearerEnv(env.SYSTEM_ACCESSTOKEN);
  if (!ensureShaReachable(previousSha, bearerEnvVars)) {
    writeFailure(
      dir,
      promptPath,
      `could not fetch previous SHA ${previousSha} after deepening`,
    );
    return 0;
  }
  if (!ensureShaReachable(ids.currentSha, bearerEnvVars)) {
    writeFailure(
      dir,
      promptPath,
      `could not fetch current SHA ${ids.currentSha} after deepening`,
    );
    return 0;
  }

  const commits = runGit([
    "log",
    "--oneline",
    `${previousSha}..${ids.currentSha}`,
  ]);
  const changed = runGit([
    "diff",
    "--name-status",
    `${previousSha}..${ids.currentSha}`,
  ]);
  const commitsTxt = commits.status === 0 ? commits.stdout : "";
  const changedTxt = changed.status === 0 ? changed.stdout : "";
  const commitsCount = commitsTxt.split("\n").filter((l) => l.length > 0).length;
  const changedFilesCount = changedTxt.split("\n").filter((l) => l.length > 0).length;

  writeFileSync(join(dir, "current-sha"), ids.currentSha, "utf8");
  writeFileSync(join(dir, "previous-run-sha"), previousSha, "utf8");
  if (previousRunTime) {
    writeFileSync(join(dir, "previous-run-time"), previousRunTime, "utf8");
  }
  writeFileSync(join(dir, "commits.txt"), commitsTxt, "utf8");
  writeFileSync(join(dir, "changed-files.txt"), changedTxt, "utf8");

  appendToAgentPrompt(
    promptPath,
    successFragment({
      currentSha: ids.currentSha,
      previousSha,
      branchRef: ids.branchRef,
      commitsCount,
      changedFilesCount,
      previousRunTime,
    }),
  );
  process.stdout.write(
    `[aw-context] schedule context staged: current=${ids.currentSha} previous=${previousSha} commits=${commitsCount}\n`,
  );
  return 0;
}

if (
  typeof process !== "undefined" &&
  process.argv[1] &&
  process.argv[1] === fileURLToPath(import.meta.url)
) {
  main()
    .then((rc) => process.exit(rc))
    .catch((err) => {
      process.stderr.write(`[aw-context] schedule fatal: ${(err as Error).message}\n`);
      process.exit(1);
    });
}
