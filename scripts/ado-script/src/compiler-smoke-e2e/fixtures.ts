/**
 * Manifest of the six fixed compiler-smoke fixtures.
 *
 * Five reuse the release-backed sources under `tests/safe-outputs/`; the sixth
 * is candidate-only and lives beside this harness. Every source is read from
 * the detached candidate worktree (an exact checkout of
 * `BUILD_SOURCEVERSION`, never the possibly-divergent
 * `BUILD_SOURCESDIRECTORY`), transformed, compiled, and queued through its
 * fixed definition tracked in `tests/compiler-smoke-e2e/REGISTERED.md`.
 *
 * Test-harness module; not shipped in `ado-script.zip`.
 */
import type { FixtureName } from "./config.js";
import { FIXTURE_NAMES } from "./config.js";

/** Repo-relative directory containing the five release-backed fixtures. */
export const RELEASE_FIXTURE_DIR = "tests/safe-outputs";
/** Repo-relative directory containing the candidate-only custom fixture. */
export const CANDIDATE_FIXTURE_DIR = "tests/compiler-smoke-e2e";
export const CUSTOM_COMPONENT_CACHE_PATH =
  ".ado-aw/imports/AgentPlayground/ado-aw-e2e-fixture/aa711dd17c4dfcde492b2bfad62e5fb1baad71f6/components/custom-build-tags/component.md";
export const CUSTOM_COMPONENT_DIGEST_PATH = `${CUSTOM_COMPONENT_CACHE_PATH}.sha256`;
export const IMPORT_CACHE_ATTRIBUTES_PATH = ".ado-aw/imports/.gitattributes";

export interface FixturePaths {
  readonly name: FixtureName;
  /** Repo-relative path to the fixture markdown source, e.g. tests/safe-outputs/canary.md. */
  readonly relMd: string;
  /** Repo-relative path to the compiled lock file, e.g. tests/safe-outputs/canary.lock.yml. */
  readonly relLock: string;
  /** Observable ADO build tags that must exist after this child succeeds. */
  readonly requiredBuildTags?: (buildId: number) => readonly string[];
}

/** Repo-relative paths and signal contract for one fixture. */
export function fixturePaths(name: FixtureName): FixturePaths {
  const directory =
    name === "custom-safe-output" ? CANDIDATE_FIXTURE_DIR : RELEASE_FIXTURE_DIR;
  const requiredBuildTags =
    name === "custom-safe-output"
      ? (buildId: number): readonly string[] => [
          `ado-aw-custom-script-${buildId}`,
          `ado-aw-custom-job-${buildId}`,
        ]
      : undefined;
  return {
    name,
    relMd: `${directory}/${name}.md`,
    relLock: `${directory}/${name}.lock.yml`,
    requiredBuildTags,
  };
}

/** All six fixtures in the stable declaration order used throughout the harness. */
export const ALL_FIXTURES: readonly FixturePaths[] = FIXTURE_NAMES.map(fixturePaths);

export function fixtureByName(name: FixtureName): FixturePaths {
  const fixture = ALL_FIXTURES.find((candidate) => candidate.name === name);
  if (!fixture) {
    throw new Error(`unknown compiler-smoke fixture '${name}'`);
  }
  return fixture;
}

/**
 * The exact set of repo-relative paths the candidate-staging commit may touch:
 * six markdown sources, six compiled locks, and the compiler-managed
 * `.gitattributes` block. Any other changed path fails before push.
 */
export function allowedChangedPaths(): Set<string> {
  const paths = new Set<string>([
    ".gitattributes",
    IMPORT_CACHE_ATTRIBUTES_PATH,
    CUSTOM_COMPONENT_CACHE_PATH,
    CUSTOM_COMPONENT_DIGEST_PATH,
  ]);
  for (const fixture of ALL_FIXTURES) {
    paths.add(fixture.relMd);
    paths.add(fixture.relLock);
  }
  return paths;
}
