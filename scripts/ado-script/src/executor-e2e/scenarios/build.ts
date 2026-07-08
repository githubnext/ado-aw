/**
 * Build-scoped safe-output scenarios: add-build-tag, queue-build,
 * upload-build-attachment, upload-pipeline-artifact.
 *
 * These operate on the *current* build, so they SkipError when the harness is
 * not running inside a real ADO build (no resolvable BUILD_BUILDID).
 * queue-build additionally needs a target pipeline id (E2E_QUEUE_PIPELINE_ID).
 *
 * Test-harness module; not shipped in `ado-script.zip`.
 */
import type { Scenario, ScenarioContext } from "../scenario.js";
import { SkipError } from "../scenario.js";
import { numResult, requireEnv, stagedSafeOutputFile, strResult } from "./common.js";
import type { StagedSafeOutputFile } from "./common.js";

async function currentBuildId(ctx: ScenarioContext, tool: string): Promise<number> {
  const raw = process.env.BUILD_BUILDID?.trim();
  const id = raw ? Number(raw) : NaN;
  if (!Number.isInteger(id) || id <= 0) {
    throw new SkipError(`${tool}: no current build (BUILD_BUILDID unset); run inside a pipeline`);
  }
  // Confirm the build is reachable with our token.
  try {
    await ctx.rest.getBuild(id);
  } catch (err) {
    throw new SkipError(`${tool}: current build #${id} not reachable (${(err as Error).message})`);
  }
  return id;
}

export const addBuildTag: Scenario<{ buildId: number; tag: string }> = {
  tool: "add-build-tag",
  config: () => ({ max: 1 }),
  setup: async (ctx) => {
    const buildId = await currentBuildId(ctx, "add-build-tag");
    return { buildId, tag: `ado-aw-det-e2e-${ctx.buildId}` };
  },
  ndjson: async (_ctx, state) => ({ build_id: state.buildId, tag: state.tag }),
  assert: async (ctx, state) => {
    const tags = await ctx.rest.getBuildTags(state.buildId);
    if (!tags.includes(state.tag)) {
      throw new Error(`tag '${state.tag}' not present on build #${state.buildId}`);
    }
  },
  cleanup: async (ctx, state) => ctx.rest.removeBuildTag(state.buildId, state.tag),
};

export const queueBuild: Scenario<{ pipelineId: number; queuedBuildId?: number }> = {
  tool: "queue-build",
  config: (_ctx, state) => ({
    "allowed-pipelines": [state.pipelineId],
    "allowed-branches": ["main"],
    max: 1,
  }),
  setup: async () => {
    const pipelineId = Number(requireEnv("E2E_QUEUE_PIPELINE_ID", "queue-build"));
    if (!Number.isInteger(pipelineId) || pipelineId <= 0) {
      throw new SkipError("queue-build: E2E_QUEUE_PIPELINE_ID is not a positive integer");
    }
    return { pipelineId };
  },
  ndjson: async (_ctx, state) => ({
    pipeline_id: state.pipelineId,
    branch: "main",
    reason: "deterministic executor e2e queue-build",
  }),
  assert: async (ctx, state, record) => {
    const buildId = numResult(record, "build_id");
    state.queuedBuildId = buildId;
    await ctx.rest.getBuild(buildId); // throws if not created
  },
  cleanup: async (ctx, state) => {
    if (state.queuedBuildId !== undefined) await ctx.rest.cancelBuild(state.queuedBuildId);
  },
};

export const uploadBuildAttachment: Scenario<{
  buildId: number;
  artifactName: string;
  staged: StagedSafeOutputFile;
}> = {
  tool: "upload-build-attachment",
  config: () => ({ "allowed-extensions": ["txt"], max: 1 }),
  setup: async (ctx) => {
    const buildId = await currentBuildId(ctx, "upload-build-attachment");
    const artifactName = `ado-aw-det-${buildId}`;
    const contents = `deterministic build attachment for build ${buildId}\n`;
    return {
      buildId,
      artifactName,
      staged: stagedSafeOutputFile("upload-build-attachment", artifactName, "build-att.txt", contents),
    };
  },
  files: async (_ctx, state) => state.staged.files,
  ndjson: async (_ctx, state) => ({
    artifact_name: state.artifactName,
    ...state.staged.result,
  }),
  assert: async (_ctx, _state, record) => {
    // The executor returns an attachment_url only after a successful ADO POST.
    strResult(record, "attachment_url");
  },
  cleanup: async () => {
    /* build attachments are pruned with the build; nothing to delete */
  },
};

export const uploadPipelineArtifact: Scenario<{
  buildId: number;
  artifactName: string;
  staged: StagedSafeOutputFile;
}> = {
  tool: "upload-pipeline-artifact",
  config: () => ({ "allowed-extensions": ["txt"], max: 1 }),
  setup: async (ctx) => {
    const buildId = await currentBuildId(ctx, "upload-pipeline-artifact");
    const artifactName = `ado-aw-det-art-${buildId}`;
    const contents = `deterministic pipeline artifact for build ${buildId}\n`;
    return {
      buildId,
      artifactName,
      staged: stagedSafeOutputFile("upload-pipeline-artifact", artifactName, "artifact.txt", contents),
    };
  },
  files: async (_ctx, state) => state.staged.files,
  ndjson: async (_ctx, state) => ({
    artifact_name: state.artifactName,
    ...state.staged.result,
  }),
  assert: async (_ctx, _state, record) => {
    strResult(record, "download_url");
  },
  cleanup: async () => {
    /* pipeline artifacts are pruned with the build; nothing to delete */
  },
};

export const buildScenarios: Scenario<unknown>[] = [
  addBuildTag,
  queueBuild,
  uploadBuildAttachment,
  uploadPipelineArtifact,
];
