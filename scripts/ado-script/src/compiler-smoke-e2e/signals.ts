/**
 * Post-build verification for case-specific observable signals.
 *
 * A successful child build is not sufficient for every case: e.g. a custom
 * safe-output job must leave its deterministic build tag on the actual child
 * run. Which tags are required is declared per case in `tests/smoke/cases.json`
 * rather than hardcoded here, so a new case with a tag assertion is a manifest
 * change and never a code change.
 *
 * Test-harness module; not shipped in `ado-script.zip`.
 */
import { expandBuildTag, type ResolvedCase } from "./cases.js";
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

export interface SignalVerificationOutcome {
  readonly ok: boolean;
  readonly results: FixtureBuildResult[];
}

/** Verify every declared `requiredBuildTags` assertion against the real child runs. */
export async function verifyCaseSignals(
  client: BuildTagClient,
  cases: readonly ResolvedCase[],
  results: readonly FixtureBuildResult[],
): Promise<SignalVerificationOutcome> {
  const byId = new Map(cases.map((entry) => [entry.id, entry]));
  const verified: FixtureBuildResult[] = [];

  for (const result of results) {
    const declared = byId.get(result.caseId)?.assertions?.requiredBuildTags;
    const buildId = result.buildId;
    if (result.status !== "succeeded" || buildId === undefined || !declared?.length) {
      verified.push({ ...result });
      continue;
    }

    try {
      const expected = declared.map((tag) => expandBuildTag(tag, buildId));
      const actual = await client.getBuildTags(buildId, { required: expected });
      const missing = expected.filter((tag) => !actual.includes(tag));
      if (missing.length === 0) {
        verified.push({ ...result });
        continue;
      }
      verified.push({
        ...result,
        status: "failed",
        message:
          `build #${buildId} is missing required tag(s): ${missing.join(", ")}; ` +
          `observed: ${actual.length > 0 ? actual.join(", ") : "<none>"}`,
      });
    } catch (error) {
      verified.push({
        ...result,
        status: "failed",
        message: `build #${buildId} tag verification failed: ${
          error instanceof Error ? error.message : String(error)
        }`,
      });
    }
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
          downloaded_files?: unknown[];
        };
        if (audit.overview?.build_id !== target.buildId || !audit.downloaded_files?.length) {
          error = "JSON report did not contain the child build id and published artifact files";
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
