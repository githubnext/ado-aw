/**
 * The one GitHub REST client used by the executor E2E harness.
 *
 * Three GitHub API clients exist in this repository, and they are deliberately
 * separate because they sit on different sides of a trust or language boundary:
 *
 *  1. `src/safe_outputs/create_github_issue.rs` + `set_github_issue_type.rs`
 *     (Rust / `reqwest`) — the shipped executor, i.e. the code under test.
 *  2. `scripts/ado-script/src/github-app-token/index.ts` — a shipped bundle that
 *     mints a GitHub App installation token inside the Agent/Detection jobs.
 *  3. **This module** — test-harness only, used by both the harness's own
 *     failure reporter (`github-issue.ts`) and the GitHub issue scenarios
 *     (`scenarios/github-issue.ts`).
 *
 * Scenario code must reuse this module rather than adding a fourth ad-hoc
 * `fetch` wrapper.
 *
 * Test-harness module; not shipped in `ado-script.zip`.
 */

export type FetchImpl = typeof fetch;

/** Default per-request timeout for GitHub API calls, matching AdoRest's 30s. */
export const DEFAULT_GITHUB_TIMEOUT_MS = 30_000;

export interface GitHubClientOptions {
  token: string;
  /** `owner/repo` slug. */
  repo: string;
  fetchImpl?: FetchImpl;
  /** Per-request timeout in ms (defaults to DEFAULT_GITHUB_TIMEOUT_MS). */
  timeoutMs?: number;
}

export function ghHeaders(token: string): Record<string, string> {
  return {
    Authorization: `Bearer ${token}`,
    Accept: "application/vnd.github+json",
    "X-GitHub-Api-Version": "2022-11-28",
    "User-Agent": "ado-aw-executor-e2e",
  };
}

function ghFetch(opts: GitHubClientOptions): FetchImpl {
  return opts.fetchImpl ?? fetch;
}

function ghSignal(opts: GitHubClientOptions): AbortSignal {
  // Bound every GitHub call so a hung response can't stall the harness
  // indefinitely and burn the ADO job's wall-clock limit.
  return AbortSignal.timeout(opts.timeoutMs ?? DEFAULT_GITHUB_TIMEOUT_MS);
}

/**
 * Trim a pipeline env value, treating an UNEXPANDED ADO macro (e.g. the literal
 * `$(EXECUTOR_E2E_ISSUE_REPO)`) as absent. ADO passes a `$(VAR)` reference
 * through verbatim when VAR is undefined, so without this guard an unset
 * override would be used as a bogus repo slug instead of falling back to the
 * default.
 */
export function cleanVar(raw: string | undefined): string | undefined {
  const value = raw?.trim();
  if (!value || /^\$\(.*\)$/.test(value)) return undefined;
  return value;
}

/** Split an `owner/repo` slug; throws when it is not in that form. */
export function splitRepo(repo: string): { owner: string; name: string } {
  const [owner, name, ...rest] = repo.split("/");
  if (!owner || !name || rest.length > 0) {
    throw new Error(`expected an 'owner/repo' slug, got '${repo}'`);
  }
  return { owner, name };
}

/** Return the number of an open issue with this exact title, if one exists. */
export async function findOpenIssueByTitle(
  opts: GitHubClientOptions,
  title: string,
): Promise<number | undefined> {
  const q = `repo:${opts.repo} is:issue is:open in:title ${JSON.stringify(title)}`;
  // GitHub search does partial-phrase matching, so many open issues can share
  // the title's words. Page at 100 (scoped to repo + is:open + in:title, so
  // this comfortably covers the expected scale) to avoid the exact-match
  // .find() missing an existing issue and filing a duplicate.
  const url = `https://api.github.com/search/issues?q=${encodeURIComponent(q)}&per_page=100`;
  const res = await ghFetch(opts)(url, {
    headers: ghHeaders(opts.token),
    signal: ghSignal(opts),
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
  const url = `https://api.github.com/repos/${opts.repo}/issues`;
  const res = await ghFetch(opts)(url, {
    method: "POST",
    headers: { ...ghHeaders(opts.token), "Content-Type": "application/json" },
    body: JSON.stringify({ title, body, labels }),
    signal: ghSignal(opts),
  });
  if (!res.ok) {
    const text = await res.text().catch(() => "");
    throw new Error(`GitHub create issue failed: HTTP ${res.status}: ${text}`);
  }
  const json = (await res.json()) as { html_url?: string };
  return json.html_url ?? "(created)";
}

/** One GitHub issue, reduced to the fields the scenarios assert on. */
export interface GitHubIssue {
  number: number;
  title: string;
  body: string | null;
  state: string;
  labels: string[];
  /** Native issue type name, or undefined when the issue has no type. */
  type?: string;
}

interface RawIssue {
  number?: number;
  title?: string;
  body?: string | null;
  state?: string;
  labels?: (string | { name?: string })[];
  type?: { name?: string } | null;
}

function toIssue(raw: RawIssue): GitHubIssue {
  return {
    number: typeof raw.number === "number" ? raw.number : 0,
    title: raw.title ?? "",
    body: raw.body ?? null,
    state: raw.state ?? "",
    labels: (raw.labels ?? [])
      .map((l) => (typeof l === "string" ? l : (l.name ?? "")))
      .filter((l) => l.length > 0),
    type: raw.type?.name ?? undefined,
  };
}

/** Fetch a single issue. Returns undefined on 404. */
export async function getIssue(
  opts: GitHubClientOptions,
  issueNumber: number,
): Promise<GitHubIssue | undefined> {
  const url = `https://api.github.com/repos/${opts.repo}/issues/${issueNumber}`;
  const res = await ghFetch(opts)(url, {
    headers: ghHeaders(opts.token),
    signal: ghSignal(opts),
  });
  if (res.status === 404) return undefined;
  if (!res.ok) {
    const text = await res.text().catch(() => "");
    throw new Error(`GitHub get issue #${issueNumber} failed: HTTP ${res.status}: ${text}`);
  }
  return toIssue((await res.json()) as RawIssue);
}

/**
 * PATCH an issue. Returns the HTTP status rather than throwing so callers can
 * use it as a capability probe (e.g. "does this repo accept a type clear?").
 */
export async function patchIssue(
  opts: GitHubClientOptions,
  issueNumber: number,
  payload: Record<string, unknown>,
): Promise<{ ok: boolean; status: number; body: string }> {
  const url = `https://api.github.com/repos/${opts.repo}/issues/${issueNumber}`;
  const res = await ghFetch(opts)(url, {
    method: "PATCH",
    headers: { ...ghHeaders(opts.token), "Content-Type": "application/json" },
    body: JSON.stringify(payload),
    signal: ghSignal(opts),
  });
  const body = await res.text().catch(() => "");
  return { ok: res.ok, status: res.status, body };
}

/**
 * Close an issue as `not_planned`.
 *
 * GitHub has no REST endpoint to DELETE an issue, so "close" is the strongest
 * teardown available to the harness — see the close-not-delete note in
 * `tests/executor-e2e/README.md`.
 */
export async function closeIssue(
  opts: GitHubClientOptions,
  issueNumber: number,
): Promise<void> {
  const res = await patchIssue(opts, issueNumber, {
    state: "closed",
    state_reason: "not_planned",
  });
  if (!res.ok) {
    throw new Error(`GitHub close issue #${issueNumber} failed: HTTP ${res.status}: ${res.body}`);
  }
}

/**
 * List an organisation's native issue types.
 *
 * Issue types are an **organisation-level** construct: there is no user-account
 * equivalent of `GET /orgs/{org}/issue-types`. A user-owned repository
 * therefore always yields an empty list, which callers must treat as "skip",
 * not "fail".
 */
export async function listOrgIssueTypes(
  opts: GitHubClientOptions,
  owner: string,
): Promise<string[]> {
  const url = `https://api.github.com/orgs/${encodeURIComponent(owner)}/issue-types`;
  const res = await ghFetch(opts)(url, {
    headers: ghHeaders(opts.token),
    signal: ghSignal(opts),
  });
  // 404: not an org (or the feature is unavailable). 403: the token cannot read
  // org metadata. Both mean "no discoverable named type", never a hard failure.
  if (res.status === 404 || res.status === 403) return [];
  if (!res.ok) {
    throw new Error(`GitHub list issue types for '${owner}' failed: HTTP ${res.status}`);
  }
  const json = (await res.json()) as { name?: string }[] | { message?: string };
  if (!Array.isArray(json)) return [];
  return json.map((t) => t.name ?? "").filter((n) => n.length > 0);
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
  try {
    const res = await ghFetch(opts)("https://api.github.com/user", {
      headers: ghHeaders(opts.token),
      signal: ghSignal(opts),
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

/** Extract a trailing "HTTP <status>" code from a thrown GitHub client error. */
export function statusFromError(err: unknown): number | undefined {
  const message = err instanceof Error ? err.message : String(err);
  const match = message.match(/HTTP (\d{3})/);
  return match ? Number(match[1]) : undefined;
}
