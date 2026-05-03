/**
 * Self-cancel the current build when the gate decides not to run the
 * agent. Best-effort: missing env vars or API failures emit a warning
 * but do not throw.
 */
import type { GateSpec } from "../shared/types.gen.js";
import { cancelBuild } from "../shared/ado-client.js";
import { logWarning, addBuildTag } from "../shared/vso-logger.js";

export async function selfCancelIfRequested(spec: GateSpec): Promise<void> {
  addBuildTag(`${spec.context.tag_prefix}:skipped`);

  const project = process.env.ADO_PROJECT ?? "";
  const buildIdRaw = process.env.ADO_BUILD_ID ?? "";
  const buildId = buildIdRaw ? Number(buildIdRaw) : NaN;
  if (!project || !Number.isFinite(buildId)) {
    logWarning("Cannot self-cancel: missing ADO_PROJECT or ADO_BUILD_ID env vars");
    return;
  }

  try {
    await cancelBuild(project, buildId);
  } catch (e) {
    logWarning(`Self-cancel failed: ${(e as Error).message}`);
  }
}
