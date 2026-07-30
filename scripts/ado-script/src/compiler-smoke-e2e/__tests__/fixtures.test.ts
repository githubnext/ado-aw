import { describe, expect, it } from "vitest";

import {
  ALL_FIXTURES,
  allowedChangedPaths,
  CANDIDATE_FIXTURE_DIR,
  fixturePaths,
  FIXTURE_DIR,
} from "../fixtures.js";

describe("fixturePaths", () => {
  it("builds repo-relative md/lock paths under tests/safe-outputs", () => {
    expect(fixturePaths("canary")).toEqual({
      name: "canary",
      relMd: "tests/safe-outputs/canary.md",
      relLock: "tests/safe-outputs/canary.lock.yml",
    });
  });

  it("keeps candidate-only fixtures out of the release-owned directory", () => {
    expect(fixturePaths("multi-repo")).toEqual({
      name: "multi-repo",
      relMd: "tests/compiler-smoke-e2e/fixtures/multi-repo.md",
      relLock: "tests/compiler-smoke-e2e/fixtures/multi-repo.lock.yml",
    });
  });
});

describe("ALL_FIXTURES", () => {
  it("has exactly the five fixtures in the required stable order", () => {
    expect(ALL_FIXTURES.map((f) => f.name)).toEqual([
      "canary",
      "azure-cli",
      "noop-target",
      "smoke-failure-reporter",
      "multi-repo",
    ]);
  });

  it("every fixture path lives under a known fixture directory", () => {
    for (const f of ALL_FIXTURES) {
      const dir = f.name === "multi-repo" ? CANDIDATE_FIXTURE_DIR : FIXTURE_DIR;
      expect(f.relMd.startsWith(`${dir}/`)).toBe(true);
      expect(f.relLock.startsWith(`${dir}/`)).toBe(true);
    }
  });
});

describe("allowedChangedPaths", () => {
  it("contains exactly each md file, each lock file, and .gitattributes", () => {
    const allowed = allowedChangedPaths();
    expect(allowed.size).toBe(ALL_FIXTURES.length * 2 + 1);
    expect(allowed.has(".gitattributes")).toBe(true);
    for (const f of ALL_FIXTURES) {
      expect(allowed.has(f.relMd)).toBe(true);
      expect(allowed.has(f.relLock)).toBe(true);
    }
  });

  it("does not allow an arbitrary unrelated path", () => {
    const allowed = allowedChangedPaths();
    expect(allowed.has("src/main.rs")).toBe(false);
    expect(allowed.has("tests/safe-outputs/README.md")).toBe(false);
  });
});
