import { gitOk as defaultGitOk, runGit as defaultRunGit, type GitResult } from "./git.js";

export type MergeBaseSuccess = {
  ok: true;
  baseSha: string;
  headSha: string;
};

export type MergeBaseFailure = {
  ok: false;
  reason: string;
};

export type MergeBaseResult = MergeBaseSuccess | MergeBaseFailure;

/**
 * Injectable git runners. Production callers pass nothing (defaults
 * to the real `runGit`/`gitOk`); tests pass stubs that simulate the
 * sequence of fetch attempts + rev-parse + merge-base queries.
 */
export type GitRunners = {
  runGit: (args: string[], env?: Record<string, string>) => GitResult;
  gitOk: (args: string[], env?: Record<string, string>) => string | null;
};

const defaultRunners: GitRunners = {
  runGit: defaultRunGit,
  gitOk: defaultGitOk,
};

/**
 * Count the parents reported by `git rev-list --parents -n 1 HEAD`.
 * Output is `"<commit> <parent1> [<parent2> ...]"`. Three or more
 * tokens indicates a merge commit (1 commit + 2+ parents).
 *
 * Returns 0 on any git failure (the bash version does the same via
 * `|| true` + `wc -w` of empty input, then parameter expansion).
 */
function countParents(runners: GitRunners): number {
  const result = runners.runGit(["rev-list", "--parents", "-n", "1", "HEAD"]);
  if (result.status !== 0) return 0;
  const tokens = result.stdout.trim().split(/\s+/).filter((t) => t.length > 0);
  return tokens.length;
}

/**
 * Fetch the PR target branch from origin into
 * `refs/remotes/origin/<short>` at the given depth.
 *
 * `depthArg` is one of `--depth=N` or `--unshallow` — passed
 * verbatim so the caller can iterate the progressive-deepening loop.
 */
function fetchTargetAtDepth(
  runners: GitRunners,
  targetShort: string,
  depthArg: string,
  env: Record<string, string>,
): boolean {
  const result = runners.runGit(
    [
      "fetch",
      "--no-tags",
      depthArg,
      "origin",
      `+refs/heads/${targetShort}:refs/remotes/origin/${targetShort}`,
    ],
    env,
  );
  return result.status === 0;
}

/**
 * Resolve `BASE_SHA` and `HEAD_SHA` for the PR.
 *
 * Two paths, both producing the SAME "merge-base of target tip and PR
 * head" semantics:
 *
 *  1. **Synthetic merge commit**: when `HEAD` has ≥2 parents (ADO's
 *     default checkout mode for PR builds), `HEAD^1` is the target tip
 *     at PR preparation time and `HEAD^2` is the PR head. We compute
 *     `merge-base HEAD^1 HEAD^2` to match the deepening path's
 *     semantics. Falls back to `HEAD^1` if `merge-base` cannot resolve.
 *
 *  2. **Progressive deepening**: when HEAD is a normal commit, fetch
 *     the target branch with `--depth=200`, `500`, `2000`, `--unshallow`
 *     until `git merge-base origin/<target> HEAD` resolves.
 *
 * `env` is the result of `bearerEnv(token)` — passed to git's fetch
 * subprocess so the bearer never leaks into argv or to other tools.
 */
export function resolveMergeBase(
  targetShort: string,
  env: Record<string, string>,
  runners: GitRunners = defaultRunners,
): MergeBaseResult {
  const headSha = runners.gitOk(["rev-parse", "HEAD"]) ?? "";
  const parents = countParents(runners);

  let baseSha = "";
  let headTipSha = "";

  if (parents >= 3) {
    // Synthetic merge commit.
    const p1 = runners.gitOk(["rev-parse", "HEAD^1"]) ?? "";
    const p2 = runners.gitOk(["rev-parse", "HEAD^2"]) ?? "";
    headTipSha = p2;
    if (p1.length > 0 && p2.length > 0) {
      const mergeBase = runners.gitOk(["merge-base", p1, p2]) ?? "";
      baseSha = mergeBase.length > 0 ? mergeBase : p1;
    }
  } else {
    headTipSha = headSha;
    // Progressive deepening: stop ONLY when merge-base actually
    // resolves against the deepened target ref.
    const depths = ["--depth=200", "--depth=500", "--depth=2000", "--unshallow"];
    for (const depthArg of depths) {
      if (!fetchTargetAtDepth(runners, targetShort, depthArg, env)) {
        // Fetch failed at this depth (e.g. --unshallow on an
        // already-unshallowed repo). Continue to the next depth or
        // bail out after the loop.
        continue;
      }
      const mb = runners.gitOk(["merge-base", `origin/${targetShort}`, "HEAD"]) ?? "";
      if (mb.length > 0) {
        baseSha = mb;
        break;
      }
    }
  }

  if (baseSha.length === 0 || headTipSha.length === 0) {
    return {
      ok: false,
      reason: `Could not resolve base/head SHAs after progressive deepening of '${targetShort}' (HEAD=${headSha}, parents=${parents}).`,
    };
  }

  return { ok: true, baseSha, headSha: headTipSha };
}
