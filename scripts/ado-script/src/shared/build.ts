/**
 * Shared ADO Build REST helpers.
 *
 * Introduced in Stage 2 of the execution-context contributor build-out
 * (plan.md): the `pipeline` contributor needs to fetch metadata about
 * an upstream build (status, source branch/SHA, artifact list) so the
 * agent can decide what to do based on the run that triggered it.
 *
 * This module sits beside `ado-client.ts` (which carries the existing
 * gate-evaluator's PR + cancelBuild helpers) and is the natural home
 * for the build-related operations the contributors will grow into:
 *
 *   Stage 2 — pipeline: getBuildById + listArtifacts
 *   Stage 3 — ci-push: listSuccessfulBuildsForBranch (next caller)
 *   Stage 6 — pr.checks: listBuildsForPr (third caller, two-caller rule
 *                                          already satisfied by Stage 3)
 *
 * All exports preserve the same posture as `ado-client.ts`:
 *   - withRetry wrapper for transient 5xx + per-attempt timeout
 *   - returns the native `Build` interface objects from
 *     `azure-devops-node-api/interfaces/BuildInterfaces`; callers
 *     pick the fields they care about
 *   - failure modes throw — callers translate to the per-contributor
 *     failure-fragment path
 */
import { getWebApi } from "./auth.js";
import { withRetry } from "./ado-client.js";
import type {
  Build,
  BuildArtifact,
} from "azure-devops-node-api/interfaces/BuildInterfaces.js";

/**
 * Fetch a single build by its numeric ID.
 *
 * Used by the `pipeline` contributor (Stage 2) to read the upstream
 * triggering build's status, source SHA, source branch, and other
 * top-level metadata.
 *
 * The `Build` shape includes hundreds of optional fields; callers
 * read only the ones they need. Common fields used by the contributors:
 *   - `id` (number)
 *   - `status` (BuildStatus enum)
 *   - `result` (BuildResult enum: succeeded/failed/canceled/...)
 *   - `sourceVersion` (string SHA)
 *   - `sourceBranch` (string ref, e.g. `refs/heads/main`)
 *   - `definition.name` (string)
 */
export async function getBuildById(
  project: string,
  buildId: number,
): Promise<Build> {
  return withRetry("getBuildById", async () => {
    const build = await (await getWebApi()).getBuildApi();
    return build.getBuild(project, buildId);
  });
}

/**
 * List the artifacts produced by a build.
 *
 * Returns the artifact INDEX (name, type, resource URL) — bytes are
 * NOT downloaded. The `pipeline` contributor stages this list as
 * `aw-context/pipeline/upstream-artifacts.json` so the agent can
 * decide whether to download specific artifacts via the ADO MCP
 * tool (`build_download_artifact`) or `az pipelines runs artifact
 * download`. See `docs/execution-context.md` for the full layout.
 */
export async function listArtifacts(
  project: string,
  buildId: number,
): Promise<BuildArtifact[]> {
  return withRetry("listArtifacts", async () => {
    const build = await (await getWebApi()).getBuildApi();
    return build.getArtifacts(project, buildId);
  });
}
