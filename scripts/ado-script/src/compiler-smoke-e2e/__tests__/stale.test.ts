import { describe, expect, it } from "vitest";

import type { RemoteRef } from "../git.js";
import { scanStaleRefs, type StaleScanBuild, type StaleScanClient } from "../stale.js";

const NOW = new Date("2024-06-01T00:00:00Z").getTime();
const HOUR = 3_600_000;
const CHILD_DEFINITION_IDS = [901, 902, 903];

function ref(buildId: number): RemoteRef {
  return { ref: `refs/heads/ado-aw-smoke-candidate/${buildId}`, sha: `sha-${buildId}` };
}

interface ClientOpts {
  /** child (definitionId, branch) -> builds; defaults to "no builds found" (i.e. safely deletable) for every child */
  childBuilds?: Record<string, StaleScanBuild[]>;
  /** definitionId that should throw when queried for child builds, regardless of branch */
  childLookupErrorFor?: number;
}

function client(builds: Record<number, StaleScanBuild>, opts: ClientOpts = {}): StaleScanClient {
  return {
    async getBuild(buildId) {
      const b = builds[buildId];
      if (!b) throw new Error(`no such build ${buildId}`);
      return b;
    },
    async listBuildsForDefinitionBranch(definitionId, branch) {
      if (opts.childLookupErrorFor === definitionId) {
        throw new Error(`child lookup failed for definition ${definitionId}`);
      }
      return opts.childBuilds?.[`${definitionId}:${branch}`] ?? [];
    },
  };
}

const baseOpts = {
  baseRef: "refs/heads/main",
  ownRef: "refs/heads/ado-aw-smoke-candidate/999",
  definitionId: 42,
  childDefinitionIds: CHILD_DEFINITION_IDS,
  staleRefHours: 24,
};

describe("scanStaleRefs", () => {
  it("marks a completed, own-definition, old-enough build as eligible when no child builds are found", async () => {
    const decisions = await scanStaleRefs({
      ...baseOpts,
      refs: [ref(1)],
      client: client({
        1: { status: "completed", result: "succeeded", definition: { id: 42 }, finishTime: new Date(NOW - 30 * HOUR).toISOString() },
      }),
      now: () => NOW,
    });
    expect(decisions).toEqual([
      expect.objectContaining({ ref: ref(1).ref, outcome: "eligible" }),
    ]);
  });

  it("marks a too-recently-completed build as too-recent, not eligible", async () => {
    const decisions = await scanStaleRefs({
      ...baseOpts,
      refs: [ref(2)],
      client: client({
        2: { status: "completed", result: "succeeded", definition: { id: 42 }, finishTime: new Date(NOW - 2 * HOUR).toISOString() },
      }),
      now: () => NOW,
    });
    expect(decisions[0]?.outcome).toBe("too-recent");
  });

  it("marks a still-in-progress build as active, never eligible", async () => {
    const decisions = await scanStaleRefs({
      ...baseOpts,
      refs: [ref(3)],
      client: client({
        3: { status: "inProgress", definition: { id: 42 } },
      }),
      now: () => NOW,
    });
    expect(decisions[0]?.outcome).toBe("active");
  });

  it("fails closed (ambiguous) for an unparseable ref name", async () => {
    const decisions = await scanStaleRefs({
      ...baseOpts,
      refs: [{ ref: "refs/heads/ado-aw-smoke-candidate/not-a-number", sha: "x" }],
      client: client({}),
      now: () => NOW,
    });
    expect(decisions[0]?.outcome).toBe("ambiguous");
  });

  it("fails closed (ambiguous) when the build lookup throws", async () => {
    const decisions = await scanStaleRefs({
      ...baseOpts,
      refs: [ref(4)],
      client: {
        getBuild: async () => {
          throw new Error("network error");
        },
        listBuildsForDefinitionBranch: async () => [],
      },
      now: () => NOW,
    });
    expect(decisions[0]?.outcome).toBe("ambiguous");
    expect(decisions[0]?.reason).toMatch(/network error/);
  });

  it("fails closed (ambiguous) when the build belongs to a different definition", async () => {
    const decisions = await scanStaleRefs({
      ...baseOpts,
      refs: [ref(5)],
      client: client({
        5: { status: "completed", result: "succeeded", definition: { id: 43 }, finishTime: new Date(NOW - 30 * HOUR).toISOString() },
      }),
      now: () => NOW,
    });
    expect(decisions[0]?.outcome).toBe("ambiguous");
    expect(decisions[0]?.reason).toMatch(/definition 43/);
  });

  it("fails closed (ambiguous) when a completed build has no usable timestamp", async () => {
    const decisions = await scanStaleRefs({
      ...baseOpts,
      refs: [ref(6)],
      client: client({
        6: { status: "completed", result: "succeeded", definition: { id: 42 } },
      }),
      now: () => NOW,
    });
    expect(decisions[0]?.outcome).toBe("ambiguous");
  });

  it("never treats the base ref or this run's own ref as a candidate to evaluate", async () => {
    const decisions = await scanStaleRefs({
      ...baseOpts,
      refs: [
        { ref: "refs/heads/main", sha: "base" },
        { ref: "refs/heads/ado-aw-smoke-candidate/999", sha: "own" },
      ],
      client: client({}),
      now: () => NOW,
    });
    expect(decisions).toEqual([]);
  });

  it("respects a custom staleRefHours threshold at the boundary", async () => {
    const builds = client({
      7: { status: "completed", result: "succeeded", definition: { id: 42 }, finishTime: new Date(NOW - 7 * HOUR).toISOString() },
    });
    const eligible = await scanStaleRefs({
      ...baseOpts,
      refs: [ref(7)],
      staleRefHours: 6,
      client: builds,
      now: () => NOW,
    });
    expect(eligible[0]?.outcome).toBe("eligible");

    const notYet = await scanStaleRefs({
      ...baseOpts,
      refs: [ref(7)],
      staleRefHours: 8,
      client: builds,
      now: () => NOW,
    });
    expect(notYet[0]?.outcome).toBe("too-recent");
  });

  // ---- Fix #5: an old, terminal orchestrator build does NOT prove its
  // queued fixture ("child") builds have also finished ----

  it("marks a ref as active (not eligible) when a fixture/child definition still has a non-completed build on that branch", async () => {
    const decisions = await scanStaleRefs({
      ...baseOpts,
      refs: [ref(8)],
      client: client(
        {
          8: { status: "completed", result: "succeeded", definition: { id: 42 }, finishTime: new Date(NOW - 30 * HOUR).toISOString() },
        },
        {
          childBuilds: {
            [`902:${ref(8).ref}`]: [{ status: "inProgress" }],
          },
        },
      ),
      now: () => NOW,
    });
    expect(decisions[0]?.outcome).toBe("active");
    expect(decisions[0]?.reason).toMatch(/902/);
  });

  it("fails closed (ambiguous) when a child-definition build lookup throws, even if the parent is old and completed", async () => {
    const decisions = await scanStaleRefs({
      ...baseOpts,
      refs: [ref(9)],
      client: client(
        {
          9: { status: "completed", result: "succeeded", definition: { id: 42 }, finishTime: new Date(NOW - 30 * HOUR).toISOString() },
        },
        { childLookupErrorFor: 903 },
      ),
      now: () => NOW,
    });
    expect(decisions[0]?.outcome).toBe("ambiguous");
    expect(decisions[0]?.reason).toMatch(/903/);
  });

  it("is eligible only once every fixed child definition has zero non-completed builds on the exact branch", async () => {
    const decisions = await scanStaleRefs({
      ...baseOpts,
      refs: [ref(10)],
      client: client(
        {
          10: { status: "completed", result: "succeeded", definition: { id: 42 }, finishTime: new Date(NOW - 30 * HOUR).toISOString() },
        },
        {
          childBuilds: {
            [`901:${ref(10).ref}`]: [{ status: "completed", result: "succeeded" }],
            [`902:${ref(10).ref}`]: [{ status: "completed", result: "canceled" }],
            [`903:${ref(10).ref}`]: [],
          },
        },
      ),
      now: () => NOW,
    });
    expect(decisions[0]?.outcome).toBe("eligible");
  });

  it("classifies an abruptly-cancelled orchestrator that completes while a child keeps running as active, never eligible", async () => {
    // Regression for the exact scenario in Fix #5: the parent (orchestrator)
    // build is 'completed' (e.g. cancelled) and old, but one of its queued
    // fixture children is still 'inProgress' on the same branch.
    const decisions = await scanStaleRefs({
      ...baseOpts,
      refs: [ref(11)],
      client: client(
        {
          11: { status: "completed", result: "canceled", definition: { id: 42 }, finishTime: new Date(NOW - 48 * HOUR).toISOString() },
        },
        {
          childBuilds: {
            [`901:${ref(11).ref}`]: [{ status: "inProgress" }],
          },
        },
      ),
      now: () => NOW,
    });
    expect(decisions[0]?.outcome).toBe("active");
  });
});
