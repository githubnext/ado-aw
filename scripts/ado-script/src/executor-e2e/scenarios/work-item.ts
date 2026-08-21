/**
 * Work-item safe-output scenarios: create-work-item, assign-work-item,
 * update-work-item, comment-on-work-item, link-work-items,
 * upload-workitem-attachment.
 * Test-harness module; not shipped in `ado-script.zip`.
 */
import type { ExecutedRecord, PriorEntry, Scenario, ScenarioContext } from "../scenario.js";
import { SkipError } from "../scenario.js";
import { detBody, numResult, strResult, Teardown } from "./common.js";
import renderingCorpus from "./markdown-rendering-corpus.json" with { type: "json" };

const WORK_ITEM_TYPE = "Task";
const CREATE_TEMPORARY_ID = "#aw_wicreate";
const ASSIGN_TEMPORARY_ID = "#aw_wiassign";

/**
 * Rendering-fidelity corpus shared with the Rust golden test in
 * `src/sanitize/markdown.rs` (which `include_str!`s the same JSON). The Rust
 * test proves the sanitizer produces `RENDERING_EXPECTED`; these scenarios
 * prove Azure DevOps stores it back byte-for-byte, so the two together pin
 * what a human actually sees in a work item.
 */
const RENDERING_INPUT = renderingCorpus.input.join("\n");
const RENDERING_EXPECTED = renderingCorpus.expected.join("\n");

/**
 * Constructs the sanitizer must never let reach a rendered work item.
 * Compared case-insensitively, so a folded `<SCRIPT >` is covered too.
 */
const DENIED_CONSTRUCTS = ["<script", "onerror=", "<iframe"];

/**
 * Fenced-code lines that must survive verbatim. They contain the same
 * constructs as `DENIED_CONSTRUCTS`, so an over-eager "strip it everywhere"
 * regression fails here rather than silently mangling documentation.
 */
const FENCED_VERBATIM = [
  '<script>alert("fenced code is verbatim")</script>',
  '<a href="javascript:alert(1)">fenced javascript link</a>',
];


function usableEnvValue(value: string | undefined): string | undefined {
  const trimmed = value?.trim();
  if (!trimmed || /^\$\([^)]+\)$/.test(trimmed)) return undefined;
  return trimmed;
}

export function resolveWorkItemAssignee(
  env: NodeJS.ProcessEnv = process.env,
): string {
  const assignee =
    usableEnvValue(env.E2E_WORK_ITEM_ASSIGNEE) ??
    usableEnvValue(env.BUILD_REQUESTEDFOREMAIL);
  if (!assignee) {
    throw new SkipError(
      "assign-work-item requires E2E_WORK_ITEM_ASSIGNEE or BUILD_REQUESTEDFOREMAIL",
    );
  }
  if (["agency", "github copilot"].includes(assignee.toLowerCase())) {
    throw new SkipError(`assign-work-item test identity '${assignee}' is reserved`);
  }
  return assignee;
}

function assignedIdentityValues(value: unknown): string[] {
  if (typeof value === "string") return [value];
  if (!value || typeof value !== "object" || Array.isArray(value)) return [];
  const identity = value as Record<string, unknown>;
  return ["displayName", "uniqueName", "mail"]
    .map((key) => identity[key])
    .filter((entry): entry is string => typeof entry === "string");
}

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
    temporary_id: CREATE_TEMPORARY_ID,
  }),
  assert: async (ctx, state, record: ExecutedRecord) => {
    // Populate state.createdId BEFORE the fallible title check so cleanup can
    // still delete the work item if that later assertion throws (per the
    // Scenario.assert contract). numResult validates BOTH typeof number and
    // Number.isFinite — a bare `typeof === "number"` would let a NaN id through
    // (typeof NaN === "number"), leaking as deleteWorkItem(NaN) in cleanup.
    const id = numResult(record, "id");
    state.createdId = id;
    if (strResult(record, "temporary_id") !== CREATE_TEMPORARY_ID) {
      throw new Error(
        `create-work-item reported temporary_id '${strResult(record, "temporary_id")}', expected '${CREATE_TEMPORARY_ID}'`,
      );
    }
    const wi = await ctx.rest.getWorkItem(id);
    const title = wi.fields["System.Title"];
    if (title !== ctx.prefix("create-work-item")) {
      throw new Error(`created work item #${id} has unexpected title '${String(title)}'`);
    }
    const assignedTo = wi.fields["System.AssignedTo"];
    const isUnassigned =
      assignedTo === undefined ||
      assignedTo === null ||
      (typeof assignedTo === "string" && assignedTo.trim() === "");
    if (!isUnassigned) {
      throw new Error(
        `created work item #${id} unexpectedly has System.AssignedTo=${JSON.stringify(assignedTo)}`,
      );
    }
  },
  cleanup: async (ctx, state) => {
    if (state.createdId !== undefined) await ctx.rest.deleteWorkItem(state.createdId);
  },
};

export const assignWorkItemTemporaryIdHandoff: Scenario<{
  assignee: string;
  title: string;
  createdId?: number;
}> = {
  id: "assign-work-item-temporary-id-handoff",
  tool: "assign-work-item",
  config: (_ctx, state) => ({
    allowed: [state.assignee],
    max: 1,
  }),
  setup: async (ctx) => ({
    assignee: resolveWorkItemAssignee(),
    title: ctx.prefix("assign-work-item-temporary-id-handoff"),
  }),
  priorEntries: async (ctx, state): Promise<PriorEntry[]> => [
    {
      tool: "create-work-item",
      config: {
        "work-item-type": WORK_ITEM_TYPE,
        "include-stats": false,
        max: 1,
      },
      entry: {
        // The deterministic harness bypasses MCP, so this emulates the
        // MCP-generated field persisted in the internal NDJSON proposal.
        title: state.title,
        description: detBody(ctx, "assign-work-item-temporary-id-handoff"),
        tags: [],
        temporary_id: ASSIGN_TEMPORARY_ID,
      },
    },
  ],
  ndjson: async (_ctx, state) => ({
    work_item_id: ASSIGN_TEMPORARY_ID,
    assignee: state.assignee,
  }),
  assert: async (ctx, state, record) => {
    const id = numResult(record, "id");
    state.createdId = id;
    const wi = await ctx.rest.getWorkItem(id);
    const assignedTo = assignedIdentityValues(wi.fields["System.AssignedTo"]);
    const matches = assignedTo.some(
      (value) =>
        value.localeCompare(state.assignee, undefined, { sensitivity: "accent" }) === 0,
    );
    if (!matches) {
      throw new Error(
        `work item #${id} was not assigned to '${state.assignee}' (got ${JSON.stringify(assignedTo)})`,
      );
    }
  },
  cleanup: async (ctx, state) => {
    const id = state.createdId ?? (await ctx.rest.findWorkItemByTitle(state.title));
    if (id !== undefined) await ctx.rest.deleteWorkItem(id);
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
    // Delete both work items independently: a throw deleting the source must
    // not leave the target orphaned.
    await new Teardown()
      .add("delete source work item", () => ctx.rest.deleteWorkItem(state.source))
      .add("delete target work item", () => ctx.rest.deleteWorkItem(state.target))
      .run();
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
    // Assert the specific file we uploaded is attached (ADO surfaces the file
    // name in the AttachedFile relation attributes), not just that some
    // attachment exists — the scratch item starts with none.
    const hasAttachment = relations.some(
      (r) => r.rel === "AttachedFile" && r.attributes?.["name"] === "attachment.txt",
    );
    if (!hasAttachment) {
      throw new Error(`work item #${state.id} has no 'attachment.txt' attached file`);
    }
  },
  cleanup: async (ctx, state) => ctx.rest.deleteWorkItem(state.id),
};

/** Strip fenced code blocks so denied constructs are only checked in prose. */
function outsideFencedCode(text: string): string {
  const lines = text.split("\n");
  const kept: string[] = [];
  let inFence = false;
  for (const line of lines) {
    const trimmed = line.trimStart();
    if (trimmed.startsWith("```") || trimmed.startsWith("~~~")) {
      inFence = !inFence;
      continue;
    }
    if (!inFence) kept.push(line);
  }
  return kept.join("\n");
}

/** First line that differs, to make a golden mismatch debuggable. */
function firstDifference(actual: string, expected: string): string {
  const a = actual.split("\n");
  const e = expected.split("\n");
  for (let i = 0; i < Math.max(a.length, e.length); i++) {
    if (a[i] !== e[i]) {
      return `line ${i + 1}: expected ${JSON.stringify(e[i])}, got ${JSON.stringify(a[i])}`;
    }
  }
  return "no line differs (trailing content mismatch)";
}

/**
 * Build a rendering-fidelity scenario for one description field.
 *
 * `create-work-item` is the only safe output whose body goes through the
 * Markdown sanitizer, and it writes either `System.Description` (default) or
 * `Microsoft.VSTS.TCM.ReproSteps` (Bug) with a
 * `/multilineFieldsFormat/<field>` = `Markdown` patch. One scenario per field
 * therefore covers every path a sanitizer change can break.
 */
function renderingScenario(options: {
  readonly id: string;
  readonly workItemType: string;
  readonly descriptionField: string;
}): Scenario<{ title: string; createdId?: number }> {
  const { id: scenarioId, workItemType, descriptionField } = options;
  return {
    id: scenarioId,
    tool: "create-work-item",
    config: () => ({
      "work-item-type": workItemType,
      max: 1,
      // Appended agent stats would change the stored text and defeat the
      // byte-for-byte golden comparison.
      "include-stats": false,
    }),
    setup: async (ctx) => {
      if (!(await ctx.rest.workItemTypeExists(workItemType))) {
        throw new SkipError(`project does not define the '${workItemType}' work item type`);
      }
      return { title: ctx.prefix(scenarioId) };
    },
    ndjson: async (_ctx, state) => ({
      title: state.title,
      description: RENDERING_INPUT,
      tags: [],
      temporary_id: CREATE_TEMPORARY_ID,
    }),
    assert: async (ctx, state, record: ExecutedRecord) => {
      // Record the id before any fallible check so cleanup can still delete it.
      const id = numResult(record, "id");
      state.createdId = id;

      const wi = await ctx.rest.getWorkItem(id);
      const stored = wi.fields[descriptionField];
      if (typeof stored !== "string") {
        throw new Error(
          `work item #${id} has no string '${descriptionField}' (got ${JSON.stringify(stored)})`,
        );
      }

      // 1. Security: denied constructs must not survive in prose, while their
      //    fenced-code twins must survive untouched. Checked before the golden
      //    so a leak is reported as a leak rather than as a generic mismatch.
      const prose = outsideFencedCode(stored).toLowerCase();
      for (const construct of DENIED_CONSTRUCTS) {
        if (prose.includes(construct.toLowerCase())) {
          throw new Error(
            `work item #${id} '${descriptionField}' still contains '${construct}' outside code`,
          );
        }
      }
      if (prose.includes("javascript:")) {
        throw new Error(
          `work item #${id} '${descriptionField}' still contains a javascript: URL outside code`,
        );
      }
      for (const line of FENCED_VERBATIM) {
        if (!stored.includes(line)) {
          throw new Error(
            `work item #${id} '${descriptionField}' lost fenced code line ${JSON.stringify(line)}`,
          );
        }
      }

      // 2. Format: stored as Markdown, otherwise the body renders as literal
      //    text. Not every organization surfaces this on read — when it is
      //    absent the executor-side patch is pinned by the Rust unit tests in
      //    src/safe_outputs/create_work_item.rs instead.
      const format = wi.multilineFieldsFormat?.[descriptionField];
      if (format === undefined) {
        ctx.log(
          `[${scenarioId}] note: ADO did not surface multilineFieldsFormat for ` +
            `'${descriptionField}'; format is covered by the executor unit tests`,
        );
      } else if (format !== "Markdown") {
        throw new Error(
          `work item #${id} stores '${descriptionField}' as ${JSON.stringify(format)}, expected "Markdown"`,
        );
      }

      // 3. Golden round-trip: what ADO stored must equal what the sanitizer
      //    produced, byte for byte. This is the decisive check — any change in
      //    what the allowlist keeps, drops or rewrites moves the golden, as
      //    does ADO normalising the payload on the way in.
      if (stored !== RENDERING_EXPECTED) {
        throw new Error(
          `work item #${id} '${descriptionField}' does not match the sanitized golden ` +
            `(${firstDifference(stored, RENDERING_EXPECTED)})`,
        );
      }
    },
    cleanup: async (ctx, state) => {
      const id = state.createdId ?? (await ctx.rest.findWorkItemByTitle(state.title));
      if (id !== undefined) await ctx.rest.deleteWorkItem(id);
    },
  };
}

/** Task / `System.Description` rendering fidelity. */
export const createWorkItemRendering = renderingScenario({
  id: "create-work-item-rendering",
  workItemType: WORK_ITEM_TYPE,
  descriptionField: "System.Description",
});

/** Bug / `Microsoft.VSTS.TCM.ReproSteps` rendering fidelity. */
export const createBugWorkItemRendering = renderingScenario({
  id: "create-work-item-rendering-bug",
  workItemType: "Bug",
  descriptionField: "Microsoft.VSTS.TCM.ReproSteps",
});

export const workItemScenarios: Scenario<unknown>[] = [
  createWorkItem,
  createWorkItemRendering,
  createBugWorkItemRendering,
  assignWorkItemTemporaryIdHandoff,
  updateWorkItem,
  commentOnWorkItem,
  linkWorkItems,
  uploadWorkitemAttachment,
];
