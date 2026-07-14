/**
 * Queue one victim-pipeline build and collect its terminal outcome.
 *
 * Thin orchestration over the shared {@link AdoRest} client: queue the victim
 * definition with the scenario's source branch + template parameters, poll to
 * completion, and read the final build tags. The returned {@link BuildOutcome}
 * is what the runner asserts against.
 *
 * Test-harness module; not shipped in `ado-script.zip`.
 */
import type { BuildOutcome, TriggerContext, VictimQueue } from "./scenario.js";

/** Drop `undefined` values so ADO receives a flat string→string parameter map. */
function flattenParams(params: Record<string, string | undefined>): Record<string, string> {
  const out: Record<string, string> = {};
  for (const [k, v] of Object.entries(params)) {
    if (typeof v === "string") out[k] = v;
  }
  return out;
}

/**
 * Resolve the victim-build poll tuning from trigger-e2e env vars, falling back
 * to `waitForBuild`'s generic defaults when unset. These knobs live HERE (in
 * trigger-e2e code) rather than in the shared `AdoRest` client so an
 * executor-e2e run can never be silently retimed by a `TRIGGER_E2E_*` variable
 * that happens to be set in its shell.
 */
function pollOptions(): { timeoutMs?: number; pollMs?: number } {
  const timeoutMs = Number(process.env.TRIGGER_E2E_BUILD_TIMEOUT_MS) || undefined;
  const pollMs = Number(process.env.TRIGGER_E2E_BUILD_POLL_MS) || undefined;
  return { timeoutMs, pollMs };
}

export async function runVictim(
  ctx: TriggerContext,
  queue: VictimQueue,
  onQueued?: (buildId: number) => void,
): Promise<BuildOutcome> {
  const templateParameters = flattenParams(queue.templateParameters);
  const queued = await ctx.rest.queueBuild(ctx.victimDefinitionId, {
    sourceBranch: queue.sourceBranch,
    templateParameters,
  });
  // Report the id as soon as the build is queued so the caller can cancel it on
  // cleanup even if the subsequent `waitForBuild` poll throws (e.g. a timeout)
  // and never returns a BuildOutcome.
  onQueued?.(queued.id);
  ctx.log(`  queued victim build #${queued.id}${queue.sourceBranch ? ` on ${queue.sourceBranch}` : ""}`);

  const terminal = await ctx.rest.waitForBuild(queued.id, pollOptions());
  const tags = await ctx.rest.getBuildTags(queued.id);
  ctx.log(
    `  victim build #${queued.id} completed: result=${terminal.result ?? "?"} tags=[${tags.join(", ")}]`,
  );

  return { status: terminal.status, result: terminal.result, tags, buildId: queued.id };
}
