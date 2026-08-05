/**
 * Case manifest for the smoke suite.
 *
 * The manifest (`tests/smoke/cases.json`) is the single source of truth for
 * what the smoke lanes run. Each entry names a markdown (or, for
 * `kind: "raw"`, a hand-written YAML) source, the credential *lane* it runs
 * in, and which compiler-source *modes* it participates in.
 *
 * Adding a smoke is a manifest entry plus a source file — no ADO definition
 * registration, no orchestrator variable, and no change to this file.
 *
 * Parsing is strict and fail-closed. `id` in particular is interpolated into
 * a git ref name, so it is validated against a tight allowlist before any
 * git invocation can see it.
 *
 * Test-harness module; not shipped in `ado-script.zip`.
 */
import { readFile } from "node:fs/promises";
import { join } from "node:path";

/** Which compiler the staged pipelines are built with and sourced from. */
export type CompilerSource = "candidate" | "released";

export const COMPILER_SOURCES: readonly CompilerSource[] = ["candidate", "released"];

/** How a case's `.smoke/pipeline.yml` is produced. */
export type CaseKind = "compiled" | "raw";

export const CASE_KINDS: readonly CaseKind[] = ["compiled", "raw"];

/** Repo-relative path of the case manifest, read from the candidate worktree. */
export const CASES_MANIFEST_PATH = "tests/smoke/cases.json";

const SUPPORTED_SCHEMA = "ado-aw/smoke-cases/1";

/**
 * Case ids become the last segment of a git ref
 * (`refs/heads/ado-aw-smoke-candidate/<buildId>/<caseId>`), so the allowlist
 * is deliberately narrow: lowercase alphanumerics and single hyphens only.
 * This runs before any `git push` argument is constructed.
 */
const CASE_ID_RE = /^[a-z0-9][a-z0-9-]{0,48}$/;

/** Lane ids appear only in logs and env var lookups, but are kept equally tight. */
const LANE_ID_RE = /^[a-z0-9][a-z0-9-]{0,48}$/;

/** Env var names must look like env var names before we read them. */
const ENV_NAME_RE = /^[A-Z][A-Z0-9_]{0,64}$/;

/** The only placeholder supported inside `assertions.requiredBuildTags`. */
const BUILD_ID_PLACEHOLDER = "{buildId}";

export interface AgentCommandAssertion {
  readonly required: readonly string[];
  readonly forbidden: readonly string[];
}

export interface CaseAssertions {
  /** Snippets that must / must not appear in the Agent execution step's bash body. */
  readonly agentCommand?: AgentCommandAssertion;
  /** Build tags the child run must carry, with `{buildId}` expanded to the child build id. */
  readonly requiredBuildTags?: readonly string[];
}

export interface SmokeLane {
  readonly id: string;
  readonly definitionIdEnv: string;
  readonly description?: string;
}

export interface SmokeCase {
  readonly id: string;
  readonly lane: string;
  readonly kind: CaseKind;
  readonly modes: readonly CompilerSource[];
  /** Repo-relative source path (`.md` for compiled, `.yml`/`.yaml` for raw). */
  readonly source: string;
  readonly assertions?: CaseAssertions;
}

export interface SmokeManifest {
  /** Fixed repo-relative path every case's pipeline is staged to. */
  readonly yamlPath: string;
  readonly lanes: readonly SmokeLane[];
  readonly cases: readonly SmokeCase[];
}

export interface ResolvedCase extends SmokeCase {
  /** The registered ADO definition id of this case's lane. */
  readonly definitionId: number;
}

export interface ResolvedCases {
  readonly yamlPath: string;
  readonly mode: CompilerSource;
  /** Cases participating in `mode`, in manifest declaration order. */
  readonly cases: readonly ResolvedCase[];
  /** Distinct lane definition ids in play for `mode`. */
  readonly laneDefinitionIds: readonly number[];
}

function fail(message: string): never {
  throw new Error(`${CASES_MANIFEST_PATH}: ${message}`);
}

function asRecord(value: unknown, what: string): Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    fail(`${what} must be an object`);
  }
  return value as Record<string, unknown>;
}

function asArray(value: unknown, what: string): unknown[] {
  if (!Array.isArray(value)) fail(`${what} must be an array`);
  return value;
}

function asString(value: unknown, what: string): string {
  if (typeof value !== "string" || value.length === 0) {
    fail(`${what} must be a non-empty string`);
  }
  return value;
}

function asStringArray(value: unknown, what: string): string[] {
  return asArray(value, what).map((entry, i) => asString(entry, `${what}[${i}]`));
}

/**
 * Validate a repo-relative source path.
 *
 * Rejects absolute paths, backslashes, `.` / `..` segments, and anything that
 * does not normalise to itself — the value is joined against the worktree root
 * and handed to the compiler, so path traversal here would escape the
 * checkout.
 */
function validateSourcePath(raw: unknown, caseId: string): string {
  const value = asString(raw, `case '${caseId}' source`);
  if (value.startsWith("/") || /^[a-zA-Z]:/.test(value)) {
    fail(`case '${caseId}' source must be repo-relative (got '${value}')`);
  }
  if (value.includes("\\")) {
    fail(`case '${caseId}' source must use forward slashes (got '${value}')`);
  }
  const segments = value.split("/");
  if (segments.some((segment) => segment === "" || segment === "." || segment === "..")) {
    fail(`case '${caseId}' source must not contain empty, '.' or '..' segments (got '${value}')`);
  }
  return value;
}

function validateKindMatchesExtension(kind: CaseKind, source: string, caseId: string): void {
  const isMarkdown = source.endsWith(".md");
  const isYaml = source.endsWith(".yml") || source.endsWith(".yaml");
  if (kind === "compiled" && !isMarkdown) {
    fail(`case '${caseId}' is kind 'compiled' so its source must end in .md (got '${source}')`);
  }
  if (kind === "raw" && !isYaml) {
    fail(`case '${caseId}' is kind 'raw' so its source must end in .yml or .yaml (got '${source}')`);
  }
}

function parseAssertions(raw: unknown, caseId: string): CaseAssertions | undefined {
  if (raw === undefined) return undefined;
  const obj = asRecord(raw, `case '${caseId}' assertions`);

  let agentCommand: AgentCommandAssertion | undefined;
  if (obj.agentCommand !== undefined) {
    const ac = asRecord(obj.agentCommand, `case '${caseId}' assertions.agentCommand`);
    agentCommand = {
      required: asStringArray(ac.required ?? [], `case '${caseId}' assertions.agentCommand.required`),
      forbidden: asStringArray(ac.forbidden ?? [], `case '${caseId}' assertions.agentCommand.forbidden`),
    };
    if (agentCommand.required.length === 0 && agentCommand.forbidden.length === 0) {
      fail(`case '${caseId}' assertions.agentCommand must declare at least one snippet`);
    }
  }

  let requiredBuildTags: string[] | undefined;
  if (obj.requiredBuildTags !== undefined) {
    requiredBuildTags = asStringArray(
      obj.requiredBuildTags,
      `case '${caseId}' assertions.requiredBuildTags`,
    );
    for (const tag of requiredBuildTags) {
      // Catch typo'd placeholders (e.g. `{buildid}`) rather than silently
      // asserting a tag that can never match.
      const unknown = tag.match(/\{[^}]*\}/g)?.filter((token) => token !== BUILD_ID_PLACEHOLDER);
      if (unknown && unknown.length > 0) {
        fail(
          `case '${caseId}' requiredBuildTags '${tag}' uses unsupported placeholder(s) ${unknown.join(", ")}; only ${BUILD_ID_PLACEHOLDER} is supported`,
        );
      }
    }
  }

  if (agentCommand === undefined && requiredBuildTags === undefined) {
    fail(`case '${caseId}' assertions must declare agentCommand and/or requiredBuildTags`);
  }
  return { agentCommand, requiredBuildTags };
}

/** Expand `{buildId}` in a declared build tag. */
export function expandBuildTag(tag: string, buildId: number): string {
  return tag.split(BUILD_ID_PLACEHOLDER).join(String(buildId));
}

/** Parse and strictly validate the manifest document. Never touches the environment. */
export function parseManifest(text: string): SmokeManifest {
  let doc: unknown;
  try {
    doc = JSON.parse(text);
  } catch (err) {
    fail(`is not valid JSON: ${err instanceof Error ? err.message : String(err)}`);
  }
  const root = asRecord(doc, "manifest");

  const schema = asString(root.schema, "schema");
  if (schema !== SUPPORTED_SCHEMA) {
    fail(`unsupported schema '${schema}' (expected '${SUPPORTED_SCHEMA}')`);
  }

  const yamlPath = validateSourcePath(root.yamlPath, "<yamlPath>");

  const lanesObj = asRecord(root.lanes, "lanes");
  const lanes: SmokeLane[] = [];
  for (const [id, value] of Object.entries(lanesObj)) {
    if (!LANE_ID_RE.test(id)) {
      fail(`lane id '${id}' must match ${LANE_ID_RE}`);
    }
    const lane = asRecord(value, `lane '${id}'`);
    const definitionIdEnv = asString(lane.definitionIdEnv, `lane '${id}' definitionIdEnv`);
    if (!ENV_NAME_RE.test(definitionIdEnv)) {
      fail(`lane '${id}' definitionIdEnv '${definitionIdEnv}' must match ${ENV_NAME_RE}`);
    }
    lanes.push({
      id,
      definitionIdEnv,
      description: lane.description === undefined ? undefined : asString(lane.description, `lane '${id}' description`),
    });
  }
  if (lanes.length === 0) fail("lanes must declare at least one lane");

  const envSeen = new Map<string, string>();
  for (const lane of lanes) {
    const existing = envSeen.get(lane.definitionIdEnv);
    if (existing) {
      fail(`lanes '${existing}' and '${lane.id}' share definitionIdEnv '${lane.definitionIdEnv}'`);
    }
    envSeen.set(lane.definitionIdEnv, lane.id);
  }

  const laneIds = new Set(lanes.map((lane) => lane.id));
  const cases: SmokeCase[] = [];
  const seenIds = new Set<string>();

  for (const [i, raw] of asArray(root.cases, "cases").entries()) {
    const entry = asRecord(raw, `cases[${i}]`);
    const id = asString(entry.id, `cases[${i}].id`);
    if (!CASE_ID_RE.test(id)) {
      fail(`case id '${id}' must match ${CASE_ID_RE} (it becomes a git ref segment)`);
    }
    if (seenIds.has(id)) fail(`duplicate case id '${id}'`);
    seenIds.add(id);

    const lane = asString(entry.lane, `case '${id}' lane`);
    if (!laneIds.has(lane)) {
      fail(`case '${id}' references unknown lane '${lane}'`);
    }

    const kind = asString(entry.kind, `case '${id}' kind`) as CaseKind;
    if (!CASE_KINDS.includes(kind)) {
      fail(`case '${id}' kind '${kind}' must be one of ${CASE_KINDS.join(", ")}`);
    }

    const modes = asStringArray(entry.modes, `case '${id}' modes`) as CompilerSource[];
    if (modes.length === 0) fail(`case '${id}' modes must not be empty`);
    for (const mode of modes) {
      if (!COMPILER_SOURCES.includes(mode)) {
        fail(`case '${id}' mode '${mode}' must be one of ${COMPILER_SOURCES.join(", ")}`);
      }
    }
    if (new Set(modes).size !== modes.length) {
      fail(`case '${id}' modes must not contain duplicates`);
    }

    const source = validateSourcePath(entry.source, id);
    validateKindMatchesExtension(kind, source, id);

    cases.push({ id, lane, kind, modes, source, assertions: parseAssertions(entry.assertions, id) });
  }

  if (cases.length === 0) fail("cases must declare at least one case");

  for (const mode of COMPILER_SOURCES) {
    if (!cases.some((entry) => entry.modes.includes(mode))) {
      fail(`no case participates in mode '${mode}'`);
    }
  }

  return { yamlPath, lanes, cases };
}

/** Parse a lane definition id from the environment. Strict: positive integers only. */
function laneDefinitionId(env: NodeJS.ProcessEnv, lane: SmokeLane): number {
  const raw = env[lane.definitionIdEnv]?.trim();
  if (!raw || /^\$\([^)]*\)$/.test(raw)) {
    throw new Error(
      `lane '${lane.id}' requires env var ${lane.definitionIdEnv} (unset, empty, or an unexpanded ADO macro)`,
    );
  }
  const parsed = Number(raw);
  if (!Number.isInteger(parsed) || parsed <= 0) {
    throw new Error(`${lane.definitionIdEnv} must be a positive integer (got '${raw}')`);
  }
  return parsed;
}

/**
 * Load the manifest from the detached candidate worktree and resolve every
 * case participating in `mode` to its lane's ADO definition id.
 *
 * Read from the worktree — an exact checkout of `BUILD_SOURCEVERSION` — rather
 * than `BUILD_SOURCESDIRECTORY`, which may sit at a different commit. Only the
 * lanes actually used by `mode` require their env var to be set.
 */
export async function loadCases(
  worktreeDir: string,
  env: NodeJS.ProcessEnv,
  mode: CompilerSource,
): Promise<ResolvedCases> {
  const text = await readFile(join(worktreeDir, CASES_MANIFEST_PATH), "utf8");
  const manifest = parseManifest(text);

  const selected = manifest.cases.filter((entry) => entry.modes.includes(mode));
  if (selected.length === 0) {
    throw new Error(`${CASES_MANIFEST_PATH}: no case participates in mode '${mode}'`);
  }

  const lanesById = new Map(manifest.lanes.map((lane) => [lane.id, lane]));
  const idByLane = new Map<string, number>();
  for (const entry of selected) {
    if (idByLane.has(entry.lane)) continue;
    idByLane.set(entry.lane, laneDefinitionId(env, lanesById.get(entry.lane)!));
  }

  const seen = new Map<number, string>();
  for (const [lane, id] of idByLane) {
    const existing = seen.get(id);
    if (existing) {
      throw new Error(`lanes '${existing}' and '${lane}' resolve to the same definition id ${id}`);
    }
    seen.set(id, lane);
  }

  return {
    yamlPath: manifest.yamlPath,
    mode,
    cases: selected.map((entry) => ({ ...entry, definitionId: idByLane.get(entry.lane)! })),
    laneDefinitionIds: [...idByLane.values()],
  };
}
