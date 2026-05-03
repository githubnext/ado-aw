/**
 * Gate evaluator entry point.
 *
 * Reads a base64-encoded `GateSpec` from `GATE_SPEC` env, runs the bypass
 * logic, acquires runtime facts, evaluates predicates, and emits a single
 * `SHOULD_RUN` setvariable. On failure to acquire facts or evaluate
 * predicates, logs via the VSO logger and exits non-zero.
 */
import type { GateSpec } from "../shared/types.gen.js";
import { runBypass } from "./bypass.js";
import { acquireFacts } from "./facts.js";
import { evaluatePredicates } from "./predicates.js";
import { selfCancelIfRequested } from "./selfcancel.js";
import { PolicyTracker } from "../shared/policy.js";
import { setOutput, complete, logError } from "../shared/vso-logger.js";

async function main(): Promise<void> {
  const raw = process.env.GATE_SPEC;
  if (!raw) {
    logError("GATE_SPEC env var missing");
    complete("Failed");
    process.exit(1);
  }

  let spec: GateSpec;
  try {
    spec = JSON.parse(Buffer.from(raw, "base64").toString("utf8")) as GateSpec;
  } catch (e) {
    logError(`Failed to decode GATE_SPEC: ${(e as Error).message}`);
    complete("Failed");
    process.exit(1);
  }

  if (await runBypass(spec)) {
    return; // bypass handler set SHOULD_RUN and emitted complete
  }

  const tracker = new PolicyTracker(spec.facts);
  const facts = await acquireFacts(spec, tracker);
  const results = evaluatePredicates(spec, facts, tracker);

  const shouldRun = results.every((r) => r === "pass" || r === "skip");
  setOutput("SHOULD_RUN", shouldRun ? "true" : "false");

  if (!shouldRun) {
    await selfCancelIfRequested(spec);
  }

  const sm = tracker.summary();
  complete(
    shouldRun ? "Succeeded" : "SucceededWithIssues",
    `gate: passed=${sm.passed} failed=${sm.failed} skipped=${sm.skipped}`,
  );
}

main().catch((e) => {
  logError(`gate evaluator crashed: ${(e as Error).message}`);
  complete("Failed");
  process.exit(1);
});
