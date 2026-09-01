import { beforeEach, describe, expect, it, vi } from "vitest";

import type { ResolvedCase } from "../cases.js";
import type { FixtureBuildResult } from "../runner.js";
import { verifyCandidateAudit, verifyCaseSignals } from "../signals.js";

const safeSpawnMock = vi.hoisted(() => vi.fn());

vi.mock("../process.js", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../process.js")>();
  return {
    ...actual,
    safeSpawn: safeSpawnMock,
  };
});

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

beforeEach(() => {
  safeSpawnMock.mockReset();
});

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

describe("verifyCandidateAudit", () => {
  const options = {
    adoAwBin: "ado-aw",
    cwd: "/repo",
    orgUrl: "https://dev.azure.com/org",
    project: "project",
    token: "secret-token",
    timeoutMs: 5000,
  };

  it("accepts an audit containing the child build and every artifact family", async () => {
    safeSpawnMock.mockResolvedValue({
      status: 0,
      stdout: JSON.stringify({
        overview: { build_id: 42 },
        downloaded_files: [
          { path: "agent_outputs_42/agent-output.json" },
          { path: "analyzed_outputs_42/verdict.json" },
          { path: "safe_outputs\\executed-safe-outputs.ndjson" },
        ],
      }),
      stderr: "",
      timedOut: false,
      stdoutTruncated: false,
      stderrTruncated: false,
    });

    const canary = result({ caseId: "canary" });
    const outcome = await verifyCandidateAudit([canary], options);

    expect(outcome).toEqual({ ok: true, results: [canary] });
    expect(safeSpawnMock).toHaveBeenCalledWith(
      expect.objectContaining({
        cmd: "ado-aw",
        cwd: "/repo",
        env: { AZURE_DEVOPS_EXT_PAT: "secret-token" },
        args: expect.arrayContaining(["audit", "42", "--no-cache"]),
      }),
    );
  });

  it("redacts the token when the audit subprocess fails", async () => {
    safeSpawnMock.mockResolvedValue({
      status: 1,
      stdout: "",
      stderr: "request failed with secret-token",
      timedOut: false,
      stdoutTruncated: false,
      stderrTruncated: false,
    });

    const outcome = await verifyCandidateAudit([result({ caseId: "canary" })], options);

    expect(outcome.ok).toBe(false);
    expect(outcome.results[0]?.message).toContain("***");
    expect(outcome.results[0]?.message).not.toContain("secret-token");
  });

  it("fails closed without spawning when no successful canary build exists", async () => {
    const outcome = await verifyCandidateAudit(
      [result({ caseId: "canary", status: "failed", result: "failed" })],
      options,
    );

    expect(outcome.ok).toBe(false);
    expect(safeSpawnMock).not.toHaveBeenCalled();
  });
});
