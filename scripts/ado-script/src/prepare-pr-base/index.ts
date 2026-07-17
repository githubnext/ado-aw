/**
 * Prepare local refs required by create-pull-request on shallow Azure
 * Pipelines checkouts.
 *
 * `patch-base` runs in the Agent job. For same-organization Azure Repos it
 * asks ADO for the exact common commit and divergence counts, fetches only the
 * source/target ranges needed to reach that base, and verifies the result
 * locally. Other remotes use bounded dual-ref deepening (200/500/2000).
 *
 * `target-worktree` runs in SafeOutputs. It only fetches the target tip needed
 * by the executor's `git worktree add`.
 */
import {
  getCommitDiffMetadata as defaultGetCommitDiffMetadata,
  httpStatusCode,
  type CommitDiffMetadata,
} from "../shared/ado-client.js";
import {
  isCurrentAdoOrganization,
  parseAdoRepoUrl,
} from "../shared/ado-remote.js";
import { bearerEnv, gitOk as defaultGitOk, runGit as defaultRunGit } from "../shared/git.js";
import {
  ensureExactMergeBaseFetched,
  ensureRefsForMergeBaseFetched,
  ensureTargetTipFetched,
  type GitRunners,
} from "../shared/merge-base.js";
import { logWarning } from "../shared/vso-logger.js";

const defaultRunners: GitRunners = {
  runGit: defaultRunGit,
  gitOk: defaultGitOk,
};

export type PrepareMode = "patch-base" | "target-worktree";

export interface RepoTarget {
  dir: string;
  target: string;
  sourceRef?: string;
}

export interface PrepareArgs {
  mode: PrepareMode;
  repos: RepoTarget[];
  fallbackTarget: string;
  fallbackSourceRef?: string;
}

export interface PrepareDependencies {
  runners: GitRunners;
  chdir: (dir: string) => void;
  getCommitDiffMetadata: (
    project: string,
    repository: string,
    targetBranch: string,
    sourceCommit: string,
  ) => Promise<CommitDiffMetadata>;
}

const defaultDependencies: PrepareDependencies = {
  runners: defaultRunners,
  chdir: process.chdir.bind(process),
  getCommitDiffMetadata: defaultGetCommitDiffMetadata,
};

function shortTargetBranch(name: string): string {
  const short = name.replace(/^refs\/heads\//, "");
  return short.length > 0 ? short : name;
}

function oneLine(value: unknown, maxLength = 500): string {
  const text = String(value instanceof Error ? value.message : value)
    .replace(/[\r\n]+/g, " ")
    .trim();
  return text.length <= maxLength ? text : `${text.slice(0, maxLength)}...`;
}

function flushPending(
  repos: RepoTarget[],
  pending: Partial<RepoTarget> | null,
  fallbackTarget: string,
  fallbackSourceRef?: string,
): void {
  if (!pending?.dir) return;
  repos.push({
    dir: pending.dir,
    target: pending.target ?? fallbackTarget,
    sourceRef: pending.sourceRef ?? fallbackSourceRef,
  });
}

export function parseArgs(argv: string[]): PrepareArgs {
  const repos: RepoTarget[] = [];
  let mode: PrepareMode = "patch-base";
  let fallbackTarget = "main";
  let fallbackSourceRef: string | undefined;
  let pending: Partial<RepoTarget> | null = null;

  for (let i = 0; i < argv.length; i++) {
    const flag = argv[i];
    const value = argv[i + 1] ?? "";
    if (flag === "--mode") {
      if (value !== "patch-base" && value !== "target-worktree") {
        throw new Error(`Unsupported prepare-pr-base mode '${value}'.`);
      }
      mode = value;
      i++;
    } else if (flag === "--repo-dir") {
      flushPending(repos, pending, fallbackTarget, fallbackSourceRef);
      pending = { dir: value };
      i++;
    } else if (flag === "--source-ref") {
      if (pending?.dir) pending.sourceRef = value;
      else fallbackSourceRef = value;
      i++;
    } else if (flag === "--target-branch") {
      const target = shortTargetBranch(value || "main");
      if (pending?.dir) pending.target = target;
      else fallbackTarget = target;
      i++;
    }
  }
  flushPending(repos, pending, fallbackTarget, fallbackSourceRef);
  return { mode, repos, fallbackTarget, fallbackSourceRef };
}

function pointOriginHead(
  repoDir: string,
  target: string,
  runners: GitRunners,
): void {
  const sym = runners.runGit([
    "symbolic-ref",
    "refs/remotes/origin/HEAD",
    `refs/remotes/origin/${target}`,
  ]);
  if (sym.status !== 0) {
    process.stdout.write(
      `[prepare-pr-base] note: could not set origin/HEAD for '${repoDir}' (${oneLine(sym.stderr)}).\n`,
    );
  }
}

function warnRepo(repoDir: string, target: string, reason: string): void {
  logWarning(
    `[prepare-pr-base] '${repoDir}' target '${target}': ${oneLine(reason)} ` +
      "create-pull-request may fail. Set this checkout's fetch-depth to 0 only if the repository can afford full history.",
  );
}

async function preparePatchBase(
  repo: RepoTarget,
  env: NodeJS.ProcessEnv,
  fetchEnv: Record<string, string>,
  deps: PrepareDependencies,
): Promise<boolean> {
  const { runners } = deps;
  const headSha = runners.gitOk(["rev-parse", "HEAD"]) ?? "";
  const sourceRef = repo.sourceRef ?? env.BUILD_SOURCEBRANCH ?? headSha;
  let restReason = "origin is not an eligible same-organization Azure Repos remote";

  const remote = runners.gitOk(["remote", "get-url", "origin"]) ?? "";
  const identity = parseAdoRepoUrl(remote);
  const restDisabled = env.ADO_AW_PREPARE_PR_BASE_DISABLE_REST === "1";
  if (restDisabled) {
    restReason = "ADO REST disabled for deterministic fallback testing";
  } else if (identity && isCurrentAdoOrganization(identity, env)) {
    try {
      const metadata = await deps.getCommitDiffMetadata(
        identity.project,
        identity.repository,
        repo.target,
        headSha,
      );
      const exact = ensureExactMergeBaseFetched(repo.target, metadata, fetchEnv, runners);
      if (exact.ok) {
        pointOriginHead(repo.dir, repo.target, runners);
        process.stdout.write(
          `[prepare-pr-base] base ready in '${repo.dir}' via ado-rest ` +
            `(merge-base=${exact.baseSha}, ahead=${metadata.aheadCount}, behind=${metadata.behindCount}).\n`,
        );
        return true;
      }
      restReason = exact.reason;
    } catch (err) {
      const status = httpStatusCode(err);
      restReason = `${status ? `ADO REST ${status}: ` : "ADO REST unavailable: "}${oneLine(err)}`;
    }
  }

  const bounded = ensureRefsForMergeBaseFetched(
    sourceRef,
    repo.target,
    fetchEnv,
    runners,
  );
  if (!bounded.ok) {
    warnRepo(repo.dir, repo.target, `${restReason}; ${bounded.reason}`);
    return false;
  }
  pointOriginHead(repo.dir, repo.target, runners);
  process.stdout.write(
    `[prepare-pr-base] base ready in '${repo.dir}' via bounded git fetch ` +
      `(merge-base=${bounded.baseSha}; REST=${oneLine(restReason)}).\n`,
  );
  return true;
}

function prepareTargetWorktree(
  repo: RepoTarget,
  fetchEnv: Record<string, string>,
  deps: PrepareDependencies,
): boolean {
  const fetched = ensureTargetTipFetched(repo.target, fetchEnv, deps.runners);
  if (!fetched.ok) {
    warnRepo(repo.dir, repo.target, fetched.reason);
    return false;
  }
  pointOriginHead(repo.dir, repo.target, deps.runners);
  process.stdout.write(
    `[prepare-pr-base] target tip ready in '${repo.dir}' ` +
      `(origin/${repo.target}=${fetched.baseSha}).\n`,
  );
  return true;
}

export async function main(
  args: PrepareArgs,
  env: NodeJS.ProcessEnv = process.env,
  deps: PrepareDependencies = defaultDependencies,
): Promise<number> {
  let repos = args.repos;
  if (repos.length === 0) {
    repos = [
      {
        dir: env.BUILD_SOURCESDIRECTORY || ".",
        target: args.fallbackTarget,
        sourceRef: args.fallbackSourceRef ?? env.BUILD_SOURCEBRANCH,
      },
    ];
  }
  const fetchEnv = bearerEnv(env.SYSTEM_ACCESSTOKEN);

  for (const repo of repos) {
    try {
      deps.chdir(repo.dir);
    } catch (err) {
      warnRepo(repo.dir, repo.target, `could not enter checkout: ${oneLine(err)}`);
      continue;
    }
    if (args.mode === "target-worktree") {
      prepareTargetWorktree(repo, fetchEnv, deps);
    } else {
      await preparePatchBase(repo, env, fetchEnv, deps);
    }
  }
  return 0;
}

if (
  typeof process !== "undefined" &&
  process.argv[1] &&
  /prepare-pr-base(\/index)?\.js$/.test(process.argv[1].replace(/\\/g, "/"))
) {
  let args: PrepareArgs;
  try {
    args = parseArgs(process.argv.slice(2));
  } catch (err) {
    process.stderr.write(`[prepare-pr-base] ${oneLine(err)}\n`);
    process.exit(1);
  }
  void main(args).then(
    (code) => process.exit(code),
    (err) => {
      process.stderr.write(`[prepare-pr-base] fatal: ${oneLine(err)}\n`);
      process.exit(1);
    },
  );
}
