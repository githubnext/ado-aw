/**
 * exec-context-pr-synth — Setup-job script that synthesises
 * `Build.Reason == PullRequest` semantics on CI-triggered builds when
 * an open PR matches the agent's `on.pr.branches` / `on.pr.paths`
 * filters.
 *
 * Why this exists: Azure DevOps Services ignores the YAML `pr:` block
 * unless a per-branch Build Validation policy is registered server-
 * side. Without that policy, a push to a feature branch fires the
 * pipeline as `Build.Reason = IndividualCI` even when an open PR
 * exists, so the gate evaluator's "not a PR build" bypass triggers
 * and `exec-context-pr.js` is skipped entirely.
 *
 * This script runs in the Setup job before `prGate`, calls the ADO
 * REST API to find the active PR for `Build.SourceBranch`, applies
 * the front-matter filters, and emits `AW_SYNTHETIC_PR*` outputs
 * that downstream gate + exec-context-pr steps coalesce with the
 * real `System.PullRequest.*` variables.
 *
 * Runtime contract (all soft skips exit 0; only spec-decode errors
 * and infra failures exit non-zero):
 *
 *   1. real PR build (BUILD_REASON=PullRequest) → no-op
 *   2. GitHub-typed repo (BUILD_REPOSITORY_PROVIDER=GitHub) → no-op
 *   3. Decode PR_SYNTH_SPEC (hard fail on corruption)
 *   4. branches.include/exclude miss on BUILD_SOURCEBRANCH → skip
 *   5. fetch open PRs by sourceRefName + filter by targetRefName
 *   6. count != 1 → skip
 *   7. paths.include/exclude reject everything → skip
 *   8. emit AW_SYNTHETIC_PR* outputs
 */
import {
  getIterationChanges,
  getPullRequestIterations,
  listActivePullRequestsBySourceRef,
} from "../shared/ado-client.js";
import { logError, logInfo, setOutput } from "../shared/vso-logger.js";

import { matchesIncludeExclude, normalisePath, pathMatchesIncludeExclude } from "./match.js";
import { decodeSpec, type PrSynthSpec } from "./spec.js";

const SKIP_OUTPUT = "AW_SYNTHETIC_PR_SKIP";

function emitSkip(reason: string): void {
  setOutput(SKIP_OUTPUT, "true");
  logInfo(`[synth-pr] skip: ${reason}`);
}

export async function main(env: NodeJS.ProcessEnv = process.env): Promise<number> {
  // Step 1 — real PR build owns the path; the bundle has nothing to add.
  if ((env.BUILD_REASON ?? "") === "PullRequest") {
    logInfo("[synth-pr] BUILD_REASON=PullRequest; real PR build, synth skipped");
    return 0;
  }

  // Step 2 — GitHub-typed repos already get correct pr: semantics from
  // ADO. Synthesising would double-fire.
  if ((env.BUILD_REPOSITORY_PROVIDER ?? "").toLowerCase() === "github") {
    logInfo("[synth-pr] GitHub-typed repo; ADO already provides PR semantics, synth skipped");
    return 0;
  }

  // Step 3 — decode the spec. Corruption is a hard failure.
  let spec: PrSynthSpec;
  try {
    spec = decodeSpec(env.PR_SYNTH_SPEC);
  } catch (e) {
    logError(`[synth-pr] ${(e as Error).message}`);
    return 1;
  }

  const sourceBranch = env.BUILD_SOURCEBRANCH ?? "";
  if (sourceBranch.length === 0) {
    emitSkip("BUILD_SOURCEBRANCH is empty");
    return 0;
  }

  const project = env.ADO_PROJECT ?? "";
  const repoId = env.ADO_REPO_ID ?? "";
  if (project.length === 0 || repoId.length === 0) {
    logError(
      "[synth-pr] required env vars ADO_PROJECT and ADO_REPO_ID must be set by the compiler",
    );
    return 1;
  }

  // Step 4 — fetch active PRs whose source branch is exactly ours.
  // (Skipping a source-branch pre-filter: `on.pr.branches` filters the
  // PR's TARGET branch, not the build's source branch. For most CI
  // builds on a non-PR branch the API returns [] cheaply.)
  let prs;
  try {
    prs = await listActivePullRequestsBySourceRef(project, repoId, sourceBranch);
  } catch (e) {
    logError(`[synth-pr] listActivePullRequestsBySourceRef failed: ${(e as Error).message}`);
    return 1;
  }

  const matched = prs.filter((pr) =>
    matchesIncludeExclude(
      pr.targetRefName ?? "",
      spec.branches.include,
      spec.branches.exclude,
    ),
  );

  // Step 6 — exactly-one rule.
  if (matched.length === 0) {
    emitSkip(`no active PR for source ${sourceBranch} matches on.pr.branches`);
    return 0;
  }
  if (matched.length > 1) {
    emitSkip(
      `${matched.length} active PRs match source ${sourceBranch}; refusing to choose`,
    );
    return 0;
  }

  const pr = matched[0]!;
  const prId = pr.pullRequestId;
  if (typeof prId !== "number") {
    emitSkip("matched PR has no pullRequestId");
    return 0;
  }

  // Step 7 — path filter. Only fetch iteration changes if the agent
  // declared a path filter; otherwise short-circuit (the
  // pathMatchesIncludeExclude call would accept everything anyway, but
  // the API call is wasted).
  const hasPathFilter = spec.paths.include.length > 0 || spec.paths.exclude.length > 0;
  if (hasPathFilter) {
    let iterations;
    try {
      iterations = await getPullRequestIterations(project, repoId, prId);
    } catch (e) {
      logError(`[synth-pr] getPullRequestIterations failed: ${(e as Error).message}`);
      return 1;
    }
    if (iterations.length === 0) {
      emitSkip(`PR ${prId} has no iterations`);
      return 0;
    }
    const latest = iterations[iterations.length - 1]!;
    const iterationId = latest.id;
    if (typeof iterationId !== "number") {
      emitSkip(`PR ${prId} latest iteration has no id`);
      return 0;
    }
    let changes;
    try {
      changes = await getIterationChanges(project, repoId, prId, iterationId);
    } catch (e) {
      logError(`[synth-pr] getIterationChanges failed: ${(e as Error).message}`);
      return 1;
    }
    const changedPaths: string[] = [];
    for (const entry of changes.changeEntries ?? []) {
      const itemPath = entry.item?.path;
      if (typeof itemPath === "string" && itemPath.length > 0) {
        changedPaths.push(normalisePath(itemPath));
      }
    }
    const anyAccepted = changedPaths.some((p) =>
      pathMatchesIncludeExclude(p, spec.paths.include, spec.paths.exclude),
    );
    if (!anyAccepted) {
      emitSkip(
        `no changed file in PR ${prId} matches on.pr.paths (${changedPaths.length} files inspected)`,
      );
      return 0;
    }
  }

  // Step 8 — emit synthetic PR identifiers. Downstream gate +
  // exec-context-pr steps coalesce these with the corresponding
  // `System.PullRequest.*` predefined variables via `$[ coalesce(...) ]`
  // env wiring on the consumer steps.
  //
  // Format note: `System.PullRequest.TargetBranch` (and `SourceBranch`)
  // are documented as the full ref form `refs/heads/<name>`, matching
  // the `targetRefName` / `sourceRefName` shape returned by the ADO
  // REST API (`refs/heads/main`, `refs/heads/feature/x`, etc.). The
  // coalesce on consumer steps therefore yields a consistent
  // refs-prefixed value whether the build was a real PR or synth-
  // promoted. (The unprefixed short form is `TargetBranchName` —
  // a separate predefined variable we deliberately do not use here.)
  // See <https://learn.microsoft.com/en-us/azure/devops/pipelines/build/variables>.
  setOutput("AW_SYNTHETIC_PR", "true");
  setOutput("AW_SYNTHETIC_PR_ID", String(prId));
  setOutput("AW_SYNTHETIC_PR_TARGETBRANCH", pr.targetRefName ?? "");
  setOutput("AW_SYNTHETIC_PR_SOURCEBRANCH", pr.sourceRefName ?? sourceBranch);
  setOutput("AW_SYNTHETIC_PR_IS_DRAFT", pr.isDraft === true ? "true" : "false");

  logInfo(
    `[synth-pr] matched PR #${prId} (source=${pr.sourceRefName} target=${pr.targetRefName})`,
  );
  return 0;
}

// Top-level invocation. `process.exit` is called here (not in `main`)
// so tests can call `main(env)` and inspect the return value without
// terminating the test process.
//
// Guard: only execute when this module is the program entry point.
// When the test harness imports `main`, the import-side-effect block
// below is skipped (vitest sets `process.argv[1]` to the runner, not
// to this module). When ncc bundles for production, `process.argv[1]`
// is the resolved bundle path, which matches `import.meta.url`.
import { fileURLToPath } from "node:url";

if (
  typeof process.argv[1] === "string" &&
  process.argv[1] === fileURLToPath(import.meta.url)
) {
  main()
    .then((code) => process.exit(code))
    .catch((e: unknown) => {
      logError(`[synth-pr] unhandled error: ${(e as Error).message}`);
      process.exit(1);
    });
}
