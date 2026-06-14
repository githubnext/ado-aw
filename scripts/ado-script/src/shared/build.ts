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
import {
  BuildResult,
  BuildStatus,
  type Build,
  type BuildArtifact,
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

/**
 * Find the most recent successful (completed + result=Succeeded) build of
 * `definitionId` on `branchName`, EXCLUDING the current build (`currentBuildId`).
 *
 * Used by the `ci-push` contributor (Stage 3) to resolve the
 * "previous green build" SHA so the agent can scope its diff to
 * "what landed since the last green run on this branch".
 *
 * Returns `null` when no qualifying build exists — first ever push,
 * branch was just created, last green build was age-pruned, etc.
 * Callers translate `null` into the contributor's empty-history
 * failure fragment (do NOT fabricate "diff is empty").
 *
 * Implementation note: ADO's `getBuilds` accepts both `resultFilter`
 * and `statusFilter`. We pass both — `Succeeded` AND `Completed` —
 * because a build in progress can technically have `result=Succeeded`
 * if it was partially graded; we want runs that are fully settled.
 * `top=2` because the current build may already be in the result set
 * (especially if the build's status was Succeeded by the time the
 * agent's prepare step runs, which it usually is — the contributor
 * runs in the Agent job, which is downstream of the build's earlier
 * stages). We filter out the current build below.
 */
export async function listLastSuccessfulBuildOnBranch(
  project: string,
  definitionId: number,
  branchName: string,
  currentBuildId: number,
): Promise<Build | null> {
  return withRetry("listLastSuccessfulBuildOnBranch", async () => {
    const build = await (await getWebApi()).getBuildApi();
    // SDK signature for getBuilds is long — only the first six
    // positional params we use are relevant:
    //   getBuilds(project, definitions?, queues?, buildNumber?,
    //             minTime?, maxTime?, requestedFor?, reasonFilter?,
    //             statusFilter?, resultFilter?, tagFilters?,
    //             properties?, top?, continuationToken?, maxBuildsPerDefinition?,
    //             deletedFilter?, queryOrder?, branchName?, ...)
    const builds = await build.getBuilds(
      project,
      [definitionId],
      undefined, // queues
      undefined, // buildNumber
      undefined, // minTime
      undefined, // maxTime
      undefined, // requestedFor
      undefined, // reasonFilter
      BuildStatus.Completed,
      BuildResult.Succeeded,
      undefined, // tagFilters
      undefined, // properties
      2, // top
      undefined, // continuationToken
      undefined, // maxBuildsPerDefinition
      undefined, // deletedFilter
      undefined, // queryOrder (default is finishTimeDescending)
      branchName,
    );
    const candidates = builds.filter((b) => b.id !== currentBuildId);
    return candidates.length > 0 ? (candidates[0] ?? null) : null;
  });
}
