/**
 * Entry point for the deterministic trigger-condition (gate / synth-PR) E2E
 * harness.
 *
 * Reads its configuration from the environment (ADO org/project/token, the
 * registered victim pipeline definition id, target repo), queues the victim
 * pipeline under a battery of real trigger conditions, asserts the observable
 * gate decision (build tags + result), files a GitHub issue on failure, and
 * exits non-zero when any scenario failed so the pipeline goes red.
 *
 * Required env:
 *   - SYSTEM_COLLECTIONURI (or AZURE_DEVOPS_ORG_URL) — ADO collection URI
 *   - SYSTEM_TEAMPROJECT — ADO project
 *   - SYSTEM_ACCESSTOKEN — write-capable ADO token (queue builds, cancel, PRs)
 *   - TRIGGER_E2E_VICTIM_DEFINITION_ID — registered victim pipeline id
 * Optional env:
 *   - TRIGGER_E2E_VICTIM_REPO — ADO Git repo backing the victim's `self`
 *       (where PRs are created). When unset, PR/synth/gate scenarios SKIP and
 *       only the bypass baseline runs.
 *   - TRIGGER_E2E_GITHUB_TOKEN — scoped PAT for issue filing
 *   - TRIGGER_E2E_ISSUE_REPO — GitHub repo for issues (default githubnext/ado-aw)
 *   - TRIGGER_E2E_BUILD_TIMEOUT_MS / TRIGGER_E2E_BUILD_POLL_MS — poll tuning
 *   - TRIGGER_E2E_CONCURRENCY — concurrent scenarios (default 4, max 8)
 *
 * Test-harness module; not shipped in `ado-script.zip`.
 */
import { AdoRest } from "../executor-e2e/ado-rest.js";
import { fileFailureIssue, loadIssueEnv } from "./github-issue.js";
import { runMirrorSyncPreflight } from "./mirror.js";
import { runAll } from "./runner.js";
import { allScenarios } from "./scenarios/index.js";
import type { ScenarioResult, TriggerContext } from "./scenario.js";

function requireEnv(name: string, alt?: string): string {
  const value = process.env[name]?.trim() || (alt ? process.env[alt]?.trim() : undefined);
  if (!value) throw new Error(`required env var ${name}${alt ? ` (or ${alt})` : ""} is not set`);
  return value;
}

function log(msg: string): void {
  // Percent-encode a leading '#' so a message cannot smuggle a ##vso command.
  process.stdout.write(msg.replace(/^#/gm, "%23") + "\n");
}

export function summarise(results: ScenarioResult[]): string {
  const passed = results.filter((r) => r.ok && !r.skipped).length;
  const failed = results.filter((r) => !r.ok).length;
  const skipped = results.filter((r) => r.skipped).length;
  const lines = [
    "",
    "=== Trigger E2E summary ===",
    ...results.map((r) => {
      const state = r.skipped ? "SKIP" : r.ok ? "PASS" : "FAIL";
      const suffix = r.ok && !r.skipped ? "" : ` (${r.phase}: ${r.message ?? ""})`;
      return `  [${state}] ${r.id}${suffix}`;
    }),
    `Total: ${results.length} | Passed: ${passed} | Failed: ${failed} | Skipped: ${skipped}`,
  ];
  return lines.join("\n");
}

export async function main(): Promise<number> {
  const orgUrl = requireEnv("SYSTEM_COLLECTIONURI", "AZURE_DEVOPS_ORG_URL");
  const project = requireEnv("SYSTEM_TEAMPROJECT");
  const token = requireEnv("SYSTEM_ACCESSTOKEN");
  const victimDefinitionIdRaw = requireEnv("TRIGGER_E2E_VICTIM_DEFINITION_ID");
  const victimDefinitionId = Number(victimDefinitionIdRaw);
  if (!Number.isInteger(victimDefinitionId) || victimDefinitionId <= 0) {
    throw new Error(
      `TRIGGER_E2E_VICTIM_DEFINITION_ID must be a positive integer (got '${victimDefinitionIdRaw}')`,
    );
  }
  // Optional: the ADO Git repo backing the victim's `self`. Empty → PR/synth/
  // gate scenarios skip (see requirePrRepo).
  const adoRepo = process.env.TRIGGER_E2E_VICTIM_REPO?.trim() || "";
  const buildId = process.env.BUILD_BUILDID?.trim() || `local-${Date.now()}`;

  const rest = new AdoRest({ orgUrl, project, token, log });

  const ctx: TriggerContext = {
    orgUrl,
    project,
    adoRepo,
    buildId,
    token,
    victimDefinitionId,
    rest,
    log,
    prefix: (id) => `ado-aw-trig-${buildId}-${id}`,
  };

  const preflight = await runMirrorSyncPreflight(process.env, log);
  let results: ScenarioResult[];
  if (preflight && !preflight.ok) {
    results = [preflight];
  } else {
    log(
      `Running ${allScenarios.length} trigger E2E scenarios against ${orgUrl}${project} ` +
        `(victim def #${victimDefinitionId}${adoRepo ? `, repo ${adoRepo}` : ", no PR repo — PR scenarios will skip"})`,
    );
    const scenarioResults = await runAll(ctx, allScenarios);
    results = preflight ? [preflight, ...scenarioResults] : scenarioResults;
  }
  log(summarise(results));

  const issueEnv = loadIssueEnv();
  try {
    await fileFailureIssue(results, issueEnv, log);
  } catch (err) {
    log(`WARNING: failed to file GitHub issue: ${(err as Error).message}`);
  }

  const failed = results.filter((r) => !r.ok).length;
  return failed > 0 ? 1 : 0;
}

// Run as the bundle entry point. Skipped under Vitest so unit tests can import
// these modules without launching the whole suite.
if (process.env.VITEST !== "true") {
  main().then(
    (code) => process.exit(code),
    (err: unknown) => {
      const e = err as Error;
      log(`trigger-e2e crashed: ${e.stack ?? e.message}`);
      process.exit(1);
    },
  );
}
