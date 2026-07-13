/**
 * Deterministic trigger-condition (gate / synth-PR) E2E scenario contract.
 *
 * Unlike the executor E2E harness (which runs `ado-aw execute` directly), each
 * trigger scenario exercises the *runtime trigger conditions managed by
 * ado-script* — the `gate.js` and `exec-context-pr-synth.js` bundles — by
 * queuing a real build of a hand-authored, parameterized **victim pipeline**
 * and asserting the observable gate decision via **build tags + build result**.
 *
 * A scenario optionally sets up real ADO PR context (branch + PR + labels +
 * draft state), queues the victim with a per-scenario `GATE_SPEC` /
 * `PR_SYNTH_SPEC` (and pipeline-var fact overrides) as template parameters,
 * polls the build to completion, then asserts the tags/result and cleans up.
 *
 * ## Victim observable contract (REST-visible build tags)
 *   - `trig.synth.promoted` | `trig.synth.skipped` | `trig.synth.real-pr`
 *       — emitted by the victim after `exec-context-pr-synth.js`.
 *   - `pr-gate.passed`       — gate bypassed (not a PR/synth build).
 *   - `pr-gate.skipped` + `pr-gate.<check-suffix>` — a filter failed;
 *       the gate self-cancels the build (`result == "canceled"`).
 *   - `trig.should-run.true` — emitted by a post-gate step that only runs
 *       when the gate did NOT self-cancel (i.e. `SHOULD_RUN == true`).
 *
 * Test-harness module; not shipped in `ado-script.zip`.
 */
import type { AdoRest } from "../executor-e2e/ado-rest.js";

/** Shared, read-only context handed to every scenario phase. */
export interface TriggerContext {
  /** ADO collection URI, e.g. https://dev.azure.com/msazuresphere/ */
  readonly orgUrl: string;
  /** ADO project name, e.g. AgentPlayground. */
  readonly project: string;
  /** ADO Git repo used to create real PRs (initialised `main`). */
  readonly adoRepo: string;
  /** Current orchestrator build id (drives the per-run object prefix). */
  readonly buildId: string;
  /** Write-capable ADO token (System.AccessToken or a PAT). */
  readonly token: string;
  /** Registered definition id of the hand-authored victim pipeline. */
  readonly victimDefinitionId: number;
  /** REST helper bound to {orgUrl, project, token}. */
  readonly rest: AdoRest;
  /** Structured logger (writes to the pipeline log). */
  readonly log: (msg: string) => void;
  /** Deterministic object-name prefix: `ado-aw-trig-<buildId>-<id>`. */
  readonly prefix: (id: string) => string;
}

/** Template parameters + source branch used to queue one victim build. */
export interface VictimQueue {
  /** Source branch the victim builds (points synth at the PR); optional. */
  sourceBranch?: string;
  /** Runtime parameters passed to the victim pipeline. */
  templateParameters: VictimParameters;
}

/**
 * Victim pipeline runtime parameters. `gateSpec` is required; the rest are
 * optional overrides. Empty overrides fall back to the victim's real ADO
 * predefined variables.
 */
export interface VictimParameters {
  /** base64 GATE_SPEC (required). */
  gateSpec: string;
  /** base64 PR_SYNTH_SPEC (default: empty include/exclude). */
  prSynthSpec?: string;
  /** Override for the `pr_title` fact (ADO_PR_TITLE). */
  prTitle?: string;
  /** Override for the `author_email` fact (ADO_AUTHOR_EMAIL). */
  authorEmail?: string;
  /** Override for the `source_branch` fact (ADO_SOURCE_BRANCH). */
  sourceBranchFact?: string;
  /** Override for the `target_branch` fact (ADO_TARGET_BRANCH). */
  targetBranchFact?: string;
  /** Override for the `commit_message` fact (ADO_COMMIT_MESSAGE). */
  commitMessage?: string;
  /** [key: string]: string — ADO template parameters must be flat strings. */
  [k: string]: string | undefined;
}

/** Terminal build state + tags read after the victim run completes. */
export interface BuildOutcome {
  /** ADO build status — always `completed` once polling returns. */
  status: string;
  /** succeeded | partiallySucceeded | failed | canceled. */
  result?: string;
  /** All build tags present at completion. */
  tags: string[];
  /** The queued victim build id (for logging / debugging). */
  buildId: number;
}

/** Declarative expectation, checked by the runner's default assertion. */
export interface Expected {
  /** Required build result; default `succeeded`. */
  result?: string;
  /** Tags that MUST all be present. */
  tags?: string[];
  /** Tags that MUST NOT be present. */
  absentTags?: string[];
}

/**
 * A single deterministic trigger scenario.
 *
 * `State` is threaded from `setup` through `queue`/`assert`/`cleanup` so a
 * scenario can remember the PR id / branch it created.
 */
export interface TriggerScenario<State = unknown> {
  /** Unique harness id for reporting and object naming. */
  readonly id: string;
  /** One-line human description of what this scenario proves. */
  readonly description: string;
  /**
   * Create real ADO preconditions (branch/PR/labels); return remembered state.
   *
   * When this throws (a non-`SkipError`), the runner will NOT call `cleanup()`.
   * Any ADO objects partially created before the throw must be torn down
   * explicitly inside this function before rethrowing (see `scenarios/common.ts`
   * `createPrContext`, which follows the executor-e2e pattern).
   */
  setup(ctx: TriggerContext): Promise<State>;
  /** Build the victim queue request (source branch + template parameters). */
  queue(ctx: TriggerContext, state: State): VictimQueue;
  /** Declarative expectation for the completed victim build. */
  expected(ctx: TriggerContext, state: State): Expected;
  /** Optional extra assertion beyond `expected` (throw on failure). */
  assert?(ctx: TriggerContext, state: State, outcome: BuildOutcome): Promise<void>;
  /** Best-effort teardown of everything `setup` created. */
  cleanup(ctx: TriggerContext, state: State): Promise<void>;
}

/** Outcome of running one scenario. */
export interface ScenarioResult {
  id: string;
  ok: boolean;
  /** "setup" | "queue" | "assert" | "cleanup" | "skipped". */
  phase?: string;
  message?: string;
  durationMs: number;
  /** True when skipped for a missing precondition (not a failure). */
  skipped?: boolean;
}

/**
 * Thrown by a scenario's `setup` when a required precondition is unavailable
 * (e.g. the victim definition id was not supplied). The runner records the
 * scenario as **skipped** rather than failed.
 */
export class SkipError extends Error {
  constructor(reason: string) {
    super(reason);
    this.name = "SkipError";
  }
}
