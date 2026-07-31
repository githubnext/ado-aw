/**
 * Environment configuration for the deterministic compiler-smoke E2E harness.
 *
 * This harness stages the compiler candidate produced by the current build as
 * a pinned `supply-chain.pipeline-artifact` source across five registered ADO
 * pipeline fixtures, pushes the staged candidate to a per-run branch on a
 * mirror repo, queues all five, and asserts they all go green.
 *
 * Strict, fail-closed parsing lives here so every other module can trust a
 * fully validated {@link CompilerSmokeConfig} rather than re-checking env
 * vars ad hoc.
 *
 * Test-harness module; not shipped in `ado-script.zip`.
 */

/** Per-run candidate branch prefix (never the base ref). */
export const CANDIDATE_BRANCH_PREFIX = "ado-aw-smoke-candidate";

export const DEFAULT_CONCURRENCY = 5;
export const MIN_CONCURRENCY = 1;
export const MAX_CONCURRENCY = 5;

export const DEFAULT_CHILD_TIMEOUT_MS = 7_200_000;
export const DEFAULT_POLL_MS = 10_000;

export const DEFAULT_STALE_REF_HOURS = 24;
export const MIN_STALE_REF_HOURS = 6;

/** Stable declaration order for the five workflows in the live candidate lane. */
export const CANDIDATE_FIXTURE_NAMES = [
  "canary",
  "azure-cli",
  "noop-target",
  "smoke-failure-reporter",
  "custom-safe-output",
] as const;

export type FixtureName = (typeof CANDIDATE_FIXTURE_NAMES)[number];

export interface CompilerSmokeConfig {
  /** ADO collection URI, e.g. https://dev.azure.com/org/. */
  readonly orgUrl: string;
  /** ADO project name (also the pinned pipeline-artifact project). */
  readonly project: string;
  /** Write-capable ADO token (System.AccessToken). */
  readonly token: string;
  /** Current orchestrator build id (also the pinned pipeline-artifact run-id). */
  readonly buildId: number;
  /** Full ref of the checked-out base branch, e.g. refs/heads/main. Never used as the candidate ref. */
  readonly sourceBranch: string;
  /** Commit SHA of the checked-out base branch — the candidate commit's parent context. */
  readonly sourceVersion: string;
  /** Local checkout root (self repo), used as the base for the detached worktree. */
  readonly sourcesDirectory: string;
  /** This orchestrator pipeline's own definition id (used to age-check stale candidate refs). */
  readonly definitionId: number;
  /** Path to the candidate ado-aw binary under test. */
  readonly adoAwBin: string;
  /** Pipeline artifact name pinned into each fixture's supply-chain config. */
  readonly artifactName: string;
  /** ADO Git repo hosting the five registered candidate definitions. */
  readonly mirrorRepo: string;
  /** Registered ADO pipeline definition id, keyed by fixture name. */
  readonly definitionIds: Readonly<Record<FixtureName, number>>;
  /** Bounded fixture polling concurrency (1..5, default 5). */
  readonly concurrency: number;
  /** Bounded per-fixture build wait, in ms (default 2h). */
  readonly childTimeoutMs: number;
  /** Build poll interval, in ms (default 10s). */
  readonly pollMs: number;
  /** Minimum age (hours) before a leftover candidate ref is eligible for cleanup (default 24, min 6). */
  readonly staleRefHours: number;
}

const REQUIRED_STRING_VARS = [
  "SYSTEM_COLLECTIONURI",
  "SYSTEM_TEAMPROJECT",
  "SYSTEM_ACCESSTOKEN",
  "BUILD_SOURCEBRANCH",
  "BUILD_SOURCEVERSION",
  "BUILD_SOURCESDIRECTORY",
  "COMPILER_SMOKE_ADO_AW_BIN",
  "COMPILER_SMOKE_ARTIFACT_NAME",
  "COMPILER_SMOKE_MIRROR_REPO",
] as const;

const DEFINITION_ID_ENV_BY_FIXTURE: Readonly<Record<FixtureName, string>> = {
  canary: "COMPILER_SMOKE_CANARY_DEFINITION_ID",
  "azure-cli": "COMPILER_SMOKE_AZURE_CLI_DEFINITION_ID",
  "noop-target": "COMPILER_SMOKE_NOOP_TARGET_DEFINITION_ID",
  "smoke-failure-reporter": "COMPILER_SMOKE_REPORTER_DEFINITION_ID",
  "custom-safe-output": "COMPILER_SMOKE_CUSTOM_SAFE_OUTPUT_DEFINITION_ID",
};

/** ADO macros that failed to expand look like `$(NAME)`; treat them as unset. */
const UNEXPANDED_MACRO_RE = /^\$\([^)]*\)$/;

function cleanVar(raw: string | undefined): string | undefined {
  const value = raw?.trim();
  if (!value) return undefined;
  if (UNEXPANDED_MACRO_RE.test(value)) return undefined;
  return value;
}

function requireString(env: NodeJS.ProcessEnv, name: string): string {
  const value = cleanVar(env[name]);
  if (value === undefined) {
    throw new Error(
      `required env var ${name} is not set (or contains an unexpanded ADO macro)`,
    );
  }
  return value;
}

/** Parse a required positive integer env var, rejecting malformed/zero/negative values. */
function requirePositiveInt(env: NodeJS.ProcessEnv, name: string): number {
  const raw = requireString(env, name);
  const parsed = Number(raw);
  if (!Number.isInteger(parsed) || parsed <= 0) {
    throw new Error(`${name} must be a positive integer (got '${raw}')`);
  }
  return parsed;
}

/** Parse an optional bounded integer env var: default when unset, reject when malformed/out of range. */
function optionalBoundedInt(
  env: NodeJS.ProcessEnv,
  name: string,
  opts: { default: number; min: number; max?: number },
): number {
  const raw = cleanVar(env[name]);
  if (raw === undefined) return opts.default;
  const parsed = Number(raw);
  if (!Number.isInteger(parsed)) {
    throw new Error(`${name} must be an integer (got '${raw}')`);
  }
  if (parsed < opts.min || (opts.max !== undefined && parsed > opts.max)) {
    const range = opts.max !== undefined ? `${opts.min}..${opts.max}` : `>= ${opts.min}`;
    throw new Error(`${name} must be in range ${range} (got '${raw}')`);
  }
  return parsed;
}

/** Load and strictly validate the harness configuration. Throws on any invalid input. */
export function loadConfig(env: NodeJS.ProcessEnv = process.env): CompilerSmokeConfig {
  for (const name of REQUIRED_STRING_VARS) {
    requireString(env, name);
  }

  const orgUrl = requireString(env, "SYSTEM_COLLECTIONURI");
  const project = requireString(env, "SYSTEM_TEAMPROJECT");
  const token = requireString(env, "SYSTEM_ACCESSTOKEN");
  const sourceBranch = requireString(env, "BUILD_SOURCEBRANCH");
  const sourceVersion = requireString(env, "BUILD_SOURCEVERSION");
  const sourcesDirectory = requireString(env, "BUILD_SOURCESDIRECTORY");
  const adoAwBin = requireString(env, "COMPILER_SMOKE_ADO_AW_BIN");
  const artifactName = requireString(env, "COMPILER_SMOKE_ARTIFACT_NAME");
  const mirrorRepo = requireString(env, "COMPILER_SMOKE_MIRROR_REPO");

  const buildId = requirePositiveInt(env, "BUILD_BUILDID");
  const definitionId = requirePositiveInt(env, "SYSTEM_DEFINITIONID");

  const definitionIds = {} as Record<FixtureName, number>;
  for (const fixture of CANDIDATE_FIXTURE_NAMES) {
    definitionIds[fixture] = requirePositiveInt(env, DEFINITION_ID_ENV_BY_FIXTURE[fixture]);
  }

  const seen = new Map<number, FixtureName[]>();
  for (const fixture of CANDIDATE_FIXTURE_NAMES) {
    const id = definitionIds[fixture];
    const existing = seen.get(id);
    if (existing) {
      existing.push(fixture);
    } else {
      seen.set(id, [fixture]);
    }
  }
  const duplicates = [...seen.entries()].filter(([, fixtures]) => fixtures.length > 1);
  if (duplicates.length > 0) {
    const detail = duplicates
      .map(([id, fixtures]) => `${id} used by [${fixtures.join(", ")}]`)
      .join("; ");
    throw new Error(`fixture definition ids must be distinct; duplicates found: ${detail}`);
  }

  const concurrency = optionalBoundedInt(env, "COMPILER_SMOKE_CONCURRENCY", {
    default: DEFAULT_CONCURRENCY,
    min: MIN_CONCURRENCY,
    max: MAX_CONCURRENCY,
  });
  const childTimeoutMs = optionalBoundedInt(env, "COMPILER_SMOKE_CHILD_TIMEOUT_MS", {
    default: DEFAULT_CHILD_TIMEOUT_MS,
    min: 1,
  });
  const pollMs = optionalBoundedInt(env, "COMPILER_SMOKE_POLL_MS", {
    default: DEFAULT_POLL_MS,
    min: 1,
  });
  const staleRefHours = optionalBoundedInt(env, "COMPILER_SMOKE_STALE_REF_HOURS", {
    default: DEFAULT_STALE_REF_HOURS,
    min: MIN_STALE_REF_HOURS,
  });

  return {
    orgUrl,
    project,
    token,
    buildId,
    sourceBranch,
    sourceVersion,
    sourcesDirectory,
    definitionId,
    adoAwBin,
    artifactName,
    mirrorRepo,
    definitionIds,
    concurrency,
    childTimeoutMs,
    pollMs,
    staleRefHours,
  };
}

/** Deterministic per-run candidate ref, e.g. refs/heads/ado-aw-smoke-candidate/12345. Never the base ref. */
export function candidateRef(buildId: number): string {
  return `refs/heads/${CANDIDATE_BRANCH_PREFIX}/${buildId}`;
}
