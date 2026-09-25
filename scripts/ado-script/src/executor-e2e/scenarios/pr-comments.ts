import { join } from "node:path";
import { createHash } from "node:crypto";
import type { Scenario, ScenarioContext } from "../scenario.js";
import { runExecute } from "../execute-cli.js";
import { setupPr, teardownPr, type PrState } from "./pr.js";
import { defaultBranchShortName, Teardown } from "./common.js";

const original = "An owned automated report with `code` and preserved history.";
const replacement = "The updated automated report with `Vec<T>` preserved.";

function updatedOwnedContent(content: string): string {
  return `${content}\n\n<!-- ado-aw-content-sha256:${createHash("sha256").update(content).digest("hex")} -->`;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

interface OwnedState extends PrState {
  threadId: number;
  commentId: number;
  before: string;
}

async function seed(ctx: ScenarioContext, id: string, mutation?: "edit" | "reply"): Promise<OwnedState> {
  const pr = await setupPr(ctx, id, false);
  try {
    const run = await runExecute({
      adoAwBin: ctx.adoAwBin, scenarioDir: join(ctx.workDir, `${id}-seed`),
      tool: "add-pull-request-comment", config: { target: "*", "include-stats": false, "comment-key": "owned-report" },
      entry: { pull_request_id: pr.prId, repository: pr.repo, content: original, status: "active" },
      adoRepo: pr.repo, orgUrl: ctx.orgUrl, project: ctx.project, token: ctx.token, log: ctx.log,
      extraEnv: { BUILD_BUILDID: "1" },
    });
    const threadId = run.record?.result?.thread_id;
    if (run.exitCode !== 0 || typeof threadId !== "number") {
      throw new Error(`Owned-comment seed failed: ${run.stderr}`);
    }
    const thread = await ctx.rest.getThread(pr.repo, pr.prId, threadId);
    const commentId = thread.comments?.[0]?.id;
    if (typeof commentId !== "number") throw new Error("Seed comment has no ID");
    let before = original;
    if (mutation) {
      const path = `${ctx.orgUrl.replace(/\/+$/, "")}/${encodeURIComponent(ctx.project)}`
        + `/_apis/git/repositories/${encodeURIComponent(pr.repo)}/pullRequests/${pr.prId}/threads/${threadId}/comments`
        + (mutation === "edit" ? `/${commentId}` : "");
      before = mutation === "edit" ? "A manual edit to the old automated report." : original;
      const response = await fetch(`${path}?api-version=7.1`, {
        method: mutation === "edit" ? "PATCH" : "POST",
        headers: { Authorization: `Basic ${Buffer.from(`:${ctx.token}`).toString("base64")}`, "Content-Type": "application/json" },
        body: JSON.stringify(mutation === "edit" ? { content: before } :
          { parentCommentId: commentId, content: "A discussion reply must remain untouched.", commentType: 1 }),
        signal: AbortSignal.timeout(30_000),
      });
      if (!response.ok) throw new Error(`Owned-comment precondition failed: HTTP ${response.status}`);
    }
    return { ...pr, threadId, commentId, before };
  } catch (error) {
    await teardownPr(ctx, pr);
    throw error;
  }
}

const update: Scenario<OwnedState> = {
  tool: "update-pull-request-comment",
  targetsAdoRepo: true,
  config: () => ({ target: "*", "comment-key": "owned-report" }),
  setup: (ctx) => seed(ctx, "update-owned-comment"),
  ndjson: async (_ctx, state) => ({
    pull_request_id: state.prId, repository: state.repo,
    thread_id: state.threadId, comment_id: state.commentId, content: replacement,
  }),
  assert: async (ctx, state) => {
    const thread = await ctx.rest.getThread(state.repo, state.prId, state.threadId);
    if (thread.comments?.find((comment) => comment.id === state.commentId)?.content !== updatedOwnedContent(replacement)) {
      throw new Error("Owned comment content was not updated faithfully");
    }
    if (String(thread.status).toLowerCase() !== "active" && thread.status !== 1) {
      throw new Error("Owned content update unexpectedly changed thread status");
    }
  },
  cleanup: teardownPr,
};

const denied: Scenario<OwnedState> = {
  ...update,
  id: "pr-owned-comment-human-edit-denied",
  setup: (ctx) => seed(ctx, "owned-comment-edit-denied", "edit"),
  expectedFailure: { error: /content hash/ },
  assertFailure: async (ctx, state) => {
    const thread = await ctx.rest.getThread(state.repo, state.prId, state.threadId);
    if (thread.comments?.find((comment) => comment.id === state.commentId)?.content !== state.before) {
      throw new Error("The manually edited bot comment was overwritten");
    }
  },
};

const supersession: Scenario<OwnedState>[] = [false, true].map((withReply): Scenario<OwnedState> => ({
  id: withReply ? "pr-owned-comment-replies-preserved" : "pr-owned-comment-superseded",
  tool: "add-pull-request-comment",
  targetsAdoRepo: true,
  config: () => ({
    target: "*", "comment-key": "owned-report", "include-stats": false,
    "supersede-older-comments": true, "max-superseded-comments": 2,
  }),
  setup: (ctx) => seed(ctx, withReply ? "preserve-owned-conversation" : "supersede-owned-comment", withReply ? "reply" : undefined),
  ndjson: async (_ctx, state) => ({ pull_request_id: state.prId, repository: state.repo, content: replacement, status: "active" }),
  assert: async (ctx, state, record) => {
    const thread = await ctx.rest.getThread(state.repo, state.prId, state.threadId);
    const content = thread.comments?.find((comment) => comment.id === state.commentId)?.content;
    if (withReply) {
      if (content !== original || thread.comments?.length !== 2 || !["1", "active"].includes(String(thread.status).toLowerCase())) {
        throw new Error("Supersession modified a conversation with replies");
      }
    } else if (!content?.startsWith(original) || !content.includes("Superseded") ||
      !["4", "closed"].includes(String(thread.status).toLowerCase())) {
      throw new Error("Supersession did not preserve history and close the old thread");
    }
    const newId = record.result?.thread_id;
    if (typeof newId !== "number" || newId === state.threadId) throw new Error("Replacement comment was not created");
    const newer = await ctx.rest.getThread(state.repo, state.prId, newId);
    if (newer.comments?.[0]?.content !== replacement) throw new Error("Replacement comment content was not persisted");
  },
  cleanup: teardownPr,
}));

interface InlineState extends PrState {
  target?: string;
  head: string;
  file: string;
}

async function inlineSetup(ctx: ScenarioContext, side: "left" | "right"): Promise<InlineState> {
  if (side === "right") {
    const state = await setupPr(ctx, "inline-right", false);
    try {
      const head = await ctx.rest.getRefObjectId(state.repo, `heads/${state.branch}`);
      if (!head) throw new Error("Inline fixture source head missing");
      return { ...state, head, file: `ado-aw-det/${ctx.buildId}/inline-right.md` };
    } catch (error) {
      await teardownPr(ctx, state);
      throw error;
    }
  }
  const repo = ctx.adoRepo;
  const base = await defaultBranchShortName(ctx, repo);
  const baseSha = await ctx.rest.getRefObjectId(repo, `heads/${base}`);
  if (!baseSha) throw new Error("Inline deletion fixture base missing");
  const target = `${ctx.prefix("inline-left")}-target`;
  const branch = `${ctx.prefix("inline-left")}-source`;
  const file = `ado-aw-det/${ctx.buildId}/deleted.md`;
  const parent = await ctx.rest.pushAddFileBranch(repo, target, baseSha, `/${file}`, "Deleted fixture line.\n", "Disposable inline target");
  let head: string;
  try {
    head = await ctx.rest.pushDeleteFileBranch(repo, branch, parent, `/${file}`);
    const pr = await ctx.rest.createPullRequest(repo, branch, target, `${ctx.prefix("inline-left")} (do not merge)`, "Disposable left-side deleted-file test.", true);
    return { repo, prId: pr.pullRequestId, branch, target, head, file };
  } catch (error) {
    await new Teardown()
      .add("delete owned inline source", () => ctx.rest.deleteRef(repo, `refs/heads/${branch}`))
      .add("delete owned inline target", () => ctx.rest.deleteRef(repo, `refs/heads/${target}`)).run();
    throw error;
  }
}

const inlineCases: Scenario<InlineState>[] = (["right", "left", "stale"] as const).map((mode): Scenario<InlineState> => ({
  id: `pr-inline-${mode}`,
  tool: "add-pull-request-comment",
  targetsAdoRepo: true,
  config: () => ({ target: "*", "include-stats": false }),
  setup: (ctx) => inlineSetup(ctx, mode === "left" ? "left" : "right"),
  ndjson: async (_ctx, state) => ({
    pull_request_id: state.prId, repository: state.repo, file_path: state.file,
    side: mode === "left" ? "left" : "right", line: 1,
    expected_head_sha: mode === "stale" ? "f".repeat(40) : state.head,
    content: "Comment on the exact reviewed file revision.", status: "active",
  }),
  ...(mode === "stale" ? { expectedFailure: { error: /head changed/ } } : {}),
  assertFailure: async (ctx, state) => {
    if ((await ctx.rest.listThreads(state.repo, state.prId)).length !== 0) throw new Error("Stale inline proposal created a thread");
  },
  assert: async (ctx, state, record) => {
    const id = record.result?.thread_id;
    if (typeof id !== "number") throw new Error("Inline comment response has no thread ID");
    const thread = await ctx.rest.getThread(state.repo, state.prId, id);
    if (!thread.comments?.some((comment) => comment.content === "Comment on the exact reviewed file revision.")) {
      throw new Error("Inline comment was not persisted");
    }
    const url = `${ctx.orgUrl.replace(/\/+$/, "")}/${encodeURIComponent(ctx.project)}/_apis/git/repositories/${encodeURIComponent(state.repo)}`
      + `/pullRequests/${state.prId}/threads/${id}?api-version=7.1`;
    const response = await fetch(url, { headers: { Authorization: `Basic ${Buffer.from(`:${ctx.token}`).toString("base64")}` } });
    if (!response.ok) throw new Error(`Inline readback failed: HTTP ${response.status}`);
    const detail: unknown = await response.json();
    if (!isRecord(detail) || !isRecord(detail.threadContext) || !isRecord(detail.pullRequestThreadContext)) {
      throw new Error("Inline readback omitted its context");
    }
    const start = mode === "left" ? detail.threadContext.leftFileStart : detail.threadContext.rightFileStart;
    if (!isRecord(start) || start.line !== 1 || detail.threadContext.filePath !== `/${state.file}` ||
      typeof detail.pullRequestThreadContext.changeTrackingId !== "number") {
      throw new Error("Inline side/path/iteration tracking was not persisted");
    }
  },
  cleanup: async (ctx, state) => {
    const cleanup = new Teardown().add("PR/source cleanup", () => teardownPr(ctx, state));
    if (state.target) cleanup.add("inline target cleanup", () => ctx.rest.deleteRef(state.repo, `refs/heads/${state.target}`));
    await cleanup.run();
  },
}));

const batches: Scenario<InlineState>[] = (["valid", "invalid-last", "not-enabled"] as const).map((mode): Scenario<InlineState> => ({
  id: `pr-review-batch-${mode}`,
  tool: "submit-pull-request-review",
  targetsAdoRepo: true,
  config: () => ({
    target: "*", "allowed-events": ["comment"], "max-comments": mode === "not-enabled" ? 0 : 2,
    "comment-key": "batch-report",
  }),
  setup: (ctx) => inlineSetup(ctx, "right"),
  ndjson: async (_ctx, state) => ({
    pull_request_id: state.prId, repository: state.repo, event: "comment",
    body: "Consolidated review summary without a vote.", expected_head_sha: state.head,
    comments: [
      { file_path: state.file, side: "right", line: 1, content: "First inline finding in this review." },
      { file_path: state.file, side: "right", line: mode === "invalid-last" ? 99999 : 1,
        content: "Second inline finding in this review." },
    ],
  }),
  ...(mode !== "valid" ? {
    expectedFailure: { error: mode === "not-enabled" ? /max-comments/ : /ending line.*outside/ },
  } : {}),
  assertFailure: async (ctx, state) => {
    const threads = await ctx.rest.listThreads(state.repo, state.prId);
    if (threads.some((thread) => thread.comments?.some((comment) =>
      comment.content?.includes("inline finding") || comment.content?.includes("Consolidated review summary")))) {
      throw new Error("Rejected review batch made a partial comment write");
    }
  },
  assert: async (ctx, state, record) => {
    const inline = record.result?.inline_comments;
    if (!Array.isArray(inline) || inline.length !== 2 || record.result?.vote_changed !== false) {
      throw new Error("Review batch did not report exactly two non-voting findings");
    }
    const ids = inline.map((value: unknown) => {
      if (!isRecord(value) || typeof value.thread_id !== "number" || value.status !== "posted") {
        throw new Error("Review batch reported an unconfirmed finding");
      }
      return value.thread_id;
    });
    const summary = record.result?.thread_id;
    if (typeof summary !== "number" || new Set([...ids, summary]).size !== 3) {
      throw new Error("Review batch did not create distinct finding and summary threads");
    }
    for (const id of [...ids, summary]) {
      const thread = await ctx.rest.getThread(state.repo, state.prId, id);
      if (!thread.comments?.[0]?.content) throw new Error("A confirmed review thread was not persisted");
    }
    if ((await ctx.rest.listReviewers(state.repo, state.prId)).some((reviewer) => reviewer.vote !== 0)) {
      throw new Error("Comment-only review batch unexpectedly changed a vote");
    }
  },
  cleanup: teardownPr,
}));

export const prOwnedCommentScenarios: Scenario<unknown>[] = [update, denied, ...supersession, ...inlineCases, ...batches] as Scenario<unknown>[];
