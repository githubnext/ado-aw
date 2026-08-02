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
    SMOKE_ADO_AW_BIN: "C:\\bin\\ado-aw.exe",
    SMOKE_ARTIFACT_NAME: "ado-aw-candidate",
    SMOKE_MIRROR_REPO: "ado-aw-mirror",
    SMOKE_COMPILER_SOURCE: "candidate",
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
    expect(config.compilerSource).toBe("candidate");
    expect(config.concurrency).toBe(5);
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
    "SMOKE_ADO_AW_BIN",
    "SMOKE_ARTIFACT_NAME",
    "SMOKE_MIRROR_REPO",
    "SMOKE_COMPILER_SOURCE",
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
    expect(() => loadConfig(baseEnv({ SMOKE_COMPILER_SOURCE: undefined }))).toThrow(/not set/);
  });

  it("rejects an unknown compiler source", () => {
    expect(() => loadConfig(baseEnv({ SMOKE_COMPILER_SOURCE: "nightly" }))).toThrow(
      /SMOKE_COMPILER_SOURCE must be one of candidate, released/,
    );
  });

  it("accepts the released compiler source", () => {
    expect(loadConfig(baseEnv({ SMOKE_COMPILER_SOURCE: "released" })).compilerSource).toBe(
      "released",
    );
  });

  describe("SMOKE_CONCURRENCY bounds", () => {
    it("defaults to 5 when unset", () => {
      expect(loadConfig(baseEnv()).concurrency).toBe(5);
    });

    it("accepts the lower bound (1)", () => {
      expect(loadConfig(baseEnv({ SMOKE_CONCURRENCY: "1" })).concurrency).toBe(1);
    });

    it("accepts the upper bound (5)", () => {
      expect(loadConfig(baseEnv({ SMOKE_CONCURRENCY: "5" })).concurrency).toBe(5);
    });

    it("rejects 0", () => {
      expect(() => loadConfig(baseEnv({ SMOKE_CONCURRENCY: "0" }))).toThrow(/range/);
    });

    it("rejects 11", () => {
      expect(() => loadConfig(baseEnv({ SMOKE_CONCURRENCY: "11" }))).toThrow(/range/);
    });

    it("rejects a non-integer value", () => {
      expect(() => loadConfig(baseEnv({ SMOKE_CONCURRENCY: "2.5" }))).toThrow(/integer/);
    });
  });

  describe("SMOKE_CHILD_TIMEOUT_MS", () => {
    it("defaults to 7200000ms", () => {
      expect(loadConfig(baseEnv()).childTimeoutMs).toBe(7_200_000);
    });

    it("accepts an explicit override", () => {
      expect(loadConfig(baseEnv({ SMOKE_CHILD_TIMEOUT_MS: "60000" })).childTimeoutMs).toBe(60_000);
    });
  });

  describe("SMOKE_POLL_MS", () => {
    it("defaults to 10000ms", () => {
      expect(loadConfig(baseEnv()).pollMs).toBe(10_000);
    });

    it("accepts an explicit override", () => {
      expect(loadConfig(baseEnv({ SMOKE_POLL_MS: "5000" })).pollMs).toBe(5_000);
    });
  });

  describe("SMOKE_STALE_REF_HOURS bounds", () => {
    it("defaults to 24", () => {
      expect(loadConfig(baseEnv()).staleRefHours).toBe(24);
    });

    it("accepts the minimum (6)", () => {
      expect(loadConfig(baseEnv({ SMOKE_STALE_REF_HOURS: "6" })).staleRefHours).toBe(6);
    });

    it("rejects below the minimum (5)", () => {
      expect(() => loadConfig(baseEnv({ SMOKE_STALE_REF_HOURS: "5" }))).toThrow(/range/);
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
  it("builds the deterministic per-run, per-case candidate ref", () => {
    expect(candidateRef(42, "canary")).toBe("refs/heads/ado-aw-smoke-candidate/42/canary");
  });

  it("never collides with a plausible base ref name", () => {
    expect(candidateRef(1, "canary")).not.toBe("refs/heads/main");
  });
});
