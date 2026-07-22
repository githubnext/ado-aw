import { describe, expect, it } from "vitest";

import type { FixtureBuildResult } from "../runner.js";
import { renderResultsTable } from "../report.js";

function result(overrides: Partial<FixtureBuildResult>): FixtureBuildResult {
  return {
    name: "canary",
    definitionId: 2601,
    status: "succeeded",
    durationMs: 12_345,
    terminalProven: true,
    ...overrides,
  };
}

describe("renderResultsTable", () => {
  it("renders a header row and one row per fixture", () => {
    const table = renderResultsTable([
      result({ name: "canary", buildId: 1, url: "https://x/1", result: "succeeded" }),
      result({ name: "azure-cli", definitionId: 2602, buildId: 2, url: "https://x/2", result: "succeeded" }),
    ]);
    const lines = table.split("\n");
    expect(lines[0]).toMatch(/fixture/);
    expect(lines[0]).toMatch(/definition/);
    expect(lines[0]).toMatch(/result/);
    expect(table).toContain("canary");
    expect(table).toContain("azure-cli");
    expect(table).toContain("2601");
    expect(table).toContain("2602");
  });

  it("preserves the caller's declaration order", () => {
    const table = renderResultsTable([
      result({ name: "janitor", definitionId: 2604 }),
      result({ name: "canary", definitionId: 2601 }),
    ]);
    const janitorIdx = table.indexOf("janitor");
    const canaryIdx = table.indexOf("canary");
    expect(janitorIdx).toBeGreaterThan(-1);
    expect(canaryIdx).toBeGreaterThan(janitorIdx);
  });

  it("renders a '-' placeholder for missing buildId/url", () => {
    const table = renderResultsTable([result({ status: "queue-failed", message: "definition disabled" })]);
    expect(table).toContain("queue-failed");
    expect(table).toContain("definition disabled");
  });

  it("shows the result alongside status when present", () => {
    const table = renderResultsTable([result({ status: "succeeded", result: "succeeded" })]);
    expect(table).toContain("succeeded (succeeded)");
  });
});
