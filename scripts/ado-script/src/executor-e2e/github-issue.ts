/**
 * Direct GitHub issue filing for the deterministic executor E2E harness.
 *
 * When one or more scenarios fail, the harness files a single GitHub issue on
 * the target repo (default `githubnext/ado-aw`) using a scoped PAT. Filing is
 * deduped by exact title so a recurring failure signature does not spam new
 * issues.
 *
 * Test-harness module; not shipped in `ado-script.zip`.
 */
import {
  createGitHubIssue,
  diagnoseGitHubAuthFailure,
  findOpenIssueByTitle,
  cleanVar,
  statusFromError,
} from "./github-client.js";
import type { FetchImpl, GitHubClientOptions } from "./github-client.js";
import type { ScenarioResult } from "./scenario.js";

// Re-exported for the harness's own tests and for scenario modules that need
// the same primitives; there is exactly one GitHub client (github-client.ts).
export { createGitHubIssue, diagnoseGitHubAuthFailure, findOpenIssueByTitle };

export const ISSUE_TITLE_PREFIX = "[executor-e2e-failure] ";
const DEFAULT_REPO = "githubnext/ado-aw";
const DEFAULT_LABELS = ["executor-e2e-failure", "pipeline-failure"];
const MAX_TITLE_LEN = 200;

export interface IssueEnv {
  token?: string;
  repo: string;
  labels: string[];
  buildId?: string;
  buildUrl?: string;
  project?: string;
}

export function loadIssueEnv(env: NodeJS.ProcessEnv = process.env): IssueEnv {
  const labelsRaw = env.EXECUTOR_E2E_ISSUE_LABELS?.trim();
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
    token: env.EXECUTOR_E2E_GITHUB_TOKEN?.trim() || env.ADO_AW_GITHUB_TOKEN?.trim(),
    repo: cleanVar(env.EXECUTOR_E2E_ISSUE_REPO) || DEFAULT_REPO,
    labels,
    buildId: env.BUILD_BUILDID?.trim(),
    buildUrl:
      env.EXECUTOR_E2E_BUILD_URL?.trim() ||
      (env.SYSTEM_COLLECTIONURI && env.SYSTEM_TEAMPROJECT && env.BUILD_BUILDID
        ? `${env.SYSTEM_COLLECTIONURI.replace(/\/+$/, "")}/${encodeURIComponent(env.SYSTEM_TEAMPROJECT)}/_build/results?buildId=${env.BUILD_BUILDID}`
        : undefined),
    project: env.SYSTEM_TEAMPROJECT?.trim(),
  };
}

/**
 * Build a stable issue title keyed on the sorted set of failing tools, so a
 * recurring failure signature dedupes to a single open issue.
 */
export function buildIssueTitle(failed: ScenarioResult[]): string {
  const tools = [...new Set(failed.map((r) => r.tool))].sort();
  const title = `${ISSUE_TITLE_PREFIX}${tools.join(", ")}`;
  return title.length <= MAX_TITLE_LEN ? title : title.slice(0, MAX_TITLE_LEN);
}

export function renderIssueBody(
  results: ScenarioResult[],
  env: IssueEnv,
): string {
  const failed = results.filter((r) => !r.ok);
  const skipped = results.filter((r) => r.skipped);
  const passed = results.filter((r) => r.ok && !r.skipped);

  const lines: string[] = [
    "The deterministic Stage 3 (executor) safe-output E2E suite reported failures.",
    "",
    "## Failed scenarios",
    "",
    "| Tool | Phase | Message |",
    "| --- | --- | --- |",
    ...failed.map(
      (r) =>
        // Collapse newlines to spaces so a multi-line message (e.g. an embedded
        // stderr/partial-output dump) can't terminate the table row and corrupt
        // the rendered report; escape pipes; then bound the length.
        `| \`${r.tool}\` | ${r.phase ?? "?"} | ${(r.message ?? "").replace(/\r?\n/g, " ").replace(/\|/g, "\\|").slice(0, 400)} |`,
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
    for (const s of skipped) lines.push(`- \`${s.tool}\`: ${s.message ?? ""}`);
  }
  lines.push(
    "",
    "> Filed automatically by the executor-e2e pipeline. Re-runs with the same",
    "> failing-tool signature update this issue rather than opening a new one.",
  );
  return lines.join("\n");
}

export interface FileIssueOutcome {
  filed: boolean;
  reason?: string;
  url?: string;
}

/**
 * File (or dedupe) a failure issue. No-op when there are no failures or when no
 * token is configured.
 */
export async function fileFailureIssue(
  results: ScenarioResult[],
  env: IssueEnv,
  log: (msg: string) => void,
  fetchImpl?: FetchImpl,
): Promise<FileIssueOutcome> {
  const failed = results.filter((r) => !r.ok);
  if (failed.length === 0) return { filed: false, reason: "no failures" };
  if (!env.token) {
    log("no GitHub token configured (EXECUTOR_E2E_GITHUB_TOKEN); skipping issue filing");
    return { filed: false, reason: "no token" };
  }

  const opts: GitHubClientOptions = { token: env.token, repo: env.repo, fetchImpl };
  // Log the resolved target up front: the repo can come from a definition
  // variable OR the YAML default, and a wrong target is a common failure cause.
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
    // Surface an actionable diagnosis for auth/permission failures before
    // rethrowing so the caller's WARNING still carries the raw error too.
    const status = statusFromError(err);
    if (status !== undefined) await diagnoseGitHubAuthFailure(opts, status, log);
    throw err;
  }
}
