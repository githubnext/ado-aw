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
 * (before the AWF-wrapped Copilot run, using `$(System.AccessToken)`). For each
 * allowed create-pull-request repo dir (`self` + every `checkout:` alias, passed
 * as repeated `--repo-dir` flags in the same form the MCP server resolves them),
 * it fetches the configured target branch into `refs/remotes/origin/<target>` and
 * progressively deepens local history to the merge base — reusing the exact
 * `shared/merge-base.ts::ensureTargetRefFetched` logic that the PR
 * execution-context precompute already uses. It also points each dir's
 * `refs/remotes/origin/HEAD` at the target so `mcp.rs`'s symbolic-ref default
 * branch detection resolves the right base. Because the MCP server operates on
 * those same dirs, the base then resolves with no in-sandbox network.
 *
 * The single `--target-branch` is the create-pull-request **destination/base**
 * branch (default `main`), applied uniformly to every repo dir — NOT the per-repo
 * `checkout:` ref (which is the source/HEAD side).
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
 *   Invocation: node prepare-pr-base.js --target-branch <name> \
 *                 --repo-dir <dir> [--repo-dir <dir> ...]
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
  /**
   * The checkout dirs to deepen — one per allowed create-pull-request repo, in
   * the SAME form the host-side SafeOutputs MCP server resolves them
   * (`resolve_git_dir_for_patch`): `working_directory` for `self`, and
   * `working_directory/<alias>` for each `checkout:` alias. Empty when none were
   * passed (falls back to `[BUILD_SOURCESDIRECTORY]`, then the process cwd).
   */
  repoDirs: string[];
}

/**
 * Parse `--target-branch <name>` and repeated `--repo-dir <path>`.
 * `--target-branch` defaults to `main` (matching the compiler's `CreatePrConfig`
 * default) and strips a leading `refs/heads/` so callers may pass either the
 * short name or a full ref. Every `--repo-dir` is collected in order.
 */
export function parseArgs(argv: string[]): PrepareArgs {
  let targetBranch = "main";
  const repoDirs: string[] = [];
  for (let i = 0; i < argv.length; i++) {
    if (argv[i] === "--target-branch") {
      targetBranch = argv[i + 1] ?? "main";
      i++;
    } else if (argv[i] === "--repo-dir") {
      const dir = argv[i + 1] ?? "";
      if (dir.length > 0) {
        repoDirs.push(dir);
      }
      i++;
    }
  }
  targetBranch = targetBranch.replace(/^refs\/heads\//, "");
  if (targetBranch.length === 0) {
    targetBranch = "main";
  }
  return { targetBranch, repoDirs };
}

/**
 * Deepen `origin/<targetShort>` in a single checkout dir and point that dir's
 * `origin/HEAD` at it. Returns `true` when the base resolved. Never throws — a
 * dir that isn't a git repo (a quirky-workspace path the MCP would also fail on)
 * or a fetch that can't reach the base is a benign, isolated skip.
 */
function prepareOneRepo(
  repoDir: string,
  targetShort: string,
  fetchEnv: Record<string, string>,
  runners: GitRunners,
  chdir: (dir: string) => void,
): boolean {
  try {
    chdir(repoDir);
  } catch (err) {
    // Non-fatal: the MCP server would fail on this same path too. Skip and let
    // other repos proceed.
    process.stdout.write(
      `[prepare-pr-base] note: skipping '${repoDir}' (could not chdir: ${(err as Error).message}).\n`,
    );
    return false;
  }

  const fetched = ensureTargetRefFetched(targetShort, fetchEnv, runners);
  if (!fetched.ok) {
    process.stdout.write(
      `[prepare-pr-base] warning: ${fetched.reason} create-pull-request may fail to compute a diff base for '${repoDir}' on this pool.\n`,
    );
    return false;
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
      `[prepare-pr-base] note: could not set refs/remotes/origin/HEAD -> origin/${targetShort} in '${repoDir}' (${sym.stderr.trim()}); relying on origin/${targetShort} directly.\n`,
    );
  }

  process.stdout.write(
    `[prepare-pr-base] base ref ready in '${repoDir}': origin/${targetShort} fetched/deepened (merge-base=${fetched.baseSha}).\n`,
  );
  return true;
}

export function main(
  args: PrepareArgs,
  env: NodeJS.ProcessEnv = process.env,
  runners: GitRunners = defaultRunners,
  chdir: (dir: string) => void = process.chdir.bind(process),
): number {
  const targetShort = args.targetBranch;

  // Deepen every checkout dir the MCP server might generate a patch from — one
  // per allowed create-pull-request repo (`self` + each `checkout:` alias). When
  // the compiler passed none, fall back to BUILD_SOURCESDIRECTORY then cwd.
  let repoDirs = args.repoDirs;
  if (repoDirs.length === 0) {
    const fallback = env.BUILD_SOURCESDIRECTORY;
    repoDirs = fallback && fallback.length > 0 ? [fallback] : ["."];
  }

  const fetchEnv = bearerEnv(env.SYSTEM_ACCESSTOKEN);

  // Per-dir failures are isolated (logged + skipped) so one unreachable repo
  // never blocks the others or the agent run.
  for (const repoDir of repoDirs) {
    prepareOneRepo(repoDir, targetShort, fetchEnv, runners, chdir);
  }
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
