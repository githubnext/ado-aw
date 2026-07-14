/**
 * Failure-issue filing for the trigger-condition E2E harness.
 *
 * Reuses the low-level GitHub client primitives from the executor-e2e harness
 * (`findOpenIssueByTitle` / `createGitHubIssue` / `diagnoseGitHubAuthFailure`)
 * but frames issues with a trigger-specific title prefix + labels so the two
 * suites never dedupe into each other's issues.
 *
 * Test-harness module; not shipped in `ado-script.zip`.
 */
import {
  createGitHubIssue,
  diagnoseGitHubAuthFailure,
  findOpenIssueByTitle,
} from "../executor-e2e/github-issue.js";
import type { ScenarioResult } from "./scenario.js";

export const ISSUE_TITLE_PREFIX = "[trigger-e2e-failure] ";
const DEFAULT_REPO = "githubnext/ado-aw";
const DEFAULT_LABELS = ["trigger-e2e-failure", "pipeline-failure"];
const MAX_TITLE_LEN = 200;

export interface IssueEnv {
  token?: string;
  repo: string;
  labels: string[];
  buildId?: string;
  buildUrl?: string;
  project?: string;
}

/** Treat an UNEXPANDED ADO macro (`$(VAR)`) as absent. */
function cleanVar(raw: string | undefined): string | undefined {
  const value = raw?.trim();
  if (!value || /^\$\(.*\)$/.test(value)) return undefined;
  return value;
}

export function loadIssueEnv(env: NodeJS.ProcessEnv = process.env): IssueEnv {
  const labelsRaw = env.TRIGGER_E2E_ISSUE_LABELS?.trim();
  let labels = DEFAULT_LABELS;
  if (labelsRaw) {
    try {
      const parsed: unknown = JSON.parse(labelsRaw);
      if (Array.isArray(parsed)) labels = parsed.filter((v): v is string => typeof v === "string");
    } catch {
      /* keep defaults */
    }
  }
  return {
    token: env.TRIGGER_E2E_GITHUB_TOKEN?.trim() || env.ADO_AW_DEBUG_GITHUB_TOKEN?.trim(),
    repo: cleanVar(env.TRIGGER_E2E_ISSUE_REPO) || DEFAULT_REPO,
    labels,
    buildId: env.BUILD_BUILDID?.trim(),
    buildUrl:
      env.TRIGGER_E2E_BUILD_URL?.trim() ||
      (env.SYSTEM_COLLECTIONURI && env.SYSTEM_TEAMPROJECT && env.BUILD_BUILDID
        ? `${env.SYSTEM_COLLECTIONURI.replace(/\/+$/, "")}/${encodeURIComponent(env.SYSTEM_TEAMPROJECT)}/_build/results?buildId=${env.BUILD_BUILDID}`
        : undefined),
    project: env.SYSTEM_TEAMPROJECT?.trim(),
  };
}

/** Stable title keyed on the sorted set of failing scenario ids (dedupes). */
export function buildIssueTitle(failed: ScenarioResult[]): string {
  const ids = [...new Set(failed.map((r) => r.id))].sort();
  const title = `${ISSUE_TITLE_PREFIX}${ids.join(", ")}`;
  return title.length <= MAX_TITLE_LEN ? title : title.slice(0, MAX_TITLE_LEN);
}

export function renderIssueBody(results: ScenarioResult[], env: IssueEnv): string {
  const failed = results.filter((r) => !r.ok);
  const skipped = results.filter((r) => r.skipped);
  const passed = results.filter((r) => r.ok && !r.skipped);

  const lines: string[] = [
    "The deterministic trigger-condition (gate / synth-PR) E2E suite reported failures.",
    "",
    "## Failed scenarios",
    "",
    "| Scenario | Phase | Message |",
    "| --- | --- | --- |",
    ...failed.map(
      (r) =>
        `| \`${r.id}\` | ${r.phase ?? "?"} | ${(r.message ?? "").replace(/\r?\n/g, " ").replace(/\|/g, "\\|").slice(0, 400)} |`,
    ),
    "",
    "## Run",
    `- Project: ${env.project ?? "unknown"}`,
    `- Build ID: ${env.buildId ?? "unknown"}`,
    `- Build URL: ${env.buildUrl ?? "unknown"}`,
    `- Passed: ${passed.length} | Failed: ${failed.length} | Skipped: ${skipped.length}`,
  ];
  if (skipped.length > 0) {
    lines.push("", "## Skipped (missing precondition)", "");
    for (const s of skipped) lines.push(`- \`${s.id}\`: ${s.message ?? ""}`);
  }
  lines.push(
    "",
    "> Filed automatically by the trigger-e2e pipeline. Re-runs with the same",
    "> failing-scenario signature update this issue rather than opening a new one.",
  );
  return lines.join("\n");
}

export interface FileIssueOutcome {
  filed: boolean;
  reason?: string;
  url?: string;
}

/** Extract a trailing "HTTP <status>" code from a thrown GitHub client error. */
function statusFromError(err: unknown): number | undefined {
  const message = err instanceof Error ? err.message : String(err);
  const match = message.match(/HTTP (\d{3})/);
  return match ? Number(match[1]) : undefined;
}

/** File (or dedupe) a failure issue. No-op with no failures or no token. */
export async function fileFailureIssue(
  results: ScenarioResult[],
  env: IssueEnv,
  log: (msg: string) => void,
  fetchImpl?: typeof fetch,
): Promise<FileIssueOutcome> {
  const failed = results.filter((r) => !r.ok);
  if (failed.length === 0) return { filed: false, reason: "no failures" };
  if (!env.token) {
    log("no GitHub token configured (TRIGGER_E2E_GITHUB_TOKEN); skipping issue filing");
    return { filed: false, reason: "no token" };
  }

  const opts = { token: env.token, repo: env.repo, fetchImpl };
  log(`filing failure issue to '${env.repo}' (${failed.length} failed scenario(s))`);
  const title = buildIssueTitle(failed);
  try {
    const existing = await findOpenIssueByTitle(opts, title);
    if (existing !== undefined) {
      log(`open issue #${existing} already tracks this failure signature; skipping`);
      return { filed: false, reason: `deduped to #${existing}` };
    }
    const url = await createGitHubIssue(opts, title, renderIssueBody(results, env), env.labels);
    log(`filed GitHub issue: ${url}`);
    return { filed: true, url };
  } catch (err) {
    const status = statusFromError(err);
    if (status !== undefined) await diagnoseGitHubAuthFailure(opts, status, log);
    throw err;
  }
}
