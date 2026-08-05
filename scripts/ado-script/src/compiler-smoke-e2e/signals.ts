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
