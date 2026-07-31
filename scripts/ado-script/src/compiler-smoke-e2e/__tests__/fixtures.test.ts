import { describe, expect, it } from "vitest";

import {
  ALL_FIXTURES,
  CANDIDATE_FIXTURE_DIR,
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
    });
  });

  it("uses the candidate-only directory for candidate-only fixtures", () => {
    const fixture = fixturePaths("custom-safe-output");
    expect(fixture.relMd).toBe(
      "tests/compiler-smoke-e2e/custom-safe-output.md",
    );
    expect(fixture.relLock).toBe(
      "tests/compiler-smoke-e2e/custom-safe-output.lock.yml",
    );
    expect(fixture.requiredBuildTags?.(42)).toEqual(["ado-aw-custom-job-42"]);

    const multiRepo = fixturePaths("multi-repo");
    expect(multiRepo.relMd).toBe("tests/compiler-smoke-e2e/multi-repo.md");
    expect(multiRepo.relLock).toBe(
      "tests/compiler-smoke-e2e/multi-repo.lock.yml",
    );
    // Its assertions run inside the pipeline, so it publishes no build tag.
    expect(multiRepo.requiredBuildTags).toBeUndefined();
  });
});

describe("ALL_FIXTURES", () => {
  it("has exactly the candidate fixtures in the required stable order", () => {
    expect(ALL_FIXTURES.map((f) => f.name)).toEqual([
      "canary",
      "azure-cli",
      "noop-target",
      "smoke-failure-reporter",
      "custom-safe-output",
      "multi-repo",
    ]);
    expect(ALL_FIXTURES.map((f) => f.name)).not.toContain("janitor");
  });

  it("keeps release and candidate-only fixture paths separate", () => {
    for (const fixture of ALL_FIXTURES) {
      const directory =
        fixture.name === "custom-safe-output" || fixture.name === "multi-repo"
          ? CANDIDATE_FIXTURE_DIR
          : RELEASE_FIXTURE_DIR;
      expect(fixture.relMd.startsWith(`${directory}/`)).toBe(true);
      expect(fixture.relLock.startsWith(`${directory}/`)).toBe(true);
    }
  });
});

describe("allowedChangedPaths", () => {
  it("contains every source/lock pair and root compiler-managed attributes", () => {
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
