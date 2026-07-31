/**
 * Manifest of the fixed compiler-smoke fixtures.
 *
 * Four reuse release-backed sources under `tests/safe-outputs/`; the rest are
 * candidate-only and live beside this harness. The weekly janitor is
 * deliberately excluded from candidate checks. The harness reads every source
 * from the detached candidate worktree (an exact checkout of
 * `BUILD_SOURCEVERSION`, never `BUILD_SOURCESDIRECTORY`, which may sit at a
 * different commit), stages a pinned `supply-chain.pipeline-artifact`
 * transform, recompiles, and queues the fixed candidate-lane definitions
 * tracked in `tests/compiler-smoke-e2e/REGISTERED.md`.
 *
 * Test-harness module; not shipped in `ado-script.zip`.
 */
import type { FixtureName } from "./config.js";
import { CANDIDATE_FIXTURE_NAMES } from "./config.js";

/** Repo-relative directory containing release-backed fixture sources. */
export const RELEASE_FIXTURE_DIR = "tests/safe-outputs";
/** Repo-relative directory containing the candidate-only custom fixture. */
export const CANDIDATE_FIXTURE_DIR = "tests/compiler-smoke-e2e";

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
  const candidateOnly = name === "custom-safe-output" || name === "multi-repo";
  const directory = candidateOnly ? CANDIDATE_FIXTURE_DIR : RELEASE_FIXTURE_DIR;
  const requiredBuildTags =
    name === "custom-safe-output"
      ? (buildId: number): readonly string[] => [`ado-aw-custom-job-${buildId}`]
      : undefined;
  return {
    name,
    relMd: `${directory}/${name}.md`,
    relLock: `${directory}/${name}.lock.yml`,
    requiredBuildTags,
  };
}

/** Every candidate fixture in the stable order used throughout the harness. */
export const ALL_FIXTURES: readonly FixturePaths[] =
  CANDIDATE_FIXTURE_NAMES.map(fixturePaths);

export function fixtureByName(name: FixtureName): FixturePaths {
  const fixture = ALL_FIXTURES.find((candidate) => candidate.name === name);
  if (!fixture) {
    throw new Error(`unknown compiler-smoke fixture '${name}'`);
  }
  return fixture;
}

/**
 * The exact set of repo-relative paths the candidate-staging commit may touch:
 * every markdown source, its compiled lock, and the compiler-managed root
 * `.gitattributes` block. Any other changed path fails before push.
 */
export function allowedChangedPaths(): Set<string> {
  const paths = new Set<string>([".gitattributes"]);
  for (const fixture of ALL_FIXTURES) {
    paths.add(fixture.relMd);
    paths.add(fixture.relLock);
  }
  return paths;
}
