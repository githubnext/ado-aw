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

interface CandidateAuditReport {
  overview?: {
    build_id?: number;
    aw_info?: {
      model?: string | null;
      detection_model?: string | null;
    };
  };
  downloaded_files?: { path?: string }[];
}

/** Audit candidate children through the released CLI contract. */
export async function verifyCandidateAudit(
  cases: readonly ResolvedCase[],
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
  const canary = results.find(
    (result) => result.caseId === "canary" && result.status === "succeeded" && result.buildId !== undefined,
  );
  if (!canary?.buildId) return { ok: false, results: results.map((result) => ({ ...result })) };

  const casesById = new Map(cases.map((entry) => [entry.id, entry]));
  const targets = results.filter((result) => {
    if (result.status !== "succeeded" || result.buildId === undefined) return false;
    return (
      result.caseId === "canary" ||
      casesById.get(result.caseId)?.assertions?.requestedModels !== undefined
    );
  });
  const verified = results.map((result) => ({ ...result }));

  for (const target of targets) {
    const outputDir = await mkdtemp(join(tmpdir(), "ado-aw-smoke-audit-"));
    let error: string | undefined;
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
      if (outcome.timedOut || outcome.status !== 0) {
        error = `exit=${outcome.status ?? "signal"} timedOut=${outcome.timedOut}; stderr=${redact(outcome.stderr, [options.token])}`;
      } else {
        try {
          const audit = JSON.parse(outcome.stdout) as CandidateAuditReport;
          if (audit.overview?.build_id !== target.buildId) {
            error = `JSON report build id was ${audit.overview?.build_id ?? "<missing>"}; expected ${target.buildId}`;
          }

          if (!error && target.caseId === "canary") {
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
            if (missingRoots.length > 0) {
              error =
                `JSON report did not contain every published artifact family; ` +
                `missing roots: ${missingRoots.join(", ")}`;
            }
          }

          const requested = casesById.get(target.caseId)?.assertions?.requestedModels;
          if (!error && requested?.agent !== undefined) {
            const actual = audit.overview?.aw_info?.model;
            if (actual !== requested.agent) {
              error = `requested Agent model was ${JSON.stringify(actual ?? null)}; expected ${JSON.stringify(requested.agent)}`;
            }
          }
          if (!error && requested?.detection !== undefined) {
            const actual = audit.overview?.aw_info?.detection_model;
            if (actual !== requested.detection) {
              error = `requested Detection model was ${JSON.stringify(actual ?? null)}; expected ${JSON.stringify(requested.detection)}`;
            }
          }
        } catch (parseError) {
          error = `invalid JSON report: ${parseError instanceof Error ? parseError.message : String(parseError)}`;
        }
      }
    } finally {
      await rm(outputDir, { recursive: true, force: true });
    }

    if (error) {
      const index = verified.findIndex((result) => result.caseId === target.caseId);
      verified[index] = {
        ...verified[index]!,
        status: "failed",
        message:
          `candidate audit verification failed for build #${target.buildId} (${target.url ?? "URL unavailable"}): ${error}`,
      };
    }
  }

  return {
    ok: verified.every((result) => result.status === "succeeded"),
    results: verified,
  };
}
