import { describe, expect, it } from "vitest";

import type { ResolvedCase } from "../cases.js";
import type { FixtureBuildResult } from "../runner.js";
import { verifyCaseSignals } from "../signals.js";

/**
 * Tag requirements are declared per case in the manifest, not hardcoded in
 * `signals.ts` — `custom-safe-output` declares one, `canary` declares none.
 */
const CASES: ResolvedCase[] = [
  {
    id: "custom-safe-output",
    lane: "agentic",
    kind: "compiled",
    modes: ["candidate"],
    source: "tests/smoke/custom-safe-output.md",
    assertions: { requiredBuildTags: ["ado-aw-custom-job-{buildId}"] },
    definitionId: 3006,
  },
  {
    id: "canary",
    lane: "agentic",
    kind: "compiled",
    modes: ["candidate"],
    source: "tests/safe-outputs/canary.md",
    definitionId: 3006,
  },
];

function result(overrides: Partial<FixtureBuildResult> = {}): FixtureBuildResult {
  return {
    caseId: "custom-safe-output",
    lane: "agentic",
    definitionId: 3006,
    buildId: 42,
    url: "https://example/42",
    status: "succeeded",
    result: "succeeded",
    durationMs: 1,
    terminalProven: true,
    ...overrides,
  };
}

describe("verifyCaseSignals", () => {
  it("expands {buildId} and passes when the custom job tag exists", async () => {
    const outcome = await verifyCaseSignals(
      { getBuildTags: async () => ["unrelated", "ado-aw-custom-job-42"] },
      CASES,
      [result()],
    );
    expect(outcome.ok).toBe(true);
    expect(outcome.results[0]?.status).toBe("succeeded");
  });

  it("fails a successful child when the declared tag is missing", async () => {
    const outcome = await verifyCaseSignals(
      { getBuildTags: async () => ["unrelated"] },
      CASES,
      [result()],
    );
    expect(outcome.ok).toBe(false);
    expect(outcome.results[0]).toMatchObject({
      status: "failed",
      terminalProven: true,
      result: "succeeded",
    });
    expect(outcome.results[0]?.message).toMatch(/ado-aw-custom-job-42/);
  });

  it("reports tag API failures without losing terminal proof", async () => {
    const outcome = await verifyCaseSignals(
      {
        getBuildTags: async () => {
          throw new Error("tag API unavailable");
        },
      },
      CASES,
      [result()],
    );
    expect(outcome.ok).toBe(false);
    expect(outcome.results[0]?.terminalProven).toBe(true);
    expect(outcome.results[0]?.message).toMatch(/tag API unavailable/);
  });

  it("does not query tags for a child that already failed", async () => {
    let calls = 0;
    const outcome = await verifyCaseSignals(
      {
        getBuildTags: async () => {
          calls++;
          return [];
        },
      },
      CASES,
      [result({ status: "failed", result: "failed" })],
    );
    expect(calls).toBe(0);
    expect(outcome.ok).toBe(false);
  });

  it("leaves cases without declared tag assertions unchanged", async () => {
    let calls = 0;
    const canary = result({ caseId: "canary" });
    const outcome = await verifyCaseSignals(
      {
        getBuildTags: async () => {
          calls++;
          return [];
        },
      },
      CASES,
      [canary],
    );
    expect(calls).toBe(0);
    expect(outcome).toEqual({ ok: true, results: [canary] });
  });

  it("ignores a result whose case is not in the manifest", async () => {
    let calls = 0;
    const orphan = result({ caseId: "not-declared" });
    const outcome = await verifyCaseSignals(
      {
        getBuildTags: async () => {
          calls++;
          return [];
        },
      },
      CASES,
      [orphan],
    );
    expect(calls).toBe(0);
    expect(outcome.ok).toBe(true);
  });
});
