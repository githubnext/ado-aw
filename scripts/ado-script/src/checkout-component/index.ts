/**
 * checkout-component — make a SHA-pinned custom safe-output component available
 * at the exact pinned commit on a shallow-default Azure DevOps agent pool.
 *
 * ## Why this exists
 *
 * A cross-repository custom safe-output component is checked out via an ADO
 * repository resource, whose `ref` can only be a branch/tag — never a commit
 * SHA (an ADO limitation). On a shallow-default pool the resource checkout
 * pulls only the tip of `refs/heads/main` (`fetchDepth: 1`), so the pinned
 * commit object is usually absent and a plain `git checkout --detach <sha>`
 * fails with `fatal: reference is not a tree`. That defeats the whole point of
 * SHA-pinning: the pinned revision must actually run, reproducibly, regardless
 * of where `main` has since moved.
 *
 * This bundle runs as a **credentialed step in the isolated custom
 * safe-output job** (using `$(System.AccessToken)`). It obtains the pinned
 * commit object — first via a direct `git fetch origin <sha>` (supported by
 * GitHub, GitHub Enterprise, and Azure Repos), then, if the server refuses a
 * by-SHA fetch, by progressively deepening the checked-out branch until the
 * object is present — then checks it out detached and **verifies HEAD equals
 * the pin, failing closed** on any mismatch or unrecoverable fetch.
 *
 * ## Trust boundary
 *
 * Mirrors the other credentialed bundles: the bearer (`SYSTEM_ACCESSTOKEN`) is
 * passed to the spawned `git` child via `GIT_CONFIG_*` env vars (see
 * `shared/git.ts::bearerEnv`) — never in argv, never written to `.git/config`.
 * The compiler-owned, non-secret `--dir` / `--sha` are argv flags (immune to
 * ADO pipeline-variable shadowing).
 *
 * ## Posture — FAIL CLOSED
 *
 * Unlike `prepare-pr-base` (which is a best-effort optimization and exits 0 on
 * failure), this bundle is a **security control**: if the exact pinned commit
 * cannot be obtained and verified, it exits non-zero so the custom job — and
 * the pipeline — fails rather than running an unverified revision.
 *
 *   Invocation: node checkout-component.js --dir <checkout-dir> --sha <40-hex>
 *               env: SYSTEM_ACCESSTOKEN (bearer for the git fetch)
 */
import {
  bearerEnv,
  gitOk as defaultGitOk,
  runGit as defaultRunGit,
  type GitResult,
} from "../shared/git.js";

const SHA40_RE = /^[0-9a-f]{40}$/i;

/** Injectable git runners (production uses the real ones; tests stub them). */
export type GitRunners = {
  runGit: (args: string[], env?: Record<string, string>) => GitResult;
  gitOk: (args: string[], env?: Record<string, string>) => string | null;
};

const defaultRunners: GitRunners = {
  runGit: defaultRunGit,
  gitOk: defaultGitOk,
};

export interface CheckoutArgs {
  /** The component checkout directory (as the compiler resolved it). */
  dir: string;
  /** The full 40-char commit SHA the component is pinned to. */
  sha: string;
}

/** Parse `--dir <path>` / `--sha <40-hex>` flags. */
export function parseArgs(argv: string[]): CheckoutArgs {
  let dir = "";
  let sha = "";
  for (let i = 0; i < argv.length; i++) {
    if (argv[i] === "--dir") {
      dir = argv[i + 1] ?? "";
      i++;
    } else if (argv[i] === "--sha") {
      sha = argv[i + 1] ?? "";
      i++;
    }
  }
  return { dir, sha };
}

/** True when the pinned commit object is already present locally. */
function shaPresent(sha: string, runners: GitRunners): boolean {
  return runners.runGit(["cat-file", "-e", `${sha}^{commit}`]).status === 0;
}

/**
 * Obtain the pinned commit object in the current working directory. Tries a
 * direct by-SHA fetch first, then progressively deepens the existing shallow
 * history until the object appears. Returns `true` once the object is present.
 */
function ensureShaFetched(
  sha: string,
  fetchEnv: Record<string, string>,
  runners: GitRunners,
): boolean {
  if (shaPresent(sha, runners)) {
    return true;
  }

  // 1. Direct by-SHA fetch (GitHub / GHE / Azure Repos support this).
  runners.runGit(["fetch", "--no-tags", "--depth", "1", "origin", sha], fetchEnv);
  if (shaPresent(sha, runners)) {
    return true;
  }

  // 2. Fall back to progressively deepening the checked-out history until the
  //    pinned object is reachable (servers that refuse by-SHA fetch).
  const depths = ["--depth=200", "--depth=500", "--depth=2000", "--unshallow"];
  for (const depthArg of depths) {
    runners.runGit(["fetch", "--no-tags", depthArg, "origin"], fetchEnv);
    if (shaPresent(sha, runners)) {
      return true;
    }
  }

  return false;
}

export function main(
  args: CheckoutArgs,
  env: NodeJS.ProcessEnv = process.env,
  runners: GitRunners = defaultRunners,
  chdir: (dir: string) => void = process.chdir.bind(process),
): number {
  const { dir, sha } = args;

  if (!SHA40_RE.test(sha)) {
    process.stderr.write(
      `[checkout-component] error: '--sha' must be a full 40-character commit SHA, got '${sha}'.\n`,
    );
    return 1;
  }
  if (dir.length === 0) {
    process.stderr.write("[checkout-component] error: '--dir' is required.\n");
    return 1;
  }

  try {
    chdir(dir);
  } catch (err) {
    // The pipeline just checked this dir out; a missing/unusable dir is a real
    // infra error, not a benign skip — fail closed.
    process.stderr.write(
      `[checkout-component] error: could not enter component dir '${dir}': ${(err as Error).message}.\n`,
    );
    return 1;
  }

  const fetchEnv = bearerEnv(env.SYSTEM_ACCESSTOKEN);

  if (!ensureShaFetched(sha, fetchEnv, runners)) {
    process.stderr.write(
      `[checkout-component] error: could not obtain pinned commit ${sha} in '${dir}' ` +
        "after a direct fetch and progressive deepening.\n",
    );
    return 1;
  }

  const checkout = runners.runGit(["checkout", "--detach", sha]);
  if (checkout.status !== 0) {
    process.stderr.write(
      `[checkout-component] error: 'git checkout --detach ${sha}' failed in '${dir}': ${checkout.stderr.trim()}.\n`,
    );
    return 1;
  }

  const actual = runners.gitOk(["rev-parse", "HEAD"]) ?? "";
  if (actual.toLowerCase() !== sha.toLowerCase()) {
    process.stderr.write(
      `[checkout-component] error: checkout resolved '${actual}', expected pinned '${sha}' in '${dir}'.\n`,
    );
    return 1;
  }

  process.stdout.write(
    `[checkout-component] verified component checkout at ${actual} in '${dir}'.\n`,
  );
  return 0;
}

// CLI entry guard: only run when invoked directly (not when imported by tests).
if (
  typeof process !== "undefined" &&
  process.argv[1] &&
  /checkout-component(\/index)?\.js$/.test(process.argv[1])
) {
  const args = parseArgs(process.argv.slice(2));
  process.exit(main(args));
}
