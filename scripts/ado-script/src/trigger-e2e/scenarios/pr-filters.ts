/**
 * Gate filter scenarios (`gate.js`).
 *
 * Each creates a real PR, promotes it to synthetic-PR semantics (empty synth
 * spec = match-all), and queues the victim with a single-filter `GATE_SPEC`.
 * The gate then evaluates that filter against the real PR facts (labels /
 * changed files / draft state / branch / build reason / change count / time)
 * and either lets the run proceed or self-cancels the build.
 *
 * Assertion via build tags + result:
 *   - pass → `result=succeeded`, `trig.synth.promoted` + `trig.should-run.true`,
 *            no `pr-gate.skipped`.
 *   - skip → `result=canceled`, `trig.synth.promoted` + `pr-gate.skipped` +
 *            `pr-gate.<suffix>`, no `trig.should-run.true`.
 *
 * Test-harness module; not shipped in `ado-script.zip`.
 */
import {
  buildGateSpec,
  buildReasonIncludeCheck,
  changeCountCheck,
  changedFilesCheck,
  draftCheck,
  encodeGateSpec,
  labelsCheck,
  targetBranchCheck,
  timeWindowCheck,
  type Check,
} from "../gate-spec.js";
import type { Expected, TriggerScenario, VictimParameters } from "../scenario.js";
import {
  createPrContext,
  promoteSynthSpec,
  requirePrRepo,
  teardownPrContext,
  type PrContext,
} from "./common.js";
import type { CreatePrOptions } from "./common.js";

interface FilterScenarioSpec {
  readonly id: string;
  readonly description: string;
  /** PR preconditions to create (labels / files / draft / title). */
  readonly pr: Omit<CreatePrOptions, "id">;
  /** The gate check under test. */
  readonly check: Check | ((now: Date) => Check);
  /** Whether the gate should let the run proceed (`pass`) or skip it. */
  readonly outcome: "pass" | "skip";
  /** For `skip`, the `pr-gate.<suffix>` tag the failing check must emit. */
  readonly failTag?: string;
  /** Extra victim parameters (pipeline-var fact overrides), if any. */
  readonly params?: Omit<VictimParameters, "gateSpec" | "prSynthSpec">;
}

function makeFilterScenario(spec: FilterScenarioSpec): TriggerScenario<PrContext> {
  return {
    id: spec.id,
    description: spec.description,
    async setup(ctx) {
      requirePrRepo(ctx);
      return createPrContext(ctx, { id: spec.id, ...spec.pr });
    },
    queue(_ctx, state) {
      const check = typeof spec.check === "function" ? spec.check(new Date()) : spec.check;
      const gateSpec = encodeGateSpec(buildGateSpec("pull-request", [check]));
      return {
        sourceBranch: state.sourceRef,
        // Promote against the PR's REAL target branch (not a hardcoded "main")
        // so the gate evaluates the filter under test on any default branch.
        templateParameters: {
          gateSpec,
          prSynthSpec: promoteSynthSpec(state.targetBranch),
          ...spec.params,
        },
      };
    },
    expected(): Expected {
      if (spec.outcome === "pass") {
        return {
          result: "succeeded",
          tags: ["trig.synth.promoted", "trig.should-run.true"],
          absentTags: ["pr-gate.skipped"],
        };
      }
      const tags = ["trig.synth.promoted", "pr-gate.skipped"];
      if (spec.failTag) tags.push(spec.failTag);
      return { result: "canceled", tags, absentTags: ["trig.should-run.true"] };
    },
    async cleanup(ctx, state) {
      await teardownPrContext(ctx, state);
    },
  };
}

/** HH:MM string for `minutes` (mod 1440) since UTC midnight. */
function hhmm(minutes: number): string {
  const m = ((minutes % 1440) + 1440) % 1440;
  const h = Math.floor(m / 60);
  const mm = m % 60;
  return `${String(h).padStart(2, "0")}:${String(mm).padStart(2, "0")}`;
}

const specs: FilterScenarioSpec[] = [
  // ── labels ────────────────────────────────────────────────────────────
  {
    id: "labels-pass",
    description: "label_set_match any-of matches the PR's labels",
    pr: { labels: ["run-agent"] },
    check: labelsCheck({ anyOf: ["run-agent"] }),
    outcome: "pass",
  },
  {
    id: "labels-skip",
    description: "label_set_match any-of misses the PR's labels → self-cancel",
    pr: { labels: ["do-not-run"] },
    check: labelsCheck({ anyOf: ["run-agent"] }),
    outcome: "skip",
    failTag: "pr-gate.labels-mismatch",
  },
  // ── changed files ─────────────────────────────────────────────────────
  {
    id: "changed-files-pass",
    description: "file_glob_match include matches a changed file",
    pr: { files: { "/src/trig-changed-pass.txt": "x\n" } },
    check: changedFilesCheck({ include: ["src/**"] }),
    outcome: "pass",
  },
  {
    id: "changed-files-skip",
    description: "file_glob_match include matches no changed file → self-cancel",
    pr: { files: { "/docs/trig-changed-skip.md": "x\n" } },
    check: changedFilesCheck({ include: ["src/**"] }),
    outcome: "skip",
    failTag: "pr-gate.changed-files-mismatch",
  },
  // ── draft ─────────────────────────────────────────────────────────────
  {
    id: "draft-pass",
    description: "draft equals true and the PR is a draft",
    pr: { draft: true },
    check: draftCheck(true),
    outcome: "pass",
  },
  {
    id: "draft-skip",
    description: "draft equals true but PR is not a draft → self-cancel",
    pr: {},
    check: draftCheck(true),
    outcome: "skip",
    failTag: "pr-gate.draft-mismatch",
  },
  // ── target branch ─────────────────────────────────────────────────────
  {
    id: "target-branch-pass",
    description: "target_branch glob matches the PR's target (main)",
    pr: {},
    check: targetBranchCheck("main"),
    outcome: "pass",
  },
  {
    id: "target-branch-skip",
    description: "target_branch glob does not match the PR's target → self-cancel",
    pr: {},
    check: targetBranchCheck("release/*"),
    outcome: "skip",
    failTag: "pr-gate.target-branch-mismatch",
  },
  // ── build reason ──────────────────────────────────────────────────────
  {
    id: "build-reason-pass",
    description: "build_reason include matches the queued build reason (Manual)",
    pr: {},
    check: buildReasonIncludeCheck(["Manual"]),
    outcome: "pass",
  },
  {
    id: "build-reason-skip",
    description: "build_reason include requires PullRequest but reason is Manual → self-cancel",
    pr: {},
    check: buildReasonIncludeCheck(["PullRequest"]),
    outcome: "skip",
    failTag: "pr-gate.build-reason-mismatch",
  },
  // ── change count ──────────────────────────────────────────────────────
  {
    id: "change-count-pass",
    description: "numeric_range min=1 max=10 and the PR changed 2 files → runs",
    // Two files also exercises the batched multi-file push in createPrContext.
    pr: {
      files: {
        "/src/trig-count-pass-a.txt": "a\n",
        "/src/trig-count-pass-b.txt": "b\n",
      },
    },
    check: changeCountCheck({ min: 1, max: 10 }),
    outcome: "pass",
  },
  {
    id: "change-count-skip",
    description: "numeric_range min=5 but the PR changed 1 file → self-cancel",
    pr: { files: { "/src/trig-count-skip.txt": "x\n" } },
    check: changeCountCheck({ min: 5 }),
    outcome: "skip",
    failTag: "pr-gate.changes-mismatch",
  },
  // ── time window ───────────────────────────────────────────────────────
  {
    id: "time-window-skip",
    description: "time_window that excludes now → self-cancel",
    pr: {},
    // A 1-hour window starting 2h in the future never contains the eval time.
    check: (now: Date) => {
      const nowMin = now.getUTCHours() * 60 + now.getUTCMinutes();
      return timeWindowCheck(hhmm(nowMin + 120), hhmm(nowMin + 180));
    },
    outcome: "skip",
    failTag: "pr-gate.time-window-mismatch",
  },
];

export const filterScenarios: TriggerScenario<PrContext>[] = specs.map(makeFilterScenario);
