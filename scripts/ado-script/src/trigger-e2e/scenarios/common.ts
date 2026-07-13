/**
 * Shared helpers for trigger E2E scenarios: creating and tearing down real ADO
 * PR context.
 *
 * Reuses the executor-e2e `AdoRest` client and the ctx-free {@link Teardown}
 * helper so the create-branch → push-file → open-PR → set-labels pattern (and
 * its guaranteed-cleanup teardown) lives in one place, mirroring
 * `executor-e2e/scenarios/pr.ts`.
 *
 * Test-harness module; not shipped in `ado-script.zip`.
 */
import { Teardown } from "../../executor-e2e/scenarios/common.js";
import { SkipError } from "../scenario.js";
import type { TriggerContext } from "../scenario.js";

/** Remembered state for a created PR (ids needed for assertion + cleanup). */
export interface PrContext {
  readonly repo: string;
  readonly prId: number;
  /** Short branch name (no `refs/heads/`). */
  readonly branch: string;
  /** Full source ref (`refs/heads/<branch>`). */
  readonly sourceRef: string;
  /** Short target branch name (repo default, e.g. `main`). */
  readonly targetBranch: string;
}

/** Resolve a repo's default branch short name (e.g. `main`). */
async function defaultBranchShortName(ctx: TriggerContext, repo: string): Promise<string> {
  const info = await ctx.rest.getRepository(repo);
  return (info.defaultBranch ?? "refs/heads/main").replace(/^refs\/heads\//, "");
}

export interface CreatePrOptions {
  /** Scenario id, used for the deterministic branch/PR name prefix. */
  readonly id: string;
  /** Extra files to add on the source branch (path → contents). */
  readonly files?: Record<string, string>;
  /** PR labels to attach after creation. */
  readonly labels?: string[];
  /** PR title (default: `<prefix> (do not merge)`). */
  readonly title?: string;
  /** Open the PR in draft state (for the gate `draft` predicate). */
  readonly draft?: boolean;
}

/**
 * Create a real transient PR against the repo's default branch: pushes a new
 * source branch carrying one or more files (so ADO accepts a non-empty diff),
 * opens the PR, and attaches any labels.
 *
 * Follows the executor-e2e contract: because a `setup()` throw means the runner
 * will NOT call cleanup, this tears down anything it created before rethrowing.
 */
export async function createPrContext(
  ctx: TriggerContext,
  opts: CreatePrOptions,
): Promise<PrContext> {
  const repo = ctx.adoRepo;
  const targetBranch = await defaultBranchShortName(ctx, repo);
  const baseSha = await ctx.rest.getRefObjectId(repo, `heads/${targetBranch}`);
  if (!baseSha) throw new Error(`could not resolve ${targetBranch} HEAD in repo '${repo}'`);

  const branch = `${ctx.prefix(opts.id)}-src`;
  const sourceRef = `refs/heads/${branch}`;

  // Default file guarantees a real diff even when the scenario needs none.
  const files = opts.files ?? {
    [`/ado-aw-trig/${ctx.buildId}/${opts.id}.md`]: `trigger e2e ${opts.id} for build ${ctx.buildId}. Safe to delete.\n`,
  };

  // Push the branch with the first file, then add the rest in follow-up pushes.
  const entries = Object.entries(files);
  const [firstPath, firstContent] = entries[0]!;
  let baseCommit = baseSha;
  baseCommit = await ctx.rest.pushAddFileBranch(
    repo,
    branch,
    baseCommit,
    firstPath,
    firstContent,
    `trigger e2e ${opts.id}`,
  );

  // From here the source branch exists; a later throw must clean it up.
  try {
    for (const [path, content] of entries.slice(1)) {
      baseCommit = await ctx.rest.pushAddFileBranch(
        repo,
        sourceRef,
        baseCommit,
        path,
        content,
        `trigger e2e ${opts.id} (${path})`,
      );
    }

    const pr = await ctx.rest.createPullRequest(
      repo,
      branch,
      targetBranch,
      opts.title ?? `${ctx.prefix(opts.id)} (do not merge)`,
      `Deterministic trigger E2E ${opts.id} for build ${ctx.buildId}. Safe to delete.`,
      opts.draft,
    );

    if (opts.labels && opts.labels.length > 0) {
      await ctx.rest.setPullRequestLabels(repo, pr.pullRequestId, opts.labels);
    }

    return { repo, prId: pr.pullRequestId, branch, sourceRef, targetBranch };
  } catch (err) {
    // Best-effort delete of the branch we pushed before the failure.
    await ctx.rest.deleteRef(repo, sourceRef).catch(() => {});
    throw err;
  }
}

/** Abandon the PR and delete its source branch — every step always attempted. */
export async function teardownPrContext(ctx: TriggerContext, pr: PrContext): Promise<void> {
  await new Teardown()
    .add("abandon PR", () => ctx.rest.abandonPullRequest(pr.repo, pr.prId))
    .add("delete branch", () => ctx.rest.deleteRef(pr.repo, pr.sourceRef))
    .run();
}

/** Register cleanup of a queued victim build id (cancel if somehow still running). */
export async function cancelVictimIfRunning(ctx: TriggerContext, buildId: number): Promise<void> {
  const build = await ctx.rest.getBuild(buildId).catch(() => undefined);
  if (build && build.status !== "completed") {
    await ctx.rest.cancelBuild(buildId).catch(() => {});
  }
}

/**
 * Ensure a real ADO Git repo is available for PR creation. PR/synth/gate
 * scenarios need the victim pipeline's own `self` repo (where `exec-context-
 * pr-synth` looks up active PRs); when it is not supplied the scenario skips
 * rather than fails.
 */
export function requirePrRepo(ctx: TriggerContext): string {
  const repo = ctx.adoRepo?.trim();
  if (!repo) {
    throw new SkipError(
      "TRIGGER_E2E_VICTIM_REPO is not set; supply the ADO Git repo backing the victim pipeline to enable PR/synth/gate scenarios",
    );
  }
  return repo;
}
