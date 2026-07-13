/**
 * Bypass scenario (`gate/bypass.ts`).
 *
 * When a build is neither a real PR nor a synth-promoted CI build, the PR gate
 * auto-passes ("not a PR build — gate passes automatically") and tags the build
 * `pr-gate.passed`. This scenario queues the victim on its default branch with
 * NO open PR, so `exec-context-pr-synth` skips and the gate bypasses.
 *
 * Needs no PR context, so it runs even when `TRIGGER_E2E_VICTIM_REPO` is unset
 * — a useful baseline that the victim pipeline is wired correctly.
 *
 * Test-harness module; not shipped in `ado-script.zip`.
 */
import { buildGateSpec, encodeGateSpec, encodePrSynthSpec, labelsCheck } from "../gate-spec.js";
import type { TriggerScenario } from "../scenario.js";

const bypassManual: TriggerScenario<undefined> = {
  id: "bypass-no-pr",
  description: "no PR + Manual build → PR gate bypasses (pr-gate.passed)",
  async setup() {
    return undefined;
  },
  queue() {
    // A non-trivial gate spec proves the bypass short-circuits BEFORE any
    // filter evaluation (a label check would otherwise fail with no PR).
    return {
      // No sourceBranch → the victim builds its default branch; no PR exists
      // for it, so synth skips and the gate bypasses.
      templateParameters: {
        gateSpec: encodeGateSpec(
          buildGateSpec("pull-request", [labelsCheck({ anyOf: ["run-agent"] })]),
        ),
        prSynthSpec: encodePrSynthSpec(),
      },
    };
  },
  expected() {
    return {
      result: "succeeded",
      tags: ["pr-gate.passed", "trig.synth.skipped"],
      absentTags: ["pr-gate.skipped", "trig.synth.promoted"],
    };
  },
  async cleanup() {
    // Nothing created.
  },
};

export const bypassScenarios: TriggerScenario<undefined>[] = [bypassManual];
