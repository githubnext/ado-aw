/**
 * Bounded-concurrency queue/poll/cancel state machine for the five staged
 * fixture builds.
 *
 * Contract:
 *   - every fixture is queued concurrently (a queue failure for one fixture
 *     never prevents queueing the rest — "partial queue failure still
 *     polls/cancels queued" — but DOES immediately signal the shared abort
 *     flag so already-queued siblings are cancelled rather than left to run
 *     to completion for a build that's already doomed to fail overall),
 *   - a `queueBuild` error is treated as AMBIGUOUS, never as proof nothing
 *     was created: ADO may have accepted and started the build before the
 *     client lost the response (timeout, connection reset, etc). This
 *     harness has no cheap way to reconcile that here, so a queue failure
 *     always sets `terminalProven = false` (the simpler, fail-closed
 *     choice) — the caller must retain the candidate ref, and the startup
 *     stale-ref scanner's per-child-definition build check is what
 *     eventually proves (or disproves) that an orphaned build exists,
 *   - successfully queued builds are polled with bounded concurrency,
 *   - the FIRST failure or timeout flips a shared abort flag; every other
 *     still-polling build is cancelled and polled to a terminal state
 *     before this function returns,
 *   - each polled build's identity (definition id, sourceBranch,
 *     sourceVersion) is verified against what was requested with exact
 *     equality on all three fields — a missing field is itself treated as
 *     a mismatch (never as "no data to compare, assume fine") — and any
 *     mismatch is treated as a failure and also flips the abort flag,
 *   - this function NEVER silently claims a build "must have stopped"
 *     merely because polling gave up (a `getBuild` error, a cancellation
 *     that never gets confirmed, or a grace-period expiry). Every result
 *     that isn't a positively-observed terminal state is reported via
 *     `RunFixturesOutcome.allTerminal = false` so the caller knows it must
 *     NOT delete the shared git ref out from under a build that might still
 *     be running,
 *   - results preserve the caller's declaration order regardless of
 *     completion order.
 *
 * Test-harness module; not shipped in `ado-script.zip`.
 */
import type { FixtureName } from "./config.js";
import { sleep as defaultSleep } from "./process.js";

/** What a queued build looks like once polled — kept narrow (a subset of `AdoRest.BuildSummary`) so tests never need a full AdoRest fake. */
export interface PolledBuild {
  status?: string;
  result?: string;
  definition?: { id?: number };
  sourceBranch?: string;
  sourceVersion?: string;
}

/** The minimal ADO Build surface this state machine needs. */
export interface FixtureBuildClient {
  queueBuild(definitionId: number, opts: { sourceBranch: string; sourceVersion: string }): Promise<{ id: number }>;
  getBuild(buildId: number): Promise<PolledBuild>;
  cancelBuild(buildId: number): Promise<void>;
  buildUrl(buildId: number): string;
}

export interface FixtureBuildRequest {
  name: FixtureName;
  definitionId: number;
  sourceBranch: string;
  sourceVersion: string;
}

export type FixtureBuildStatus =
  | "succeeded"
  | "failed"
  | "canceled"
  | "timed-out"
  | "queue-failed";

export interface FixtureBuildResult {
  name: FixtureName;
  definitionId: number;
  buildId?: number;
  url?: string;
  status: FixtureBuildStatus;
  result?: string;
  message?: string;
  durationMs: number;
  /**
   * Whether this fixture's terminal state was positively confirmed.
   * `false` means the harness could not prove ADO actually stopped this
   * build — callers must treat this as "possibly still running" and never
   * delete the candidate ref. For `queue-failed`, `false` also covers the
   * case where the `queueBuild` request itself may have been accepted by
   * ADO despite the client observing an error (ambiguous network/timeout
   * failures) — the harness never assumes "no response" means "no build".
   */
  terminalProven: boolean;
}

export interface RunFixturesOptions {
  concurrency: number;
  /** Bounded per-build wait before this harness gives up and cancels it. */
  timeoutMs: number;
  pollMs: number;
  log: (msg: string) => void;
  sleepImpl?: (ms: number) => Promise<void>;
  /** Extra bounded wait for a cancelled build to actually reach 'completed' (default: max(pollMs*6, 60s)). */
  cancelGraceMs?: number;
}

export interface RunFixturesOutcome {
  ok: boolean;
  /** True only when EVERY fixture's terminal state was positively proven. See {@link FixtureBuildResult.terminalProven}. */
  allTerminal: boolean;
  results: FixtureBuildResult[];
}

function errMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

async function safeCancel(
  client: FixtureBuildClient,
  buildId: number,
  log: (msg: string) => void,
): Promise<void> {
  try {
    await client.cancelBuild(buildId);
  } catch (err) {
    log(`WARNING: cancelBuild(${buildId}) failed: ${errMessage(err)}`);
  }
}

interface AbortSignalLike {
  aborted: boolean;
  signal(): void;
}

function makeAbortFlag(): AbortSignalLike {
  return {
    aborted: false,
    signal(): void {
      this.aborted = true;
    },
  };
}

/** Compare a polled build's identity against what this harness requested. `undefined` is itself a mismatch — missing identity data is never treated as proof of the requested build. Returns `undefined` only when every field is present and exactly equal. */
function describeMismatch(
  build: PolledBuild,
  expected: { definitionId: number; sourceBranch: string; sourceVersion: string },
): string | undefined {
  const problems: string[] = [];
  if (build.definition?.id !== expected.definitionId) {
    problems.push(`definition id ${build.definition?.id ?? "<missing>"} (expected ${expected.definitionId})`);
  }
  if (build.sourceBranch !== expected.sourceBranch) {
    problems.push(`sourceBranch '${build.sourceBranch ?? "<missing>"}' (expected '${expected.sourceBranch}')`);
  }
  if (build.sourceVersion !== expected.sourceVersion) {
    problems.push(`sourceVersion '${build.sourceVersion ?? "<missing>"}' (expected '${expected.sourceVersion}')`);
  }
  if (problems.length === 0) return undefined;
  return `does not match the requested queue parameters: ${problems.join("; ")}`;
}

interface PollOneResult {
  status: FixtureBuildStatus;
  result?: string;
  message?: string;
  terminalProven: boolean;
}

/**
 * Poll a single queued build to a terminal state. Only ever resolves with
 * `terminalProven: true` once ADO has positively confirmed the build
 * reached `status === "completed"` — a `getBuild` error, a cancellation
 * that can't be confirmed, or a grace-period expiry all resolve with
 * `terminalProven: false` instead of guessing.
 */
async function pollOne(
  client: FixtureBuildClient,
  buildId: number,
  expected: { definitionId: number; sourceBranch: string; sourceVersion: string },
  opts: {
    deadlineAt: number;
    cancelGraceMs: number;
    pollMs: number;
    abort: AbortSignalLike;
    sleepImpl: (ms: number) => Promise<void>;
    log: (msg: string) => void;
  },
): Promise<PollOneResult> {
  let cancelRequestedAt: number | undefined;
  let mismatchReason: string | undefined;
  let verified = false;

  const requestCancel = async (): Promise<void> => {
    opts.abort.signal();
    if (cancelRequestedAt === undefined) {
      cancelRequestedAt = Date.now();
      await safeCancel(client, buildId, opts.log);
    }
  };

  for (;;) {
    let build: PolledBuild;
    try {
      build = await client.getBuild(buildId);
    } catch (err) {
      // A poll error never proves the build stopped. Request cancellation
      // and keep retrying (bounded by the same cancellation grace period)
      // in case a LATER call confirms a genuinely terminal state; only
      // give up as "unproven" once that grace period elapses.
      opts.log(`WARNING: getBuild(${buildId}) failed: ${errMessage(err)}`);
      const hadCancelRequest = cancelRequestedAt !== undefined;
      await requestCancel();
      if (hadCancelRequest && Date.now() - cancelRequestedAt! >= opts.cancelGraceMs) {
        return {
          status: "failed",
          message: `build #${buildId}: getBuild kept failing and never confirmed a terminal state within the cancellation grace period: ${errMessage(err)}`,
          terminalProven: false,
        };
      }
      await opts.sleepImpl(opts.pollMs);
      continue;
    }

    if (!verified) {
      const mismatch = describeMismatch(build, expected);
      if (mismatch) {
        mismatchReason = `build #${buildId} ${mismatch}`;
        opts.log(`WARNING: ${mismatchReason}`);
        await requestCancel();
      }
      verified = true;
    }

    if (build.status === "completed") {
      if (mismatchReason) {
        return { status: "failed", result: build.result, message: mismatchReason, terminalProven: true };
      }
      if (cancelRequestedAt !== undefined) {
        return { status: "canceled", result: build.result, terminalProven: true };
      }
      if (build.result === "succeeded") {
        return { status: "succeeded", result: build.result, terminalProven: true };
      }
      opts.abort.signal();
      return { status: "failed", result: build.result, terminalProven: true };
    }

    if (cancelRequestedAt === undefined) {
      const ownTimeout = Date.now() >= opts.deadlineAt;
      if (ownTimeout || opts.abort.aborted) {
        if (ownTimeout) opts.abort.signal();
        await requestCancel();
      }
    } else if (Date.now() - cancelRequestedAt >= opts.cancelGraceMs) {
      return {
        status: "timed-out",
        message:
          mismatchReason ??
          `build #${buildId} did not reach a terminal state within the cancellation grace period`,
        terminalProven: false,
      };
    }

    await opts.sleepImpl(opts.pollMs);
  }
}

/** Queue and poll every fixture request. See module docstring for the abort/cancel/terminal-proof contract. */
export async function runFixtures(
  client: FixtureBuildClient,
  requests: readonly FixtureBuildRequest[],
  opts: RunFixturesOptions,
): Promise<RunFixturesOutcome> {
  const sleepImpl = opts.sleepImpl ?? defaultSleep;
  const cancelGraceMs = opts.cancelGraceMs ?? Math.max(opts.pollMs * 6, 60_000);

  const results: FixtureBuildResult[] = requests.map((r) => ({
    name: r.name,
    definitionId: r.definitionId,
    status: "queue-failed",
    durationMs: 0,
    // Placeholder until the queueing attempt below resolves one way or the
    // other; a failed attempt overwrites this with `false` (see comment at
    // the catch site) and a successful attempt is overwritten again once
    // Phase 2 polls it to a real terminal state.
    terminalProven: false,
  }));

  // Shared abort flag: created up front so a Phase 1 queue failure can
  // immediately cancel any sibling already queued, instead of only taking
  // effect once Phase 2 starts.
  const abort = makeAbortFlag();

  // ---- Phase 1: queue every fixture concurrently (never stops on a single failure) ----
  const queued: { index: number; buildId: number; start: number }[] = [];
  await Promise.all(
    requests.map(async (req, i) => {
      const start = Date.now();
      try {
        const build = await client.queueBuild(req.definitionId, {
          sourceBranch: req.sourceBranch,
          sourceVersion: req.sourceVersion,
        });
        queued.push({ index: i, buildId: build.id, start });
        results[i] = {
          ...results[i]!,
          buildId: build.id,
          url: client.buildUrl(build.id),
          status: "queue-failed", // overwritten once polling resolves
        };
        opts.log(`[${req.name}] queued build #${build.id} on ${req.sourceBranch}`);
      } catch (err) {
        abort.signal();
        // A queueBuild error is ambiguous — ADO may have accepted the
        // request before the client lost the response — so this is never
        // treated as proof no build exists. `terminalProven: false` forces
        // the caller to retain the candidate ref; the stale-ref scanner's
        // per-child-definition check is what can later prove no orphaned
        // build was actually created.
        results[i] = {
          ...results[i]!,
          message: errMessage(err),
          durationMs: Date.now() - start,
          terminalProven: false,
        };
        opts.log(`[${req.name}] queue FAILED: ${errMessage(err)}`);
      }
    }),
  );

  // ---- Phase 2: poll all queued builds, bounded concurrency, shared abort ----
  const deadlineAt = Date.now() + opts.timeoutMs;
  let nextQueuedIdx = 0;

  const worker = async (): Promise<void> => {
    for (;;) {
      const qi = nextQueuedIdx++;
      if (qi >= queued.length) return;
      const q = queued[qi]!;
      const req = requests[q.index]!;
      try {
        const outcome = await pollOne(
          client,
          q.buildId,
          { definitionId: req.definitionId, sourceBranch: req.sourceBranch, sourceVersion: req.sourceVersion },
          {
            deadlineAt,
            cancelGraceMs,
            pollMs: opts.pollMs,
            abort,
            sleepImpl,
            log: opts.log,
          },
        );
        results[q.index] = {
          ...results[q.index]!,
          status: outcome.status,
          result: outcome.result,
          message: outcome.message,
          durationMs: Date.now() - q.start,
          terminalProven: outcome.terminalProven,
        };
        opts.log(`[${req.name}] build #${q.buildId} -> ${outcome.status}${outcome.result ? ` (${outcome.result})` : ""}`);
      } catch (err) {
        // pollOne itself is designed never to throw — this is a defensive
        // backstop only. Treat as unproven, never as a confirmed stop.
        abort.signal();
        results[q.index] = {
          ...results[q.index]!,
          status: "failed",
          message: errMessage(err),
          durationMs: Date.now() - q.start,
          terminalProven: false,
        };
        opts.log(`[${req.name}] poll FAILED: ${errMessage(err)}`);
      }
    }
  };

  const workerCount = Math.min(opts.concurrency, queued.length);
  await Promise.all(Array.from({ length: workerCount }, worker));

  const ok = results.every((r) => r.status === "succeeded");
  const allTerminal = results.every((r) => r.terminalProven);
  return { ok, allTerminal, results };
}
