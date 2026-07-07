/**
 * Work-item safe-output scenarios: create-work-item, update-work-item,
 * comment-on-work-item, link-work-items, upload-workitem-attachment.
 * Test-harness module; not shipped in `ado-script.zip`.
 */
import type { ExecutedRecord, Scenario, ScenarioContext } from "../scenario.js";
import { detBody } from "./common.js";

const WORK_ITEM_TYPE = "Task";

/** Create a scratch Task work item and return its id. */
async function makeScratchWorkItem(ctx: ScenarioContext, tool: string): Promise<number> {
  const wi = await ctx.rest.createWorkItem(WORK_ITEM_TYPE, {
    "System.Title": `${ctx.prefix(tool)}-precondition`,
    "System.Description": detBody(ctx, tool),
  });
  return wi.id;
}

export const createWorkItem: Scenario<{ createdId?: number }> = {
  tool: "create-work-item",
  config: () => ({ "work-item-type": WORK_ITEM_TYPE, max: 1, "include-stats": false }),
  setup: async () => ({}),
  ndjson: async (ctx) => ({
    title: `${ctx.prefix("create-work-item")}`,
    description: detBody(ctx, "create-work-item"),
    tags: [],
  }),
  assert: async (ctx, state, record: ExecutedRecord) => {
    // Populate state.createdId BEFORE the fallible numeric check so cleanup can
    // still delete the work item if the executor returned a non-numeric id
    // (per the Scenario.assert contract).
    const rawId = record.result?.["id"];
    state.createdId = typeof rawId === "number" ? rawId : undefined;
    if (typeof rawId !== "number" || !Number.isFinite(rawId)) {
      throw new Error(`executor result.id is not a number (got ${JSON.stringify(rawId)})`);
    }
    const id = rawId;
    const wi = await ctx.rest.getWorkItem(id);
    const title = wi.fields["System.Title"];
    if (title !== ctx.prefix("create-work-item")) {
      throw new Error(`created work item #${id} has unexpected title '${String(title)}'`);
    }
  },
  cleanup: async (ctx, state) => {
    if (state.createdId !== undefined) await ctx.rest.deleteWorkItem(state.createdId);
  },
};

export const updateWorkItem: Scenario<{ id: number }> = {
  tool: "update-work-item",
  config: () => ({
    target: "*",
    status: true,
    title: true,
    body: true,
    max: 1,
    "include-stats": false,
  }),
  setup: async (ctx) => ({ id: await makeScratchWorkItem(ctx, "update-work-item") }),
  ndjson: async (ctx, state) => ({
    id: state.id,
    title: `${ctx.prefix("update-work-item")}-updated`,
    body: `${detBody(ctx, "update-work-item")} (updated)`,
  }),
  assert: async (ctx, state) => {
    const wi = await ctx.rest.getWorkItem(state.id);
    const title = wi.fields["System.Title"];
    if (title !== `${ctx.prefix("update-work-item")}-updated`) {
      throw new Error(`work item #${state.id} title was not updated (got '${String(title)}')`);
    }
  },
  cleanup: async (ctx, state) => ctx.rest.deleteWorkItem(state.id),
};

export const commentOnWorkItem: Scenario<{ id: number }> = {
  tool: "comment-on-work-item",
  config: () => ({ target: "*", max: 1, "include-stats": false }),
  setup: async (ctx) => ({ id: await makeScratchWorkItem(ctx, "comment-on-work-item") }),
  ndjson: async (ctx, state) => ({
    work_item_id: state.id,
    body: detBody(ctx, "comment-on-work-item"),
  }),
  assert: async (ctx, state) => {
    const comments = await ctx.rest.getWorkItemComments(state.id);
    const found = comments.some((c) => c.text.includes(`build ${ctx.buildId}`));
    if (!found) throw new Error(`no matching comment found on work item #${state.id}`);
  },
  cleanup: async (ctx, state) => ctx.rest.deleteWorkItem(state.id),
};

export const linkWorkItems: Scenario<{ source: number; target: number }> = {
  tool: "link-work-items",
  config: () => ({ target: "*", "allowed-link-types": ["related"], max: 1 }),
  setup: async (ctx) => ({
    source: await makeScratchWorkItem(ctx, "link-work-items"),
    target: await makeScratchWorkItem(ctx, "link-work-items"),
  }),
  ndjson: async (_ctx, state) => ({
    source_id: state.source,
    target_id: state.target,
    link_type: "related",
    comment: "deterministic link",
  }),
  assert: async (ctx, state) => {
    const relations = await ctx.rest.getWorkItemRelations(state.source);
    const linked = relations.some((r) => r.url.endsWith(`/${state.target}`));
    if (!linked) {
      throw new Error(`work item #${state.source} is not linked to #${state.target}`);
    }
  },
  cleanup: async (ctx, state) => {
    await ctx.rest.deleteWorkItem(state.source);
    await ctx.rest.deleteWorkItem(state.target);
  },
};

export const uploadWorkitemAttachment: Scenario<{ id: number }> = {
  tool: "upload-workitem-attachment",
  config: () => ({ "allowed-extensions": ["txt"], max: 1 }),
  setup: async (ctx) => ({ id: await makeScratchWorkItem(ctx, "upload-workitem-attachment") }),
  files: async (ctx) => ({
    "attachment.txt": `deterministic attachment for build ${ctx.buildId}\n`,
  }),
  ndjson: async (ctx, state) => ({
    work_item_id: state.id,
    file_path: "attachment.txt",
    comment: "deterministic executor e2e attachment",
  }),
  assert: async (ctx, state) => {
    const relations = await ctx.rest.getWorkItemRelations(state.id);
    const hasAttachment = relations.some((r) => r.rel === "AttachedFile");
    if (!hasAttachment) {
      throw new Error(`work item #${state.id} has no attached file`);
    }
  },
  cleanup: async (ctx, state) => ctx.rest.deleteWorkItem(state.id),
};

export const workItemScenarios: Scenario<unknown>[] = [
  createWorkItem,
  updateWorkItem,
  commentOnWorkItem,
  linkWorkItems,
  uploadWorkitemAttachment,
];
