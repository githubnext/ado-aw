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
import type { ScenarioResult } from "./scenario.js";

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
    token: env.EXECUTOR_E2E_GITHUB_TOKEN?.trim() || env.ADO_AW_DEBUG_GITHUB_TOKEN?.trim(),
    repo: env.EXECUTOR_E2E_ISSUE_REPO?.trim() || DEFAULT_REPO,
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

type FetchImpl = typeof fetch;

interface GitHubClientOptions {
  token: string;
  repo: string;
  fetchImpl?: FetchImpl;
}

function ghHeaders(token: string): Record<string, string> {
  return {
    Authorization: `Bearer ${token}`,
    Accept: "application/vnd.github+json",
    "X-GitHub-Api-Version": "2022-11-28",
    "User-Agent": "ado-aw-executor-e2e",
  };
}

/** Return the number of an open issue with this exact title, if one exists. */
export async function findOpenIssueByTitle(
  opts: GitHubClientOptions,
  title: string,
): Promise<number | undefined> {
  const fetchImpl = opts.fetchImpl ?? fetch;
  const q = `repo:${opts.repo} is:issue is:open in:title ${JSON.stringify(title)}`;
  // GitHub search does partial-phrase matching, so many open issues can share
  // the title's words. Page at 100 (scoped to repo + is:open + in:title, so
  // this comfortably covers the expected scale) to avoid the exact-match
  // .find() missing an existing issue and filing a duplicate.
  const url = `https://api.github.com/search/issues?q=${encodeURIComponent(q)}&per_page=100`;
  const res = await fetchImpl(url, { headers: ghHeaders(opts.token) });
  if (!res.ok) throw new Error(`GitHub search failed: HTTP ${res.status}`);
  const json = (await res.json()) as { items?: { number: number; title: string }[] };
  return json.items?.find((i) => i.title === title)?.number;
}

export async function createGitHubIssue(
  opts: GitHubClientOptions,
  title: string,
  body: string,
  labels: string[],
): Promise<string> {
  const fetchImpl = opts.fetchImpl ?? fetch;
  const url = `https://api.github.com/repos/${opts.repo}/issues`;
  const res = await fetchImpl(url, {
    method: "POST",
    headers: { ...ghHeaders(opts.token), "Content-Type": "application/json" },
    body: JSON.stringify({ title, body, labels }),
  });
  if (!res.ok) {
    const text = await res.text().catch(() => "");
    throw new Error(`GitHub create issue failed: HTTP ${res.status}: ${text}`);
  }
  const json = (await res.json()) as { html_url?: string };
  return json.html_url ?? "(created)";
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
  const title = buildIssueTitle(failed);
  const existing = await findOpenIssueByTitle(opts, title);
  if (existing !== undefined) {
    log(`open issue #${existing} already tracks this failure signature; skipping`);
    return { filed: false, reason: `deduped to #${existing}` };
  }
  const url = await createGitHubIssue(opts, title, renderIssueBody(results, env), env.labels);
  log(`filed GitHub issue: ${url}`);
  return { filed: true, url };
}
