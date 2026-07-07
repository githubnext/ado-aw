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
 * Trim a pipeline env value, treating an UNEXPANDED ADO macro (e.g. the literal
 * `$(EXECUTOR_E2E_ISSUE_REPO)`) as absent. ADO passes a `$(VAR)` reference
 * through verbatim when VAR is undefined, so without this guard an unset
 * override would be used as a bogus repo slug instead of falling back to the
 * default.
 */
function cleanVar(raw: string | undefined): string | undefined {
  const value = raw?.trim();
  if (!value || /^\$\(.*\)$/.test(value)) return undefined;
  return value;
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

/** Default per-request timeout for GitHub API calls, matching AdoRest's 30s. */
const DEFAULT_GITHUB_TIMEOUT_MS = 30_000;

interface GitHubClientOptions {
  token: string;
  repo: string;
  fetchImpl?: FetchImpl;
  /** Per-request timeout in ms (defaults to DEFAULT_GITHUB_TIMEOUT_MS). */
  timeoutMs?: number;
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
  const res = await fetchImpl(url, {
    headers: ghHeaders(opts.token),
    // Bound every GitHub call so a hung response can't stall main() indefinitely
    // after all scenarios complete and burn the ADO job's wall-clock limit.
    signal: AbortSignal.timeout(opts.timeoutMs ?? DEFAULT_GITHUB_TIMEOUT_MS),
  });
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
    signal: AbortSignal.timeout(opts.timeoutMs ?? DEFAULT_GITHUB_TIMEOUT_MS),
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
 * On a GitHub auth/permission failure (401/403), probe `GET /user` to report
 * exactly what went wrong instead of leaving the operator to guess. Turns an
 * opaque "HTTP 403" into an actionable line naming the target repo, the
 * authenticated login (or "token invalid/revoked" on 401), and the token's
 * accepted permissions. Best-effort: never throws.
 */
export async function diagnoseGitHubAuthFailure(
  opts: GitHubClientOptions,
  status: number,
  log: (msg: string) => void,
): Promise<void> {
  if (status !== 401 && status !== 403) return;
  const fetchImpl = opts.fetchImpl ?? fetch;
  try {
    const res = await fetchImpl("https://api.github.com/user", {
      headers: ghHeaders(opts.token),
      signal: AbortSignal.timeout(opts.timeoutMs ?? DEFAULT_GITHUB_TIMEOUT_MS),
    });
    const accepted = res.headers.get("x-accepted-github-permissions") ?? "(none reported)";
    if (res.status === 401) {
      log(
        `GitHub token diagnosis: HTTP 401 from /user — the token is invalid, expired, or REVOKED ` +
          `(GitHub auto-revokes tokens shared in plaintext). Generate a fresh token. Target repo: ${opts.repo}.`,
      );
      return;
    }
    if (res.ok) {
      const user = (await res.json()) as { login?: string };
      log(
        `GitHub token diagnosis: authenticated as '${user.login ?? "?"}' but got HTTP ${status} filing to ` +
          `'${opts.repo}'. The token authenticates but lacks Issues:write on that repo (or, for a fine-grained ` +
          `PAT, its resource-owner/repository-access does not include it). Accepted perms: ${accepted}.`,
      );
      return;
    }
    log(
      `GitHub token diagnosis: HTTP ${status} filing to '${opts.repo}'; /user probe returned ${res.status}. ` +
        `Check the token's Issues:write permission and repository access.`,
    );
  } catch (err) {
    log(`GitHub token diagnosis probe failed: ${err instanceof Error ? err.message : String(err)}`);
  }
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

/** Extract a trailing "HTTP <status>" code from a thrown GitHub client error. */
function statusFromError(err: unknown): number | undefined {
  const message = err instanceof Error ? err.message : String(err);
  const match = message.match(/HTTP (\d{3})/);
  return match ? Number(match[1]) : undefined;
}
