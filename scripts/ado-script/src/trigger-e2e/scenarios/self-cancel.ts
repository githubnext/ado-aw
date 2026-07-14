/**
 * Self-cancel scenario (`gate/selfcancel.ts`).
 *
 * When the gate decides not to run (a filter fails on a synth-promoted build),
 * it self-cancels the whole build via the ADO REST API and tags it
 * `pr-gate.skipped`. This scenario explicitly asserts that the build reaches a
 * terminal `canceled` result (not merely `succeeded`/`failed`), which is the
 * behaviour agent pipelines rely on to avoid running the agent job.
 *
 * Test-harness module; not shipped in `ado-script.zip`.
 */
import { buildGateSpec, encodeGateSpec, targetBranchCheck } from "../gate-spec.js";
import type { BuildOutcome, TriggerScenario } from "../scenario.js";
import {
  createPrContext,
  promoteSynthSpec,
  requirePrRepo,
  teardownPrContext,
  type PrContext,
} from "./common.js";

const selfCancelOnFilterFail: TriggerScenario<PrContext> = {
  id: "self-cancel-on-filter-fail",
  description: "a failing filter on a synth-promoted build self-cancels the whole build",
  async setup(ctx) {
    requirePrRepo(ctx);
    return createPrContext(ctx, { id: "self-cancel-on-filter-fail" });
  },
  queue(_ctx, state) {
    // The PR targets its default branch; require release/* so the target-branch
    // check fails. Promote against the real target branch so the gate runs.
    return {
      sourceBranch: state.sourceRef,
      templateParameters: {
        gateSpec: encodeGateSpec(buildGateSpec("pull-request", [targetBranchCheck("release/*")])),
        prSynthSpec: promoteSynthSpec(state.targetBranch),
      },
    };
  },
  expected() {
    return {
      result: "canceled",
      tags: ["trig.synth.promoted", "pr-gate.skipped", "pr-gate.target-branch-mismatch"],
      absentTags: ["trig.should-run.true"],
    };
  },
  async assert(_ctx, _state, outcome: BuildOutcome) {
    // Belt-and-braces: the runner already checks result==="canceled", but make
    // the intent explicit so a future refactor of the default assertion can't
    // silently drop the self-cancel guarantee.
    if (outcome.result !== "canceled") {
      throw new Error(`self-cancel expected result 'canceled' but got '${outcome.result ?? "?"}'`);
    }
  },
  async cleanup(ctx, state) {
    await teardownPrContext(ctx, state);
  },
};

export const selfCancelScenarios: TriggerScenario<PrContext>[] = [selfCancelOnFilterFail];
