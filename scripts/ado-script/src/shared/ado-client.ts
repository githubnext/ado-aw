/**
 * Thin wrapper around `azure-devops-node-api` exposing only the methods
 * the gate evaluator uses, with a one-shot retry on transient (5xx) errors.
 */
import { getWebApi } from "./auth.js";
import { logWarning } from "./vso-logger.js";
import type {
  GitPullRequest,
  GitPullRequestIteration,
  GitPullRequestIterationChanges,
} from "azure-devops-node-api/interfaces/GitInterfaces.js";
import { BuildStatus, type Build } from "azure-devops-node-api/interfaces/BuildInterfaces.js";

const SLEEP_MS = 1000;

const sleep = (ms: number): Promise<void> =>
  new Promise((resolve) => setTimeout(resolve, ms));

function isTransient(err: unknown): boolean {
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
  try {
    return await fn();
  } catch (err) {
    if (!isTransient(err)) throw err;
    logWarning(`${label} failed with transient error; retrying once in ${SLEEP_MS}ms`);
    await sleep(SLEEP_MS);
    return await fn();
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

export async function getIterationChanges(
  project: string,
  repoId: string,
  prId: number,
  iterationId: number,
): Promise<GitPullRequestIterationChanges> {
  return withRetry("getIterationChanges", async () => {
    const git = await (await getWebApi()).getGitApi();
    return git.getPullRequestIterationChanges(repoId, prId, iterationId, project);
  });
}

export async function cancelBuild(project: string, buildId: number): Promise<void> {
  await withRetry("cancelBuild", async () => {
    const build = await (await getWebApi()).getBuildApi();
    const patch: Build = { status: BuildStatus.Cancelling } as Build;
    await build.updateBuild(patch, project, buildId);
  });
}
