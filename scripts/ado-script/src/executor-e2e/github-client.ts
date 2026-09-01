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

async function githubJson<T>(
  opts: GitHubClientOptions,
  method: string,
  path: string,
  payload?: unknown,
): Promise<T> {
  const res = await ghFetch(opts)(`https://api.github.com${path}`, {
    method,
    headers: {
      ...ghHeaders(opts.token),
      ...(payload === undefined ? {} : { "Content-Type": "application/json" }),
    },
    body: payload === undefined ? undefined : JSON.stringify(payload),
    signal: ghSignal(opts),
  });
  if (!res.ok) {
    const text = await res.text().catch(() => "");
    throw new Error(`GitHub ${method} ${path} failed: HTTP ${res.status}: ${text}`);
  }
  if (res.status === 204) return undefined as T;
  return (await res.json()) as T;
}

function repoPath(opts: GitHubClientOptions): string {
  const { owner, name } = splitRepo(opts.repo);
  return `/repos/${encodeURIComponent(owner)}/${encodeURIComponent(name)}`;
}

export interface GraphQLResponse<T> {
  data?: T;
  errors?: { message?: string; type?: string }[];
}

/** Execute a GitHub GraphQL request and surface product errors verbatim. */
export async function githubGraphql<T>(
  opts: GitHubClientOptions,
  query: string,
  variables: Record<string, unknown> = {},
): Promise<T> {
  const json = await githubJson<GraphQLResponse<T>>(opts, "POST", "/graphql", {
    query,
    variables,
  });
  if (json.errors?.length) {
    throw new Error(
      `GitHub GraphQL failed: ${json.errors.map((e) => e.message ?? e.type ?? "unknown error").join("; ")}`,
    );
  }
  if (json.data === undefined) throw new Error("GitHub GraphQL response omitted data");
  return json.data;
}

/**
 * Probe a preview GraphQL field without mutating repository state.
 *
 * Callers turn a false result into SkipError. Once a field exists, later
 * product/permission errors remain scenario failures rather than skips.
 */
export async function supportsGraphqlField(
  opts: GitHubClientOptions,
  typeName: string,
  fieldName: string,
): Promise<boolean> {
  const data = await githubGraphql<{
    __type?: { fields?: { name?: string }[] | null } | null;
  }>(
    opts,
    `query($type: String!) {
      __type(name: $type) {
        fields { name }
      }
    }`,
    { type: typeName },
  );
  return Boolean(data.__type?.fields?.some((field) => field.name === fieldName));
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
  nodeId?: string;
  title: string;
  body: string | null;
  state: string;
  stateReason?: string | null;
  labels: string[];
  assignees?: string[];
  milestone?: { number: number; title: string } | null;
  /** Native issue type name, or undefined when the issue has no type. */
  type?: string;
}

interface RawIssue {
  number?: number;
  node_id?: string;
  title?: string;
  body?: string | null;
  state?: string;
  state_reason?: string | null;
  labels?: (string | { name?: string })[];
  assignees?: { login?: string }[];
  milestone?: { number?: number; title?: string } | null;
  type?: { name?: string } | null;
}

function toIssue(raw: RawIssue): GitHubIssue {
  return {
    number: typeof raw.number === "number" ? raw.number : 0,
    nodeId: raw.node_id,
    title: raw.title ?? "",
    body: raw.body ?? null,
    state: raw.state ?? "",
    stateReason: raw.state_reason,
    labels: (raw.labels ?? [])
      .map((l) => (typeof l === "string" ? l : (l.name ?? "")))
      .filter((l) => l.length > 0),
    assignees: (raw.assignees ?? []).map((a) => a.login ?? "").filter((a) => a.length > 0),
    milestone:
      raw.milestone?.number !== undefined && raw.milestone.title !== undefined
        ? { number: raw.milestone.number, title: raw.milestone.title }
        : null,
    type: raw.type?.name ?? undefined,
  };
}

export interface GitHubIssueComment {
  id: number;
  nodeId: string;
  body: string;
  user: string;
}

export async function listIssueComments(
  opts: GitHubClientOptions,
  issueNumber: number,
): Promise<GitHubIssueComment[]> {
  const comments = await githubJson<
    { id?: number; node_id?: string; body?: string; user?: { login?: string } }[]
  >(opts, "GET", `${repoPath(opts)}/issues/${issueNumber}/comments?per_page=100`);
  return comments
    .filter((c) => typeof c.id === "number" && typeof c.node_id === "string")
    .map((c) => ({
      id: c.id!,
      nodeId: c.node_id!,
      body: c.body ?? "",
      user: c.user?.login ?? "",
    }));
}

export async function createIssueComment(
  opts: GitHubClientOptions,
  issueNumber: number,
  body: string,
): Promise<GitHubIssueComment> {
  const comment = await githubJson<{
    id: number;
    node_id: string;
    body?: string;
    user?: { login?: string };
  }>(opts, "POST", `${repoPath(opts)}/issues/${issueNumber}/comments`, { body });
  return {
    id: comment.id,
    nodeId: comment.node_id,
    body: comment.body ?? "",
    user: comment.user?.login ?? "",
  };
}

export async function deleteIssueComment(
  opts: GitHubClientOptions,
  commentId: number,
): Promise<void> {
  await githubJson<void>(opts, "DELETE", `${repoPath(opts)}/issues/comments/${commentId}`);
}

export async function createRepoLabel(
  opts: GitHubClientOptions,
  name: string,
  color = "5319e7",
): Promise<void> {
  const path = `${repoPath(opts)}/labels`;
  const res = await ghFetch(opts)(`https://api.github.com${path}`, {
    method: "POST",
    headers: { ...ghHeaders(opts.token), "Content-Type": "application/json" },
    body: JSON.stringify({
      name,
      color,
      description: "Temporary ado-aw executor E2E label",
    }),
    signal: ghSignal(opts),
  });
  if (res.ok) return;
  const text = await res.text().catch(() => "");
  // A same-build job retry may encounter the deterministic label left by the
  // interrupted attempt. Treat that exact GitHub conflict as "already set up".
  if (res.status === 422 && /already_exists|already exists/i.test(text)) return;
  throw new Error(`GitHub POST ${path} failed: HTTP ${res.status}: ${text}`);
}

export async function deleteRepoLabel(
  opts: GitHubClientOptions,
  name: string,
): Promise<void> {
  await githubJson<void>(
    opts,
    "DELETE",
    `${repoPath(opts)}/labels/${encodeURIComponent(name)}`,
  );
}

export interface GitHubMilestone {
  number: number;
  title: string;
}

export async function createMilestone(
  opts: GitHubClientOptions,
  title: string,
): Promise<GitHubMilestone> {
  const path = `${repoPath(opts)}/milestones`;
  const res = await ghFetch(opts)(`https://api.github.com${path}`, {
    method: "POST",
    headers: { ...ghHeaders(opts.token), "Content-Type": "application/json" },
    body: JSON.stringify({
      title,
      description: "Temporary ado-aw executor E2E milestone",
    }),
    signal: ghSignal(opts),
  });
  if (res.ok) return (await res.json()) as GitHubMilestone;
  const text = await res.text().catch(() => "");
  if (res.status === 422 && /already_exists|already exists/i.test(text)) {
    const milestones = await githubJson<GitHubMilestone[]>(
      opts,
      "GET",
      `${path}?state=all&per_page=100`,
    );
    const existing = milestones.find((milestone) => milestone.title === title);
    if (existing) return existing;
  }
  throw new Error(`GitHub POST ${path} failed: HTTP ${res.status}: ${text}`);
}

export async function deleteMilestone(
  opts: GitHubClientOptions,
  milestoneNumber: number,
): Promise<void> {
  await githubJson<void>(
    opts,
    "DELETE",
    `${repoPath(opts)}/milestones/${milestoneNumber}`,
  );
}

export async function getAuthenticatedUser(opts: GitHubClientOptions): Promise<string> {
  const user = await githubJson<{ login?: string }>(opts, "GET", "/user");
  if (!user.login) throw new Error("GitHub /user response omitted login");
  return user.login;
}

export async function addIssueAssignees(
  opts: GitHubClientOptions,
  issueNumber: number,
  assignees: string[],
): Promise<void> {
  await githubJson(opts, "POST", `${repoPath(opts)}/issues/${issueNumber}/assignees`, {
    assignees,
  });
}

export async function removeIssueAssignees(
  opts: GitHubClientOptions,
  issueNumber: number,
  assignees: string[],
): Promise<void> {
  await githubJson(opts, "DELETE", `${repoPath(opts)}/issues/${issueNumber}/assignees`, {
    assignees,
  });
}

export interface GitHubIssueField {
  id: string;
  name: string;
  type: string;
  options: { id: string; name: string }[];
}

export interface GitHubIssueFieldValue {
  fieldId: string;
  fieldName: string;
  fieldType: string;
  valueType: string;
  value: string | number;
}

/** Discover repository issue fields using the same preview surface as the executor. */
export async function listRepositoryIssueFields(
  opts: GitHubClientOptions,
): Promise<GitHubIssueField[]> {
  const { owner, name } = splitRepo(opts.repo);
  const data = await githubGraphql<{
    repository?: {
      issueFields?: {
        nodes?: {
          id?: string;
          name?: string;
          __typename?: string;
          options?: { id?: string; name?: string }[];
        }[];
      };
    };
  }>(
    opts,
    `query($owner: String!, $repo: String!) {
      repository(owner: $owner, name: $repo) {
        issueFields(first: 100) {
          nodes {
            __typename
            ... on IssueFieldText { id name }
            ... on IssueFieldNumber { id name }
            ... on IssueFieldDate { id name }
            ... on IssueFieldSingleSelect { id name options { id name } }
            ... on IssueFieldMultiSelect { id name options { id name } }
          }
        }
      }
    }`,
    { owner, repo: name },
  );
  return (data.repository?.issueFields?.nodes ?? [])
    .filter((field) => typeof field.id === "string" && typeof field.name === "string")
    .map((field) => ({
      id: field.id!,
      name: field.name!,
      type: field.__typename ?? "",
      options: (field.options ?? [])
        .filter((option) => typeof option.id === "string" && typeof option.name === "string")
        .map((option) => ({ id: option.id!, name: option.name! })),
    }));
}

/** Read one persisted repository-defined field value from an issue. */
export async function getIssueFieldValue(
  opts: GitHubClientOptions,
  issueNumber: number,
  fieldId: string,
): Promise<GitHubIssueFieldValue | undefined> {
  const { owner, name } = splitRepo(opts.repo);
  const data = await githubGraphql<{
    repository?: {
      issue?: {
        issueFieldValues?: {
          nodes?: {
            __typename?: string;
            value?: string | number;
            name?: string;
            field?: {
              id?: string;
              name?: string;
              __typename?: string;
            } | null;
          }[];
        };
      } | null;
    };
  }>(
    opts,
    `query($owner: String!, $repo: String!, $number: Int!) {
      repository(owner: $owner, name: $repo) {
        issue(number: $number) {
          issueFieldValues(first: 100) {
            nodes {
              __typename
              ... on IssueFieldTextValue {
                value
                field { __typename ... on IssueFieldText { id name } }
              }
              ... on IssueFieldNumberValue {
                value
                field { __typename ... on IssueFieldNumber { id name } }
              }
              ... on IssueFieldDateValue {
                value
                field { __typename ... on IssueFieldDate { id name } }
              }
              ... on IssueFieldSingleSelectValue {
                name
                field { __typename ... on IssueFieldSingleSelect { id name } }
              }
            }
          }
        }
      }
    }`,
    { owner, repo: name, number: issueNumber },
  );
  const node = (data.repository?.issue?.issueFieldValues?.nodes ?? []).find(
    (candidate) => candidate.field?.id === fieldId,
  );
  if (!node) return undefined;

  const fieldName = node.field?.name;
  const fieldType = node.field?.__typename;
  const valueType = node.__typename;
  if (!fieldName || !fieldType || !valueType) {
    throw new Error(`GitHub issue field value '${fieldId}' omitted type or field metadata`);
  }

  const value = valueType === "IssueFieldSingleSelectValue" ? node.name : node.value;
  if (typeof value !== "string" && typeof value !== "number") {
    throw new Error(`GitHub issue field value '${fieldId}' omitted its persisted value`);
  }
  return { fieldId, fieldName, fieldType, valueType, value };
}

export async function getCommentMinimization(
  opts: GitHubClientOptions,
  nodeId: string,
): Promise<{ isMinimized: boolean; reason?: string | null }> {
  const data = await githubGraphql<{
    node?: { isMinimized?: boolean; minimizedReason?: string | null } | null;
  }>(
    opts,
    `query($id: ID!) {
      node(id: $id) {
        ... on Minimizable {
          isMinimized
          minimizedReason
        }
      }
    }`,
    { id: nodeId },
  );
  return {
    isMinimized: data.node?.isMinimized === true,
    reason: data.node?.minimizedReason,
  };
}

export async function getSubIssueParent(
  opts: GitHubClientOptions,
  issueNumber: number,
): Promise<number | undefined> {
  const { owner, name } = splitRepo(opts.repo);
  const data = await githubGraphql<{
    repository?: { issue?: { parent?: { number?: number } | null } | null };
  }>(
    opts,
    `query($owner: String!, $repo: String!, $number: Int!) {
      repository(owner: $owner, name: $repo) {
        issue(number: $number) { parent { number } }
      }
    }`,
    { owner, repo: name, number: issueNumber },
  );
  return data.repository?.issue?.parent?.number;
}

export async function unlinkSubIssue(
  opts: GitHubClientOptions,
  parentIssueNumber: number,
  subIssueNumber: number,
): Promise<void> {
  const [parent, sub] = await Promise.all([
    getIssue(opts, parentIssueNumber),
    getIssue(opts, subIssueNumber),
  ]);
  if (!parent?.nodeId || !sub?.nodeId) {
    throw new Error("cannot unlink sub-issue: GitHub issue node IDs are unavailable");
  }
  await githubGraphql(
    opts,
    `mutation($parentId: ID!, $subIssueId: ID!) {
      removeSubIssue(input: { issueId: $parentId, subIssueId: $subIssueId }) {
        clientMutationId
      }
    }`,
    { parentId: parent.nodeId, subIssueId: sub.nodeId },
  );
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
