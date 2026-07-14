/**
 * Scenario runner for the trigger-condition E2E harness.
 *
 * Runs scenarios sequentially (deterministic ordering; queuing many victim
 * builds in parallel would contend for the agent pool). Each scenario is fully
 * isolated: a failure or skip never prevents later scenarios from running, and
 * cleanup always runs once `setup` succeeded.
 *
 * Test-harness module; not shipped in `ado-script.zip`.
 */
import { runVictim } from "./queue.js";
import { SkipError } from "./scenario.js";
import type {
  BuildOutcome,
  Expected,
  ScenarioResult,
  TriggerContext,
  TriggerScenario,
} from "./scenario.js";

function errMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

/** Default declarative assertion: build result + required/forbidden tags. */
function assertExpected(expected: Expected, outcome: BuildOutcome): void {
  const wantResult = expected.result ?? "succeeded";
  if (outcome.result !== wantResult) {
    throw new Error(
      `expected build result '${wantResult}' but got '${outcome.result ?? "?"}' (tags: [${outcome.tags.join(", ")}])`,
    );
  }
  for (const tag of expected.tags ?? []) {
    if (!outcome.tags.includes(tag)) {
      throw new Error(`expected build tag '${tag}' but tags were [${outcome.tags.join(", ")}]`);
    }
  }
  for (const tag of expected.absentTags ?? []) {
    if (outcome.tags.includes(tag)) {
      throw new Error(`build tag '${tag}' should be absent but tags were [${outcome.tags.join(", ")}]`);
    }
  }
}

export async function runScenario<S>(
  ctx: TriggerContext,
  scenario: TriggerScenario<S>,
): Promise<ScenarioResult> {
  const start = Date.now();
  const id = scenario.id;
  let state: S | undefined;
  let setupDone = false;
  // Captured as soon as the victim build is queued, so a queue-phase throw
  // (e.g. a waitForBuild timeout) still lets the finally block cancel the
  // orphaned build rather than leaking a running build for the whole suite.
  let queuedBuildId: number | undefined;

  const finish = (partial: Omit<ScenarioResult, "id" | "durationMs">): ScenarioResult => ({
    id,
    durationMs: Date.now() - start,
    ...partial,
  });

  try {
    // ---- setup ----
    ctx.log(`[${id}] setup — ${scenario.description}`);
    try {
      state = await scenario.setup(ctx);
      setupDone = true;
    } catch (err) {
      if (err instanceof SkipError) {
        ctx.log(`[${id}] SKIPPED: ${err.message}`);
        return finish({ ok: true, skipped: true, phase: "skipped", message: err.message });
      }
      return finish({ ok: false, phase: "setup", message: errMessage(err) });
    }

    // ---- queue + poll ----
    let outcome: BuildOutcome;
    try {
      const queue = scenario.queue(ctx, state);
      outcome = await runVictim(ctx, queue, (buildId) => {
        queuedBuildId = buildId;
      });
    } catch (err) {
      return finish({ ok: false, phase: "queue", message: errMessage(err) });
    }

    // ---- assert ----
    try {
      assertExpected(scenario.expected(ctx, state), outcome);
      if (scenario.assert) await scenario.assert(ctx, state, outcome);
    } catch (err) {
      return finish({ ok: false, phase: "assert", message: errMessage(err) });
    }

    ctx.log(`[${id}] OK`);
    return finish({ ok: true });
  } finally {
    // ---- cancel an orphaned victim build (best-effort) ----
    // A completed build is a no-op; only a build still running after a
    // queue-phase failure needs cancelling.
    if (queuedBuildId !== undefined) {
      try {
        const build = await ctx.rest.getBuild(queuedBuildId);
        if (build.status !== "completed") {
          await ctx.rest.cancelBuild(queuedBuildId);
          ctx.log(`[${id}] cancelled orphaned victim build #${queuedBuildId}`);
        }
      } catch (err) {
        ctx.log(`[${id}] orphaned-build cleanup WARNING: ${errMessage(err)}`);
      }
    }

    // ---- cleanup (always, best-effort) ----
    if (setupDone) {
      try {
        await scenario.cleanup(ctx, state as S);
        ctx.log(`[${id}] cleanup done`);
      } catch (err) {
        ctx.log(`[${id}] cleanup WARNING: ${errMessage(err)}`);
      }
    }
  }
}

export async function runAll(
  ctx: TriggerContext,
  scenarios: TriggerScenario<unknown>[],
): Promise<ScenarioResult[]> {
  const results: ScenarioResult[] = [];
  for (const scenario of scenarios) {
    results.push(await runScenario(ctx, scenario));
  }
  return results;
}
