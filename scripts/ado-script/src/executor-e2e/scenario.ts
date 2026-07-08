/**
 * Deterministic Stage 3 (executor) end-to-end scenario contract.
 *
 * Each scenario exercises exactly one safe-output tool without any LLM in the
 * loop: it sets up preconditions via the ADO REST API, crafts the executor's
 * NDJSON input directly, runs the real `ado-aw execute` binary, asserts the
 * effect via the ADO REST API, and cleans up everything it created.
 *
 * This module is a **test harness** — it deliberately does NOT ship in the
 * released `ado-script.zip` (see package.json `build:executor-e2e` and the
 * `NON_BUNDLE_DIRS` carve-out in `src/__tests__/bundle-coverage.test.ts`).
 */
import type { AdoRest } from "./ado-rest.js";

/** One parsed line of `safe-outputs-executed.ndjson` written by Stage 3. */
export interface ExecutedRecord {
  /** snake_case tool name (dashes replaced by the executor). */
  name: string;
  /** "succeeded" | "failed" | "warning" | "budget_exhausted". */
  status: string;
  context?: string | null;
  /** Present only on success; carries the tool's result data. */
  result?: Record<string, unknown> | null;
  /** Present only on non-success; the failure message. */
  error?: string | null;
  timestamp?: string;
}

/** Shared, read-only context handed to every scenario phase. */
export interface ScenarioContext {
  /** ADO collection URI, e.g. https://dev.azure.com/msazuresphere/ */
  readonly orgUrl: string;
  /** ADO project name, e.g. AgentPlayground. */
  readonly project: string;
  /** ADO Git repo used by PR/git scenarios (initialised `main`). */
  readonly adoRepo: string;
  /** Current build id (drives the per-run object prefix). */
  readonly buildId: string;
  /** Write-capable ADO token (System.AccessToken or a PAT). */
  readonly token: string;
  /** Absolute path to the `ado-aw` binary under test. */
  readonly adoAwBin: string;
  /** Per-scenario scratch root; each scenario gets a child dir. */
  readonly workDir: string;
  /** REST helper bound to {orgUrl, project, token}. */
  readonly rest: AdoRest;
  /** Structured logger (writes to the pipeline log). */
  readonly log: (msg: string) => void;
  /** Deterministic object-name prefix: `ado-aw-det-<buildId>-<tool>`. */
  readonly prefix: (tool: string) => string;
}

/**
 * A single deterministic executor scenario.
 *
 * `State` is threaded from `setup` through the later phases so a scenario can
 * remember the ids it created (work-item id, PR id, thread id, …) for
 * assertion and cleanup.
 */
export interface Scenario<State = unknown> {
  /**
   * Unique harness id for reporting and scratch directories. Defaults to
   * `tool`; set this when multiple scenarios exercise the same safe-output.
   */
  readonly id?: string;
  /** kebab-case safe-output tool name (matches the `safe-outputs:` key). */
  readonly tool: string;
  /**
   * When true, the scenario targets the ADO `agent-definitions` repo rather
   * than the pipeline's own repo, so the runner emits a `repos:` block and the
   * per-tool `allowed-repositories` config.
   */
  readonly targetsAdoRepo?: boolean;
  /** Per-tool `safe-outputs: <tool>:` front-matter config fragment. */
  config(ctx: ScenarioContext, state: State): Record<string, unknown>;
  /**
   * Create ADO preconditions; return remembered state.
   *
   * When this throws (a non-`SkipError`), the runner will NOT call `cleanup()`
   * (state is considered unreliable). Any ADO objects partially created before
   * the throw must therefore be torn down explicitly inside this function
   * before rethrowing — see `pr.ts` `setupPr` for the pattern.
   *
   * Note: this applies only to failures in `setup()` itself. Once `setup()`
   * completes, a later throw in `config()`/`ndjson()`/`files()`/`env()` or
   * `assert()` DOES run `cleanup()` (the runner gates cleanup on `setupDone`).
   */
  setup(ctx: ScenarioContext): Promise<State>;
  /**
   * Build the executor NDJSON entry (WITHOUT the `name` field — the runner
   * injects `name: <tool>`).
   */
  ndjson(ctx: ScenarioContext, state: State): Promise<Record<string, unknown>>;
  /**
   * Optional extra files to stage into the safe-output dir before running the
   * executor (relative path -> UTF-8 contents). Used by attachment and
   * create-pull-request scenarios.
   */
  files?(ctx: ScenarioContext, state: State): Promise<Record<string, string>>;
  /**
   * Optional per-scenario environment overrides for the `ado-aw execute`
   * child process (e.g. BUILD_SOURCESDIRECTORY pointing at a git checkout).
   */
  env?(ctx: ScenarioContext, state: State): Promise<Record<string, string>>;
  /**
   * Some scenarios intentionally submit invalid staged output and should pass
   * only when the executor rejects it with the expected failure.
   */
  readonly expectedFailure?: {
    readonly status?: string;
    readonly error: RegExp;
  };
  /**
   * Assert the ADO side-effect actually happened. Throw on failure.
   *
   * May populate fields on `state` (e.g. an id read from the executor result)
   * that `cleanup` needs — do this **before** any fallible check so cleanup can
   * still tear the object down if a later assertion throws.
   */
  assert(ctx: ScenarioContext, state: State, record: ExecutedRecord): Promise<void>;
  /** Best-effort teardown of everything setup/execute created. */
  cleanup(ctx: ScenarioContext, state: State): Promise<void>;
}

/** Outcome of running one scenario. */
export interface ScenarioResult {
  tool: string;
  ok: boolean;
  /** "setup" | "execute" | "assert" | "cleanup" | "skipped". */
  phase?: string;
  message?: string;
  durationMs: number;
  /** True when the scenario was skipped for a missing precondition (not a failure). */
  skipped?: boolean;
}

/**
 * Thrown by a scenario's `setup` when a required precondition is unavailable
 * in this environment (e.g. no wiki exists, or the target pipeline id was not
 * supplied). The runner records the scenario as **skipped** rather than
 * failed, so an incomplete manual handoff never turns the whole suite red.
 */
export class SkipError extends Error {
  constructor(reason: string) {
    super(reason);
    this.name = "SkipError";
  }
}
