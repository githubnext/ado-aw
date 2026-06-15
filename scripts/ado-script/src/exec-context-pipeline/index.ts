/**
 * exec-context-pipeline — Stage upstream-build context for the agent
 * on `resources.pipelines`-triggered Azure DevOps builds.
 *
 * Invoked from the Agent job's prepare phase by `pipeline.rs::prepare_step`
 * (in the Rust compiler). Reads the upstream-build identifiers ADO
 * exposes via `Build.TriggeredBy.*` env vars, fetches the upstream
 * Build via the REST API, and stages:
 *
 *   - aw-context/pipeline/upstream-build-id        — numeric build id
 *   - aw-context/pipeline/upstream-source-sha      — Build.sourceVersion
 *   - aw-context/pipeline/upstream-source-branch   — Build.sourceBranch
 *   - aw-context/pipeline/upstream-status          — translated BuildResult
 *                                                    (succeeded/failed/...)
 *   - aw-context/pipeline/upstream-definition      — pipeline name
 *   - aw-context/pipeline/upstream-artifacts.json  — artifact INDEX
 *                                                    (bytes NOT downloaded)
 *
 * On failure (REST error, missing TriggeredBy env vars, etc.):
 *
 *   - aw-context/pipeline/error.txt                — one-line reason
 *
 * It also appends a tailored success-or-failure fragment under
 * `## Pipeline-completion context` to the agent prompt.
 *
 * Trust boundary:
 *   - SYSTEM_ACCESSTOKEN is passed via the wrapping prepare-step's
 *     env: block. The bundle uses it as the bearer for the Build
 *     REST API. It is NEVER written to disk, NEVER logged, and is
 *     not visible to the agent process.
 *   - All staged artefacts are infrastructure metadata (build id,
 *     status, branch ref, artifact names) — no user-controlled HTML
 *     or free-text fields.
 */
import { mkdirSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { getBuildById, listArtifacts } from "../shared/build.js";
import { appendToAgentPrompt } from "../shared/prompt.js";
import { sanitizeForPrompt } from "../shared/validate.js";

import { BuildResult } from "azure-devops-node-api/interfaces/BuildInterfaces.js";

const DEFAULT_AGENT_PROMPT_PATH = "/tmp/awf-tools/agent-prompt.md";

function agentPromptPath(env: NodeJS.ProcessEnv): string {
  return env.AW_AGENT_PROMPT_FILE && env.AW_AGENT_PROMPT_FILE.length > 0
    ? env.AW_AGENT_PROMPT_FILE
    : DEFAULT_AGENT_PROMPT_PATH;
}

function awContextDir(env: NodeJS.ProcessEnv): string {
  const root =
    env.BUILD_SOURCESDIRECTORY && env.BUILD_SOURCESDIRECTORY.length > 0
      ? env.BUILD_SOURCESDIRECTORY
      : process.cwd();
  return join(root, "aw-context");
}

function awPipelineDir(env: NodeJS.ProcessEnv): string {
  return join(awContextDir(env), "pipeline");
}

/**
 * Translate the numeric `BuildResult` enum into the canonical
 * status string we stage. Mirrors the strings used by the ADO REST
 * API JSON shape so agents that downstream-consume this can match
 * on the same canonical names.
 *
 * Unknown values (future additions to the enum, or a Build returned
 * by an old API version) collapse to `"unknown"` rather than the
 * numeric value, so the staged file is always a recognised symbol.
 */
function statusString(result: BuildResult | undefined): string {
  switch (result) {
    case BuildResult.Succeeded:
      return "succeeded";
    case BuildResult.PartiallySucceeded:
      return "partiallySucceeded";
    case BuildResult.Failed:
      return "failed";
    case BuildResult.Canceled:
      return "canceled";
    case BuildResult.None:
    case undefined:
      return "none";
    default:
      return "unknown";
  }
}

export type IdentifiersOk = {
  ok: true;
  buildId: number;
  projectId: string;
  definitionName: string;
};
export type IdentifiersErr = {
  ok: false;
  reason: string;
};
export type Identifiers = IdentifiersOk | IdentifiersErr;

/** Validate that the four `BUILD_TRIGGEREDBY_*` env vars are present
 * and well-formed. Required because the contributor's runtime gate
 * already ensures `Build.Reason == 'ResourceTrigger'`, but a
 * misconfigured pipeline (e.g. a manually-queued build that ADO
 * miscategorised) could still reach this code path with empty
 * values — fail closed in that case. */
export function validateIdentifiers(env: NodeJS.ProcessEnv): Identifiers {
  const rawId = env.BUILD_TRIGGEREDBY_BUILDID ?? "";
  const projectId = env.BUILD_TRIGGEREDBY_PROJECTID ?? "";
  const definitionName = env.BUILD_TRIGGEREDBY_DEFINITIONNAME ?? "";
  if (!/^[0-9]+$/.test(rawId)) {
    return {
      ok: false,
      reason: `BUILD_TRIGGEREDBY_BUILDID='${sanitizeForPrompt(rawId)}' is not a positive integer; cannot fetch upstream build`,
    };
  }
  if (!/^[0-9a-fA-F-]+$/.test(projectId)) {
    return {
      ok: false,
      reason: `BUILD_TRIGGEREDBY_PROJECTID='${sanitizeForPrompt(projectId)}' is not a GUID; cannot route REST call`,
    };
  }
  const buildId = Number(rawId);
  return { ok: true, buildId, projectId, definitionName };
}

export function successFragment(args: {
  buildId: number;
  definitionName: string;
  sourceBranch: string;
  sourceSha: string;
  status: string;
  artifactCount: number;
}): string {
  const { buildId, definitionName, sourceBranch, sourceSha, status, artifactCount } = args;
  return [
    "",
    "## Pipeline-completion context",
    "",
    `This build was triggered by upstream pipeline **${sanitizeForPrompt(definitionName)}** ` +
      `build #${buildId} (status: \`${status}\`).`,
    `Upstream source: \`${sanitizeForPrompt(sourceBranch)}\` at \`${sanitizeForPrompt(sourceSha)}\`.`,
    "",
    "Staged artefacts (read locally — no network needed):",
    "",
    "  - `aw-context/pipeline/upstream-build-id` — numeric build id",
    "  - `aw-context/pipeline/upstream-source-sha` — source commit SHA",
    "  - `aw-context/pipeline/upstream-source-branch` — source ref",
    "  - `aw-context/pipeline/upstream-status` — translated build result",
    "  - `aw-context/pipeline/upstream-definition` — upstream pipeline name",
    `  - \`aw-context/pipeline/upstream-artifacts.json\` — ${artifactCount} artifact(s) (INDEX only; bytes NOT downloaded)`,
    "",
    "Example ADO MCP tool calls (if the `azure-devops` tool is configured):",
    "",
    `  build_get_build_by_id(project=<upstream-project>, buildId=${buildId})`,
    `  build_list_artifacts(project=<upstream-project>, buildId=${buildId})`,
    `  build_get_log(project=<upstream-project>, buildId=${buildId}, logId=<n>)`,
    "",
    status === "succeeded"
      ? "Upstream succeeded — proceed with the task."
      : "Upstream did NOT succeed cleanly. Surface the failure (e.g. via `report_incomplete`) rather than assuming a clean state.",
    "",
  ].join("\n");
}

export function failureFragment(reason: string): string {
  return [
    "",
    "## Pipeline-completion context",
    "",
    `Pipeline-completion context preparation failed.`,
    `Reason: ${sanitizeForPrompt(reason, 200)}`,
    "",
    "Continue with whatever context you have, but do NOT invent",
    "an upstream-build status or claim the upstream succeeded.",
    "",
  ].join("\n");
}

function writeFailure(pipelineDir: string, promptPath: string, reason: string): void {
  writeFileSync(join(pipelineDir, "error.txt"), reason, "utf8");
  appendToAgentPrompt(promptPath, failureFragment(reason));
  process.stdout.write(
    `[aw-context] pipeline context preparation failed: ${reason}\n`,
  );
}

export async function main(env: NodeJS.ProcessEnv = process.env): Promise<number> {
  const pipelineDir = awPipelineDir(env);
  const promptPath = agentPromptPath(env);

  try {
    mkdirSync(pipelineDir, { recursive: true });
  } catch (err) {
    process.stderr.write(
      `[aw-context] fatal: could not create ${pipelineDir} (check BUILD_SOURCESDIRECTORY permissions): ${(err as Error).message}\n`,
    );
    return 1;
  }

  for (const f of [
    "error.txt",
    "upstream-build-id",
    "upstream-source-sha",
    "upstream-source-branch",
    "upstream-status",
    "upstream-definition",
    "upstream-artifacts.json",
  ]) {
    rmSync(join(pipelineDir, f), { force: true });
  }

  const idsOrErr = validateIdentifiers(env);
  if (!idsOrErr.ok) {
    writeFailure(pipelineDir, promptPath, idsOrErr.reason);
    return 0;
  }
  const ids = idsOrErr;

  let build;
  try {
    build = await getBuildById(ids.projectId, ids.buildId);
  } catch (err) {
    writeFailure(
      pipelineDir,
      promptPath,
      `failed to fetch upstream build ${ids.buildId} in project ${ids.projectId}: ${(err as Error).message}`,
    );
    return 0;
  }

  let artifacts: Awaited<ReturnType<typeof listArtifacts>>;
  try {
    artifacts = await listArtifacts(ids.projectId, ids.buildId);
  } catch (err) {
    writeFailure(
      pipelineDir,
      promptPath,
      `failed to list artifacts for upstream build ${ids.buildId}: ${(err as Error).message}`,
    );
    return 0;
  }

  const status = statusString(build.result);
  const sourceSha = build.sourceVersion ?? "";
  const sourceBranch = build.sourceBranch ?? "";
  const definitionName =
    build.definition?.name ?? ids.definitionName ?? "<unknown>";

  writeFileSync(join(pipelineDir, "upstream-build-id"), String(ids.buildId), "utf8");
  writeFileSync(join(pipelineDir, "upstream-source-sha"), sourceSha, "utf8");
  writeFileSync(join(pipelineDir, "upstream-source-branch"), sourceBranch, "utf8");
  writeFileSync(join(pipelineDir, "upstream-status"), status, "utf8");
  writeFileSync(join(pipelineDir, "upstream-definition"), definitionName, "utf8");
  // Strip the raw artifact bytes / large nested objects; the agent
  // calls `build_download_artifact` itself if it needs the bits.
  // We keep `id`, `name`, `source`, `resource` — the bits a human
  // would look at first.
  const artifactIndex = artifacts.map((a) => ({
    id: a.id,
    name: a.name,
    source: a.source,
    resource: a.resource,
  }));
  writeFileSync(
    join(pipelineDir, "upstream-artifacts.json"),
    JSON.stringify(artifactIndex, null, 2) + "\n",
    "utf8",
  );

  appendToAgentPrompt(
    promptPath,
    successFragment({
      buildId: ids.buildId,
      definitionName,
      sourceBranch,
      sourceSha,
      status,
      artifactCount: artifactIndex.length,
    }),
  );

  process.stdout.write(
    `[aw-context] pipeline context staged: upstream-build=${ids.buildId} status=${status} artifacts=${artifactIndex.length}\n`,
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
      process.stderr.write(`[aw-context] pipeline fatal: ${(err as Error).message}\n`);
      process.exit(1);
    });
}
