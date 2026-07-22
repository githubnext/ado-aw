/**
 * Manifest of the five fixed compiler-smoke fixtures.
 *
 * These are the same five agentic-pipeline sources documented in
 * `tests/safe-outputs/README.md` (canary, azure-cli, noop-target, janitor,
 * smoke-failure-reporter) — this harness does not invent its own fixture
 * content. It reads the exact files from the detached candidate worktree
 * (an exact checkout of `BUILD_SOURCEVERSION`, at
 * `<worktree>/tests/safe-outputs/<name>.md` — never from
 * `BUILD_SOURCESDIRECTORY`, which may sit at a different commit), stages a
 * pinned `supply-chain.pipeline-artifact` transform of each onto the mirror
 * repo, recompiles, and queues the five FIXED "candidate lane" pipeline
 * definitions tracked in `tests/compiler-smoke-e2e/REGISTERED.md` (distinct
 * from the release-backed definitions those same sources also feed).
 *
 * Test-harness module; not shipped in `ado-script.zip`.
 */
import type { FixtureName } from "./config.js";
import { FIXTURE_NAMES } from "./config.js";

/** Repo-relative directory containing every fixture source + compiled lock. */
export const FIXTURE_DIR = "tests/safe-outputs";

export interface FixturePaths {
  readonly name: FixtureName;
  /** Repo-relative path to the fixture markdown source, e.g. tests/safe-outputs/canary.md. */
  readonly relMd: string;
  /** Repo-relative path to the compiled lock file, e.g. tests/safe-outputs/canary.lock.yml. */
  readonly relLock: string;
}

/** Repo-relative path for a fixture's markdown source. Always POSIX-separated (a git path, not a filesystem path). */
export function fixturePaths(name: FixtureName): FixturePaths {
  return {
    name,
    relMd: `${FIXTURE_DIR}/${name}.md`,
    relLock: `${FIXTURE_DIR}/${name}.lock.yml`,
  };
}

/** All five fixtures in the stable declaration order used throughout the harness. */
export const ALL_FIXTURES: readonly FixturePaths[] = FIXTURE_NAMES.map(fixturePaths);

/**
 * The exact set of repo-relative paths the candidate-staging commit is
 * allowed to touch: the five markdown sources, their five compiled locks,
 * and the compiler-managed `.gitattributes` block. Any other changed path
 * fails the run before it pushes anything.
 */
export function allowedChangedPaths(): Set<string> {
  const paths = new Set<string>([".gitattributes"]);
  for (const fixture of ALL_FIXTURES) {
    paths.add(fixture.relMd);
    paths.add(fixture.relLock);
  }
  return paths;
}
