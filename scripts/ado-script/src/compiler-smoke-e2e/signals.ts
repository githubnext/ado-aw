/**
 * Post-build verification for case-specific observable signals.
 *
 * A successful child build is not sufficient for every case: e.g. a custom
 * safe-output job must leave its deterministic build tag on the actual child
 * run, and an agent that never reached its safe-output tools still produces a
 * green build with nothing executed. Which tags and executed safe outputs are
 * required is declared per case in `tests/smoke/cases.json` rather than
 * hardcoded here, so a new case with a signal assertion is a manifest change
 * and never a code change.
 *
 * Test-harness module; not shipped in `ado-script.zip`.
 */
import { expandBuildTag, type RequiredSafeOutput, type ResolvedCase } from "./cases.js";
import type { FixtureBuildResult } from "./runner.js";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { redact, safeSpawn } from "./process.js";

export interface BuildTagClient {
  getBuildTags(
    buildId: number,
    opts?: { required?: readonly string[] },
  ): Promise<string[]>;
}

/** Reads one file out of a child build's published artifact. */
export interface ArtifactFileClient {
  downloadArtifactFile(
    buildId: number,
    artifactName: string,
    fileName: string,
  ): Promise<string | undefined>;
}

export type SignalClient = BuildTagClient & ArtifactFileClient;

/** Artifact the (automatic) SafeOutputs job publishes its execution log in. */
export const SAFE_OUTPUTS_ARTIFACT = "safe_outputs";
/** Stage 3 execution log: one record per executed safe output. */
export const EXECUTED_SAFE_OUTPUTS_FILE = "safe-outputs-executed.ndjson";
/** Execution statuses that count as "the safe output ran". */
const EXECUTED_STATUSES = new Set(["succeeded", "warning"]);

export interface ExecutionRecord {
  name: string;
  status: string;
  context: string;
}

export interface SignalVerificationOutcome {
  readonly ok: boolean;
  readonly results: FixtureBuildResult[];
}

/** Normalise a safe-output name; Stage 3 records `create-work-item` as `create_work_item`. */
function normaliseToolName(name: string): string {
  return name.replace(/-/g, "_");
}

/** Parse the execution log, skipping lines that are not well-formed records. */
export function parseExecutionLog(ndjson: string): ExecutionRecord[] {
  const records: ExecutionRecord[] = [];
  for (const line of ndjson.split(/\r?\n/)) {
    if (!line.trim()) continue;
    try {
      const value = JSON.parse(line) as Record<string, unknown>;
      if (typeof value.name !== "string") continue;
      records.push({
        name: value.name,
        status: typeof value.status === "string" ? value.status : "",
        context: typeof value.context === "string" ? value.context : "",
      });
    } catch {
      continue;
    }
  }
  return records;
}

/**
 * Return a failure message for every required safe output with no matching
 * executed record, or an empty array when all are satisfied.
 */
export function missingSafeOutputs(
  required: readonly RequiredSafeOutput[],
  records: readonly ExecutionRecord[],
  buildId: number,
): string[] {
  return required.flatMap((expected) => {
    const name = normaliseToolName(expected.name);
    const context =
      expected.contextIncludes === undefined
        ? undefined
        : expandBuildTag(expected.contextIncludes, buildId);
    const found = records.some(
      (record) =>
        normaliseToolName(record.name) === name &&
        EXECUTED_STATUSES.has(record.status) &&
        (context === undefined || record.context.includes(context)),
    );
    if (found) return [];
    return [context === undefined ? expected.name : `${expected.name} (context containing '${context}')`];
  });
}

function describeRecords(records: readonly ExecutionRecord[]): string {
  if (records.length === 0) return "<none>";
  return records.map((record) => `${record.name}:${record.status || "?"}`).join(", ");
}

async function verifyRequiredSafeOutputs(
  client: ArtifactFileClient,
  buildId: number,
  required: readonly RequiredSafeOutput[],
): Promise<string | undefined> {
  const log = await client.downloadArtifactFile(
    buildId,
    SAFE_OUTPUTS_ARTIFACT,
    EXECUTED_SAFE_OUTPUTS_FILE,
  );
  if (log === undefined) {
    return (
      `build #${buildId} executed no safe outputs: '${SAFE_OUTPUTS_ARTIFACT}' has no ${EXECUTED_SAFE_OUTPUTS_FILE}; ` +
      `required: ${required.map((entry) => entry.name).join(", ")}`
    );
  }
  const records = parseExecutionLog(log);
  const missing = missingSafeOutputs(required, records, buildId);
  if (missing.length === 0) return undefined;
  return (
    `build #${buildId} did not execute required safe output(s): ${missing.join(", ")}; ` +
    `executed: ${describeRecords(records)}`
  );
}

async function verifyRequiredTags(
  client: BuildTagClient,
  buildId: number,
  declared: readonly string[],
): Promise<string | undefined> {
  const expected = declared.map((tag) => expandBuildTag(tag, buildId));
  const actual = await client.getBuildTags(buildId, { required: expected });
  const missing = expected.filter((tag) => !actual.includes(tag));
  if (missing.length === 0) return undefined;
  return (
    `build #${buildId} is missing required tag(s): ${missing.join(", ")}; ` +
    `observed: ${actual.length > 0 ? actual.join(", ") : "<none>"}`
  );
}

/**
 * Verify every declared `requiredBuildTags` and `requiredSafeOutputs`
 * assertion against the real child runs.
 */
export async function verifyCaseSignals(
  client: SignalClient,
  cases: readonly ResolvedCase[],
  results: readonly FixtureBuildResult[],
): Promise<SignalVerificationOutcome> {
  const byId = new Map(cases.map((entry) => [entry.id, entry]));
  const verified: FixtureBuildResult[] = [];

  for (const result of results) {
    const assertions = byId.get(result.caseId)?.assertions;
    const tags = assertions?.requiredBuildTags;
    const safeOutputs = assertions?.requiredSafeOutputs;
    const buildId = result.buildId;
    if (
      result.status !== "succeeded" ||
      buildId === undefined ||
      (!tags?.length && !safeOutputs?.length)
    ) {
      verified.push({ ...result });
      continue;
    }

    const failures: string[] = [];
    if (tags?.length) {
      try {
        const failure = await verifyRequiredTags(client, buildId, tags);
        if (failure) failures.push(failure);
      } catch (error) {
        failures.push(
          `build #${buildId} tag verification failed: ${
            error instanceof Error ? error.message : String(error)
          }`,
        );
      }
    }
    if (safeOutputs?.length) {
      try {
        const failure = await verifyRequiredSafeOutputs(client, buildId, safeOutputs);
        if (failure) failures.push(failure);
      } catch (error) {
        failures.push(
          `build #${buildId} safe-output verification failed: ${
            error instanceof Error ? error.message : String(error)
          }`,
        );
      }
    }
    verified.push(
      failures.length === 0
        ? { ...result }
        : { ...result, status: "failed", message: failures.join("; ") },
    );
  }

  return {
    ok: verified.every((result) => result.status === "succeeded"),
    results: verified,
  };
}

/** Audit one completed candidate child through the released CLI contract. */
export async function verifyCandidateAudit(
  results: readonly FixtureBuildResult[],
  options: {
    adoAwBin: string;
    cwd: string;
    orgUrl: string;
    project: string;
    token: string;
    timeoutMs: number;
  },
): Promise<SignalVerificationOutcome> {
  const target = results.find(
    (result) => result.caseId === "canary" && result.status === "succeeded" && result.buildId !== undefined,
  );
  if (!target?.buildId) return { ok: false, results: results.map((result) => ({ ...result })) };

  const outputDir = await mkdtemp(join(tmpdir(), "ado-aw-smoke-audit-"));
  try {
    const outcome = await safeSpawn({
      cmd: options.adoAwBin,
      args: [
        "audit",
        String(target.buildId),
        "--json",
        "--no-cache",
        "--output",
        outputDir,
        "--org",
        options.orgUrl,
        "--project",
        options.project,
      ],
      cwd: options.cwd,
      env: { AZURE_DEVOPS_EXT_PAT: options.token },
      timeoutMs: options.timeoutMs,
    });
    let error: string | undefined;
    if (outcome.timedOut || outcome.status !== 0) {
      error = `exit=${outcome.status ?? "signal"} timedOut=${outcome.timedOut}; stderr=${redact(outcome.stderr, [options.token])}`;
    } else {
      try {
        const audit = JSON.parse(outcome.stdout) as {
          overview?: { build_id?: number };
          downloaded_files?: { path?: string }[];
        };
        const paths =
          audit.downloaded_files
            ?.flatMap((file) => file.path ?? [])
            .map((path) => path.replaceAll("\\", "/")) ?? [];
        const expectedRoots = [
          `agent_outputs_${target.buildId}/`,
          `analyzed_outputs_${target.buildId}/`,
          "safe_outputs/",
        ];
        const missingRoots = expectedRoots.filter(
          (root) => !paths.some((path) => path.startsWith(root)),
        );
        if (audit.overview?.build_id !== target.buildId || missingRoots.length > 0) {
          error =
            `JSON report did not contain the child build id and every published artifact family; ` +
            `missing roots: ${missingRoots.join(", ") || "<none>"}`;
        }
      } catch (parseError) {
        error = `invalid JSON report: ${parseError instanceof Error ? parseError.message : String(parseError)}`;
      }
    }
    if (!error) return { ok: true, results: results.map((result) => ({ ...result })) };

    return {
      ok: false,
      results: results.map((result) =>
        result.caseId === target.caseId
          ? {
              ...result,
              status: "failed",
              message:
                `candidate audit contract failed for build #${target.buildId} (${target.url ?? "URL unavailable"}); ` +
                `expected artifacts agent_outputs_${target.buildId}, analyzed_outputs_${target.buildId}, and safe_outputs: ${error}`,
            }
          : { ...result },
      ),
    };
  } finally {
    await rm(outputDir, { recursive: true, force: true });
  }
}
