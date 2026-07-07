/**
 * Pull-request safe-output scenarios against the ADO `agent-definitions` repo:
 * add-pr-comment, reply-to-pr-comment, resolve-pr-thread, submit-pr-review,
 * update-pr.
 *
 * Each scenario deterministically creates a transient PR (with a real commit,
 * so ADO accepts it) and, where needed, a comment thread; asserts the effect;
 * then abandons the PR and deletes the source branch.
 *
 * Test-harness module; not shipped in `ado-script.zip`.
 */
import type { Scenario, ScenarioContext } from "../scenario.js";
import { defaultBranchShortName, detBody } from "./common.js";

interface PrState {
  repo: string;
  prId: number;
  branch: string;
  threadId?: number;
}

async function setupPr(
  ctx: ScenarioContext,
  tool: string,
  withThread: boolean,
): Promise<PrState> {
  const repo = ctx.adoRepo;
  const baseBranch = await defaultBranchShortName(ctx, repo);
  const baseSha = await ctx.rest.getRefObjectId(repo, `heads/${baseBranch}`);
  if (!baseSha) throw new Error(`could not resolve ${baseBranch} HEAD in repo '${repo}'`);

  const branch = `${ctx.prefix(tool)}-src`;
  await ctx.rest.pushAddFileBranch(
    repo,
    branch,
    baseSha,
    `/ado-aw-det/${ctx.buildId}/${tool}.md`,
    `${detBody(ctx, tool)}\n`,
    `deterministic executor e2e ${tool}`,
  );

  const pr = await ctx.rest.createPullRequest(
    repo,
    branch,
    baseBranch,
    `${ctx.prefix(tool)} (do not merge)`,
    detBody(ctx, tool),
  );

  const state: PrState = { repo, prId: pr.pullRequestId, branch };
  if (withThread) {
    try {
      const thread = await ctx.rest.createThread(repo, pr.pullRequestId, "seed thread for e2e");
      state.threadId = thread.id;
    } catch (err) {
      // The PR + source branch already exist; setup() throwing leaves
      // setupDone=false so the runner won't call cleanup. Abandon them here so
      // a flaky createThread doesn't leak dangling ADO objects, then rethrow.
      await teardownPr(ctx, state).catch(() => {});
      throw err;
    }
  }
  return state;
}

async function teardownPr(ctx: ScenarioContext, state: PrState): Promise<void> {
  await ctx.rest.abandonPullRequest(state.repo, state.prId);
  await ctx.rest.deleteRef(state.repo, `refs/heads/${state.branch}`);
}

export const addPrComment: Scenario<PrState> = {
  tool: "add-pr-comment",
  targetsAdoRepo: true,
  config: (ctx) => ({
    "allowed-repositories": [ctx.adoRepo],
    max: 1,
    "include-stats": false,
  }),
  setup: (ctx) => setupPr(ctx, "add-pr-comment", false),
  ndjson: async (ctx, state) => ({
    pull_request_id: state.prId,
    content: detBody(ctx, "add-pr-comment"),
    repository: ctx.adoRepo,
    status: "active",
  }),
  assert: async (ctx, state) => {
    const threads = await ctx.rest.listThreads(state.repo, state.prId);
    const found = threads.some((t) =>
      (t.comments ?? []).some((c) => (c.content ?? "").includes(`build ${ctx.buildId}`)),
    );
    if (!found) throw new Error(`no matching comment thread on PR #${state.prId}`);
  },
  cleanup: teardownPr,
};

export const replyToPrComment: Scenario<PrState> = {
  tool: "reply-to-pr-comment",
  targetsAdoRepo: true,
  config: (ctx) => ({ "allowed-repositories": [ctx.adoRepo], max: 1 }),
  setup: (ctx) => setupPr(ctx, "reply-to-pr-comment", true),
  ndjson: async (ctx, state) => {
    if (state.threadId === undefined) throw new Error(`[reply-to-pr-comment] threadId not set by setup`);
    return {
      pull_request_id: state.prId,
      thread_id: state.threadId,
      content: detBody(ctx, "reply-to-pr-comment"),
      repository: ctx.adoRepo,
    };
  },
  assert: async (ctx, state) => {
    if (state.threadId === undefined) throw new Error(`[reply-to-pr-comment] threadId not set by setup`);
    const thread = await ctx.rest.getThread(state.repo, state.prId, state.threadId);
    const replied = (thread.comments ?? []).some((c) => (c.content ?? "").includes(`build ${ctx.buildId}`));
    if (!replied) throw new Error(`reply not found on thread #${state.threadId}`);
  },
  cleanup: teardownPr,
};

export const resolvePrThread: Scenario<PrState> = {
  tool: "resolve-pr-thread",
  targetsAdoRepo: true,
  config: (ctx) => ({
    "allowed-repositories": [ctx.adoRepo],
    "allowed-statuses": ["fixed"],
    max: 1,
  }),
  setup: (ctx) => setupPr(ctx, "resolve-pr-thread", true),
  ndjson: async (ctx, state) => {
    if (state.threadId === undefined) throw new Error(`[resolve-pr-thread] threadId not set by setup`);
    return {
      pull_request_id: state.prId,
      thread_id: state.threadId,
      status: "fixed",
      repository: ctx.adoRepo,
    };
  },
  assert: async (ctx, state) => {
    if (state.threadId === undefined) throw new Error(`[resolve-pr-thread] threadId not set by setup`);
    const thread = await ctx.rest.getThread(state.repo, state.prId, state.threadId);
    // ADO returns thread status as either a numeric enum (2=fixed) or its
    // string name. We requested "fixed", so accept ONLY the "fixed" states —
    // resolving to wontFix/closed/byDesign instead would be an executor
    // regression the test must catch, not pass.
    const status = String(thread.status ?? "").toLowerCase();
    const resolved = new Set(["2", "fixed"]);
    if (!resolved.has(status)) {
      throw new Error(`thread #${state.threadId} not resolved to 'fixed' (got '${status}')`);
    }
  },
  cleanup: teardownPr,
};

export const submitPrReview: Scenario<PrState> = {
  tool: "submit-pr-review",
  targetsAdoRepo: true,
  config: (ctx) => ({ "allowed-events": ["approve"], "allowed-repositories": [ctx.adoRepo], max: 1 }),
  setup: (ctx) => setupPr(ctx, "submit-pr-review", false),
  ndjson: async (ctx, state) => ({
    pull_request_id: state.prId,
    event: "approve",
    body: detBody(ctx, "submit-pr-review"),
    repository: ctx.adoRepo,
  }),
  assert: async (ctx, state) => {
    const reviewers = await ctx.rest.listReviewers(state.repo, state.prId);
    const approved = reviewers.some((r) => r.vote > 0);
    if (!approved) throw new Error(`PR #${state.prId} has no approving reviewer vote`);
  },
  cleanup: teardownPr,
};

export const updatePr: Scenario<PrState> = {
  tool: "update-pr",
  targetsAdoRepo: true,
  config: (ctx) => ({
    "allowed-operations": ["update-description"],
    "allowed-repositories": [ctx.adoRepo],
    max: 1,
  }),
  setup: (ctx) => setupPr(ctx, "update-pr", false),
  ndjson: async (ctx, state) => ({
    pull_request_id: state.prId,
    repository: ctx.adoRepo,
    operation: "update-description",
    description: `${detBody(ctx, "update-pr")} (updated)`,
  }),
  assert: async (ctx, state) => {
    const pr = await ctx.rest.getPullRequest(state.repo, state.prId);
    if (!(pr.description ?? "").includes("(updated)")) {
      throw new Error(`PR #${state.prId} description was not updated`);
    }
  },
  cleanup: teardownPr,
};

export const prScenarios: Scenario<unknown>[] = [
  addPrComment,
  replyToPrComment,
  resolvePrThread,
  submitPrReview,
  updatePr,
];
