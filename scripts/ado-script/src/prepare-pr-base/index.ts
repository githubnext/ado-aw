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
 * as repeated `--repo-dir <dir> --target-branch <branch>` pairs in the same form
 * the MCP server resolves the dirs), it fetches that repo's target branch into
 * `refs/remotes/origin/<target>` and progressively deepens local history to the
 * merge base — reusing the exact `shared/merge-base.ts::ensureTargetRefFetched`
 * logic that the PR execution-context precompute already uses. It also points
 * each dir's `refs/remotes/origin/HEAD` at its target so `mcp.rs`'s symbolic-ref
 * default-branch detection resolves the right base. Because the MCP server
 * operates on those same dirs, the base then resolves with no in-sandbox network.
 *
 * Each `--target-branch` is the create-pull-request **destination/base** branch
 * for its `--repo-dir` (which in a multi-checkout "meta repo" setup may differ
 * per repo) — NOT the per-repo `checkout:` ref (the source/HEAD side).
 *
 * ## Trust boundary
 *
 * Mirrors the exec-context bundles: the bearer (`SYSTEM_ACCESSTOKEN`) is passed
 * to the spawned `git` child via `GIT_CONFIG_*` env vars (see
 * `shared/git.ts::bearerEnv`) — never in argv, never written to `.git/config`.
 * The compiler-owned, non-secret `--repo-dir` / `--target-branch` are argv flags
 * (immune to ADO pipeline-variable shadowing).
 *
 * ## Posture
 *
 * Benign fetch failures (e.g. a pool that refuses shallow deepening) are logged
 * and the step still exits 0 so the agent runs; the agent then simply hits the
 * existing `mcp.rs` diff-base error if truly unrecoverable. Only genuine infra
 * errors (an unusable `BUILD_SOURCESDIRECTORY`) hard-fail.
 *
 *   Invocation: node prepare-pr-base.js \
 *                 --repo-dir <dir> --target-branch <branch> \
 *                 [--repo-dir <dir> --target-branch <branch> ...]
 *               env: SYSTEM_ACCESSTOKEN (bearer for the git fetch)
 */
import { bearerEnv, gitOk as defaultGitOk, runGit as defaultRunGit } from "../shared/git.js";
import { ensureTargetRefFetched, type GitRunners } from "../shared/merge-base.js";

const defaultRunners: GitRunners = {
  runGit: defaultRunGit,
  gitOk: defaultGitOk,
};

/** A single checkout dir to deepen, with the target branch to deepen there. */
export interface RepoTarget {
  /** Checkout dir (as `mcp.rs::resolve_git_dir_for_patch` resolves it). */
  dir: string;
  /** Short target branch to fetch/deepen in that dir (no `refs/heads/`). */
  target: string;
}

export interface PrepareArgs {
  /**
   * The checkout dirs to deepen with their per-repo target branch — one per
   * allowed create-pull-request repo, in the SAME form the host-side SafeOutputs
   * MCP server resolves them (`resolve_git_dir_for_patch`): `working_directory`
   * for `self`, and `working_directory/<alias>` for each `checkout:` alias. In a
   * multi-checkout ("meta repo") setup each dir may carry a different target.
   * Empty when none were passed (falls back to `[{ BUILD_SOURCESDIRECTORY,
   * fallbackTarget }]`, then the process cwd).
   */
  repos: RepoTarget[];
  /** Target for the fallback dir when no `--repo-dir` was passed. */
  fallbackTarget: string;
}

function shortBranch(name: string): string {
  const s = name.replace(/^refs\/heads\//, "");
  // Empty after stripping (a degenerate `refs/heads/`) returns the original so
  // it stays in lock-step with the Rust `short_branch` resolver — a malformed
  // ref must resolve to the SAME (loudly-failing) branch on both sides rather
  // than silently diverging. `parseArgs` supplies its own "main" default for a
  // genuinely absent `--target-branch`.
  return s.length > 0 ? s : name || "main";
}

/**
 * Parse repeated `--repo-dir <path>` / `--target-branch <name>` flags into
 * ordered `{ dir, target }` pairs. Each `--repo-dir` takes the target from the
 * `--target-branch` that immediately follows it (compiler emits them adjacent);
 * a `--target-branch` with no preceding un-paired `--repo-dir` sets the fallback
 * target. Branch names are normalized to short form (`refs/heads/x` → `x`).
 */
export function parseArgs(argv: string[]): PrepareArgs {
  const repos: RepoTarget[] = [];
  let fallbackTarget = "main";
  let pendingDir: string | null = null;
  for (let i = 0; i < argv.length; i++) {
    if (argv[i] === "--repo-dir") {
      // A previous dir without its own target inherits the fallback.
      if (pendingDir !== null && pendingDir.length > 0) {
        repos.push({ dir: pendingDir, target: fallbackTarget });
      }
      pendingDir = argv[i + 1] ?? "";
      i++;
    } else if (argv[i] === "--target-branch") {
      const target = shortBranch(argv[i + 1] ?? "main");
      if (pendingDir !== null && pendingDir.length > 0) {
        repos.push({ dir: pendingDir, target });
        pendingDir = null;
      } else {
        fallbackTarget = target;
      }
      i++;
    }
  }
  // A trailing dir with no following --target-branch inherits the fallback.
  if (pendingDir !== null && pendingDir.length > 0) {
    repos.push({ dir: pendingDir, target: fallbackTarget });
  }
  return { repos, fallbackTarget };
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
  // Deepen every checkout dir the MCP server might generate a patch from — one
  // per allowed create-pull-request repo (`self` + each `checkout:` alias), each
  // with its own target branch. When the compiler passed none, fall back to
  // BUILD_SOURCESDIRECTORY then cwd (with the fallback target).
  let repos = args.repos;
  if (repos.length === 0) {
    const fallbackDir = env.BUILD_SOURCESDIRECTORY;
    repos = [
      { dir: fallbackDir && fallbackDir.length > 0 ? fallbackDir : ".", target: args.fallbackTarget },
    ];
  }

  const fetchEnv = bearerEnv(env.SYSTEM_ACCESSTOKEN);

  // Per-dir failures are isolated (logged + skipped) so one unreachable repo
  // never blocks the others or the agent run.
  for (const { dir, target } of repos) {
    prepareOneRepo(dir, target, fetchEnv, runners, chdir);
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
