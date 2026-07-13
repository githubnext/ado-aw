/**
 * Signal safe-output scenarios: noop, missing-tool, missing-data,
 * report-incomplete.
 *
 * These tools have no ADO write path — they emit a signal record to
 * `safe_outputs.ndjson` and the executor writes back the result without
 * touching any ADO API. Accordingly:
 *   - `setup()` is trivial (no REST calls, returns an empty state).
 *   - `assert()` is a no-op for the succeeded tools; `report-incomplete`
 *     uses `expectedFailure` so `assert()` is never reached.
 *   - `cleanup()` is a no-op.
 *
 * Adding these scenarios closes the executor-e2e coverage gap left by
 * removing the dedicated per-tool agentic smoke pipelines.
 *
 * Test-harness module; not shipped in `ado-script.zip`.
 */
import type { Scenario } from "../scenario.js";

export const noop: Scenario<unknown> = {
  tool: "noop",
  config: () => ({}),
  setup: async () => ({}),
  ndjson: async (ctx) => ({
    context: `deterministic executor e2e noop for build ${ctx.buildId}`,
  }),
  assert: async () => {
    /* no ADO side effect to verify */
  },
  cleanup: async () => {
    /* nothing to tear down */
  },
};

export const missingTool: Scenario<unknown> = {
  id: "missing-tool",
  tool: "missing-tool",
  config: () => ({}),
  setup: async () => ({}),
  ndjson: async (ctx) => ({
    tool_name: `ado-aw-det-${ctx.buildId}-bash`,
    context: `deterministic executor e2e missing-tool for build ${ctx.buildId}`,
  }),
  assert: async () => {
    /* no ADO side effect to verify */
  },
  cleanup: async () => {
    /* nothing to tear down */
  },
};

export const missingData: Scenario<unknown> = {
  id: "missing-data",
  tool: "missing-data",
  config: () => ({}),
  setup: async () => ({}),
  ndjson: async (ctx) => ({
    data_type: "deterministic-e2e-data-type",
    reason: `deterministic executor e2e missing-data for build ${ctx.buildId}`,
  }),
  assert: async () => {
    /* no ADO side effect to verify */
  },
  cleanup: async () => {
    /* nothing to tear down */
  },
};

export const reportIncomplete: Scenario<unknown> = {
  id: "report-incomplete",
  tool: "report-incomplete",
  config: () => ({}),
  setup: async () => ({}),
  ndjson: async (ctx) => ({
    // reason must be >= 10 characters (validated by ReportIncompleteParams).
    reason: `deterministic executor e2e report-incomplete for build ${ctx.buildId}`,
    context: `build ${ctx.buildId}`,
  }),
  // report-incomplete always returns ExecutionResult::failure(), so the
  // executor writes status="failed". Declare that as the expected outcome so
  // the runner treats it as a pass rather than a suite failure.
  expectedFailure: {
    status: "failed",
    error: /Agent reported task incomplete/,
  },
  assert: async () => {
    throw new Error("report-incomplete expected failure should have been caught before assert");
  },
  cleanup: async () => {
    /* nothing to tear down */
  },
};

export const signalScenarios: Scenario<unknown>[] = [
  noop,
  missingTool,
  missingData,
  reportIncomplete,
];
