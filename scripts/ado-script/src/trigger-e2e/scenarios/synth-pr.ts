/**
 * Synthetic-PR promotion scenarios (`exec-context-pr-synth.js`).
 *
 * Each creates a real PR in the victim's `self` repo, queues the victim on the
 * PR's source branch, and asserts the synth outcome via the victim's
 * `trig.synth.*` build tag:
 *   - a matching PR       → `trig.synth.promoted` (+ gate runs)
 *   - a branch mismatch   → `trig.synth.skipped` (+ gate bypasses)
 *   - a path mismatch     → `trig.synth.skipped`
 *
 * Test-harness module; not shipped in `ado-script.zip`.
 */
import { buildGateSpec, encodeGateSpec, encodePrSynthSpec } from "../gate-spec.js";
import type { TriggerScenario } from "../scenario.js";
import { createPrContext, requirePrRepo, teardownPrContext, type PrContext } from "./common.js";

/** Empty gate spec (no checks) — the gate passes iff it is not bypassed. */
const EMPTY_GATE = encodeGateSpec(buildGateSpec("pull-request", []));

/** A matching PR promotes the CI build to synthetic-PR semantics. */
const synthPromote: TriggerScenario<PrContext> = {
  id: "synth-promote",
  description: "matching open PR promotes the queued build to synthetic-PR",
  async setup(ctx) {
    requirePrRepo(ctx);
    return createPrContext(ctx, { id: "synth-promote" });
  },
  queue(_ctx, state) {
    return {
      sourceBranch: state.sourceRef,
      templateParameters: {
        gateSpec: EMPTY_GATE,
        // branches include-all (empty) → the PR's target branch matches.
        prSynthSpec: encodePrSynthSpec({ branches: { include: ["main"] } }),
      },
    };
  },
  expected() {
    return {
      result: "succeeded",
      tags: ["trig.synth.promoted", "trig.should-run.true"],
      absentTags: ["pr-gate.skipped"],
    };
  },
  async cleanup(ctx, state) {
    await teardownPrContext(ctx, state);
  },
};

/** A PR whose target branch is excluded by the synth spec is not promoted. */
const synthBranchMismatch: TriggerScenario<PrContext> = {
  id: "synth-branch-mismatch",
  description: "PR targeting an unmatched branch is not synth-promoted (gate bypasses)",
  async setup(ctx) {
    requirePrRepo(ctx);
    return createPrContext(ctx, { id: "synth-branch-mismatch" });
  },
  queue(_ctx, state) {
    return {
      sourceBranch: state.sourceRef,
      templateParameters: {
        gateSpec: EMPTY_GATE,
        // Only release/* targets match; the PR targets main → no promotion.
        prSynthSpec: encodePrSynthSpec({ branches: { include: ["release/*"] } }),
      },
    };
  },
  expected() {
    return {
      result: "succeeded",
      tags: ["trig.synth.skipped", "pr-gate.passed"],
      absentTags: ["trig.synth.promoted"],
    };
  },
  async cleanup(ctx, state) {
    await teardownPrContext(ctx, state);
  },
};

/** A PR none of whose changed files match the synth path filter is not promoted. */
const synthPathMismatch: TriggerScenario<PrContext> = {
  id: "synth-path-mismatch",
  description: "PR with no changed file matching the synth path filter is not promoted",
  async setup(ctx) {
    requirePrRepo(ctx);
    // Change only a docs file; the synth spec will require src/**.
    return createPrContext(ctx, {
      id: "synth-path-mismatch",
      files: { [`/docs/trig/${ctx.buildId}-synth-path.md`]: "docs only change\n" },
    });
  },
  queue(_ctx, state) {
    return {
      sourceBranch: state.sourceRef,
      templateParameters: {
        gateSpec: EMPTY_GATE,
        prSynthSpec: encodePrSynthSpec({ paths: { include: ["src/**"] } }),
      },
    };
  },
  expected() {
    return {
      result: "succeeded",
      tags: ["trig.synth.skipped", "pr-gate.passed"],
      absentTags: ["trig.synth.promoted"],
    };
  },
  async cleanup(ctx, state) {
    await teardownPrContext(ctx, state);
  },
};

export const synthScenarios: TriggerScenario<PrContext>[] = [
  synthPromote,
  synthBranchMismatch,
  synthPathMismatch,
];
