/**
 * Live platform-contract probes, not claims of executor capability coverage.
 * Writes stay on PRs/branches created and cleaned by the existing PR harness.
 */
import type { Scenario, ScenarioContext } from "../scenario.js";
import { setupPr, teardownPr, type PrState } from "./pr.js";

function object(value: unknown, label: string): Record<string, unknown> {
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    throw new Error(`${label}: expected an object`);
  }
  return value as Record<string, unknown>;
}

function required(value: boolean, message: string): asserts value {
  if (!value) throw new Error(message);
}

async function request(
  ctx: ScenarioContext, state: PrState, suffix: string,
  method = "GET", body?: unknown,
): Promise<Response> {
  required(state.branch.startsWith(ctx.prefix("")), "API probe must own the source branch");
  const base = `${ctx.orgUrl.replace(/\/+$/, "")}/${encodeURIComponent(ctx.project)}`
    + `/_apis/git/repositories/${encodeURIComponent(state.repo)}`;
  return fetch(`${base}/${suffix}?api-version=7.1`, {
    method,
    headers: {
      Authorization: `Basic ${Buffer.from(`:${ctx.token}`).toString("base64")}`,
      "Content-Type": "application/json",
    },
    body: body === undefined ? undefined : JSON.stringify(body),
    signal: AbortSignal.timeout(30_000),
  });
}

async function json(
  ctx: ScenarioContext, state: PrState, suffix: string, method = "GET", body?: unknown,
): Promise<Record<string, unknown>> {
  const response = await request(ctx, state, suffix, method, body);
  if (!response.ok) throw new Error(`PR API contract ${method} ${suffix}: HTTP ${response.status}`);
  return object(await response.json(), suffix);
}

function scenario(id: string, probe: (ctx: ScenarioContext, state: PrState) => Promise<void>): Scenario<PrState> {
  return {
    id,
    tool: "noop",
    targetsAdoRepo: true,
    config: () => ({}),
    setup: (ctx) => setupPr(ctx, id, false, true),
    ndjson: async () => ({ context: `${id}: live API prerequisite check, not executor feature coverage` }),
    assert: async (ctx, state) => probe(ctx, state),
    cleanup: teardownPr,
  };
}

const draft = scenario("pr-api-draft-publication", async (ctx, state) => {
  const path = `pullRequests/${state.prId}`;
  const before = await json(ctx, state, path);
  required(before.isDraft === true, "Draft precondition was not persisted");
  await json(ctx, state, path, "PATCH", { isDraft: false });
  const after = await json(ctx, state, path);
  required(after.isDraft === false && after.status === "active", "Draft publication was not persisted");
  for (const field of ["title", "description", "sourceRefName", "targetRefName"]) {
    required(after[field] === before[field], `Publication changed ${field}`);
  }
  required(!after.autoCompleteSetBy, "Publication unexpectedly enabled auto-complete");
});

const labels = scenario("pr-api-label-replacement", async (ctx, state) => {
  const path = `pullRequests/${state.prId}/labels`;
  const from = await json(ctx, state, path, "POST", { name: "ado-aw-probe-from" });
  const to = await json(ctx, state, path, "POST", { name: "ado-aw-probe-to" });
  required(typeof from.id === "string" && typeof to.id === "string", "Label IDs were not returned");
  const both = await ctx.rest.listPullRequestLabels(state.repo, state.prId);
  required(both.some((l) => l.name === "ado-aw-probe-from") && both.some((l) => l.name === "ado-aw-probe-to"),
    "Label addition must precede removal");
  const deleted = await request(ctx, state, `${path}/${encodeURIComponent(from.id)}`, "DELETE");
  required(deleted.ok, `Label removal failed: HTTP ${deleted.status}`);
  const after = await ctx.rest.listPullRequestLabels(state.repo, state.prId);
  required(!after.some((l) => l.name === "ado-aw-probe-from") && after.some((l) => l.name === "ado-aw-probe-to"),
    "Label replacement was not persisted");
});

const comments = scenario("pr-api-owned-comments", async (ctx, state) => {
  const path = `pullRequests/${state.prId}/threads`;
  const marker = `ado-aw-api-probe-${ctx.buildId}`;
  const created = await json(ctx, state, path, "POST", {
    comments: [{ parentCommentId: 0, content: "Original probe comment.", commentType: 1 }],
    status: 1,
    properties: { "ado-aw-probe-owner": { $type: "System.String", $value: marker } },
  });
  required(typeof created.id === "number", "Thread response missing ID");
  required(Array.isArray(created.comments) && created.comments.length === 1, "Thread response missing comment");
  const comment = object(created.comments[0], "created comment");
  required(typeof comment.id === "number", "Comment response missing ID");
  required(typeof object(comment.author, "comment author").id === "string", "Comment author identity is unavailable");
  await json(ctx, state, `${path}/${created.id}/comments/${comment.id}`, "PATCH", {
    content: "Original probe comment.\n\nSuperseded by the API contract probe.",
  });
  await json(ctx, state, `${path}/${created.id}`, "PATCH", { status: 4 });
  const after = await json(ctx, state, `${path}/${created.id}`);
  required(after.status === "closed" || after.status === 4, "Thread was not closed");
  const properties = object(after.properties, "thread properties");
  required(object(properties["ado-aw-probe-owner"], "ownership property").$value === marker,
    "Thread ownership property did not round-trip");
  required(Array.isArray(after.comments) && after.comments.length === 1, "Closing deleted the comment");
  required(object(after.comments[0], "updated comment").content
    === "Original probe comment.\n\nSuperseded by the API contract probe.", "Comment update was not preserved");

  const iterations = await json(ctx, state, `pullRequests/${state.prId}/iterations`);
  required(Array.isArray(iterations.value) && iterations.value.length > 0, "PR has no iteration context");
  const iteration = object(iterations.value[iterations.value.length - 1], "iteration");
  required(typeof iteration.id === "number", "Iteration ID missing");
  const changes = await json(ctx, state, `pullRequests/${state.prId}/iterations/${iteration.id}/changes`);
  required(Array.isArray(changes.changeEntries), "Iteration changes missing");
  const filePath = `/ado-aw-det/${ctx.buildId}/pr-api-owned-comments.md`;
  const change = changes.changeEntries.map((entry) => object(entry, "iteration change"))
    .find((entry) => object(entry.item, "change item").path === filePath);
  required(typeof change?.changeTrackingId === "number", "Probe file has no changeTrackingId");
  const inline = await json(ctx, state, path, "POST", {
    comments: [{ parentCommentId: 0, content: "Iteration-bound probe comment.", commentType: 1 }],
    status: 1,
    threadContext: {
      filePath,
      rightFileStart: { line: 1, offset: 1 },
      rightFileEnd: { line: 1, offset: 2 },
    },
    pullRequestThreadContext: {
      changeTrackingId: change.changeTrackingId,
      iterationContext: { firstComparingIteration: iteration.id, secondComparingIteration: iteration.id },
    },
  });
  const inlineAfter = await json(ctx, state, `${path}/${inline.id}`);
  required(object(inlineAfter.threadContext, "inline context").filePath === filePath, "Inline path changed");
  const extended = object(inlineAfter.pullRequestThreadContext, "extended context");
  required(extended.changeTrackingId === change.changeTrackingId, "Change tracking ID was not preserved");
});

const push = scenario("pr-api-push-concurrency", async (ctx, state) => {
  const oldHead = await ctx.rest.getRefObjectId(state.repo, `heads/${state.branch}`);
  required(typeof oldHead === "string" && oldHead.length === 40, "Source head unavailable");
  const payload = (name: string) => ({
    refUpdates: [{ name: `refs/heads/${state.branch}`, oldObjectId: oldHead }],
    commits: [{
      comment: "ADO API contract probe",
      parents: [oldHead],
      changes: [{
        changeType: "add",
        item: { path: `/ado-aw-det/${ctx.buildId}/${name}.md` },
        newContent: { content: "Probe content.", contentType: "rawtext" },
      }],
    }],
  });
  const applied = await json(ctx, state, "pushes", "POST", payload("head-guard-first"));
  required(Array.isArray(applied.commits) && applied.commits.length === 1, "Push response missing commit");
  const commit = object(applied.commits[0], "pushed commit");
  required(typeof commit.commitId === "string" && commit.commitId !== oldHead, "Push did not advance source");
  required(await ctx.rest.getRefObjectId(state.repo, `heads/${state.branch}`) === commit.commitId, "Push readback mismatch");
  const stale = await request(ctx, state, "pushes", "POST", payload("head-guard-stale"));
  required(!stale.ok, "Stale oldObjectId was unexpectedly accepted");
  const error = object(await stale.json(), "stale push error");
  required(error.typeKey === "GitReferenceStaleException" || error.typeKey === "GitReferenceUpdateException",
    `Expected stale-ref rejection, got HTTP ${stale.status} ${String(error.typeKey)}`);
  required(await ctx.rest.getRefObjectId(state.repo, `heads/${state.branch}`) === commit.commitId,
    "Rejected stale push changed source head");
});

export const prApiContractScenarios: Scenario<unknown>[] = [draft, labels, comments, push] as Scenario<unknown>[];
