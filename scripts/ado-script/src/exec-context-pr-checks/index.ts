/**
 * exec-context-pr-checks — Stage Build Validation check results for
 * remediation agents (Stage 6 of the exec-context contributor
 * build-out — see plan.md). Extension of the PR contributor.
 *
 * Invoked from the Agent job's prepare phase by
 * `pr_checks.rs::prepare_step`. Steps:
 *
 *   1. Validate identifiers (PR id + project) from env.
 *   2. Call `listBuildsForPullRequest` to enumerate Build Validation
 *      runs whose source matches `refs/pull/<id>/merge`.
 *   3. Partition by result into failing/succeeded JSON arrays.
 *   4. Stage under `aw-context/pr/checks/` and append a prompt
 *      fragment summarising counts + listing the failing builds'
 *      log URLs.
 */
import { mkdirSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { listBuildsForPullRequest } from "../shared/build.js";
import { appendToAgentPrompt } from "../shared/prompt.js";
import { sanitizeForPrompt } from "../shared/validate.js";

import {
  BuildResult,
  type Build,
} from "azure-devops-node-api/interfaces/BuildInterfaces.js";

const DEFAULT_AGENT_PROMPT_PATH = "/tmp/awf-tools/agent-prompt.md";

function agentPromptPath(env: NodeJS.ProcessEnv): string {
  return env.AW_AGENT_PROMPT_FILE && env.AW_AGENT_PROMPT_FILE.length > 0
    ? env.AW_AGENT_PROMPT_FILE
    : DEFAULT_AGENT_PROMPT_PATH;
}

function awChecksDir(env: NodeJS.ProcessEnv): string {
  const root =
    env.BUILD_SOURCESDIRECTORY && env.BUILD_SOURCESDIRECTORY.length > 0
      ? env.BUILD_SOURCESDIRECTORY
      : process.cwd();
  return join(root, "aw-context", "pr", "checks");
}

export type Identifiers =
  | { ok: true; project: string; pullRequestId: number; currentBuildId: number }
  | { ok: false; reason: string };

export function validateIdentifiers(env: NodeJS.ProcessEnv): Identifiers {
  const project = env.SYSTEM_TEAMPROJECT ?? "";
  const prIdRaw = env.SYSTEM_PULLREQUEST_PULLREQUESTID ?? "";
  const buildIdRaw = env.BUILD_BUILDID ?? "";
  if (project.length === 0) {
    return { ok: false, reason: "SYSTEM_TEAMPROJECT is empty" };
  }
  if (!/^[0-9]+$/.test(prIdRaw)) {
    return {
      ok: false,
      reason: `SYSTEM_PULLREQUEST_PULLREQUESTID='${sanitizeForPrompt(prIdRaw)}' is not a positive integer`,
    };
  }
  if (!/^[0-9]+$/.test(buildIdRaw)) {
    return {
      ok: false,
      reason: `BUILD_BUILDID='${sanitizeForPrompt(buildIdRaw)}' is not a positive integer`,
    };
  }
  return {
    ok: true,
    project,
    pullRequestId: Number(prIdRaw),
    currentBuildId: Number(buildIdRaw),
  };
}

/** Translate a numeric BuildResult into our canonical string set. */
function resultToString(r: BuildResult | undefined): string {
  switch (r) {
    case BuildResult.Succeeded:
      return "succeeded";
    case BuildResult.PartiallySucceeded:
      return "partiallySucceeded";
    case BuildResult.Failed:
      return "failed";
    case BuildResult.Canceled:
      return "canceled";
    default:
      return "none";
  }
}

/** Distill a Build into a stable shape an agent can quickly scan. */
function summariseBuild(b: Build): {
  id: number | undefined;
  buildNumber: string | undefined;
  definition: string | undefined;
  status: string;
  result: string;
  sourceVersion: string | undefined;
  startTime: string | undefined;
  finishTime: string | undefined;
  url: string | undefined;
} {
  return {
    id: b.id,
    buildNumber: b.buildNumber,
    definition: b.definition?.name,
    status: typeof b.status === "number" ? String(b.status) : "unknown",
    result: resultToString(b.result),
    sourceVersion: b.sourceVersion,
    startTime: b.startTime instanceof Date ? b.startTime.toISOString() : undefined,
    finishTime: b.finishTime instanceof Date ? b.finishTime.toISOString() : undefined,
    url: b._links?.web?.href as string | undefined,
  };
}

export function successFragment(args: {
  prId: number;
  failingCount: number;
  succeededCount: number;
  failingNames: string[];
}): string {
  const { prId, failingCount, succeededCount, failingNames } = args;
  const lines = ["", "## PR checks context", ""];
  lines.push(
    `Build validations on PR #${prId}: **${failingCount} failing**, ${succeededCount} succeeded (excluding this build).`,
  );
  if (failingCount > 0) {
    lines.push("");
    lines.push("Failing builds (read `aw-context/pr/checks/failing.json` for details):");
    for (const name of failingNames.slice(0, 10)) {
      lines.push(`  - ${sanitizeForPrompt(name)}`);
    }
    lines.push("");
    lines.push(
      "Use `build_get_build_by_id` + `build_get_log` with the ids in " +
        "`failing.json` to read the failure logs. If you propose a fix, " +
        "use `update_pr` / `add_pr_comment` to surface it.",
    );
  } else {
    lines.push("");
    lines.push("All build validations are succeeding.");
  }
  lines.push("");
  return lines.join("\n");
}

export function failureFragment(reason: string): string {
  return [
    "",
    "## PR checks context",
    "",
    "PR checks context preparation failed.",
    `Reason: ${sanitizeForPrompt(reason, 200)}`,
    "",
    "Continue without the check enumeration; do NOT invent which checks",
    "are passing or failing.",
    "",
  ].join("\n");
}

export async function main(env: NodeJS.ProcessEnv = process.env): Promise<number> {
  const dir = awChecksDir(env);
  const promptPath = agentPromptPath(env);

  try {
    mkdirSync(dir, { recursive: true });
  } catch (err) {
    process.stderr.write(
      `[aw-context] fatal: could not create ${dir}: ${(err as Error).message}\n`,
    );
    return 1;
  }
  for (const f of ["failing.json", "succeeded.json", "error.txt"]) {
    rmSync(join(dir, f), { force: true });
  }

  const idsOrErr = validateIdentifiers(env);
  if (!idsOrErr.ok) {
    writeFileSync(join(dir, "error.txt"), idsOrErr.reason, "utf8");
    appendToAgentPrompt(promptPath, failureFragment(idsOrErr.reason));
    return 0;
  }
  const ids = idsOrErr;

  // ADO PR builds use `refs/pull/<id>/merge` as Build.SourceBranch.
  const prRef = `refs/pull/${ids.pullRequestId}/merge`;

  let builds;
  try {
    builds = await listBuildsForPullRequest(ids.project, prRef, ids.currentBuildId);
  } catch (err) {
    const reason = `failed to list builds for PR #${ids.pullRequestId}: ${(err as Error).message}`;
    writeFileSync(join(dir, "error.txt"), reason, "utf8");
    appendToAgentPrompt(promptPath, failureFragment(reason));
    return 0;
  }

  const summaries = builds.map(summariseBuild);
  const failing = summaries.filter(
    (s) => s.result !== "succeeded" && s.result !== "none",
  );
  const succeeded = summaries.filter((s) => s.result === "succeeded");

  writeFileSync(
    join(dir, "failing.json"),
    JSON.stringify(failing, null, 2) + "\n",
    "utf8",
  );
  writeFileSync(
    join(dir, "succeeded.json"),
    JSON.stringify(succeeded, null, 2) + "\n",
    "utf8",
  );

  appendToAgentPrompt(
    promptPath,
    successFragment({
      prId: ids.pullRequestId,
      failingCount: failing.length,
      succeededCount: succeeded.length,
      failingNames: failing.map((f) => `${f.definition ?? "<unknown>"} #${f.id ?? "?"}`),
    }),
  );
  process.stdout.write(
    `[aw-context] pr-checks context staged: pr=#${ids.pullRequestId} failing=${failing.length} succeeded=${succeeded.length}\n`,
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
      process.stderr.write(`[aw-context] pr-checks fatal: ${(err as Error).message}\n`);
      process.exit(1);
    });
}
