/**
 * Pull-request safe-output scenarios against the ADO `agent-definitions` repo:
 * add-pull-request-comment, reply-to-pull-request-comment, resolve-pull-request-thread, submit-pull-request-review,
 * focused PR content editing and abandonment.
 *
 * Each scenario deterministically creates a transient PR (with a real commit,
 * so ADO accepts it) and, where needed, a comment thread; asserts the effect;
 * then abandons the PR and deletes the source branch.
 *
 * Test-harness module; not shipped in `ado-script.zip`.
 */
import type { Scenario, ScenarioContext } from "../scenario.js";
import { defaultBranchShortName, detBody, Teardown } from "./common.js";
import { resolveExecutorE2eReviewer } from "./create-pull-request.js";

export interface PrState {
  repo: string;
  prId: number;
  branch: string;
  threadId?: number;
}

export async function setupPr(
  ctx: ScenarioContext,
  tool: string,
  withThread: boolean,
  draft?: boolean,
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

  // From here the source branch exists in ADO. setup() throwing leaves
  // setupDone=false so the runner won't call cleanup — so any failure after
  // the push must tear down what was created before rethrowing.
  let pr: { pullRequestId: number };
  try {
    pr = await ctx.rest.createPullRequest(
      repo,
      branch,
      baseBranch,
      `${ctx.prefix(tool)} (do not merge)`,
      detBody(ctx, tool),
      draft,
    );
  } catch (err) {
    await ctx.rest.deleteRef(repo, `refs/heads/${branch}`).catch(() => {});
    throw err;
  }

  const state: PrState = { repo, prId: pr.pullRequestId, branch };
  if (withThread) {
    try {
      const thread = await ctx.rest.createThread(repo, pr.pullRequestId, "seed thread for e2e");
      state.threadId = thread.id;
    } catch (err) {
      // Abandon the PR + delete the branch so a flaky createThread doesn't leak
      // dangling ADO objects, then rethrow.
      await teardownPr(ctx, state).catch(() => {});
      throw err;
    }
  }
  return state;
}

export async function teardownPr(ctx: ScenarioContext, state: PrState): Promise<void> {
  // Attempt both cleanups independently: if abandoning the PR throws (e.g. a
  // transient network error), the source branch must still be deleted so it is
  // not left orphaned for the janitor backstop to reap.
  await new Teardown()
    .add("abandon PR", () => ctx.rest.abandonPullRequest(state.repo, state.prId))
    .add("delete source branch", () =>
      ctx.rest.deleteRef(state.repo, `refs/heads/${state.branch}`),
    )
    .run();
}

async function setupLabeledPr(ctx: ScenarioContext, id: string): Promise<PrState> {
  const state = await setupPr(ctx, id, false);
  try {
    await ctx.rest.setPullRequestLabels(state.repo, state.prId, ["existing-label"]);
    const seeded = await ctx.rest.listPullRequestLabels(state.repo, state.prId);
    if (!seeded.some((label) => label.name === "existing-label")) {
      throw new Error(`Label setup did not persist existing-label: ${JSON.stringify(seeded)}`);
    }
  } catch (error) {
    await teardownPr(ctx, state);
    throw error;
  }
  return state;
}

export const addPrComment: Scenario<PrState> = {
  tool: "add-pull-request-comment",
  targetsAdoRepo: true,
  config: (ctx) => ({
    target: "*",
    "allowed-repositories": [ctx.adoRepo],
    max: 1,
    "include-stats": false,
  }),
  setup: (ctx) => setupPr(ctx, "add-pull-request-comment", false),
  ndjson: async (ctx, state) => ({
    pull_request_id: state.prId,
    content: detBody(ctx, "add-pull-request-comment"),
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
  tool: "reply-to-pull-request-comment",
  targetsAdoRepo: true,
  config: (ctx) => ({ target: "*", "allowed-repositories": [ctx.adoRepo], max: 1 }),
  setup: (ctx) => setupPr(ctx, "reply-to-pull-request-comment", true),
  ndjson: async (ctx, state) => {
    if (state.threadId === undefined) throw new Error(`[reply-to-pull-request-comment] threadId not set by setup`);
    return {
      pull_request_id: state.prId,
      thread_id: state.threadId,
      content: detBody(ctx, "reply-to-pull-request-comment"),
      repository: ctx.adoRepo,
    };
  },
  assert: async (ctx, state) => {
    if (state.threadId === undefined) throw new Error(`[reply-to-pull-request-comment] threadId not set by setup`);
    const thread = await ctx.rest.getThread(state.repo, state.prId, state.threadId);
    const replied = (thread.comments ?? []).some((c) => (c.content ?? "").includes(`build ${ctx.buildId}`));
    if (!replied) throw new Error(`reply not found on thread #${state.threadId}`);
  },
  cleanup: teardownPr,
};

export const resolvePrThread: Scenario<PrState> = {
  tool: "resolve-pull-request-thread",
  targetsAdoRepo: true,
  config: (ctx) => ({
    target: "*",
    "allowed-repositories": [ctx.adoRepo],
    "allowed-statuses": ["fixed"],
    max: 1,
  }),
  setup: (ctx) => setupPr(ctx, "resolve-pull-request-thread", true),
  ndjson: async (ctx, state) => {
    if (state.threadId === undefined) throw new Error(`[resolve-pull-request-thread] threadId not set by setup`);
    return {
      pull_request_id: state.prId,
      thread_id: state.threadId,
      status: "fixed",
      repository: ctx.adoRepo,
    };
  },
  assert: async (ctx, state) => {
    if (state.threadId === undefined) throw new Error(`[resolve-pull-request-thread] threadId not set by setup`);
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
  tool: "submit-pull-request-review",
  targetsAdoRepo: true,
  config: (ctx) => ({
    target: "*",
    "allowed-events": ["request-changes"],
    "allowed-repositories": [ctx.adoRepo],
    max: 1,
  }),
  setup: (ctx) => setupPr(ctx, "submit-pull-request-review", false),
  ndjson: async (ctx, state) => ({
    pull_request_id: state.prId,
    // Use "request-changes" (vote=-5), not a positive vote: the executor's
    // self-approval guard blocks approve/approve-with-suggestions on a PR the
    // authenticated identity created, and the harness (like the real pipeline)
    // creates and reviews the PR with the SAME identity. A negative vote
    // exercises the same submit path without tripping the guard.
    event: "request-changes",
    body: detBody(ctx, "submit-pull-request-review"),
    repository: ctx.adoRepo,
  }),
  assert: async (ctx, state) => {
    const reviewers = await ctx.rest.listReviewers(state.repo, state.prId);
    // "request-changes" maps to ADO vote=-5. Assert the exact vote so an
    // executor regression producing a different vote is caught.
    const voted = reviewers.some((r) => r.vote === -5);
    if (!voted) throw new Error(`PR #${state.prId} has no request-changes (vote=-5) reviewer`);
  },
  cleanup: teardownPr,
};

export const updatePullRequest: Scenario<PrState> = {
  tool: "update-pull-request",
  targetsAdoRepo: true,
  config: (ctx) => ({
    target: "*",
    "allowed-repositories": [ctx.adoRepo],
    "include-stats": false,
  }),
  setup: (ctx) => setupPr(ctx, "update-pull-request", false),
  ndjson: async (ctx, state) => ({
    pull_request_id: state.prId,
    repository: ctx.adoRepo,
    title: `${ctx.prefix("update-pull-request")} updated`,
    body: "x".repeat(4000),
  }),
  assert: async (ctx, state) => {
    const pr = await ctx.rest.getPullRequest(state.repo, state.prId);
    if (pr.description !== "x".repeat(4000) || !pr.title.endsWith(" updated")) {
      throw new Error("PR title or exact 4000-character description was not persisted");
    }
  },
  cleanup: teardownPr,
};

export const abandonPullRequest: Scenario<PrState> = {
  tool: "abandon-pull-request",
  targetsAdoRepo: true,
  config: (ctx) => ({
    target: "*",
    "allowed-repositories": [ctx.adoRepo],
    "include-stats": false,
  }),
  setup: (ctx) => setupPr(ctx, "abandon-pull-request", false),
  ndjson: async (ctx, state) => ({
    pull_request_id: state.prId,
    repository: ctx.adoRepo,
    body: detBody(ctx, "abandon-pull-request"),
  }),
  assert: async (ctx, state) => {
    const pr = await ctx.rest.getPullRequest(state.repo, state.prId);
    if (pr.status !== "abandoned") throw new Error("PR was not abandoned");
    const threads = await ctx.rest.listThreads(state.repo, state.prId);
    if (!threads.some((thread) => thread.comments?.some(
      (comment) => comment.content === detBody(ctx, "abandon-pull-request"),
    ))) throw new Error("Abandonment comment was not posted");
  },
  cleanup: teardownPr,
};

export const updatePullRequestIsland: Scenario<PrState> = {
  id: "update-pull-request-island",
  tool: "update-pull-request",
  targetsAdoRepo: true,
  config: (ctx) => ({
    target: "*",
    "allowed-repositories": [ctx.adoRepo],
    operation: "replace-island",
    "include-stats": false,
    max: 2,
  }),
  setup: (ctx) => setupPr(ctx, "update-pull-request-island", false),
  priorEntries: async (ctx, state) => [{
    tool: "update-pull-request",
    config: {
      target: "*", "allowed-repositories": [ctx.adoRepo],
      operation: "replace-island", "include-stats": false, max: 2,
    },
    entry: { pull_request_id: state.prId, repository: ctx.adoRepo, body: "first island report" },
  }],
  env: async () => ({ SYSTEM_DEFINITIONID: "123" }),
  ndjson: async (ctx, state) => ({
    pull_request_id: state.prId, repository: ctx.adoRepo, body: "updated island report",
  }),
  assert: async (ctx, state) => {
    const pr = await ctx.rest.getPullRequest(state.repo, state.prId);
    const body = pr.description ?? "";
    if (!body.startsWith(detBody(ctx, "update-pull-request-island"))
      || !body.includes("updated island report")
      || body.includes("first island report")
      || body.split("ado-aw-pr-island-start:").length !== 2) {
      throw new Error("PR island rerun did not preserve surrounding text and replace the one section");
    }
  },
  cleanup: teardownPr,
};

export const updatePullRequestOversized: Scenario<PrState> = {
  id: "update-pull-request-oversized",
  tool: "update-pull-request",
  targetsAdoRepo: true,
  config: (ctx) => ({
    target: "*", "allowed-repositories": [ctx.adoRepo], "include-stats": false,
  }),
  setup: (ctx) => setupPr(ctx, "update-pull-request-oversized", false),
  ndjson: async (ctx, state) => ({
    pull_request_id: state.prId, repository: ctx.adoRepo, body: "x".repeat(4001),
  }),
  expectedFailure: { error: /4000|4,000/ },
  assertFailure: async (ctx, state) => {
    const pr = await ctx.rest.getPullRequest(state.repo, state.prId);
    if (pr.description !== detBody(ctx, "update-pull-request-oversized")) {
      throw new Error("Rejected oversized body changed the live description");
    }
  },
  assert: async () => {
    throw new Error("Oversized PR description unexpectedly succeeded");
  },
  cleanup: teardownPr,
};

export const updatePullRequestUnicode: Scenario<PrState> = {
  ...updatePullRequest,
  id: "update-pull-request-unicode",
  setup: (ctx) => setupPr(ctx, "update-pull-request-unicode", false),
  ndjson: async (ctx, state) => ({
    pull_request_id: state.prId, repository: ctx.adoRepo,
    body: "\u{1f600}".repeat(2000),
  }),
  assert: async (ctx, state) => {
    const pr = await ctx.rest.getPullRequest(state.repo, state.prId);
    if (pr.description !== "\u{1f600}".repeat(2000)) {
      throw new Error("ADO did not preserve the exact 4000-UTF16-unit non-BMP description");
    }
  },
};

export const updatePullRequestUnicodeOversized: Scenario<PrState> = {
  ...updatePullRequestOversized,
  id: "update-pull-request-unicode-oversized",
  setup: (ctx) => setupPr(ctx, "update-pull-request-unicode-oversized", false),
  ndjson: async (ctx, state) => ({
    pull_request_id: state.prId, repository: ctx.adoRepo,
    body: "\u{1f600}".repeat(2000) + "x",
  }),
  assertFailure: async (ctx, state) => {
    const pr = await ctx.rest.getPullRequest(state.repo, state.prId);
    if (pr.description !== detBody(ctx, "update-pull-request-unicode-oversized")) {
      throw new Error("Rejected Unicode description changed the live PR");
    }
  },
};

export const updatePullRequestComposedOversized: Scenario<PrState> = {
  ...updatePullRequestOversized,
  id: "update-pull-request-composed-oversized",
  setup: (ctx) => setupPr(ctx, "update-pull-request-composed-oversized", false),
  ndjson: async (ctx, state) => ({
    pull_request_id: state.prId, repository: ctx.adoRepo,
    body: "x".repeat(4000), operation: "append",
  }),
  assertFailure: async (ctx, state) => {
    const pr = await ctx.rest.getPullRequest(state.repo, state.prId);
    if (pr.description !== detBody(ctx, "update-pull-request-composed-oversized")) {
      throw new Error("Rejected assembled description changed the live PR");
    }
  },
};

interface ReviewerState extends PrState { reviewer: string }
export const addPrReviewers: Scenario<ReviewerState> = {
  tool: "add-pull-request-reviewers",
  targetsAdoRepo: true,
  config: (ctx, state) => ({
    target: "*",
    "allowed-repositories": [ctx.adoRepo], "allowed-reviewers": [state.reviewer], "max-reviewers": 1,
  }),
  setup: async (ctx) => {
    const name = resolveExecutorE2eReviewer();
    const reviewer = await ctx.rest.resolveIdentityId(name);
    if (!reviewer) throw new Error("Configured reviewer does not resolve exactly");
    return { ...await setupPr(ctx, "add-pull-request-reviewers", false), reviewer };
  },
  ndjson: async (ctx, state) => ({
    pull_request_id: state.prId, repository: ctx.adoRepo, reviewers: [state.reviewer],
  }),
  assert: async (ctx, state) => {
    const reviewers = await ctx.rest.listReviewers(state.repo, state.prId);
    if (!reviewers.some((reviewer) => reviewer.id.toLowerCase() === state.reviewer.toLowerCase())) {
      throw new Error("Requested reviewer is missing from the target PR");
    }
  },
  cleanup: teardownPr,
};

export const addPrLabels: Scenario<PrState> = {
  tool: "add-pull-request-labels",
  targetsAdoRepo: true,
  config: (ctx) => ({ target: "*", "allowed-repositories": [ctx.adoRepo] }),
  setup: (ctx) => setupLabeledPr(ctx, "add-pull-request-labels"),
  ndjson: async (ctx, state) => ({
    pull_request_id: state.prId, repository: ctx.adoRepo, labels: ["new-label"],
  }),
  assert: async (ctx, state) => {
    const labels = (await ctx.rest.listPullRequestLabels(state.repo, state.prId))
      .map((label) => label.name);
    if (!labels.includes("existing-label") || !labels.includes("new-label")) {
      throw new Error(`Label addition did not preserve both labels: ${JSON.stringify(labels)}`);
    }
  },
  cleanup: teardownPr,
};

interface AutoCompleteState extends PrState { targetBranch: string }

export const setPrAutoComplete: Scenario<AutoCompleteState> = {
  tool: "set-pull-request-auto-complete",
  targetsAdoRepo: true,
  config: (ctx) => ({
    target: "*",
    "allowed-repositories": [ctx.adoRepo],
    "delete-source-branch": false,
    "merge-strategy": "squash",
  }),
  setup: async (ctx) => {
    const repo = ctx.adoRepo;
    const base = await defaultBranchShortName(ctx, repo);
    const sha = await ctx.rest.getRefObjectId(repo, `heads/${base}`);
    if (!sha) throw new Error("Default branch has no tip");
    const targetBranch = `${ctx.prefix("set-pull-request-auto-complete")}-target`;
    const branch = `${ctx.prefix("set-pull-request-auto-complete")}-src`;
    await ctx.rest.pushAddFileBranch(repo, targetBranch, sha,
      `/ado-aw-det/${ctx.buildId}/autocomplete-target.md`, "isolated target", "prepare isolated completion target");
    let sourceCreated = false;
    try {
      const tip = await ctx.rest.getRefObjectId(repo, `heads/${targetBranch}`);
      if (!tip) throw new Error("Isolated target branch has no tip");
      await ctx.rest.pushAddFileBranch(repo, branch, tip,
        `/ado-aw-det/${ctx.buildId}/autocomplete-source.md`, "isolated source", "prepare completion source");
      sourceCreated = true;
      const pr = await ctx.rest.createPullRequest(repo, branch, targetBranch,
        ctx.prefix("set-pull-request-auto-complete"), "Completes only into a disposable test branch.");
      return { repo, prId: pr.pullRequestId, branch, targetBranch };
    } catch (error) {
      const cleanup = new Teardown();
      if (sourceCreated) cleanup.add("delete source", () => ctx.rest.deleteRef(repo, `refs/heads/${branch}`));
      await cleanup.add("delete target", () => ctx.rest.deleteRef(repo, `refs/heads/${targetBranch}`)).run();
      throw error;
    }
  },
  ndjson: async (ctx, state) => ({ pull_request_id: state.prId, repository: ctx.adoRepo }),
  assert: async (ctx, state) => {
    const pr = await ctx.rest.getPullRequest(state.repo, state.prId);
    if (!pr.autoCompleteSetBy?.id && pr.status !== "completed") {
      throw new Error("Auto-complete was neither set nor completed into the disposable target");
    }
  },
  cleanup: async (ctx, state) => {
    await new Teardown()
      .add("abandon active PR", async () => {
        const pr = await ctx.rest.getPullRequest(state.repo, state.prId);
        if (pr.status === "active") await ctx.rest.abandonPullRequest(state.repo, state.prId);
      })
      .add("delete source", () => ctx.rest.deleteRef(state.repo, `refs/heads/${state.branch}`))
      .add("delete isolated target", () => ctx.rest.deleteRef(state.repo, `refs/heads/${state.targetBranch}`))
      .run();
  },
};

const requiredLabelScenarios: Scenario<PrState>[] = [updatePullRequest, abandonPullRequest]
  .map((scenario) => ({
    ...scenario,
    id: `${scenario.tool}-required-labels`,
    config: (ctx, state) => ({
      ...scenario.config(ctx, state),
      "required-labels": ["existing-label"],
    }),
    setup: (ctx) => setupLabeledPr(ctx, `${scenario.tool}-required-labels`),
  }));

const updatePullRequestDeniedRepository: Scenario<PrState> = {
  ...updatePullRequest,
  id: "update-pull-request-denied-repository",
  config: () => ({ target: "*", "allowed-repositories": ["not-selected"] }),
  setup: (ctx) => setupPr(ctx, "update-pull-request-denied-repository", false),
  expectedFailure: { error: /allowed-repositories/ },
  assertFailure: async (ctx, state) => {
    const pr = await ctx.rest.getPullRequest(state.repo, state.prId);
    const id = "update-pull-request-denied-repository";
    if (pr.description !== detBody(ctx, id) || pr.title !== `${ctx.prefix(id)} (do not merge)`) {
      throw new Error("Denied repository request changed the disposable PR");
    }
  },
};

const reviewVoteScenarios: Scenario<PrState>[] = (["comment", "reset"] as const).map((event): Scenario<PrState> => ({
  id: `pr-review-${event}-vote`,
  tool: "submit-pull-request-review",
  targetsAdoRepo: true,
  config: (ctx) => ({
    target: "*", "allowed-repositories": [ctx.adoRepo],
    "allowed-events": ["request-changes", event], max: 2,
  }),
  setup: (ctx) => setupPr(ctx, `pr-review-${event}-vote`, false),
  priorEntries: async (ctx, state) => [{
    tool: "submit-pull-request-review",
    config: { target: "*", "allowed-events": ["request-changes", event], max: 2 },
    entry: { pull_request_id: state.prId, repository: state.repo, event: "request-changes",
      body: detBody(ctx, "seed negative review vote") },
  }],
  ndjson: async (ctx, state) => ({
    pull_request_id: state.prId, repository: state.repo, event,
    ...(event === "comment" ? { body: detBody(ctx, "non-voting informational review") } : {}),
  }),
  assert: async (ctx, state, record, records) => {
    const actor = records[0]?.result?.reviewer_id;
    if (typeof actor !== "string") throw new Error("Seed review did not report its authenticated reviewer ID");
    const reviewers = await ctx.rest.listReviewers(state.repo,state.prId);
    const own = reviewers.find((reviewer) => reviewer.id === actor);
    if (own?.vote !== (event === "comment" ? -5 : 0)) {
      throw new Error(`${event} did not preserve/reset the exact seeded actor's vote`);
    }
    if (record.result?.vote_changed !== (event === "reset")) {
      throw new Error(`${event} misreported its vote effect`);
    }
    if (event === "comment" && record.result?.comment_status !== "posted") {
      throw new Error("Non-voting review did not post its informational content");
    }
  },
  cleanup: teardownPr,
}));

const labelLifecycleScenarios: Scenario<PrState>[] = (["remove", "replace"] as const).map((operation): Scenario<PrState> => ({
  tool: operation === "remove" ? "remove-pull-request-labels" : "replace-pull-request-label",
  targetsAdoRepo: true,
  config: (ctx) => ({
    target: "*", "allowed-repositories": [ctx.adoRepo],
    ...(operation === "remove" ? { "allowed-labels": ["existing-label"] } : {
      "allowed-add": ["replacement-label"], "allowed-remove": ["existing-label"],
      "allowed-transitions": [{ from: "existing-label", to: "replacement-label" }],
    }),
  }),
  setup: async (ctx) => {
    const state = await setupLabeledPr(ctx, `pr-label-${operation}`);
    try {
      await ctx.rest.setPullRequestLabels(state.repo,state.prId,["preserved-label"]);
      return state;
    } catch (error) {
      await teardownPr(ctx,state);
      throw error;
    }
  },
  ndjson: async (_ctx,state) => ({
    pull_request_id: state.prId, repository: state.repo,
    ...(operation === "remove" ? { labels: ["existing-label"] } : { from: "existing-label", to: "replacement-label" }),
  }),
  assert: async (ctx,state) => {
    const labels=(await ctx.rest.listPullRequestLabels(state.repo,state.prId)).map((label)=>label.name);
    if (labels.includes("existing-label") || !labels.includes("preserved-label")
      || (operation === "replace" && !labels.includes("replacement-label"))) {
      throw new Error(`Label ${operation} did not persist exactly or lost an unrelated label`);
    }
  },
  cleanup: teardownPr,
}));

const labelPolicyScenarios: Scenario<PrState>[] = (["limit", "blocked"] as const).map((kind): Scenario<PrState> => ({
  id: `pr-label-${kind}-denied`,
  tool: "add-pull-request-labels",
  targetsAdoRepo: true,
  config: (ctx) => ({ target:"*", "allowed-repositories":[ctx.adoRepo], "max-labels":10,
    ...(kind === "blocked" ? { "allowed-labels":["permitted","blocked"], "blocked-labels":["blocked"] } : {}),
  }),
  setup: (ctx) => setupLabeledPr(ctx,`pr-label-${kind}-denied`),
  ndjson: async (_ctx,state) => ({pull_request_id:state.prId,repository:state.repo,
    labels:kind === "limit" ? Array.from({length:11},(_,index)=>`limit-${index}`) : ["permitted","blocked"]}),
  expectedFailure: { error:kind === "limit" ? /max-labels/ : /blocked-labels/ },
  assert: async () => { throw new Error("A denied label batch must not succeed"); },
  assertFailure: async (ctx,state) => {
    const labels=await ctx.rest.listPullRequestLabels(state.repo,state.prId);
    if (labels.length !== 1 || labels[0]?.name !== "existing-label") {
      throw new Error("A denied label batch made a partial mutation");
    }
  },
  cleanup: teardownPr,
}));

const publishDraft: Scenario<PrState> = {
  tool: "mark-pull-request-as-ready-for-review",
  targetsAdoRepo: true,
  config: (ctx) => ({ target:"*", "allowed-repositories":[ctx.adoRepo], max:2 }),
  setup: async (ctx) => {
    const state=await setupPr(ctx,"publish-draft",false,true);
    try {
      if ((await ctx.rest.getPullRequest(state.repo,state.prId)).isDraft !== true) {
        throw new Error("Publication test did not create a persisted draft");
      }
      return state;
    } catch(error) {
      await teardownPr(ctx,state);
      throw error;
    }
  },
  priorEntries: async (_ctx,state) => [{
    tool:"mark-pull-request-as-ready-for-review",
    config:{target:"*",max:2},
    entry:{pull_request_id:state.prId,repository:state.repo},
  }],
  ndjson: async (_ctx,state) => ({pull_request_id:state.prId,repository:state.repo}),
  assert: async (ctx,state,record,records) => {
    const pr=await ctx.rest.getPullRequest(state.repo,state.prId);
    if (pr.isDraft !== false || pr.status !== "active" || pr.autoCompleteSetBy) {
      throw new Error("Publication was not persisted or unexpectedly enabled completion");
    }
    if (pr.title !== `${ctx.prefix("publish-draft")} (do not merge)` || pr.description !== detBody(ctx,"publish-draft")) {
      throw new Error("Publication changed PR content");
    }
    if (records[0]?.result?.publication_status !== "confirmed" || record.result?.already_ready !== true) {
      throw new Error("Publication/repeat no-op results are not authoritative");
    }
  },
  cleanup:teardownPr,
};

export const prScenarios: Scenario<unknown>[] = [
  addPrComment,
  replyToPrComment,
  resolvePrThread,
  submitPrReview,
  ...reviewVoteScenarios,
  updatePullRequest,
  abandonPullRequest,
  ...requiredLabelScenarios,
  updatePullRequestDeniedRepository,
  updatePullRequestIsland,
  updatePullRequestOversized,
  updatePullRequestUnicode,
  updatePullRequestUnicodeOversized,
  updatePullRequestComposedOversized,
  addPrReviewers,
  addPrLabels,
  ...labelLifecycleScenarios,
  ...labelPolicyScenarios,
  publishDraft,
  setPrAutoComplete,
];
