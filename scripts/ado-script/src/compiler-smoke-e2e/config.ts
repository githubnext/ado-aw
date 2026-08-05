/**
 * Environment configuration for the deterministic smoke E2E harness.
 *
 * This harness stages one pipeline per smoke *case* onto a per-case branch of
 * the mirror repo, then queues each case against its credential *lane*
 * definition and asserts they all go green. Which cases run, and which lane
 * each belongs to, is declared in `tests/smoke/cases.json` and loaded by
 * `cases.ts` — this module only handles the environment.
 *
 * Strict, fail-closed parsing lives here so every other module can trust a
 * fully validated {@link SmokeConfig} rather than re-checking env vars ad hoc.
 *
 * Test-harness module; not shipped in `ado-script.zip`.
 */
import { COMPILER_SOURCES, type CompilerSource } from "./cases.js";

/** Per-run candidate branch prefix (never the base ref). */
export const CANDIDATE_BRANCH_PREFIX = "ado-aw-smoke-candidate";

export const DEFAULT_CONCURRENCY = 5;
export const MIN_CONCURRENCY = 1;
export const MAX_CONCURRENCY = 10;

export const DEFAULT_CHILD_TIMEOUT_MS = 7_200_000;
export const DEFAULT_POLL_MS = 10_000;

export const DEFAULT_STALE_REF_HOURS = 24;
export const MIN_STALE_REF_HOURS = 6;

export interface SmokeConfig {
  /** ADO collection URI, e.g. https://dev.azure.com/org/. */
  readonly orgUrl: string;
  /** ADO project name (also the pinned pipeline-artifact project). */
  readonly project: string;
  /** Write-capable ADO token (System.AccessToken). */
  readonly token: string;
  /** Current orchestrator build id (also the pinned pipeline-artifact run-id). */
  readonly buildId: number;
  /** Full ref of the checked-out base branch, e.g. refs/heads/main. Never used as a candidate ref. */
  readonly sourceBranch: string;
  /** Commit SHA of the checked-out base branch — every candidate commit's parent. */
  readonly sourceVersion: string;
  /** Local checkout root (self repo), used as the base for the detached worktree. */
  readonly sourcesDirectory: string;
  /** This orchestrator pipeline's own definition id (used to age-check stale candidate refs). */
  readonly definitionId: number;
  /** Path to the `ado-aw` binary under test (candidate build, or downloaded release). */
  readonly adoAwBin: string;
  /**
   * Which compiler the staged pipelines are built with.
   *
   * `candidate` pins every case to this run's own pipeline artifact;
   * `released` leaves the compiled output pointing at public release assets so
   * release packaging is exercised too.
   */
  readonly compilerSource: CompilerSource;
  /** Pipeline artifact name pinned into each case (candidate mode only). */
  readonly artifactName: string;
  /** ADO Git repo hosting the registered lane definitions. */
  readonly mirrorRepo: string;
  /** Bounded case polling concurrency (1..10, default 5). */
  readonly concurrency: number;
  /** Bounded per-case build wait, in ms (default 2h). */
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
  "SMOKE_ADO_AW_BIN",
  "SMOKE_ARTIFACT_NAME",
  "SMOKE_MIRROR_REPO",
] as const;

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

/**
 * Parse the compiler-source mode.
 *
 * Deliberately required rather than defaulted: the two modes assert opposite
 * things about release URLs, so silently guessing would turn a misconfigured
 * pipeline into a green run that checked nothing.
 */
function requireCompilerSource(env: NodeJS.ProcessEnv): CompilerSource {
  const raw = requireString(env, "SMOKE_COMPILER_SOURCE");
  if (!COMPILER_SOURCES.includes(raw as CompilerSource)) {
    throw new Error(
      `SMOKE_COMPILER_SOURCE must be one of ${COMPILER_SOURCES.join(", ")} (got '${raw}')`,
    );
  }
  return raw as CompilerSource;
}

/** Load and strictly validate the harness configuration. Throws on any invalid input. */
export function loadConfig(env: NodeJS.ProcessEnv = process.env): SmokeConfig {
  for (const name of REQUIRED_STRING_VARS) {
    requireString(env, name);
  }

  return {
    orgUrl: requireString(env, "SYSTEM_COLLECTIONURI"),
    project: requireString(env, "SYSTEM_TEAMPROJECT"),
    token: requireString(env, "SYSTEM_ACCESSTOKEN"),
    sourceBranch: requireString(env, "BUILD_SOURCEBRANCH"),
    sourceVersion: requireString(env, "BUILD_SOURCEVERSION"),
    sourcesDirectory: requireString(env, "BUILD_SOURCESDIRECTORY"),
    adoAwBin: requireString(env, "SMOKE_ADO_AW_BIN"),
    artifactName: requireString(env, "SMOKE_ARTIFACT_NAME"),
    mirrorRepo: requireString(env, "SMOKE_MIRROR_REPO"),
    compilerSource: requireCompilerSource(env),
    buildId: requirePositiveInt(env, "BUILD_BUILDID"),
    definitionId: requirePositiveInt(env, "SYSTEM_DEFINITIONID"),
    concurrency: optionalBoundedInt(env, "SMOKE_CONCURRENCY", {
      default: DEFAULT_CONCURRENCY,
      min: MIN_CONCURRENCY,
      max: MAX_CONCURRENCY,
    }),
    childTimeoutMs: optionalBoundedInt(env, "SMOKE_CHILD_TIMEOUT_MS", {
      default: DEFAULT_CHILD_TIMEOUT_MS,
      min: 1,
    }),
    pollMs: optionalBoundedInt(env, "SMOKE_POLL_MS", {
      default: DEFAULT_POLL_MS,
      min: 1,
    }),
    staleRefHours: optionalBoundedInt(env, "SMOKE_STALE_REF_HOURS", {
      default: DEFAULT_STALE_REF_HOURS,
      min: MIN_STALE_REF_HOURS,
    }),
  };
}

/**
 * Deterministic per-run, per-case candidate ref, e.g.
 * `refs/heads/ado-aw-smoke-candidate/12345/canary`. Never the base ref.
 *
 * The case id is validated by the manifest loader before reaching here, so it
 * is safe to interpolate into a ref name.
 */
export function candidateRef(buildId: number, caseId: string): string {
  return `refs/heads/${CANDIDATE_BRANCH_PREFIX}/${buildId}/${caseId}`;
}
