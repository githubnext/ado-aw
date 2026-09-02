/**
 * Scenario runner for the deterministic executor E2E harness.
 *
 * Runs scenarios sequentially (deterministic ordering, no ADO rate-limit
 * contention). Each scenario is fully isolated: a failure or skip in one never
 * prevents later scenarios from running, and cleanup always runs.
 *
 * Test-harness module; not shipped in `ado-script.zip`.
 */
import { mkdir } from "node:fs/promises";
import { join } from "node:path";

import { runExecute } from "./execute-cli.js";
import { SkipError } from "./scenario.js";
import type { Scenario, ScenarioContext, ScenarioResult } from "./scenario.js";

function errMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

export async function runScenario<S>(
  ctx: ScenarioContext,
  scenario: Scenario<S>,
): Promise<ScenarioResult> {
  const start = Date.now();
  const tool = scenario.tool;
  const scenarioId = scenario.id ?? tool;
  const scenarioDir = join(ctx.workDir, scenarioId);

  let state: S | undefined;
  let setupDone = false;

  const finish = (partial: Omit<ScenarioResult, "tool" | "durationMs">): ScenarioResult => ({
    tool: scenarioId,
    durationMs: Date.now() - start,
    ...partial,
  });

  try {
    // Create the scratch dir with its own catch so a failure here (disk full,
    // permission error) records a failed result instead of propagating out of
    // runScenario and aborting runAll's loop over the remaining scenarios.
    try {
      await mkdir(scenarioDir, { recursive: true });
    } catch (err) {
      return finish({ ok: false, phase: "setup", message: errMessage(err) });
    }

    // ---- setup ----
    ctx.log(`[${scenarioId}] setup`);
    try {
      state = await scenario.setup(ctx);
      // IMPORTANT: cleanup runs ONLY after this point (guarded by setupDone in
      // the finally block). If setup() throws before this line, cleanup WILL
      // NOT run — scenarios must inline-teardown any partially created remote
      // state in their own setup() catch (see setupPr in scenarios/pr.ts).
      setupDone = true;
    } catch (err) {
      if (err instanceof SkipError) {
        ctx.log(`[${scenarioId}] SKIPPED: ${err.message}`);
        return finish({ ok: true, skipped: true, phase: "skipped", message: err.message });
      }
      return finish({ ok: false, phase: "setup", message: errMessage(err) });
    }

    // ---- execute ----
    // Guard the auxiliary scenario methods too: a harness-level bug in any of
    // these must record a failed result and let the rest of the suite run,
    // not propagate out of runScenario and abort runAll early.
    let config, entry, files, extraEnv, priorEntries, source;
    try {
      config = scenario.config(ctx, state);
      entry = await scenario.ndjson(ctx, state);
      priorEntries = scenario.priorEntries ? await scenario.priorEntries(ctx, state) : undefined;
      files = scenario.files ? await scenario.files(ctx, state) : undefined;
      extraEnv = scenario.env ? await scenario.env(ctx, state) : undefined;
      source = scenario.source ? await scenario.source(ctx, state) : undefined;
    } catch (err) {
      return finish({ ok: false, phase: "execute", message: errMessage(err) });
    }

    let result;
    try {
      result = await runExecute({
        adoAwBin: ctx.adoAwBin,
        scenarioDir,
        tool,
        config,
        entry,
        priorEntries,
        adoRepo: scenario.targetsAdoRepo ? ctx.adoRepo : undefined,
        source,
        orgUrl: ctx.orgUrl,
        project: ctx.project,
        token: ctx.token,
        files,
        extraEnv,
        log: ctx.log,
      });
    } catch (err) {
      // e.g. the ado-aw execute child timed out or failed to spawn.
      return finish({ ok: false, phase: "execute", message: errMessage(err) });
    }

    // Prior entries are prerequisites, not the thing under test: surface a
    // broken one as its own execute-phase failure so it can never be mistaken
    // for an assertion failure in the primary tool.
    const priorRecordOffsets = new Map<string, number>();
    for (const prior of priorEntries ?? []) {
      const priorName = prior.tool.replaceAll("-", "_");
      const offset = priorRecordOffsets.get(priorName) ?? 0;
      const matchingRecords = result.records.filter((r) => r.name === priorName);
      const priorRecord = matchingRecords[offset];
      priorRecordOffsets.set(priorName, offset + 1);
      if (!priorRecord) {
        return finish({
          ok: false,
          phase: "execute",
          message:
            `prior entry '${prior.tool}' occurrence ${offset + 1} produced no executed record`,
        });
      }
      if (priorRecord.status !== "succeeded") {
        return finish({
          ok: false,
          phase: "execute",
          message: `prior entry '${prior.tool}' reported status='${priorRecord.status}': ${priorRecord.error ?? "no error message"}`,
        });
      }
    }

    if (!result.record) {
      return finish({
        ok: false,
        phase: "execute",
        message: `no executed record for '${tool}' (exit ${result.exitCode}); stderr: ${result.stderr.trim().slice(0, 500)}`,
      });
    }
    if (result.record.status !== "succeeded") {
      const expected = scenario.expectedFailure;
      const error = result.record.error ?? "";
      if (
        expected &&
        (expected.status === undefined || result.record.status === expected.status) &&
        expected.error.test(error)
      ) {
        ctx.log(`[${scenarioId}] expected failure: ${error}`);
        return finish({ ok: true });
      }
      return finish({
        ok: false,
        phase: "execute",
        message: `executor reported status='${result.record.status}': ${result.record.error ?? "no error message"}`,
      });
    }

    // ---- assert ----
    try {
      await scenario.assert(ctx, state, result.record, result.records);
    } catch (err) {
      return finish({ ok: false, phase: "assert", message: errMessage(err) });
    }

    ctx.log(`[${scenarioId}] OK`);
    return finish({ ok: true });
  } finally {
    // ---- cleanup (always, best-effort) ----
    // Guard only on setupDone: a scenario whose setup legitimately returns
    // void/undefined must still have cleanup run. Scenarios that never reached
    // a successful setup (SkipError or setup failure) leave setupDone false.
    if (setupDone) {
      try {
        await scenario.cleanup(ctx, state as S);
        ctx.log(`[${scenarioId}] cleanup done`);
      } catch (err) {
        ctx.log(`[${scenarioId}] cleanup WARNING: ${errMessage(err)}`);
      }
    }
  }
}

export async function runAll(
  ctx: ScenarioContext,
  scenarios: Scenario<unknown>[],
): Promise<ScenarioResult[]> {
  const results: ScenarioResult[] = [];
  for (const scenario of scenarios) {
    results.push(await runScenario(ctx, scenario));
  }
  return results;
}
