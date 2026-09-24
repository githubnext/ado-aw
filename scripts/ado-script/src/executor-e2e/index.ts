/**
 * Entry point for the deterministic Stage 3 (executor) safe-output E2E harness.
 *
 * Reads its configuration from the environment (ADO org/project/token, the
 * `ado-aw` binary path, target repo), runs every scenario against a real ADO
 * project, files a GitHub issue on failure, and exits non-zero when any
 * scenario failed so the pipeline goes red.
 *
 * Required env:
 *   - SYSTEM_COLLECTIONURI (or AZURE_DEVOPS_ORG_URL) — ADO collection URI
 *   - SYSTEM_TEAMPROJECT — ADO project
 *   - SYSTEM_ACCESSTOKEN — write-capable ADO token
 *   - EXECUTOR_E2E_ADO_AW_BIN — path to the ado-aw binary under test
 * Optional env:
 *   - EXECUTOR_E2E_ADO_REPO — ADO Git repo for PR/git scenarios (default agent-definitions)
 *   - EXECUTOR_E2E_GITHUB_TOKEN — scoped PAT for issue filing
 *   - EXECUTOR_E2E_ISSUE_REPO — GitHub repo for issues (default githubnext/ado-aw)
 *   - E2E_QUEUE_PIPELINE_ID / E2E_WIKI_NAME — optional-precondition scenarios
 *   - E2E_WORK_ITEM_ASSIGNEE — assignable ADO identity (falls back to BUILD_REQUESTEDFOREMAIL)
 *
 * Test-harness module; not shipped in `ado-script.zip`.
 */
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";

import { AdoRest } from "./ado-rest.js";
import { fileFailureIssue, loadIssueEnv } from "./github-issue.js";
import { runAll } from "./runner.js";
import { allScenarios } from "./scenarios/index.js";
import type { Scenario, ScenarioContext, ScenarioResult } from "./scenario.js";
import { resolveCrossOrgEnv } from "./scenarios/cross-org.js";
import { resolveExecutorE2eReviewer } from "./scenarios/create-pull-request.js";

export function selectScenarios(
  available: Scenario<unknown>[],
  requested: string | undefined,
): Scenario<unknown>[] {
  if (requested === undefined || requested === "") return available;
  const names = requested.split(",").map((name) => name.trim());
  if (names.some((name) => !name) || new Set(names).size !== names.length) {
    throw new Error("EXECUTOR_E2E_SCENARIOS must contain distinct nonempty scenario IDs");
  }
  return names.map((name) => {
    const scenario = available.find((scenario) => (scenario.id ?? scenario.tool) === name);
    if (!scenario) throw new Error(`Unknown executor E2E scenario '${name}'`);
    return scenario;
  });
}

export function booleanOption(value: string | undefined, fallback: boolean): boolean {
  if (value === undefined || value === "") return fallback;
  if (value.toLowerCase() === "true") return true;
  if (value.toLowerCase() === "false") return false;
  throw new Error("E2E boolean options must be true or false");
}

async function requiredPreflight(ctx: ScenarioContext, scenarios: Scenario<unknown>[]): Promise<void> {
  const names = scenarios.map((scenario) => scenario.id ?? scenario.tool);
  if (names.some((name) => name.includes("cross-org"))) resolveCrossOrgEnv(ctx);
  if (names.some((name) => name.includes("add-reviewers"))) {
    const reviewer = resolveExecutorE2eReviewer();
    if (!await ctx.rest.resolveIdentityId(reviewer)) {
      throw new Error("Required reviewer does not resolve to exactly one existing identity");
    }
  }
}

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
    "=== Executor E2E summary ===",
    ...results.map((r) => {
      const state = r.skipped ? "SKIP" : r.ok ? "PASS" : "FAIL";
      const suffix = r.ok && !r.skipped ? "" : ` (${r.phase}: ${r.message ?? ""})`;
      return `  [${state}] ${r.tool}${suffix}`;
    }),
    `Total: ${results.length} | Passed: ${passed} | Failed: ${failed} | Skipped: ${skipped}`,
  ];
  return lines.join("\n");
}

export async function main(): Promise<number> {
  const selected = selectScenarios(allScenarios, process.env.EXECUTOR_E2E_SCENARIOS);
  const required = booleanOption(process.env.EXECUTOR_E2E_REQUIRE_SELECTED, false);
  const fileIssues = booleanOption(process.env.EXECUTOR_E2E_FILE_FAILURE_ISSUE, true);
  const orgUrl = requireEnv("SYSTEM_COLLECTIONURI", "AZURE_DEVOPS_ORG_URL");
  const project = requireEnv("SYSTEM_TEAMPROJECT");
  const token = requireEnv("SYSTEM_ACCESSTOKEN");
  const adoAwBin = requireEnv("EXECUTOR_E2E_ADO_AW_BIN");
  const adoRepo = process.env.EXECUTOR_E2E_ADO_REPO?.trim() || "agent-definitions";
  const buildId = process.env.BUILD_BUILDID?.trim() || `local-${Date.now()}`;

  const workDir = await mkdtemp(join(tmpdir(), "ado-aw-e2e-"));
  const rest = new AdoRest({ orgUrl, project, token, log });

  const ctx: ScenarioContext = {
    orgUrl,
    project,
    adoRepo,
    buildId,
    token,
    adoAwBin,
    workDir,
    rest,
    log,
    prefix: (tool) => `ado-aw-det-${buildId}-${tool}`,
  };

  log(`Running ${selected.length} executor E2E scenarios against ${orgUrl}${project}`);
  let results: ScenarioResult[] = [];
  try {
    if (required) {
      try {
        await requiredPreflight(ctx, selected);
      } catch (error) {
        results = [{ tool: "required-preflight", ok: false, phase: "preflight",
          message: (error as Error).message, durationMs: 0 }];
        log(summarise(results));
        return 1;
      }
    }
    results = await runAll(ctx, selected);
    if (required) results = results.map((result) => result.skipped
      ? { ...result, ok: false, skipped: false, phase: "required-coverage", message: `Required scenario skipped: ${result.message}` }
      : result);
    log(summarise(results));

    if (fileIssues) {
      const issueEnv = loadIssueEnv();
      try {
        await fileFailureIssue(results, issueEnv, log);
      } catch (err) {
        log(`WARNING: failed to file GitHub issue: ${(err as Error).message}`);
      }
    } else {
      log("Failure-issue filing disabled for this run.");
    }

    const failed = results.filter((r) => !r.ok).length;
    return failed > 0 ? 1 : 0;
  } finally {
    // Remove the scratch dir so CI agents and local runs don't accumulate
    // ado-aw-e2e-* directories (scenarios clean their own children, but the
    // parent mkdtemp dir would otherwise persist).
    try {
      const report = process.env.EXECUTOR_E2E_RESULTS_PATH;
      if (report) {
        await mkdir(dirname(report), { recursive: true });
        await writeFile(report, JSON.stringify({
          schema: "ado-aw/executor-e2e-results/1",
          commit: process.env.BUILD_SOURCEVERSION ?? null,
          buildId,
          required,
          selected: selected.map((scenario) => scenario.id ?? scenario.tool),
          results,
        }, null, 2));
      }
    } finally {
      await rm(workDir, { recursive: true, force: true });
    }
  }
}

// Run as the bundle entry point. Skipped under Vitest so unit tests can import
// these modules without launching the whole suite.
if (process.env.VITEST !== "true") {
  main().then(
    (code) => process.exit(code),
    (err: unknown) => {
      const e = err as Error;
      log(`executor-e2e crashed: ${e.stack ?? e.message}`);
      process.exit(1);
    },
  );
}
