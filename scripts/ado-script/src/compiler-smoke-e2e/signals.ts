/**
 * Post-build verification for fixture-specific observable signals.
 *
 * A successful child build is not sufficient for custom safe outputs: both
 * executor modes must leave their distinct build tags on the actual child run.
 */
import { fixtureByName } from "./fixtures.js";
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

export async function verifyFixtureSignals(
  client: BuildTagClient,
  results: readonly FixtureBuildResult[],
): Promise<SignalVerificationOutcome> {
  const verified: FixtureBuildResult[] = [];

  for (const result of results) {
    const fixture = fixtureByName(result.name);
    if (
      result.status !== "succeeded" ||
      result.buildId === undefined ||
      fixture.requiredBuildTags === undefined
    ) {
      verified.push({ ...result });
      continue;
    }

    try {
      const expected = fixture.requiredBuildTags(result.buildId);
      const actual = await client.getBuildTags(result.buildId, {
        required: expected,
      });
      const missing = expected.filter((tag) => !actual.includes(tag));
      if (missing.length === 0) {
        verified.push({ ...result });
        continue;
      }
      verified.push({
        ...result,
        status: "failed",
        message:
          `build #${result.buildId} is missing required tag(s): ${missing.join(", ")}; ` +
          `observed: ${actual.length > 0 ? actual.join(", ") : "<none>"}`,
      });
    } catch (error) {
      verified.push({
        ...result,
        status: "failed",
        message: `build #${result.buildId} tag verification failed: ${
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
