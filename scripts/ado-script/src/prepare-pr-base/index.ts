/**
 * prepare-pr-base — make the create-pull-request diff base available on
 * shallow-default Azure DevOps agent pools (issue #1413).
 *
 * ## Why this exists
 *
 * The `create-pull-request` safe-output patch is generated at agent time by the
 * host-side SafeOutputs MCP server (`src/mcp.rs::find_merge_base`), which only
 * inspects **existing local refs** and never fetches. On an agent pool whose
 * default git fetch is shallow (`fetchDepth: 1`), `checkout: self` leaves no
 * `refs/remotes/origin/<target>` ref and too little history, so the server
 * cannot compute a merge base and the PR can't be opened.
 *
 * This bundle runs as a **credentialed Agent-job prepare step on the host**
 * (before the AWF-wrapped Copilot run, using `$(System.AccessToken)`). It
 * fetches the configured target branch into `refs/remotes/origin/<target>` and
 * progressively deepens local history to the merge base — reusing the exact
 * `shared/merge-base.ts::ensureTargetRefFetched` logic that the PR
 * execution-context precompute already uses. It also points
 * `refs/remotes/origin/HEAD` at the target so `mcp.rs`'s symbolic-ref default
 * branch detection resolves the right base. Because the MCP server operates on
 * the same `$(Build.SourcesDirectory)` checkout, the base then resolves with no
 * in-sandbox network.
 *
 * ## Trust boundary
 *
 * Mirrors the exec-context bundles: the bearer (`SYSTEM_ACCESSTOKEN`) is passed
 * to the spawned `git` child via `GIT_CONFIG_*` env vars (see
 * `shared/git.ts::bearerEnv`) — never in argv, never written to `.git/config`.
 * The compiler-owned, non-secret `--target-branch` is an argv flag (immune to
 * ADO pipeline-variable shadowing).
 *
 * ## Posture
 *
 * Benign fetch failures (e.g. a pool that refuses shallow deepening) are logged
 * and the step still exits 0 so the agent runs; the agent then simply hits the
 * existing `mcp.rs` diff-base error if truly unrecoverable. Only genuine infra
 * errors (an unusable `BUILD_SOURCESDIRECTORY`) hard-fail.
 *
 *   Invocation: node prepare-pr-base.js --target-branch <name>
 *               env: SYSTEM_ACCESSTOKEN (bearer for the git fetch)
 */
import { bearerEnv, gitOk as defaultGitOk, runGit as defaultRunGit } from "../shared/git.js";
import { ensureTargetRefFetched, type GitRunners } from "../shared/merge-base.js";

const defaultRunners: GitRunners = {
  runGit: defaultRunGit,
  gitOk: defaultGitOk,
};

export interface PrepareArgs {
  /** Short target branch name (no `refs/heads/` prefix). */
  targetBranch: string;
}

/**
 * Parse `--target-branch <name>`. Defaults to `main` (matching the compiler's
 * `CreatePrConfig` default) and strips a leading `refs/heads/` so callers may
 * pass either the short name or a full ref.
 */
export function parseArgs(argv: string[]): PrepareArgs {
  let targetBranch = "main";
  for (let i = 0; i < argv.length; i++) {
    if (argv[i] === "--target-branch") {
      targetBranch = argv[i + 1] ?? "main";
      i++;
    }
  }
  targetBranch = targetBranch.replace(/^refs\/heads\//, "");
  if (targetBranch.length === 0) {
    targetBranch = "main";
  }
  return { targetBranch };
}

export function main(
  args: PrepareArgs,
  env: NodeJS.ProcessEnv = process.env,
  runners: GitRunners = defaultRunners,
): number {
  const targetShort = args.targetBranch;

  // Operate on the same checkout the host-side SafeOutputs MCP server uses
  // (its `bounding_directory` is `$(Build.SourcesDirectory)`). chdir so every
  // spawned git runs against that repo regardless of the step's ambient cwd.
  const repoDir = env.BUILD_SOURCESDIRECTORY;
  if (repoDir && repoDir.length > 0) {
    try {
      process.chdir(repoDir);
    } catch (err) {
      process.stderr.write(
        `[prepare-pr-base] fatal: could not chdir to BUILD_SOURCESDIRECTORY ('${repoDir}'): ${(err as Error).message}\n`,
      );
      return 1;
    }
  }

  const fetchEnv = bearerEnv(env.SYSTEM_ACCESSTOKEN);

  const fetched = ensureTargetRefFetched(targetShort, fetchEnv, runners);
  if (!fetched.ok) {
    // Non-fatal: the agent still runs. If the base is genuinely unreachable the
    // SafeOutputs MCP server surfaces its own diff-base error later.
    process.stdout.write(
      `[prepare-pr-base] warning: ${fetched.reason} create-pull-request may fail to compute a diff base on this pool.\n`,
    );
    return 0;
  }

  // Point origin/HEAD at the fetched target so mcp.rs's
  // `git symbolic-ref refs/remotes/origin/HEAD` default-branch probe resolves
  // to origin/<target> even when the target is not main/master.
  const sym = runners.runGit([
    "symbolic-ref",
    "refs/remotes/origin/HEAD",
    `refs/remotes/origin/${targetShort}`,
  ]);
  if (sym.status !== 0) {
    process.stdout.write(
      `[prepare-pr-base] note: could not set refs/remotes/origin/HEAD -> origin/${targetShort} (${sym.stderr.trim()}); relying on origin/${targetShort} directly.\n`,
    );
  }

  process.stdout.write(
    `[prepare-pr-base] base ref ready: origin/${targetShort} fetched/deepened (merge-base=${fetched.baseSha}).\n`,
  );
  return 0;
}

// CLI entry guard: only run when invoked directly (not when imported by tests).
if (
  typeof process !== "undefined" &&
  process.argv[1] &&
  /prepare-pr-base(\/index)?\.js$/.test(process.argv[1])
) {
  const args = parseArgs(process.argv.slice(2));
  process.exit(main(args));
}
