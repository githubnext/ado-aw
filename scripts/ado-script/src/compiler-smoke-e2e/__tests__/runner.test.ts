import { describe, expect, it } from "vitest";

import type { FixtureBuildClient, FixtureBuildRequest } from "../runner.js";
import { runFixtures } from "../runner.js";

interface FakeBuild {
  status: string;
  result?: string;
  /** Optional identity override for a specific poll — omit to auto-match the request (see `makeFakeClient`'s default-fill behavior below). */
  definition?: { id?: number };
  sourceBranch?: string;
  sourceVersion?: string;
}

/**
 * A fully scripted fake ADO Build client: queue results + a per-build status
 * timeline.
 *
 * By default every polled build's identity (definition id, sourceBranch,
 * sourceVersion) is auto-filled to exactly match what was requested — via
 * `req()`'s `sourceBranch: "refs/heads/x"` / `sourceVersion: "sha"` defaults
 * and a definitionId reverse-derived from `queueResults` — so tests that
 * aren't specifically exercising the identity-mismatch check never need to
 * care about it. A timeline entry can still explicitly set any of these
 * fields (including to a wrong value, or omit them, via a hand-rolled
 * `FixtureBuildClient` instead of this helper) to exercise the mismatch
 * path.
 */
function makeFakeClient(opts: {
  queueResults: Record<number, { ok: true; id: number } | { ok: false; error: string }>;
  /** For each queued build id, the sequence of statuses returned on successive getBuild() polls (last value repeats). */
  timelines: Record<number, FakeBuild[]>;
  onCancel?: (buildId: number) => void;
}): { client: FixtureBuildClient; cancelled: number[] } {
  const cancelled: number[] = [];
  const pollCounts: Record<number, number> = {};
  const definitionIdByBuildId = new Map<number, number>();
  for (const [definitionIdStr, result] of Object.entries(opts.queueResults)) {
    if (result.ok) definitionIdByBuildId.set(result.id, Number(definitionIdStr));
  }
  const client: FixtureBuildClient = {
    async queueBuild(definitionId) {
      const result = opts.queueResults[definitionId];
      if (!result) throw new Error(`no queue result configured for definition ${definitionId}`);
      if (!result.ok) throw new Error(result.error);
      return { id: result.id };
    },
    async getBuild(buildId) {
      const timeline = opts.timelines[buildId];
      if (!timeline || timeline.length === 0) throw new Error(`no timeline for build ${buildId}`);
      const idx = pollCounts[buildId] ?? 0;
      pollCounts[buildId] = idx + 1;
      const entry = timeline[Math.min(idx, timeline.length - 1)]!;
      const defaultIdentity = {
        definition: { id: definitionIdByBuildId.get(buildId) },
        sourceBranch: "refs/heads/x",
        sourceVersion: "sha",
      };
      return { ...defaultIdentity, ...entry };
    },
    async cancelBuild(buildId) {
      cancelled.push(buildId);
      opts.onCancel?.(buildId);
    },
    buildUrl(buildId) {
      return `https://example/_build/results?buildId=${buildId}`;
    },
  };
  return { client, cancelled };
}

function req(name: FixtureBuildRequest["name"], definitionId: number): FixtureBuildRequest {
  return { name, definitionId, sourceBranch: "refs/heads/x", sourceVersion: "sha" };
}

const noopSleep = async (): Promise<void> => {};

describe("runFixtures", () => {
  it("succeeds when every fixture queues and completes successfully", async () => {
    const { client } = makeFakeClient({
      queueResults: {
        1: { ok: true, id: 101 },
        2: { ok: true, id: 102 },
      },
      timelines: {
        101: [{ status: "completed", result: "succeeded" }],
        102: [{ status: "completed", result: "succeeded" }],
      },
    });
    const outcome = await runFixtures(client, [req("canary", 1), req("azure-cli", 2)], {
      concurrency: 2,
      timeoutMs: 10_000,
      pollMs: 1,
      log: () => {},
      sleepImpl: noopSleep,
    });
    expect(outcome.ok).toBe(true);
    expect(outcome.results.map((r) => r.status)).toEqual(["succeeded", "succeeded"]);
    expect(outcome.allTerminal).toBe(true);
    expect(outcome.results.every((r) => r.terminalProven)).toBe(true);
  });

  it("preserves declaration order in results regardless of completion order", async () => {
    const { client } = makeFakeClient({
      queueResults: { 1: { ok: true, id: 201 }, 2: { ok: true, id: 202 } },
      timelines: {
        // build 201 (first in declaration order) takes longer to complete than 202.
        201: [
          { status: "inProgress" },
          { status: "inProgress" },
          { status: "completed", result: "succeeded" },
        ],
        202: [{ status: "completed", result: "succeeded" }],
      },
    });
    const outcome = await runFixtures(client, [req("canary", 1), req("azure-cli", 2)], {
      concurrency: 2,
      timeoutMs: 10_000,
      pollMs: 1,
      log: () => {},
      sleepImpl: noopSleep,
    });
    expect(outcome.results.map((r) => r.name)).toEqual(["canary", "azure-cli"]);
    expect(outcome.results.every((r) => r.status === "succeeded")).toBe(true);
  });

  it("a partial queue failure still polls and reports the successfully queued builds", async () => {
    const { client } = makeFakeClient({
      queueResults: {
        1: { ok: false, error: "definition disabled" },
        2: { ok: true, id: 302 },
      },
      timelines: {
        302: [{ status: "completed", result: "succeeded" }],
      },
    });
    const outcome = await runFixtures(client, [req("canary", 1), req("azure-cli", 2)], {
      concurrency: 2,
      timeoutMs: 10_000,
      pollMs: 1,
      log: () => {},
      sleepImpl: noopSleep,
    });
    expect(outcome.ok).toBe(false);
    const canary = outcome.results.find((r) => r.name === "canary")!;
    const azureCli = outcome.results.find((r) => r.name === "azure-cli")!;
    expect(canary.status).toBe("queue-failed");
    expect(canary.message).toMatch(/definition disabled/);
    expect(azureCli.status).toBe("succeeded");
    // A queueBuild error is ambiguous (ADO may have accepted the build
    // before the client observed the failure), so it can never be treated
    // as proof nothing was created — the ref must be retained.
    expect(canary.terminalProven).toBe(false);
    expect(outcome.allTerminal).toBe(false);
  });

  it("a failed build flips the shared abort flag and cancels sibling builds", async () => {
    const timelines: Record<number, FakeBuild[]> = {
      401: [{ status: "completed", result: "failed" }],
      // 402 never completes on its own; only cancellation (via cancelGraceMs) ends the poll.
      402: Array.from({ length: 50 }, () => ({ status: "inProgress" as const })),
    };
    const { client, cancelled } = makeFakeClient({
      queueResults: { 1: { ok: true, id: 401 }, 2: { ok: true, id: 402 } },
      timelines,
    });

    const outcome = await runFixtures(client, [req("canary", 1), req("azure-cli", 2)], {
      concurrency: 2,
      timeoutMs: 10_000,
      pollMs: 1,
      log: () => {},
      sleepImpl: noopSleep,
      cancelGraceMs: 5,
    });
    expect(outcome.ok).toBe(false);
    const canary = outcome.results.find((r) => r.name === "canary")!;
    const azureCli = outcome.results.find((r) => r.name === "azure-cli")!;
    expect(canary.status).toBe("failed");
    expect(azureCli.status === "canceled" || azureCli.status === "timed-out").toBe(true);
    expect(cancelled).toContain(402);
  });

  it("a per-build timeout cancels that build and flips the shared abort flag for siblings", async () => {
    const timelines: Record<number, FakeBuild[]> = {
      501: Array.from({ length: 1000 }, () => ({ status: "inProgress" })),
      502: Array.from({ length: 1000 }, () => ({ status: "inProgress" })),
    };
    const { client } = makeFakeClient({
      queueResults: { 1: { ok: true, id: 501 }, 2: { ok: true, id: 502 } },
      timelines,
    });
    const outcome = await runFixtures(client, [req("canary", 1), req("azure-cli", 2)], {
      concurrency: 2,
      timeoutMs: 5,
      pollMs: 1,
      log: () => {},
      sleepImpl: noopSleep,
      cancelGraceMs: 5,
    });
    expect(outcome.ok).toBe(false);
    for (const r of outcome.results) {
      expect(["timed-out", "canceled", "failed"]).toContain(r.status);
    }
  });

  it("never returns with a build left non-terminal (waits out the cancel grace period)", async () => {
    const timelines: Record<number, FakeBuild[]> = {
      601: [{ status: "completed", result: "failed" }],
      // 602 never transitions to completed even after cancellation is requested.
      602: Array.from({ length: 1000 }, () => ({ status: "inProgress" })),
    };
    const { client } = makeFakeClient({
      queueResults: { 1: { ok: true, id: 601 }, 2: { ok: true, id: 602 } },
      timelines,
    });
    const outcome = await runFixtures(client, [req("canary", 1), req("azure-cli", 2)], {
      concurrency: 2,
      timeoutMs: 10_000,
      pollMs: 1,
      log: () => {},
      sleepImpl: noopSleep,
      cancelGraceMs: 5,
    });
    const azureCli = outcome.results.find((r) => r.name === "azure-cli")!;
    expect(azureCli.status).toBe("timed-out");
    expect(azureCli.message).toMatch(/cancellation grace period/);
  });

  it("respects the configured concurrency (never more than N builds polled in parallel)", async () => {
    let inFlight = 0;
    let maxInFlight = 0;
    const fixtureNames = ["canary", "azure-cli", "noop-target", "janitor"] as const;
    const buildIds = [701, 702, 703, 704];

    const client: FixtureBuildClient = {
      async queueBuild(definitionId) {
        return { id: buildIds[definitionId - 1]! };
      },
      async getBuild() {
        inFlight++;
        maxInFlight = Math.max(maxInFlight, inFlight);
        await new Promise((r) => setTimeout(r, 5));
        inFlight--;
        return { status: "completed", result: "succeeded" };
      },
      async cancelBuild() {},
      buildUrl: (id) => `https://example/${id}`,
    };

    const requests = fixtureNames.map((name, i) => req(name, i + 1));
    await runFixtures(client, requests, {
      concurrency: 2,
      timeoutMs: 10_000,
      pollMs: 1,
      log: () => {},
    });
    expect(maxInFlight).toBeLessThanOrEqual(2);
  });

  // ---- Fix #3: never claim a build is terminal without positive proof ----

  it("marks allTerminal=false when getBuild keeps failing and never confirms a terminal state, even after cancellation", async () => {
    const client: FixtureBuildClient = {
      async queueBuild(definitionId) {
        return { id: 800 + definitionId };
      },
      async getBuild() {
        throw new Error("transient network error");
      },
      async cancelBuild() {},
      buildUrl: (id) => `https://example/${id}`,
    };
    const outcome = await runFixtures(client, [req("canary", 1)], {
      concurrency: 1,
      timeoutMs: 10_000,
      pollMs: 1,
      log: () => {},
      sleepImpl: noopSleep,
      cancelGraceMs: 5,
    });
    expect(outcome.allTerminal).toBe(false);
    const canary = outcome.results[0]!;
    expect(canary.status).toBe("failed");
    expect(canary.terminalProven).toBe(false);
    expect(canary.message).toMatch(/never confirmed a terminal state/);
  });

  it("recovers from a transient getBuild error: once a later call confirms completion, terminalProven is true", async () => {
    let calls = 0;
    const client: FixtureBuildClient = {
      async queueBuild(definitionId) {
        return { id: 810 + definitionId };
      },
      async getBuild() {
        calls++;
        if (calls <= 2) throw new Error("transient network error");
        return {
          status: "completed",
          result: "canceled",
          definition: { id: 1 },
          sourceBranch: "refs/heads/x",
          sourceVersion: "sha",
        };
      },
      async cancelBuild() {},
      buildUrl: (id) => `https://example/${id}`,
    };
    const outcome = await runFixtures(client, [req("canary", 1)], {
      concurrency: 1,
      timeoutMs: 10_000,
      pollMs: 1,
      log: () => {},
      sleepImpl: noopSleep,
      cancelGraceMs: 5_000,
    });
    const canary = outcome.results[0]!;
    expect(canary.terminalProven).toBe(true);
    expect(canary.status).toBe("canceled");
    expect(outcome.allTerminal).toBe(true);
  });

  it("marks allTerminal=false when cancellation is requested (grace period) but the build never actually stops, even if cancelBuild itself throws", async () => {
    const client: FixtureBuildClient = {
      async queueBuild(definitionId) {
        return { id: 820 + definitionId };
      },
      async getBuild() {
        return { status: "inProgress" };
      },
      async cancelBuild() {
        throw new Error("cancel API rejected");
      },
      buildUrl: (id) => `https://example/${id}`,
    };
    const outcome = await runFixtures(client, [req("canary", 1)], {
      concurrency: 1,
      timeoutMs: 5,
      pollMs: 1,
      log: () => {},
      sleepImpl: noopSleep,
      cancelGraceMs: 5,
    });
    const canary = outcome.results[0]!;
    expect(canary.status).toBe("timed-out");
    expect(canary.terminalProven).toBe(false);
    expect(outcome.allTerminal).toBe(false);
  });

  // ---- Fix #4: verify the queued build's identity matches the request ----

  it("treats a definitionId/sourceBranch/sourceVersion mismatch as a failure and flips the shared abort flag", async () => {
    const cancelled: number[] = [];
    const client: FixtureBuildClient = {
      async queueBuild(definitionId) {
        return { id: 900 + definitionId };
      },
      async getBuild(buildId) {
        if (buildId === 901) {
          // Wrong sourceVersion — as if ADO (or a racing queue) somehow
          // built a different commit than what was requested.
          return {
            status: "completed",
            result: "succeeded",
            definition: { id: 1 },
            sourceBranch: "refs/heads/x",
            sourceVersion: "not-the-requested-sha",
          };
        }
        // 902 (azure-cli) has correct, matching identity — isolates the
        // mismatch assertion below to the canary build only; it is only
        // cancelled because canary's mismatch flips the shared abort flag.
        return {
          status: "inProgress",
          definition: { id: 2 },
          sourceBranch: "refs/heads/x",
          sourceVersion: "sha",
        };
      },
      async cancelBuild(buildId) {
        cancelled.push(buildId);
      },
      buildUrl: (id) => `https://example/${id}`,
    };
    const outcome = await runFixtures(client, [req("canary", 1), req("azure-cli", 2)], {
      concurrency: 2,
      timeoutMs: 10_000,
      pollMs: 1,
      log: () => {},
      sleepImpl: noopSleep,
      cancelGraceMs: 5,
    });
    expect(outcome.ok).toBe(false);
    const canary = outcome.results.find((r) => r.name === "canary")!;
    expect(canary.status).toBe("failed");
    expect(canary.terminalProven).toBe(true);
    expect(canary.message).toMatch(/does not match the requested queue parameters/);
    expect(canary.message).toMatch(/sourceVersion/);
    // The mismatch must also cancel the sibling build.
    expect(cancelled).toContain(902);
  });

  it("treats a wrong definition id as a mismatch even when sourceBranch/sourceVersion are correct", async () => {
    const client: FixtureBuildClient = {
      async queueBuild(definitionId) {
        return { id: 910 + definitionId };
      },
      async getBuild() {
        return {
          status: "completed",
          result: "succeeded",
          definition: { id: 999 }, // wrong — requested definition was 1
          sourceBranch: "refs/heads/x",
          sourceVersion: "sha",
        };
      },
      async cancelBuild() {},
      buildUrl: (id) => `https://example/${id}`,
    };
    const outcome = await runFixtures(client, [req("canary", 1)], {
      concurrency: 1,
      timeoutMs: 10_000,
      pollMs: 1,
      log: () => {},
      sleepImpl: noopSleep,
      cancelGraceMs: 5,
    });
    const canary = outcome.results[0]!;
    expect(canary.status).toBe("failed");
    expect(canary.message).toMatch(/does not match the requested queue parameters/);
    expect(canary.message).toMatch(/definition id 999/);
  });

  it("treats a wrong sourceBranch as a mismatch even when definition id/sourceVersion are correct", async () => {
    const client: FixtureBuildClient = {
      async queueBuild(definitionId) {
        return { id: 920 + definitionId };
      },
      async getBuild() {
        return {
          status: "completed",
          result: "succeeded",
          definition: { id: 1 },
          sourceBranch: "refs/heads/some-other-branch", // wrong
          sourceVersion: "sha",
        };
      },
      async cancelBuild() {},
      buildUrl: (id) => `https://example/${id}`,
    };
    const outcome = await runFixtures(client, [req("canary", 1)], {
      concurrency: 1,
      timeoutMs: 10_000,
      pollMs: 1,
      log: () => {},
      sleepImpl: noopSleep,
      cancelGraceMs: 5,
    });
    const canary = outcome.results[0]!;
    expect(canary.status).toBe("failed");
    expect(canary.message).toMatch(/does not match the requested queue parameters/);
    expect(canary.message).toMatch(/sourceBranch/);
  });

  it("treats entirely missing identity fields as a mismatch — missing data is never treated as proof of the requested build", async () => {
    const client: FixtureBuildClient = {
      async queueBuild(definitionId) {
        return { id: 930 + definitionId };
      },
      async getBuild() {
        // No `definition`, `sourceBranch`, or `sourceVersion` at all.
        return { status: "completed", result: "succeeded" };
      },
      async cancelBuild() {},
      buildUrl: (id) => `https://example/${id}`,
    };
    const outcome = await runFixtures(client, [req("canary", 1)], {
      concurrency: 1,
      timeoutMs: 10_000,
      pollMs: 1,
      log: () => {},
      sleepImpl: noopSleep,
      cancelGraceMs: 5,
    });
    expect(outcome.ok).toBe(false);
    const canary = outcome.results[0]!;
    expect(canary.status).toBe("failed");
    // Even though ADO reported "succeeded", a build with unverifiable
    // identity must never be trusted or reported as succeeded.
    expect(canary.result).not.toBe(undefined);
    expect(canary.message).toMatch(/does not match the requested queue parameters/);
    expect(canary.message).toMatch(/definition id <missing>/);
    expect(canary.message).toMatch(/sourceBranch '<missing>'/);
    expect(canary.message).toMatch(/sourceVersion '<missing>'/);
    // The identity check runs BEFORE the terminal-state result is trusted,
    // so this is still positively proven terminal (ADO really did report
    // "completed") — just correctly reported as a failure, never a
    // false "succeeded".
    expect(canary.terminalProven).toBe(true);
  });

  // ---- Fix #3 (queue-failure abort propagation) ----

  it("a queue failure immediately signals the shared abort flag so an already-queued sibling is cancelled rather than run to completion", async () => {
    const cancelled: number[] = [];
    const client: FixtureBuildClient = {
      async queueBuild(definitionId) {
        if (definitionId === 1) throw new Error("definition disabled");
        return { id: 900 + definitionId };
      },
      async getBuild(buildId) {
        if (cancelled.includes(buildId)) {
          return {
            status: "completed",
            result: "canceled",
            definition: { id: buildId - 900 },
            sourceBranch: "refs/heads/x",
            sourceVersion: "sha",
          };
        }
        return {
          status: "inProgress",
          definition: { id: buildId - 900 },
          sourceBranch: "refs/heads/x",
          sourceVersion: "sha",
        };
      },
      async cancelBuild(buildId) {
        cancelled.push(buildId);
      },
      buildUrl: (id) => `https://example/${id}`,
    };
    const outcome = await runFixtures(client, [req("canary", 1), req("azure-cli", 2)], {
      concurrency: 2,
      timeoutMs: 10_000,
      pollMs: 1,
      log: () => {},
      sleepImpl: noopSleep,
      cancelGraceMs: 1_000,
    });
    expect(outcome.ok).toBe(false);
    expect(cancelled).toContain(902);
    const azureCli = outcome.results.find((r) => r.name === "azure-cli")!;
    expect(azureCli.status).toBe("canceled");
    expect(azureCli.terminalProven).toBe(true);
    const canary = outcome.results.find((r) => r.name === "canary")!;
    expect(canary.status).toBe("queue-failed");
    // Ambiguous queue failure: never proven, so the overall run can't
    // claim every build is terminal even though the sibling was cancelled
    // and confirmed.
    expect(canary.terminalProven).toBe(false);
    expect(outcome.allTerminal).toBe(false);
  });

  // ---- Fix #6: concurrent queueing ----

  it("queues every fixture concurrently rather than one at a time, while preserving declaration-order results", async () => {
    const startOrder: number[] = [];
    const resolveOrder: number[] = [];
    const client: FixtureBuildClient = {
      async queueBuild(definitionId) {
        startOrder.push(definitionId);
        // definition 1 is deliberately the slowest to resolve. If queueing
        // were sequential, definitions 2/3 could not even START until 1
        // resolved — so under sequential queueing resolveOrder would still
        // be [1, 2, 3]. Under concurrent queueing, 2 and 3 (fast) resolve
        // before 1 (slow) because all three started at once.
        const delayMs = definitionId === 1 ? 20 : 1;
        await new Promise((r) => setTimeout(r, delayMs));
        resolveOrder.push(definitionId);
        return { id: 1000 + definitionId };
      },
      async getBuild(buildId) {
        return {
          status: "completed",
          result: "succeeded",
          definition: { id: buildId - 1000 },
          sourceBranch: "refs/heads/x",
          sourceVersion: "sha",
        };
      },
      async cancelBuild() {},
      buildUrl: (id) => `https://example/${id}`,
    };
    const outcome = await runFixtures(
      client,
      [req("canary", 1), req("azure-cli", 2), req("noop-target", 3)],
      { concurrency: 3, timeoutMs: 10_000, pollMs: 1, log: () => {} },
    );
    expect(resolveOrder.indexOf(1)).toBe(2); // definition 1 resolves LAST despite being declared first
    expect(outcome.results.map((r) => r.name)).toEqual(["canary", "azure-cli", "noop-target"]);
    expect(outcome.results.every((r) => r.status === "succeeded")).toBe(true);
  });
});
