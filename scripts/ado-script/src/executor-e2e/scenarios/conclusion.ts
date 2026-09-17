/**
 * Conclusion-job scenarios: work-item filing for the diagnostic signals.
 *
 * The signal safe-outputs (`noop`, `missing-tool`, `missing-data`) have no ADO
 * write path of their own — the executor only records them in
 * `safe-outputs-executed.ndjson`. Their *observable* effect is produced one job
 * later by the Conclusion job (`conclusion.js`), which reads that manifest and
 * files (or appends to) an Azure DevOps work item per signal — see
 * `docs/conclusion.md`.
 *
 * The scenarios in `signals.ts` stop at the executor record, so nothing covered
 * the signal → manifest → work-item path end to end. These scenarios close that
 * gap: each runs the real executor, then the real conclusion bundle over the
 * manifest it just wrote, and asserts the resulting work item via the ADO REST
 * API.
 *
 * Coverage per scenario:
 *   - `conclusion-noop` — a work item is created for a `noop` signal, carrying
 *     the configured title, type, tags and rendered body.
 *   - `conclusion-missing-tool` — same for `missing-tool`, and the second
 *     conclusion run appends a comment instead of creating a duplicate
 *     (title deduplication).
 *   - `conclusion-missing-data` — same for `missing-data`, including the
 *     reported data type and reason.
 *   - `conclusion-report-as-work-item-false` — the per-tool opt-out files
 *     nothing at all.
 *
 * Test-harness module; not shipped in `ado-script.zip`.
 */
import { runConclusion } from "../conclusion-cli.js";
import type { PostExecuteRun, Scenario, ScenarioContext } from "../scenario.js";

/** Work item type used for every conclusion scenario (the reporter's default). */
const WORK_ITEM_TYPE = "Task";

/** Title prefix handed to the reporter; the rendered title appends the pipeline name. */
const TITLE_PREFIX = "[ado-aw-e2e conclusion]";

interface ConclusionState {
  /** Value of `AW_PIPELINE_NAME`; unique per build and scenario. */
  pipelineName: string;
  /** The title the reporter is expected to render: `<prefix> <pipelineName>`. */
  title: string;
  /** Tag applied to created work items (also used for cleanup diagnostics). */
  tag: string;
  /** Populated in `postExecute` once the work item is observed. */
  workItemId?: number;
  /** stdout of the last conclusion run, asserted by the opt-out scenario. */
  stdout?: string;
}

function conclusionState(ctx: ScenarioContext, scenarioId: string): ConclusionState {
  const pipelineName = ctx.prefix(scenarioId);
  return {
    pipelineName,
    title: `${TITLE_PREFIX} ${pipelineName}`,
    tag: `ado-aw-e2e-${ctx.buildId}`,
  };
}

/** Per-tool conclusion env, mirroring the flat `AW_<TOOL>_*` vars the compiler emits. */
function toolConfig(
  envPrefix: string,
  state: ConclusionState,
  extra: Record<string, string> = {},
): Record<string, string> {
  return {
    [`${envPrefix}_TITLE_PREFIX`]: TITLE_PREFIX,
    [`${envPrefix}_WORK_ITEM_TYPE`]: WORK_ITEM_TYPE,
    [`${envPrefix}_TAGS`]: JSON.stringify([state.tag]),
    ...extra,
  };
}

/**
 * Wait for the work item to become visible to WIQL.
 *
 * `findWorkItemByTitle` goes through the WIQL endpoint, whose index lags work
 * item creation by a second or two. Polling here (rather than reading once)
 * keeps the assertion deterministic, and it also guarantees the *reporter's*
 * own dedup query can see the item before a second run is asked to append.
 */
async function waitForWorkItem(
  ctx: ScenarioContext,
  title: string,
  timeoutMs = 90_000,
): Promise<number> {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const id = await ctx.rest.findWorkItemByTitle(title);
    if (id !== undefined) return id;
    if (Date.now() >= deadline) {
      throw new Error(
        `no work item titled '${title}' became visible within ${timeoutMs}ms`,
      );
    }
    await new Promise((resolve) => setTimeout(resolve, 3_000));
  }
}

/** Read a work item field as a string (missing/non-string fields fail loudly). */
function fieldText(fields: Record<string, unknown>, name: string): string {
  const value = fields[name];
  if (typeof value !== "string") {
    throw new Error(`work item field ${name} is not a string (got ${JSON.stringify(value)})`);
  }
  return value;
}

/**
 * Assert the shared shape of a conclusion-filed work item: title, type, tag and
 * the substrings the reporter is expected to render into the description.
 * Returns the asserted work item id so callers can make further checks against
 * it without re-deriving (and re-guarding) it.
 *
 * Substrings are chosen to be free of `<`, `>` and `&` so the check holds
 * whether or not Azure DevOps stores the body as Markdown or re-encodes it.
 */
async function assertFiledWorkItem(
  ctx: ScenarioContext,
  state: ConclusionState,
  expectedBodySubstrings: readonly string[],
): Promise<number> {
  const workItemId = state.workItemId;
  if (workItemId === undefined) {
    throw new Error("postExecute did not record a work item id");
  }
  const item = await ctx.rest.getWorkItem(workItemId);
  const title = fieldText(item.fields, "System.Title");
  if (title !== state.title) {
    throw new Error(`work item title is '${title}', expected '${state.title}'`);
  }
  const type = fieldText(item.fields, "System.WorkItemType");
  if (type !== WORK_ITEM_TYPE) {
    throw new Error(`work item type is '${type}', expected '${WORK_ITEM_TYPE}'`);
  }
  const tags = fieldText(item.fields, "System.Tags");
  if (!tags.split(";").map((t) => t.trim()).includes(state.tag)) {
    throw new Error(`work item tags '${tags}' do not include '${state.tag}'`);
  }
  const description = fieldText(item.fields, "System.Description");
  for (const expected of expectedBodySubstrings) {
    if (!description.includes(expected)) {
      throw new Error(
        `work item description does not contain '${expected}': ${description.slice(0, 800)}`,
      );
    }
  }
  return workItemId;
}

/** Best-effort teardown: delete the filed work item (resolving it by title if needed). */
async function cleanupWorkItem(ctx: ScenarioContext, state: ConclusionState): Promise<void> {
  const id = state.workItemId ?? (await ctx.rest.findWorkItemByTitle(state.title));
  if (id === undefined) return;
  await ctx.rest.deleteWorkItem(id);
}

/** Run the reporter once against the manifest the executor just wrote. */
async function reportOnce(
  ctx: ScenarioContext,
  state: ConclusionState,
  run: PostExecuteRun,
  config: Record<string, string>,
): Promise<string> {
  const result = await runConclusion({
    safeOutputDir: run.safeOutputDir,
    pipelineName: state.pipelineName,
    orgUrl: ctx.orgUrl,
    project: ctx.project,
    token: ctx.token,
    buildId: ctx.buildId,
    config,
    log: ctx.log,
  });
  state.stdout = result.stdout;
  return result.stdout;
}

export const conclusionNoop: Scenario<ConclusionState> = {
  id: "conclusion-noop",
  tool: "noop",
  config: () => ({}),
  setup: async (ctx) => conclusionState(ctx, "conclusion-noop"),
  ndjson: async (ctx) => ({
    context: `deterministic conclusion e2e noop for build ${ctx.buildId}`,
  }),
  postExecute: async (ctx, state, run) => {
    await reportOnce(ctx, state, run, toolConfig("AW_NOOP", state));
    state.workItemId = await waitForWorkItem(ctx, state.title);
    ctx.log(`[conclusion-noop] filed work item #${state.workItemId}`);
  },
  assert: async (ctx, state) => {
    await assertFiledWorkItem(ctx, state, [
      "noop",
      "Occurrences: 1",
      `deterministic conclusion e2e noop for build ${ctx.buildId}`,
      `Build ID: ${ctx.buildId}`,
    ]);
  },
  cleanup: cleanupWorkItem,
};

export const conclusionMissingTool: Scenario<ConclusionState> = {
  id: "conclusion-missing-tool",
  tool: "missing-tool",
  config: () => ({}),
  setup: async (ctx) => conclusionState(ctx, "conclusion-missing-tool"),
  ndjson: async (ctx) => ({
    tool_name: `ado-aw-det-${ctx.buildId}-bash`,
    context: `deterministic conclusion e2e missing-tool for build ${ctx.buildId}`,
  }),
  postExecute: async (ctx, state, run) => {
    const config = toolConfig("AW_MISSING_TOOL", state);
    await reportOnce(ctx, state, run, config);
    state.workItemId = await waitForWorkItem(ctx, state.title);
    ctx.log(`[conclusion-missing-tool] filed work item #${state.workItemId}`);
    // Second run over the same manifest: the reporter must dedup on the
    // rendered title and append a comment rather than file a duplicate.
    await reportOnce(ctx, state, run, config);
  },
  assert: async (ctx, state) => {
    const workItemId = await assertFiledWorkItem(ctx, state, [
      "missing_tool",
      `ado-aw-det-${ctx.buildId}-bash`,
      `deterministic conclusion e2e missing-tool for build ${ctx.buildId}`,
    ]);
    // Exactly one: the title is unique to this build and scenario, so the work
    // item is always freshly created by the first conclusion run (which files,
    // and does not comment). A second comment would mean the reporter appended
    // twice; zero would mean it filed a duplicate work item instead.
    const comments = await ctx.rest.getWorkItemComments(workItemId);
    if (comments.length !== 1) {
      throw new Error(
        `expected exactly one appended comment after the second conclusion run, got ${comments.length}`,
      );
    }
    const commentText = comments[0]?.text ?? "";
    if (!commentText.includes("missing_tool")) {
      throw new Error(
        `appended comment does not describe the missing_tool signal: ${commentText.slice(0, 400)}`,
      );
    }
  },
  cleanup: cleanupWorkItem,
};

export const conclusionMissingData: Scenario<ConclusionState> = {
  id: "conclusion-missing-data",
  tool: "missing-data",
  config: () => ({}),
  setup: async (ctx) => conclusionState(ctx, "conclusion-missing-data"),
  ndjson: async (ctx) => ({
    data_type: "deterministic-conclusion-e2e-data-type",
    reason: `deterministic conclusion e2e missing-data for build ${ctx.buildId}`,
  }),
  postExecute: async (ctx, state, run) => {
    await reportOnce(ctx, state, run, toolConfig("AW_MISSING_DATA", state));
    state.workItemId = await waitForWorkItem(ctx, state.title);
    ctx.log(`[conclusion-missing-data] filed work item #${state.workItemId}`);
  },
  assert: async (ctx, state) => {
    await assertFiledWorkItem(ctx, state, [
      "missing_data",
      "deterministic-conclusion-e2e-data-type",
      `deterministic conclusion e2e missing-data for build ${ctx.buildId}`,
    ]);
  },
  cleanup: cleanupWorkItem,
};

export const conclusionOptOut: Scenario<ConclusionState> = {
  id: "conclusion-report-as-work-item-false",
  tool: "noop",
  config: () => ({}),
  setup: async (ctx) => conclusionState(ctx, "conclusion-report-as-work-item-false"),
  ndjson: async (ctx) => ({
    context: `deterministic conclusion e2e report-as-work-item-false for build ${ctx.buildId}`,
  }),
  postExecute: async (ctx, state, run) => {
    await reportOnce(
      ctx,
      state,
      run,
      toolConfig("AW_NOOP", state, { AW_NOOP_REPORT_AS_WORK_ITEM: "false" }),
    );
  },
  assert: async (ctx, state) => {
    const stdout = state.stdout ?? "";
    if (!stdout.includes("report-as-work-item is false")) {
      throw new Error(
        `conclusion did not log the per-tool opt-out: ${stdout.slice(0, 800)}`,
      );
    }
    // The absence check is secondary to the log assertion above: WIQL lags
    // creation, so a filed item might not be visible yet. It still catches a
    // regression where the opt-out is ignored on a later run of the suite.
    const id = await ctx.rest.findWorkItemByTitle(state.title);
    if (id !== undefined) {
      throw new Error(
        `work item #${id} was filed despite report-as-work-item: false`,
      );
    }
  },
  cleanup: cleanupWorkItem,
};

export const conclusionScenarios: Scenario<unknown>[] = [
  conclusionNoop,
  conclusionMissingTool,
  conclusionMissingData,
  conclusionOptOut,
];
