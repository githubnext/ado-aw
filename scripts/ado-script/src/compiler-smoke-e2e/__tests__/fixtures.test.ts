import { describe, expect, it } from "vitest";

import { ALL_FIXTURES, allowedChangedPaths, fixturePaths, FIXTURE_DIR } from "../fixtures.js";

describe("fixturePaths", () => {
  it("builds repo-relative md/lock paths under tests/safe-outputs", () => {
    expect(fixturePaths("canary")).toEqual({
      name: "canary",
      relMd: "tests/safe-outputs/canary.md",
      relLock: "tests/safe-outputs/canary.lock.yml",
    });
  });
});

describe("ALL_FIXTURES", () => {
  it("has exactly the four fixtures in the required stable order", () => {
    expect(ALL_FIXTURES.map((f) => f.name)).toEqual([
      "canary",
      "azure-cli",
      "noop-target",
      "smoke-failure-reporter",
    ]);
  });

  it("every fixture path lives under the shared FIXTURE_DIR", () => {
    for (const f of ALL_FIXTURES) {
      expect(f.relMd.startsWith(`${FIXTURE_DIR}/`)).toBe(true);
      expect(f.relLock.startsWith(`${FIXTURE_DIR}/`)).toBe(true);
    }
  });
});

describe("allowedChangedPaths", () => {
  it("contains exactly the four md files, four lock files, and .gitattributes", () => {
    const allowed = allowedChangedPaths();
    expect(allowed.size).toBe(9);
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
