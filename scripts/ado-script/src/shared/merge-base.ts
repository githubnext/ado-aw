import { gitOk as defaultGitOk, runGit as defaultRunGit, type GitResult } from "./git.js";

const SHA40_RE = /^[0-9a-f]{40}$/i;
const TARGETED_DEPTH_LIMIT = 10_000;
const BOUNDED_DEPTHS = [200, 500, 2000] as const;
const SOURCE_TRACKING_REF = "refs/remotes/origin/ado-aw-prepare-source";

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
 * Count the tokens reported by `git rev-list --parents -n 1 HEAD`.
 * Output is `"<commit> <parent1> [<parent2> ...]"`, so the token count
 * is `1 + parentCount`. A normal merge commit (2 parents) yields 3
 * tokens; the synthetic merge ADO creates for PR builds also yields 3
 * tokens. We treat `>= 3` as "merge commit" for the synthetic-merge
 * branch — see [`resolveMergeBase`].
 *
 * Returns 0 on any git failure (the bash version did the same via
 * `|| true` + `wc -w` of empty input, then parameter expansion).
 */
function countParentTokens(runners: GitRunners): number {
  const result = runners.runGit(["rev-list", "--parents", "-n", "1", "HEAD"]);
  if (result.status !== 0) return 0;
  const tokens = result.stdout.trim().split(/\s+/).filter((t) => t.length > 0);
  return tokens.length;
}

/**
 * Fetch the PR target branch from origin into
 * `refs/remotes/origin/<short>` at the given depth.
 *
 * The numeric depth is emitted as `--depth=N` for bounded recovery.
 */
function fetchBranchAtDepth(
  runners: GitRunners,
  branchShort: string,
  depth: number,
  env: Record<string, string>,
): boolean {
  const result = runners.runGit(
    [
      "fetch",
      "--no-tags",
      "--no-recurse-submodules",
      `--depth=${depth}`,
      "origin",
      `+refs/heads/${branchShort}:refs/remotes/origin/${branchShort}`,
    ],
    env,
  );
  return result.status === 0;
}

/** Result of [`ensureTargetRefFetched`]. */
export type FetchDeepenResult =
  | { ok: true; baseSha: string }
  | { ok: false; reason: string };

export type ExactMergeBaseMetadata = {
  commonCommit: string;
  aheadCount: number;
  behindCount: number;
  sourceCommit: string;
  targetCommit: string;
};

function validRef(value: string): boolean {
  if (
    !value.startsWith("refs/") ||
    value.endsWith("/") ||
    value.endsWith(".") ||
    value.includes("//") ||
    value.includes("..") ||
    value.includes("@{")
  ) {
    return false;
  }
  for (const segment of value.split("/")) {
    if (!segment || segment.startsWith(".") || segment.endsWith(".lock")) {
      return false;
    }
  }
  for (const char of value) {
    const code = char.codePointAt(0) ?? 0;
    if (
      code <= 0x20 ||
      code === 0x7f ||
      ["~", "^", ":", "?", "*", "[", "\\"].includes(char)
    ) {
      return false;
    }
  }
  return true;
}

function sourceRefspec(sourceRef: string): string | null {
  if (SHA40_RE.test(sourceRef)) {
    return `+${sourceRef}:${SOURCE_TRACKING_REF}`;
  }
  const full = sourceRef.startsWith("refs/")
    ? sourceRef
    : `refs/heads/${sourceRef}`;
  if (!validRef(full)) return null;
  return `+${full}:${SOURCE_TRACKING_REF}`;
}

function targetRefspec(targetShort: string, source = `refs/heads/${targetShort}`): string | null {
  if (
    !validRef(`refs/heads/${targetShort}`) ||
    (!SHA40_RE.test(source) && !validRef(source))
  ) {
    return null;
  }
  return `+${source}:refs/remotes/origin/${targetShort}`;
}

function localMergeBases(targetShort: string, runners: GitRunners): string[] {
  const raw = runners.gitOk(["merge-base", "--all", `origin/${targetShort}`, "HEAD"]);
  if (!raw) return [];
  return raw
    .split(/\s+/)
    .filter((sha) => SHA40_RE.test(sha))
    .map((sha) => sha.toLowerCase());
}

function checkedDepth(count: number): number | null {
  if (!Number.isSafeInteger(count) || count < 0 || count >= TARGETED_DEPTH_LIMIT) {
    return null;
  }
  return count + 1;
}

function isShallowRepository(runners: GitRunners): boolean {
  return runners.gitOk(["rev-parse", "--is-shallow-repository"]) !== "false";
}

function currentHistoryFloor(runners: GitRunners): number {
  const raw = runners.gitOk(["rev-list", "--count", "HEAD"]) ?? "";
  const count = Number(raw);
  return Number.isSafeInteger(count) && count > 0 ? count : 1;
}

/**
 * Fetch exactly enough source and target history to make an Azure
 * server-computed common commit locally verifiable.
 */
export function ensureExactMergeBaseFetched(
  targetShort: string,
  metadata: ExactMergeBaseMetadata,
  env: Record<string, string>,
  runners: GitRunners = defaultRunners,
): FetchDeepenResult {
  const commonCommit = metadata.commonCommit.toLowerCase();
  const sourceCommit = metadata.sourceCommit.toLowerCase();
  const targetCommit = metadata.targetCommit.toLowerCase();
  if (
    !SHA40_RE.test(commonCommit) ||
    !SHA40_RE.test(sourceCommit) ||
    !SHA40_RE.test(targetCommit)
  ) {
    return { ok: false, reason: "Azure diff metadata contained an invalid commit SHA." };
  }

  const sourceDepth = checkedDepth(metadata.aheadCount);
  const targetDepth = checkedDepth(metadata.behindCount);
  if (sourceDepth === null || targetDepth === null) {
    return {
      ok: false,
      reason: `Azure diff depth exceeds the ${TARGETED_DEPTH_LIMIT}-commit automatic safety limit.`,
    };
  }

  const sourceSpec = sourceRefspec(sourceCommit);
  const targetSpec = targetRefspec(targetShort, targetCommit);
  if (!sourceSpec || !targetSpec) {
    return { ok: false, reason: "Could not construct safe exact-commit fetch refspecs." };
  }
  if (isShallowRepository(runners)) {
    const historyFloor = currentHistoryFloor(runners);
    const effectiveSourceDepth = Math.max(sourceDepth, historyFloor);
    const effectiveTargetDepth = Math.max(targetDepth, historyFloor);
    const sourceFetch = runners.runGit(
      [
        "fetch",
        "--no-tags",
        "--no-recurse-submodules",
        `--depth=${effectiveSourceDepth}`,
        "origin",
        sourceSpec,
      ],
      env,
    );
    if (sourceFetch.status !== 0) {
      return {
        ok: false,
        reason: `Exact source fetch failed at depth ${effectiveSourceDepth}: ${sourceFetch.stderr.trim()}`,
      };
    }
    const targetFetch = runners.runGit(
      [
        "fetch",
        "--no-tags",
        "--no-recurse-submodules",
        `--depth=${effectiveTargetDepth}`,
        "origin",
        targetSpec,
      ],
      env,
    );
    if (targetFetch.status !== 0) {
      return {
        ok: false,
        reason: `Exact target fetch failed at depth ${effectiveTargetDepth}: ${targetFetch.stderr.trim()}`,
      };
    }
  } else {
    const targetFetch = runners.runGit(
      ["fetch", "--no-tags", "--no-recurse-submodules", "origin", targetSpec],
      env,
    );
    if (targetFetch.status !== 0) {
      return {
        ok: false,
        reason: `Exact target fetch failed: ${targetFetch.stderr.trim()}`,
      };
    }
  }

  const bases = localMergeBases(targetShort, runners);
  if (!bases.includes(commonCommit)) {
    return {
      ok: false,
      reason: `Azure common commit '${commonCommit}' was not a local best merge-base after targeted fetch.`,
    };
  }
  return { ok: true, baseSha: commonCommit };
}

/**
 * Bounded git-only recovery for non-Azure remotes or unavailable REST
 * metadata. Source and target are fetched together at every depth so neither
 * side of a divergent shallow graph remains truncated.
 */
export function ensureRefsForMergeBaseFetched(
  sourceRef: string,
  targetShort: string,
  env: Record<string, string>,
  runners: GitRunners = defaultRunners,
): FetchDeepenResult {
  const sourceSpec = sourceRefspec(sourceRef);
  const targetSpec = targetRefspec(targetShort);
  if (!sourceSpec || !targetSpec) {
    return { ok: false, reason: "Source or target ref is not safe to fetch." };
  }
  if (!isShallowRepository(runners)) {
    const fetched = runners.runGit(
      ["fetch", "--no-tags", "--no-recurse-submodules", "origin", targetSpec],
      env,
    );
    if (fetched.status === 0) {
      const bases = localMergeBases(targetShort, runners);
      if (bases.length > 0) return { ok: true, baseSha: bases[0]! };
    }
    return {
      ok: false,
      reason: `Could not resolve merge-base after fetching full-checkout target 'origin/${targetShort}'.`,
    };
  }
  const historyFloor = currentHistoryFloor(runners);
  const depths = [...new Set(BOUNDED_DEPTHS.map((depth) => Math.max(depth, historyFloor)))];
  for (const depth of depths) {
    const specs = sourceSpec === targetSpec ? [sourceSpec] : [sourceSpec, targetSpec];
    const fetched = runners.runGit(
      [
        "fetch",
        "--no-tags",
        "--no-recurse-submodules",
        `--depth=${depth}`,
        "origin",
        ...specs,
      ],
      env,
    );
    if (fetched.status !== 0) continue;
    const bases = localMergeBases(targetShort, runners);
    if (bases.length > 0) {
      return { ok: true, baseSha: bases[0]! };
    }
  }
  return {
    ok: false,
    reason: `Could not resolve merge-base for source '${sourceRef}' and target 'origin/${targetShort}' after bounded depths 200/500/2000.`,
  };
}

/** Fetch only the target tip needed by Stage 3's `git worktree add`. */
export function ensureTargetTipFetched(
  targetShort: string,
  env: Record<string, string>,
  runners: GitRunners = defaultRunners,
): FetchDeepenResult {
  const spec = targetRefspec(targetShort);
  if (!spec) return { ok: false, reason: "Target branch is not safe to fetch." };
  const depthArgs = isShallowRepository(runners)
    ? [`--depth=${currentHistoryFloor(runners)}`]
    : [];
  const fetched = runners.runGit(
    ["fetch", "--no-tags", "--no-recurse-submodules", ...depthArgs, "origin", spec],
    env,
  );
  if (fetched.status !== 0) {
    return {
      ok: false,
      reason: `Target-tip fetch failed: ${fetched.stderr.trim()}`,
    };
  }
  const targetSha = runners.gitOk(["rev-parse", `origin/${targetShort}`]) ?? "";
  if (!SHA40_RE.test(targetSha)) {
    return { ok: false, reason: `Fetched target 'origin/${targetShort}' did not resolve to a commit.` };
  }
  return { ok: true, baseSha: targetSha.toLowerCase() };
}

/**
 * Fetch `origin/<targetShort>` into the local clone and progressively
 * deepen it (`--depth=200`, `500`, `2000`) until
 * `git merge-base origin/<targetShort> HEAD` resolves — i.e. until the
 * clone carries enough history to reach the merge base of HEAD and the
 * target branch.
 *
 * This isolates the *side effect* that both [`resolveMergeBase`] (which
 * also returns the SHAs) and the `prepare-pr-base` bundle (which only
 * needs the deepened clone + `refs/remotes/origin/<target>` populated so
 * the host-side SafeOutputs MCP server can later compute the base) rely
 * on. The git call sequence is identical to the loop `resolveMergeBase`
 * previously inlined, so injected `GitRunners` stubs see the same order.
 *
 * `env` is the result of `bearerEnv(token)` — passed to git's fetch
 * subprocess so the bearer never leaks into argv or to other tools.
 */
export function ensureTargetRefFetched(
  targetShort: string,
  env: Record<string, string>,
  runners: GitRunners = defaultRunners,
): FetchDeepenResult {
  for (const depth of BOUNDED_DEPTHS) {
    if (!fetchBranchAtDepth(runners, targetShort, depth, env)) continue;
    const bases = localMergeBases(targetShort, runners);
    if (bases.length > 0) {
      return { ok: true, baseSha: bases[0]! };
    }
  }
  return {
    ok: false,
    reason: `Could not resolve merge-base against 'origin/${targetShort}' after bounded depths 200/500/2000.`,
  };
}

function ensureSyntheticMergeParentsFetched(
  targetShort: string,
  sourceShort: string,
  targetParentSha: string,
  sourceParentSha: string,
  env: Record<string, string>,
  runners: GitRunners,
): FetchDeepenResult {
  const sourceSpec = sourceRefspec(sourceShort);
  const targetSpec = targetRefspec(targetShort);
  if (!sourceSpec || !targetSpec) {
    return { ok: false, reason: "Synthetic PR source or target ref is not safe to fetch." };
  }
  for (const depth of BOUNDED_DEPTHS) {
    const fetched = runners.runGit(
      [
        "fetch",
        "--no-tags",
        "--no-recurse-submodules",
        `--depth=${Math.max(depth, currentHistoryFloor(runners))}`,
        "origin",
        sourceSpec,
        targetSpec,
      ],
      env,
    );
    if (fetched.status !== 0) continue;
    const mb = runners.gitOk(["merge-base", targetParentSha, sourceParentSha]) ?? "";
    if (mb.length > 0) {
      return { ok: true, baseSha: mb };
    }
  }
  return {
    ok: false,
    reason: `Could not resolve merge-base between synthetic PR parents after bounded deepening of 'origin/${targetShort}' and '${sourceShort}'.`,
  };
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
 *     semantics. If the shallow checkout lacks enough ancestry, fetch
 *     the target and source refs with progressive deepening and retry.
 *
 *  2. **Bounded deepening**: when HEAD is a normal commit, fetch source and
 *     target together with `--depth=200`, `500`, `2000` until
 *     `git merge-base origin/<target> HEAD` resolves.
 *
 * `env` is the result of `bearerEnv(token)` — passed to git's fetch
 * subprocess so the bearer never leaks into argv or to other tools.
 */
export function resolveMergeBase(
  targetShort: string,
  env: Record<string, string>,
  runners: GitRunners = defaultRunners,
  sourceShort = "",
): MergeBaseResult {
  const headSha = runners.gitOk(["rev-parse", "HEAD"]) ?? "";
  const parentTokens = countParentTokens(runners);

  let baseSha = "";
  let headTipSha = "";

  if (parentTokens >= 3) {
    // Synthetic merge commit (3 tokens = 1 commit + 2 parents).
    const p1 = runners.gitOk(["rev-parse", "HEAD^1"]) ?? "";
    const p2 = runners.gitOk(["rev-parse", "HEAD^2"]) ?? "";
    headTipSha = p2;
    if (p1.length > 0 && p2.length > 0) {
      const mergeBase = runners.gitOk(["merge-base", p1, p2]) ?? "";
      if (mergeBase.length > 0) {
        baseSha = mergeBase;
      } else if (sourceShort.length > 0) {
        const fetched = ensureSyntheticMergeParentsFetched(
          targetShort,
          sourceShort,
          p1,
          p2,
          env,
          runners,
        );
        if (fetched.ok) {
          baseSha = fetched.baseSha;
        }
      }
    }
  } else {
    headTipSha = headSha;
    const fetched =
      sourceShort.length > 0
        ? ensureRefsForMergeBaseFetched(sourceShort, targetShort, env, runners)
        : ensureTargetRefFetched(targetShort, env, runners);
    if (fetched.ok) {
      baseSha = fetched.baseSha;
    }
  }

  if (baseSha.length === 0 || headTipSha.length === 0) {
    return {
      ok: false,
      reason: `Could not resolve base/head SHAs after progressive deepening of '${targetShort}' (HEAD=${headSha}, parentTokens=${parentTokens}).`,
    };
  }

  // Defensive: every successful return must be a full 40-char hex SHA.
  // `git rev-parse` and `git merge-base` normally output exactly that,
  // but a misconfigured `core.abbrev`, an unexpected `.gitconfig`
  // override, or a future git-version quirk could yield abbreviated or
  // multi-line output. We do NOT want a partial SHA staged into the
  // safe-output dir — the agent's `git diff $BASE..$HEAD` would error
  // out in-sandbox with a confusing message. Fail closed here instead.
  if (!SHA40_RE.test(baseSha) || !SHA40_RE.test(headTipSha)) {
    return {
      ok: false,
      reason: `Resolved SHAs are not 40-char hex (baseSha='${baseSha}', headSha='${headTipSha}', targetShort='${targetShort}').`,
    };
  }

  return { ok: true, baseSha, headSha: headTipSha };
}
