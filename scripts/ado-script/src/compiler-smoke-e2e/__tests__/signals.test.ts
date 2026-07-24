import { describe, expect, it } from "vitest";

import type { FixtureBuildResult } from "../runner.js";
import { verifyFixtureSignals } from "../signals.js";

function result(
  overrides: Partial<FixtureBuildResult> = {},
): FixtureBuildResult {
  return {
    name: "custom-safe-output",
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

describe("verifyFixtureSignals", () => {
  it("passes when both custom executor tags exist", async () => {
    const outcome = await verifyFixtureSignals(
      {
        getBuildTags: async () => [
          "unrelated",
          "ado-aw-custom-script-42",
          "ado-aw-custom-job-42",
        ],
      },
      [result()],
    );
    expect(outcome.ok).toBe(true);
    expect(outcome.results[0]?.status).toBe("succeeded");
  });

  it("fails a successful child when either route tag is missing", async () => {
    const outcome = await verifyFixtureSignals(
      { getBuildTags: async () => ["ado-aw-custom-script-42"] },
      [result()],
    );
    expect(outcome.ok).toBe(false);
    expect(outcome.results[0]).toMatchObject({
      status: "failed",
      terminalProven: true,
      result: "succeeded",
    });
    expect(outcome.results[0]?.message).toMatch(
      /ado-aw-custom-job-42/,
    );
  });

  it("reports tag API failures without losing terminal proof", async () => {
    const outcome = await verifyFixtureSignals(
      {
        getBuildTags: async () => {
          throw new Error("tag API unavailable");
        },
      },
      [result()],
    );
    expect(outcome.ok).toBe(false);
    expect(outcome.results[0]?.terminalProven).toBe(true);
    expect(outcome.results[0]?.message).toMatch(/tag API unavailable/);
  });

  it("does not query tags for a child that already failed", async () => {
    let calls = 0;
    const outcome = await verifyFixtureSignals(
      {
        getBuildTags: async () => {
          calls++;
          return [];
        },
      },
      [result({ status: "failed", result: "failed" })],
    );
    expect(calls).toBe(0);
    expect(outcome.ok).toBe(false);
  });

  it("leaves fixtures without signal requirements unchanged", async () => {
    let calls = 0;
    const canary = result({ name: "canary" });
    const outcome = await verifyFixtureSignals(
      {
        getBuildTags: async () => {
          calls++;
          return [];
        },
      },
      [canary],
    );
    expect(calls).toBe(0);
    expect(outcome).toEqual({ ok: true, results: [canary] });
  });
});
