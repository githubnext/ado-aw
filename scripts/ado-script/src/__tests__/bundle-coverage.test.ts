import { readdirSync, readFileSync, statSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

/**
 * Guard test: every bundle directory under `src/` (each `src/<name>/` that is
 * an ncc entry point, i.e. all of them except `shared`) MUST be wired into the
 * `npm run build` chain via a `build:<name>` script.
 *
 * This is the safety net for the auto-bundling release flow: the release
 * workflow globs `ado-script/*.js` to package every built bundle, so a bundle
 * only ships if `npm run build` actually produces it. Without this test a new
 * `src/<name>/` directory could be added (with its own bundle) but silently
 * omitted from the build chain — exactly how `conclusion.js` was once missing
 * from the release zip.
 */

const here = dirname(fileURLToPath(import.meta.url));
const srcDir = join(here, "..");
const packageJsonPath = join(here, "..", "..", "package.json");

/**
 * Directories under src/ that are shared modules or test-only harnesses, not
 * ncc bundle entry points that ship in `ado-script.zip`.
 *
 * `executor-e2e` is the deterministic Stage 3 executor E2E harness. It has its
 * own `build:executor-e2e` script that emits to the non-root `test-bin/` dir,
 * so the release glob (`ado-script/*.js`) never packages it — it is a test
 * harness run only by the executor-e2e pipeline, never downloaded by compiled
 * agentic pipelines at runtime.
 *
 * `trigger-e2e` is the analogous deterministic trigger-condition (gate /
 * synth-PR) E2E harness. Same treatment: its own `build:trigger-e2e` emits to
 * `test-bin/`, so it is never packaged in `ado-script.zip`.
 */
const NON_BUNDLE_DIRS = new Set(["shared", "__tests__", "executor-e2e", "trigger-e2e"]);

function listBundleDirs(): string[] {
  return readdirSync(srcDir)
    .filter((name) => {
      if (NON_BUNDLE_DIRS.has(name)) return false;
      return statSync(join(srcDir, name)).isDirectory();
    })
    .sort();
}

interface PackageJson {
  scripts: Record<string, string>;
}

function readPackageJson(): PackageJson {
  return JSON.parse(readFileSync(packageJsonPath, "utf8")) as PackageJson;
}

describe("ado-script bundle coverage", () => {
  it("has at least the known bundles", () => {
    // Sanity floor so the test itself can't silently pass on an empty dir.
    const dirs = listBundleDirs();
    expect(dirs.length).toBeGreaterThanOrEqual(11);
    expect(dirs).toContain("gate");
    expect(dirs).toContain("import");
    expect(dirs).toContain("conclusion");
  });

  it("every bundle dir has a build:<name> script", () => {
    const { scripts } = readPackageJson();
    const missing = listBundleDirs().filter(
      (name) => typeof scripts[`build:${name}`] !== "string",
    );
    expect(
      missing,
      `src/ bundle dirs missing a build:<name> script in package.json: ${missing.join(", ")}`,
    ).toEqual([]);
  });

  it("every bundle dir is referenced in the main build chain", () => {
    const { scripts } = readPackageJson();
    const buildChain = scripts.build ?? "";
    const missing = listBundleDirs().filter(
      (name) => !buildChain.includes(`build:${name}`),
    );
    expect(
      missing,
      `src/ bundle dirs not referenced in the 'build' chain: ${missing.join(", ")}`,
    ).toEqual([]);
  });

  it("every bundle dir has an index.ts entry point", () => {
    const missing = listBundleDirs().filter((name) => {
      try {
        return !statSync(join(srcDir, name, "index.ts")).isFile();
      } catch {
        return true;
      }
    });
    expect(
      missing,
      `src/ bundle dirs missing an index.ts entry point: ${missing.join(", ")}`,
    ).toEqual([]);
  });
});
