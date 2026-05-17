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
import { evaluatePredicates, validatePredicateTree } from "./predicates.js";
import { selfCancelIfRequested } from "./selfcancel.js";
import { PolicyTracker } from "../shared/policy.js";
import { setOutput, complete, logError } from "../shared/vso-logger.js";

// Cap the decoded spec at 256 KiB. ADO pipeline env vars are bounded
// (typically <32 KiB), so a legitimate spec is two orders of magnitude
// smaller than this. The cap exists to short-circuit pathological payloads
// (e.g. a deeply nested or extremely long JSON blob) before they reach
// JSON.parse, which would otherwise allocate aggressively and could stall
// the gate step.
export const MAX_SPEC_DECODED_BYTES = 256 * 1024;

async function main(): Promise<void> {
  const raw = process.env.GATE_SPEC;
  if (!raw) {
    logError("GATE_SPEC env var missing");
    complete("Failed");
    process.exit(1);
  }

  let spec: GateSpec;
  try {
    const decoded = Buffer.from(raw, "base64");
    if (decoded.length > MAX_SPEC_DECODED_BYTES) {
      logError(
        `GATE_SPEC decoded size ${decoded.length} bytes exceeds cap of ${MAX_SPEC_DECODED_BYTES} bytes`,
      );
      complete("Failed");
      process.exit(1);
    }
    spec = JSON.parse(decoded.toString("utf8")) as GateSpec;
  } catch (e) {
    logError(`Failed to decode GATE_SPEC: ${(e as Error).message}`);
    complete("Failed");
    process.exit(1);
  }

  // Pre-flight: walk the predicate tree and reject any unknown `type`
  // discriminant *before* fact acquisition. Without this, an unknown
  // predicate is only surfaced when evaluatePredicate is reached — and
  // if the required fact is unavailable, evaluatePredicate is never
  // called, masking the version drift.
  try {
    for (const check of spec.checks ?? []) {
      validatePredicateTree(check.predicate);
    }
  } catch (e) {
    logError((e as Error).message);
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
