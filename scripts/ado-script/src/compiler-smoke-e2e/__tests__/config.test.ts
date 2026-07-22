import { describe, expect, it } from "vitest";

import { candidateRef, loadConfig } from "../config.js";

function baseEnv(overrides: Record<string, string | undefined> = {}): NodeJS.ProcessEnv {
  return {
    SYSTEM_COLLECTIONURI: "https://dev.azure.com/org/",
    SYSTEM_TEAMPROJECT: "AgentPlayground",
    SYSTEM_ACCESSTOKEN: "tok",
    BUILD_BUILDID: "42",
    BUILD_SOURCEBRANCH: "refs/heads/main",
    BUILD_SOURCEVERSION: "abc123",
    BUILD_SOURCESDIRECTORY: "C:\\repo",
    SYSTEM_DEFINITIONID: "99",
    COMPILER_SMOKE_ADO_AW_BIN: "C:\\bin\\ado-aw.exe",
    COMPILER_SMOKE_ARTIFACT_NAME: "ado-aw-candidate",
    COMPILER_SMOKE_MIRROR_REPO: "ado-aw-mirror",
    COMPILER_SMOKE_CANARY_DEFINITION_ID: "2601",
    COMPILER_SMOKE_AZURE_CLI_DEFINITION_ID: "2602",
    COMPILER_SMOKE_NOOP_TARGET_DEFINITION_ID: "2603",
    COMPILER_SMOKE_JANITOR_DEFINITION_ID: "2604",
    COMPILER_SMOKE_REPORTER_DEFINITION_ID: "2605",
    COMPILER_SMOKE_CUSTOM_SAFE_OUTPUT_DEFINITION_ID: "2606",
    ...overrides,
  };
}

describe("loadConfig", () => {
  it("parses a fully valid environment", () => {
    const config = loadConfig(baseEnv());
    expect(config.orgUrl).toBe("https://dev.azure.com/org/");
    expect(config.project).toBe("AgentPlayground");
    expect(config.buildId).toBe(42);
    expect(config.definitionId).toBe(99);
    expect(config.definitionIds).toEqual({
      canary: 2601,
      "azure-cli": 2602,
      "noop-target": 2603,
      janitor: 2604,
      "smoke-failure-reporter": 2605,
      "custom-safe-output": 2606,
    });
    expect(config.concurrency).toBe(6);
    expect(config.childTimeoutMs).toBe(7_200_000);
    expect(config.pollMs).toBe(10_000);
    expect(config.staleRefHours).toBe(24);
  });

  for (const name of [
    "SYSTEM_COLLECTIONURI",
    "SYSTEM_TEAMPROJECT",
    "SYSTEM_ACCESSTOKEN",
    "BUILD_BUILDID",
    "BUILD_SOURCEBRANCH",
    "BUILD_SOURCEVERSION",
    "BUILD_SOURCESDIRECTORY",
    "SYSTEM_DEFINITIONID",
    "COMPILER_SMOKE_ADO_AW_BIN",
    "COMPILER_SMOKE_ARTIFACT_NAME",
    "COMPILER_SMOKE_MIRROR_REPO",
    "COMPILER_SMOKE_CANARY_DEFINITION_ID",
    "COMPILER_SMOKE_AZURE_CLI_DEFINITION_ID",
    "COMPILER_SMOKE_NOOP_TARGET_DEFINITION_ID",
    "COMPILER_SMOKE_JANITOR_DEFINITION_ID",
    "COMPILER_SMOKE_REPORTER_DEFINITION_ID",
    "COMPILER_SMOKE_CUSTOM_SAFE_OUTPUT_DEFINITION_ID",
  ]) {
    it(`rejects a missing ${name}`, () => {
      expect(() => loadConfig(baseEnv({ [name]: undefined }))).toThrow();
    });

    it(`rejects an unexpanded ADO macro for ${name}`, () => {
      expect(() => loadConfig(baseEnv({ [name]: "$(Some.Macro)" }))).toThrow(/unexpanded|not set/);
    });
  }

  it("rejects a malformed (non-numeric) BUILD_BUILDID", () => {
    expect(() => loadConfig(baseEnv({ BUILD_BUILDID: "abc" }))).toThrow(/positive integer/);
  });

  it("rejects a zero BUILD_BUILDID", () => {
    expect(() => loadConfig(baseEnv({ BUILD_BUILDID: "0" }))).toThrow(/positive integer/);
  });

  it("rejects a negative SYSTEM_DEFINITIONID", () => {
    expect(() => loadConfig(baseEnv({ SYSTEM_DEFINITIONID: "-5" }))).toThrow(/positive integer/);
  });

  it("rejects a non-integer fixture definition id", () => {
    expect(() => loadConfig(baseEnv({ COMPILER_SMOKE_JANITOR_DEFINITION_ID: "12.5" }))).toThrow(
      /positive integer/,
    );
  });

  it("rejects duplicate fixture definition ids", () => {
    expect(() =>
      loadConfig(
        baseEnv({
          COMPILER_SMOKE_AZURE_CLI_DEFINITION_ID: "2601",
        }),
      ),
    ).toThrow(/distinct/);
  });

  it("reports every duplicated fixture in the error message", () => {
    expect(() =>
      loadConfig(
        baseEnv({
          COMPILER_SMOKE_AZURE_CLI_DEFINITION_ID: "2601",
          COMPILER_SMOKE_NOOP_TARGET_DEFINITION_ID: "2604",
          COMPILER_SMOKE_JANITOR_DEFINITION_ID: "2604",
        }),
      ),
    ).toThrow(/canary/);
  });

  describe("COMPILER_SMOKE_CONCURRENCY bounds", () => {
    it("defaults to 6 when unset", () => {
      expect(loadConfig(baseEnv()).concurrency).toBe(6);
    });

    it("accepts the lower bound (1)", () => {
      expect(loadConfig(baseEnv({ COMPILER_SMOKE_CONCURRENCY: "1" })).concurrency).toBe(1);
    });

    it("accepts the upper bound (6)", () => {
      expect(loadConfig(baseEnv({ COMPILER_SMOKE_CONCURRENCY: "6" })).concurrency).toBe(6);
    });

    it("rejects 0", () => {
      expect(() => loadConfig(baseEnv({ COMPILER_SMOKE_CONCURRENCY: "0" }))).toThrow(/range/);
    });

    it("rejects 7", () => {
      expect(() => loadConfig(baseEnv({ COMPILER_SMOKE_CONCURRENCY: "7" }))).toThrow(/range/);
    });

    it("rejects a non-integer value", () => {
      expect(() => loadConfig(baseEnv({ COMPILER_SMOKE_CONCURRENCY: "2.5" }))).toThrow(/integer/);
    });
  });

  describe("COMPILER_SMOKE_CHILD_TIMEOUT_MS", () => {
    it("defaults to 7200000ms", () => {
      expect(loadConfig(baseEnv()).childTimeoutMs).toBe(7_200_000);
    });

    it("accepts an explicit override", () => {
      expect(loadConfig(baseEnv({ COMPILER_SMOKE_CHILD_TIMEOUT_MS: "60000" })).childTimeoutMs).toBe(60_000);
    });
  });

  describe("COMPILER_SMOKE_POLL_MS", () => {
    it("defaults to 10000ms", () => {
      expect(loadConfig(baseEnv()).pollMs).toBe(10_000);
    });

    it("accepts an explicit override", () => {
      expect(loadConfig(baseEnv({ COMPILER_SMOKE_POLL_MS: "5000" })).pollMs).toBe(5_000);
    });
  });

  describe("COMPILER_SMOKE_STALE_REF_HOURS bounds", () => {
    it("defaults to 24", () => {
      expect(loadConfig(baseEnv()).staleRefHours).toBe(24);
    });

    it("accepts the minimum (6)", () => {
      expect(loadConfig(baseEnv({ COMPILER_SMOKE_STALE_REF_HOURS: "6" })).staleRefHours).toBe(6);
    });

    it("rejects below the minimum (5)", () => {
      expect(() => loadConfig(baseEnv({ COMPILER_SMOKE_STALE_REF_HOURS: "5" }))).toThrow(/range/);
    });
  });

  it("trims surrounding whitespace from string env vars", () => {
    const config = loadConfig(baseEnv({ SYSTEM_TEAMPROJECT: "  AgentPlayground  " }));
    expect(config.project).toBe("AgentPlayground");
  });

  it("treats an empty string the same as unset", () => {
    expect(() => loadConfig(baseEnv({ SYSTEM_TEAMPROJECT: "" }))).toThrow();
  });
});

describe("candidateRef", () => {
  it("builds the deterministic per-run candidate ref", () => {
    expect(candidateRef(42)).toBe("refs/heads/ado-aw-smoke-candidate/42");
  });

  it("never collides with a plausible base ref name", () => {
    expect(candidateRef(1)).not.toBe("refs/heads/main");
  });
});
