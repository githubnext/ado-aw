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

export async function runVictim(ctx: TriggerContext, queue: VictimQueue): Promise<BuildOutcome> {
  const templateParameters = flattenParams(queue.templateParameters);
  const queued = await ctx.rest.queueBuild(ctx.victimDefinitionId, {
    sourceBranch: queue.sourceBranch,
    templateParameters,
  });
  ctx.log(`  queued victim build #${queued.id}${queue.sourceBranch ? ` on ${queue.sourceBranch}` : ""}`);

  const terminal = await ctx.rest.waitForBuild(queued.id);
  const tags = await ctx.rest.getBuildTags(queued.id);
  ctx.log(
    `  victim build #${queued.id} completed: result=${terminal.result ?? "?"} tags=[${tags.join(", ")}]`,
  );

  return { status: terminal.status, result: terminal.result, tags, buildId: queued.id };
}
