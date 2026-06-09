/**
 * Thin wrapper around `azure-devops-node-api` exposing only the methods
 * the gate evaluator uses, with a one-shot retry on transient (5xx) errors
 * and a hard timeout per attempt.
 */
import { getWebApi } from "./auth.js";
import { logWarning } from "./vso-logger.js";
import type {
  GitPullRequest,
  GitPullRequestIteration,
  GitPullRequestIterationChanges,
} from "azure-devops-node-api/interfaces/GitInterfaces.js";
import { PullRequestStatus } from "azure-devops-node-api/interfaces/GitInterfaces.js";
import { BuildStatus, type Build } from "azure-devops-node-api/interfaces/BuildInterfaces.js";

const SLEEP_MS = 1000;
const DEFAULT_TIMEOUT_MS = 30_000;
// Per-page size when listing iteration changes. ADO's server-side default
// is 100; we pick the same and just paginate explicitly so a PR with
// >100 changed files is not silently truncated.
const ITERATION_CHANGES_PAGE_SIZE = 100;
// Safety cap on pages to avoid an unbounded loop if the API ever fails
// to advance the skip cursor. 100 pages × 100 entries = 10 000 changed
// files, which is well beyond any realistic PR.
const MAX_ITERATION_CHANGE_PAGES = 100;

const sleep = (ms: number): Promise<void> =>
  new Promise((resolve) => setTimeout(resolve, ms));

function timeoutMs(): number {
  const raw = process.env.ADO_API_TIMEOUT_MS;
  if (!raw) return DEFAULT_TIMEOUT_MS;
  const parsed = Number(raw);
  if (!Number.isFinite(parsed) || parsed <= 0) return DEFAULT_TIMEOUT_MS;
  return parsed;
}

class TimeoutError extends Error {
  constructor(label: string, ms: number) {
    super(`${label} timed out after ${ms}ms`);
    this.name = "TimeoutError";
  }
}

function withTimeout<T>(label: string, ms: number, fn: () => Promise<T>): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    const handle = setTimeout(() => reject(new TimeoutError(label, ms)), ms);
    fn().then(
      (v) => {
        clearTimeout(handle);
        resolve(v);
      },
      (e) => {
        clearTimeout(handle);
        reject(e);
      },
    );
  });
}

function isTransient(err: unknown): boolean {
  if (err instanceof TimeoutError) return true;
  if (err && typeof err === "object") {
    const e = err as Record<string, any>;
    const sc =
      typeof e.statusCode === "number"
        ? e.statusCode
        : typeof e.response?.status === "number"
          ? e.response.status
          : undefined;
    if (typeof sc === "number" && sc >= 500 && sc < 600) return true;
  }
  return false;
}

export async function withRetry<T>(label: string, fn: () => Promise<T>): Promise<T> {
  const ms = timeoutMs();
  try {
    return await withTimeout(label, ms, fn);
  } catch (err) {
    if (!isTransient(err)) throw err;
    logWarning(`${label} failed with transient error; retrying once in ${SLEEP_MS}ms`);
    await sleep(SLEEP_MS);
    return await withTimeout(label, ms, fn);
  }
}

export async function getPullRequestById(
  project: string,
  _repoId: string,
  prId: number,
): Promise<GitPullRequest> {
  return withRetry("getPullRequestById", async () => {
    const git = await (await getWebApi()).getGitApi();
    return git.getPullRequestById(prId, project);
  });
}

/**
 * Lists active pull requests whose `sourceRefName` matches the given
 * value. Used by `exec-context-pr-synth` to discover the open PR for
 * `Build.SourceBranch` on CI-triggered builds (no Build Validation
 * branch policy required).
 *
 * Returns an empty array if no PRs match. The ADO REST API caps page
 * size; for the synth path we deliberately fetch only the first page
 * (the SDK default is 100 PRs without an explicit `$top`) since the
 * synth contract requires *exactly one* match — a source branch with
 * >100 simultaneous active PRs against it is pathological and the
 * bundle will skip via the "multi-match" path anyway.
 *
 * The `?? []` guard handles the SDK's habit of returning `null` for
 * empty result bodies on some REST responses; callers iterate / filter
 * the result, so an empty array is the only safe contract.
 */
export async function listActivePullRequestsBySourceRef(
  project: string,
  repoId: string,
  sourceRefName: string,
): Promise<GitPullRequest[]> {
  return withRetry("listActivePullRequestsBySourceRef", async () => {
    const git = await (await getWebApi()).getGitApi();
    return (
      (await git.getPullRequests(
        repoId,
        { sourceRefName, status: PullRequestStatus.Active },
        project,
      )) ?? []
    );
  });
}

/**
 * Fetches all pull-request iterations.
 *
 * The ADO REST API does not paginate this endpoint (no `$top` / `$skip` /
 * continuation token on `getPullRequestIterations`), and the SDK signature
 * confirms it returns the full list in one call. Callers should still
 * treat the result defensively — see `getIterationChanges` which DOES
 * paginate.
 */
export async function getPullRequestIterations(
  project: string,
  repoId: string,
  prId: number,
): Promise<GitPullRequestIteration[]> {
  return withRetry("getPullRequestIterations", async () => {
    const git = await (await getWebApi()).getGitApi();
    return git.getPullRequestIterations(repoId, prId, project);
  });
}

/**
 * Fetches all change entries for one PR iteration, transparently
 * paginating via `$top` / `$skip`. ADO's default page size is 100; we
 * pull pages until either an empty page is returned, the page is smaller
 * than the page size (last page), or `MAX_ITERATION_CHANGE_PAGES` is
 * reached (defensive cap to avoid unbounded loops on API misbehaviour).
 *
 * Returns a synthetic `GitPullRequestIterationChanges` whose
 * `changeEntries` is the concatenation of every page. Other fields are
 * inherited from the first page (callers in this codebase only read
 * `changeEntries`).
 */
export async function getIterationChanges(
  project: string,
  repoId: string,
  prId: number,
  iterationId: number,
): Promise<GitPullRequestIterationChanges> {
  return withRetry("getIterationChanges", async () => {
    const git = await (await getWebApi()).getGitApi();
    const allEntries: NonNullable<GitPullRequestIterationChanges["changeEntries"]> = [];
    let firstPage: GitPullRequestIterationChanges | undefined;

    for (let page = 0; page < MAX_ITERATION_CHANGE_PAGES; page++) {
      const skip = page * ITERATION_CHANGES_PAGE_SIZE;
      const result = await git.getPullRequestIterationChanges(
        repoId,
        prId,
        iterationId,
        project,
        ITERATION_CHANGES_PAGE_SIZE,
        skip,
      );
      if (!firstPage) firstPage = result;
      const entries = result.changeEntries ?? [];
      allEntries.push(...entries);
      if (entries.length < ITERATION_CHANGES_PAGE_SIZE) {
        return { ...(firstPage ?? {}), changeEntries: allEntries };
      }
    }

    logWarning(
      `getIterationChanges: hit ${MAX_ITERATION_CHANGE_PAGES}-page cap (${MAX_ITERATION_CHANGE_PAGES * ITERATION_CHANGES_PAGE_SIZE} entries); list may be truncated`,
    );
    return { ...(firstPage ?? {}), changeEntries: allEntries };
  });
}

export async function cancelBuild(project: string, buildId: number): Promise<void> {
  await withRetry("cancelBuild", async () => {
    const build = await (await getWebApi()).getBuildApi();
    const patch: Build = { status: BuildStatus.Cancelling } as Build;
    await build.updateBuild(patch, project, buildId);
  });
}
