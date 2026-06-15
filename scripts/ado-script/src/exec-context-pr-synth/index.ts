/**
 * exec-context-pr-synth — Setup-job script that normalises PR-identifier
 * variables for downstream consumers. Runs unconditionally so that the
 * single name `AW_PR_*` always carries the resolved PR identifiers,
 * whether the build is a real PR or a CI build promoted to PR semantics.
 *
 * ## Why "always run"
 *
 * Earlier versions gated this script on `ne(Build.Reason, 'PullRequest')`
 * and forced downstream steps to coalesce `$(System.PullRequest.X)` with
 * `$(AW_SYNTHETIC_PR_X)` inside step `env:` via a `$[ ... ]` runtime
 * expression. That fails: ADO only evaluates `$[ ... ]` inside the
 * `variables:` block and `condition:` fields, NOT inside step `env:`
 * values. Both `Stage PR execution context` and `Evaluate PR filters`
 * received the literal expression string and short-circuited (see build
 * <https://dev.azure.com/msazuresphere/4x4/_build/results?buildId=612528>).
 *
 * The fix is to do the real-vs-synth merge HERE, in TypeScript, and have
 * every downstream consumer read `$(AW_PR_*)` macros — no coalesce, no
 * runtime expressions in step env. The path is identical on real PR
 * builds and synth-promoted CI builds; only the `AW_SYNTHETIC_PR`
 * boolean flag distinguishes them, for the Agent job's `condition:`
 * which legitimately can use `dependencies.Setup.outputs[...]`.
 *
 * ## Why synth in the first place
 *
 * Azure DevOps Services ignores the YAML `pr:` block unless a per-
 * branch Build Validation policy is registered server-side. Without
 * that policy, a push to a feature branch fires the pipeline as
 * `Build.Reason = IndividualCI` even when an open PR exists.
 *
 * ## Variables emitted (each as BOTH `setOutput` and `setVar` — see
 * `shared/vso-logger.ts::setVar` for the dual-emit rationale):
 *
 *   - `AW_PR_ID`           — resolved PR id (real or synth), empty if none
 *   - `AW_PR_TARGETBRANCH` — resolved target ref (`refs/heads/<name>`)
 *   - `AW_PR_SOURCEBRANCH` — resolved source ref
 *   - `AW_PR_IS_DRAFT`     — "true"/"false"/"" (only meaningful on synth path)
 *   - `AW_SYNTHETIC_PR`    — "true" iff this build was synth-promoted
 *                            (i.e. CI build + matched open PR). Empty
 *                            on real PR builds and on non-promoted CI.
 *   - `AW_SYNTHETIC_PR_SKIP` — "true" iff synth was attempted but no
 *                              match was found (gates Agent job to skip)
 *
 * ## Runtime contract (all soft skips exit 0; only spec-decode and
 * infra errors exit non-zero):
 *
 *   1. real PR build (`SYSTEM_PULLREQUEST_PULLREQUESTID` non-empty) →
 *      copy `SYSTEM_PULLREQUEST_*` env into `AW_PR_*` and return
 *   2. GitHub-typed repo (`BUILD_REPOSITORY_PROVIDER=GitHub`) → emit
 *      empty `AW_PR_*` + `AW_SYNTHETIC_PR_SKIP=true` (GitHub PR semantics
 *      are routed natively by ADO; CI builds on GitHub repos don't get
 *      synth-promoted)
 *   3. Decode `PR_SYNTH_SPEC` (hard fail on corruption)
 *   4. fetch active PRs whose `sourceRefName == BUILD_SOURCEBRANCH`
 *   5. filter matched PRs by `targetRefName` against
 *      `spec.branches.include` / `spec.branches.exclude`
 *   6. count != 1 → emit empty `AW_PR_*` + skip
 *   7. paths.include/exclude reject every changed file → empty + skip
 *   8. on match: emit resolved `AW_PR_*` + `AW_SYNTHETIC_PR=true`
 */
import {
  getIterationChanges,
  getPullRequestIterations,
  listActivePullRequestsBySourceRef,
} from "../shared/ado-client.js";
import { logError, logInfo, setOutput, setVar } from "../shared/vso-logger.js";

import { matchesIncludeExclude, normalisePath, pathMatchesIncludeExclude } from "./match.js";
import { decodeSpec, type PrSynthSpec } from "./spec.js";

const SKIP_OUTPUT = "AW_SYNTHETIC_PR_SKIP";

/**
 * Resolve an ADO step-env value that may carry an unsubstituted
 * `$(name)` macro. ADO leaves undefined predefined-variable macros as
 * the literal string `$(Some.Variable.Name)` in step env — it does NOT
 * substitute to empty. Without this guard, the bundle would read
 * `SYSTEM_PULLREQUEST_PULLREQUESTID = "$(System.PullRequest.PullRequestId)"`
 * on a non-PR build and treat it as a non-empty PR id (regression
 * observed by @jamesadevine on the first roll-out:
 * `[synth-pr] real PR build #$(System.PullRequest.PullRequestId);
 * propagating SYSTEM_PULLREQUEST_* to AW_PR_*`).
 *
 * Returns the actual value when set, empty string when the value is
 * absent, empty, or a literal `$(<anything>)` macro. The macro pattern
 * uses balanced `$(` ... `)` with no nested parens (ADO macro
 * names never contain parens).
 */
function resolveAdoMacroEnv(value: string | undefined): string {
  if (!value) return "";
  if (/^\$\([^()]+\)$/.test(value)) return "";
  return value;
}

/**
 * Emit the canonical AW_PR_* identifier set as BOTH cross-job outputs
 * (consumed by the Agent job's `variables:` hoist via
 * `dependencies.Setup.outputs['synthPr.AW_PR_*']`) and same-job
 * regular variables (consumed by the gate step's `env:` block via
 * `$(AW_PR_*)` macros). Both forms are required: `isOutput=true` does
 * NOT register the variable in the producing job's regular variable
 * namespace, so a same-job consumer would see empty without the
 * paired `setVar`. See `setVar` doc-comment in `shared/vso-logger.ts`.
 *
 * Empty strings are valid: downstream consumers treat empty `AW_PR_ID`
 * as "not a PR build" (matching the legacy `Build.Reason` check).
 */
function emitPrIdentifiers(
  prId: string,
  targetBranch: string,
  sourceBranch: string,
  isDraft: string,
): void {
  const emitBoth = (name: string, value: string): void => {
    setOutput(name, value);
    setVar(name, value);
  };
  emitBoth("AW_PR_ID", prId);
  emitBoth("AW_PR_TARGETBRANCH", targetBranch);
  emitBoth("AW_PR_SOURCEBRANCH", sourceBranch);
  emitBoth("AW_PR_IS_DRAFT", isDraft);
}

function emitSkip(reason: string): void {
  setOutput(SKIP_OUTPUT, "true");
  logInfo(`[synth-pr] skip: ${reason}`);
}

export async function main(env: NodeJS.ProcessEnv = process.env): Promise<number> {
  // Step 1 — real PR build owns the path: propagate the predefined
  // SYSTEM_PULLREQUEST_* env into the AW_PR_* namespace so downstream
  // consumers can read a single name regardless of source. No API call
  // needed; ADO has already populated the values.
  //
  // Detection: `SYSTEM_PULLREQUEST_PULLREQUESTID` is non-empty (after
  // `resolveAdoMacroEnv` strips unsubstituted `$(name)` literals) iff
  // this is a real PR build. `BUILD_REASON=PullRequest` could also be
  // used as a sanity check, but the resolved id is the actual value we
  // need to propagate — and it can't be present without the build
  // really being a PR build.
  const realPrId = resolveAdoMacroEnv(env.SYSTEM_PULLREQUEST_PULLREQUESTID);
  if (realPrId.length > 0) {
    emitPrIdentifiers(
      realPrId,
      resolveAdoMacroEnv(env.SYSTEM_PULLREQUEST_TARGETBRANCH),
      resolveAdoMacroEnv(env.SYSTEM_PULLREQUEST_SOURCEBRANCH),
      resolveAdoMacroEnv(env.SYSTEM_PULLREQUEST_ISDRAFT),
    );
    logInfo(
      `[synth-pr] real PR build #${realPrId}; propagating SYSTEM_PULLREQUEST_* to AW_PR_*`,
    );
    return 0;
  }

  // Step 2 — GitHub-typed repos: ADO routes GitHub PR webhooks natively,
  // so a CI build on a GitHub repo means there's no associated PR (it
  // would have come in as a PR build instead). Emit empty AW_PR_* so
  // same-job consumers have stable defined variables, plus the SKIP
  // marker so the Agent job's `condition:` can opt out cleanly.
  if ((env.BUILD_REPOSITORY_PROVIDER ?? "").toLowerCase() === "github") {
    emitPrIdentifiers("", "", "", "");
    emitSkip("GitHub-typed repo; ADO already provides PR semantics natively");
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
    emitPrIdentifiers("", "", "", "");
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
    emitPrIdentifiers("", "", "", "");
    emitSkip(`no active PR for source ${sourceBranch} matches on.pr.branches`);
    return 0;
  }
  if (matched.length > 1) {
    emitPrIdentifiers("", "", "", "");
    emitSkip(
      `${matched.length} active PRs match source ${sourceBranch}; refusing to choose`,
    );
    return 0;
  }

  const pr = matched[0]!;
  const prId = pr.pullRequestId;
  if (typeof prId !== "number") {
    emitPrIdentifiers("", "", "", "");
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
      emitPrIdentifiers("", "", "", "");
      emitSkip(`PR ${prId} has no iterations`);
      return 0;
    }
    const latest = iterations[iterations.length - 1]!;
    const iterationId = latest.id;
    if (typeof iterationId !== "number") {
      emitPrIdentifiers("", "", "", "");
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
      emitPrIdentifiers("", "", "", "");
      emitSkip(
        `no changed file in PR ${prId} matches on.pr.paths (${changedPaths.length} files inspected)`,
      );
      return 0;
    }
  }

  // Step 8 — happy path: synth-promote this CI build.
  //
  // Emit the resolved PR identifiers under the canonical AW_PR_*
  // names (same names downstream consumers use whether they run on a
  // real PR build or a synth-promoted CI build). Plus the
  // `AW_SYNTHETIC_PR` flag (only true on synth promotion) which the
  // Agent job's `condition:` consults to require the PR-gate path.
  //
  // Format note: `targetRefName` / `sourceRefName` from the ADO REST
  // API are full refs (`refs/heads/main`, `refs/heads/feature/x`,
  // etc.), matching the shape of `SYSTEM_PULLREQUEST_TARGETBRANCH` on
  // real PR builds so downstream gets a consistent value either way.
  // See <https://learn.microsoft.com/en-us/azure/devops/pipelines/build/variables>.
  emitPrIdentifiers(
    String(prId),
    pr.targetRefName ?? "",
    pr.sourceRefName ?? sourceBranch,
    pr.isDraft === true ? "true" : "false",
  );
  setOutput("AW_SYNTHETIC_PR", "true");
  setVar("AW_SYNTHETIC_PR", "true");

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
