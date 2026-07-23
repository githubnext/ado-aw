import { describe, expect, it } from "vitest";

import {
  ALL_FIXTURES,
  CANDIDATE_FIXTURE_DIR,
  CUSTOM_COMPONENT_CACHE_PATH,
  CUSTOM_COMPONENT_DIGEST_PATH,
  IMPORT_CACHE_ATTRIBUTES_PATH,
  RELEASE_FIXTURE_DIR,
  allowedChangedPaths,
  fixturePaths,
} from "../fixtures.js";

describe("fixturePaths", () => {
  it("builds repo-relative md/lock paths under tests/safe-outputs", () => {
    expect(fixturePaths("canary")).toEqual({
      name: "canary",
      relMd: "tests/safe-outputs/canary.md",
      relLock: "tests/safe-outputs/canary.lock.yml",
      requiresAgentReadToken: true,
    });
  });

  it("uses the candidate-only directory for the custom fixture", () => {
    const fixture = fixturePaths("custom-safe-output");
    expect(fixture.relMd).toBe(
      "tests/compiler-smoke-e2e/custom-safe-output.md",
    );
    expect(fixture.relLock).toBe(
      "tests/compiler-smoke-e2e/custom-safe-output.lock.yml",
    );
    expect(fixture.requiresAgentReadToken).toBe(false);
    expect(fixture.requiredBuildTags?.(42)).toEqual([
      "ado-aw-custom-script-42",
      "ado-aw-custom-job-42",
    ]);
  });
});

describe("ALL_FIXTURES", () => {
  it("has exactly the six fixtures in the required stable order", () => {
    expect(ALL_FIXTURES.map((f) => f.name)).toEqual([
      "canary",
      "azure-cli",
      "noop-target",
      "janitor",
      "smoke-failure-reporter",
      "custom-safe-output",
    ]);
  });

  it("keeps release and candidate-only fixture paths separate", () => {
    for (const fixture of ALL_FIXTURES) {
      const directory =
        fixture.name === "custom-safe-output"
          ? CANDIDATE_FIXTURE_DIR
          : RELEASE_FIXTURE_DIR;
      expect(fixture.relMd.startsWith(`${directory}/`)).toBe(true);
      expect(fixture.relLock.startsWith(`${directory}/`)).toBe(true);
    }
  });
});

describe("allowedChangedPaths", () => {
  it("contains the six source/lock pairs and exact compiler-managed attribute/cache paths", () => {
    const allowed = allowedChangedPaths();
    expect(allowed.size).toBe(16);
    expect(allowed.has(".gitattributes")).toBe(true);
    expect(allowed.has(IMPORT_CACHE_ATTRIBUTES_PATH)).toBe(true);
    expect(allowed.has(CUSTOM_COMPONENT_CACHE_PATH)).toBe(true);
    expect(allowed.has(CUSTOM_COMPONENT_DIGEST_PATH)).toBe(true);
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
