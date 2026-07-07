/**
 * Minimal, self-contained Azure DevOps REST client for the deterministic
 * executor E2E harness.
 *
 * Uses the global `fetch` (Node 20+) with Basic auth (empty user + token),
 * matching how the `ado-aw` Rust executor authenticates
 * (`reqwest ... .basic_auth("", Some(token))`). Endpoints and api-versions are
 * chosen to line up with the executors under test so setup/assert/cleanup hit
 * the same surfaces the executor writes to.
 *
 * This is a **test harness** module and does not ship in `ado-script.zip`.
 */

export interface AdoRestOptions {
  orgUrl: string;
  project: string;
  token: string;
  log?: (msg: string) => void;
}

interface RequestOptions {
  method?: string;
  /** JSON body (serialised) unless `rawBody`/`contentType` override it. */
  body?: unknown;
  /** Raw request body string (used for JSON-Patch payloads). */
  rawBody?: string;
  contentType?: string;
  /** Treat 404 as `undefined` instead of throwing. */
  allow404?: boolean;
  accept?: string;
  /** Extra request headers (e.g. `If-Match` for a conditional wiki PUT). */
  headers?: Record<string, string>;
}

export class AdoRest {
  private readonly base: string;
  private readonly project: string;
  private readonly authHeader: string;
  private readonly log: (msg: string) => void;
  private readonly timeoutMs: number;

  constructor(opts: AdoRestOptions) {
    this.base = opts.orgUrl.replace(/\/+$/, "");
    this.project = opts.project;
    this.authHeader = "Basic " + Buffer.from(":" + opts.token).toString("base64");
    this.log = opts.log ?? (() => {});
    this.timeoutMs = Number(process.env.EXECUTOR_E2E_REST_TIMEOUT_MS) || 30_000;
  }

  /** Percent-encode a single path segment (project names may contain spaces). */
  private static seg(value: string): string {
    return encodeURIComponent(value);
  }

  /**
   * Centralised fetch: injects the ADO auth header and a per-request timeout
   * (AbortSignal) so a single hung endpoint can never block the whole suite.
   * All REST access — including {@link getWikiPage}, which needs the raw
   * Response for its ETag — goes through here, so auth stays in one place.
   */
  private async authedFetch(
    path: string,
    init: { method?: string; headers?: Record<string, string>; body?: string } = {},
  ): Promise<Response> {
    const url = path.startsWith("http") ? path : `${this.base}/${path}`;
    return fetch(url, {
      method: init.method ?? "GET",
      headers: { Authorization: this.authHeader, ...init.headers },
      body: init.body,
      signal: AbortSignal.timeout(this.timeoutMs),
    });
  }

  private async request<T>(path: string, opts: RequestOptions = {}): Promise<T | undefined> {
    const headers: Record<string, string> = { Accept: opts.accept ?? "application/json", ...opts.headers };
    let body: string | undefined;
    if (opts.rawBody !== undefined) {
      body = opts.rawBody;
      if (opts.contentType) headers["Content-Type"] = opts.contentType;
    } else if (opts.body !== undefined) {
      body = JSON.stringify(opts.body);
      headers["Content-Type"] = opts.contentType ?? "application/json";
    }

    const res = await this.authedFetch(path, { method: opts.method ?? "GET", headers, body });
    if (res.status === 404 && opts.allow404) return undefined;
    if (!res.ok) {
      const text = await res.text().catch(() => "<no body>");
      throw new Error(`ADO ${opts.method ?? "GET"} ${path} -> HTTP ${res.status}: ${text}`);
    }
    if (res.status === 204) return undefined;
    const text = await res.text();
    if (!text) return undefined;
    const ct = res.headers.get("content-type") ?? "";
    if (ct.includes("application/json")) return JSON.parse(text) as T;
    // Non-empty, non-JSON body (e.g. an XML/HTML error page) — surface it
    // loudly rather than silently casting garbage to T.
    throw new Error(
      `ADO ${opts.method ?? "GET"} ${path} returned unexpected content-type '${ct}': ${text.slice(0, 200)}`,
    );
  }

  private projPath(rest: string): string {
    return `${AdoRest.seg(this.project)}/${rest}`;
  }

  // ---- Connection / identity -------------------------------------------

  /** Resolve the collection host base (org URL trimmed). */
  get orgBase(): string {
    return this.base;
  }

  // ---- Work items -------------------------------------------------------

  async createWorkItem(
    type: string,
    fields: Record<string, unknown>,
  ): Promise<{ id: number }> {
    const ops = Object.entries(fields).map(([field, value]) => ({
      op: "add",
      path: `/fields/${field}`,
      value,
    }));
    const path = this.projPath(`_apis/wit/workitems/$${encodeURIComponent(type)}?api-version=7.1`);
    const res = await this.request<{ id: number }>(path, {
      method: "POST",
      rawBody: JSON.stringify(ops),
      contentType: "application/json-patch+json",
    });
    if (!res) throw new Error("createWorkItem returned no body");
    return res;
  }

  async getWorkItem(id: number): Promise<{ id: number; fields: Record<string, unknown> }> {
    const path = this.projPath(`_apis/wit/workitems/${id}?api-version=7.1`);
    const res = await this.request<{ id: number; fields: Record<string, unknown> }>(path);
    if (!res) throw new Error(`getWorkItem(${id}) returned no body`);
    return res;
  }

  async getWorkItemComments(id: number): Promise<{ text: string; id: number }[]> {
    const path = this.projPath(
      `_apis/wit/workItems/${id}/comments?api-version=7.1-preview.4`,
    );
    const res = await this.request<{ comments?: { text: string; id: number }[] }>(path);
    return res?.comments ?? [];
  }

  async getWorkItemRelations(
    id: number,
  ): Promise<{ rel: string; url: string; attributes?: Record<string, unknown> }[]> {
    const path = this.projPath(
      `_apis/wit/workitems/${id}?$expand=relations&api-version=7.1`,
    );
    const res = await this.request<{
      relations?: { rel: string; url: string; attributes?: Record<string, unknown> }[];
    }>(path);
    return res?.relations ?? [];
  }

  /** Delete a work item (moves it to the recycle bin). Best-effort. */
  async deleteWorkItem(id: number): Promise<void> {
    const path = this.projPath(`_apis/wit/workitems/${id}?api-version=7.1`);
    await this.request(path, { method: "DELETE", allow404: true });
  }

  // ---- Git: repositories, refs, tags -----------------------------------

  async getRepository(repo: string): Promise<{ id: string; defaultBranch?: string }> {
    const path = this.projPath(
      `_apis/git/repositories/${AdoRest.seg(repo)}?api-version=7.1`,
    );
    const res = await this.request<{ id: string; defaultBranch?: string }>(path);
    if (!res) throw new Error(`repository '${repo}' not found`);
    return res;
  }

  /** Resolve the object id (commit sha) a ref currently points at. */
  async getRefObjectId(repo: string, refFilter: string): Promise<string | undefined> {
    const path = this.projPath(
      `_apis/git/repositories/${AdoRest.seg(repo)}/refs?filter=${encodeURIComponent(refFilter)}&api-version=7.1`,
    );
    const res = await this.request<{ value?: { name: string; objectId: string }[] }>(path);
    // `filter` is a prefix match (heads/main also matches heads/main-foo), so
    // select the exact ref by name rather than trusting the first result.
    const fullName = `refs/${refFilter}`;
    return res?.value?.find((r) => r.name === fullName)?.objectId;
  }

  /** Delete a ref (branch or tag) by setting its newObjectId to zeros. */
  async deleteRef(repo: string, refName: string): Promise<void> {
    const oldId = await this.getRefObjectId(repo, refName.replace(/^refs\//, ""));
    if (!oldId) return;
    const path = this.projPath(
      `_apis/git/repositories/${AdoRest.seg(repo)}/refs?api-version=7.1`,
    );
    await this.request(path, {
      method: "POST",
      body: [
        {
          name: refName.startsWith("refs/") ? refName : `refs/${refName}`,
          oldObjectId: oldId,
          newObjectId: "0000000000000000000000000000000000000000",
        },
      ],
      allow404: true,
    });
  }

  /**
   * Create a branch AND a single commit on it in one Push, adding one file.
   * Returns the new commit id. Gives PR scenarios a real diff vs. the base.
   */
  async pushAddFileBranch(
    repo: string,
    branchName: string,
    baseCommitId: string,
    filePath: string,
    content: string,
    comment: string,
  ): Promise<string> {
    const path = this.projPath(
      `_apis/git/repositories/${AdoRest.seg(repo)}/pushes?api-version=7.1`,
    );
    const res = await this.request<{ commits?: { commitId: string }[] }>(path, {
      method: "POST",
      body: {
        // Creating a NEW branch: oldObjectId must be zeros (the ref does not
        // exist yet) and the commit must declare parents:[baseCommitId] so it
        // is a child of the base — mirrors the Rust executor's push in
        // src/safe_outputs/create_pull_request.rs.
        refUpdates: [
          {
            name: branchName.startsWith("refs/") ? branchName : `refs/heads/${branchName}`,
            oldObjectId: "0000000000000000000000000000000000000000",
          },
        ],
        commits: [
          {
            comment,
            parents: [baseCommitId],
            changes: [
              {
                changeType: "add",
                item: { path: filePath.startsWith("/") ? filePath : `/${filePath}` },
                newContent: { content, contentType: "rawtext" },
              },
            ],
          },
        ],
      },
    });
    const commitId = res?.commits?.[0]?.commitId;
    if (!commitId) throw new Error("pushAddFileBranch returned no commit id");
    return commitId;
  }

  // ---- Git: pull requests & threads ------------------------------------

  async createPullRequest(
    repo: string,
    sourceRef: string,
    targetRef: string,
    title: string,
    description: string,
  ): Promise<{ pullRequestId: number }> {
    const path = this.projPath(
      `_apis/git/repositories/${AdoRest.seg(repo)}/pullrequests?api-version=7.1`,
    );
    const res = await this.request<{ pullRequestId: number }>(path, {
      method: "POST",
      body: {
        sourceRefName: sourceRef.startsWith("refs/") ? sourceRef : `refs/heads/${sourceRef}`,
        targetRefName: targetRef.startsWith("refs/") ? targetRef : `refs/heads/${targetRef}`,
        title,
        description,
      },
    });
    if (!res) throw new Error("createPullRequest returned no body");
    return res;
  }

  async getPullRequest(
    repo: string,
    prId: number,
  ): Promise<{ pullRequestId: number; status: string; title: string; description?: string }> {
    const path = this.projPath(
      `_apis/git/repositories/${AdoRest.seg(repo)}/pullRequests/${prId}?api-version=7.1`,
    );
    const res = await this.request<{
      pullRequestId: number;
      status: string;
      title: string;
      description?: string;
    }>(path);
    if (!res) throw new Error(`getPullRequest(${prId}) returned no body`);
    return res;
  }

  async createThread(
    repo: string,
    prId: number,
    content: string,
  ): Promise<{ id: number }> {
    const path = this.projPath(
      `_apis/git/repositories/${AdoRest.seg(repo)}/pullRequests/${prId}/threads?api-version=7.1`,
    );
    const res = await this.request<{ id: number }>(path, {
      method: "POST",
      body: { comments: [{ parentCommentId: 0, content, commentType: 1 }], status: 1 },
    });
    if (!res) throw new Error("createThread returned no body");
    return res;
  }

  async getThread(
    repo: string,
    prId: number,
    threadId: number,
  ): Promise<{ id: number; status?: string | number; comments?: { id: number; content?: string | null }[] }> {
    const path = this.projPath(
      `_apis/git/repositories/${AdoRest.seg(repo)}/pullRequests/${prId}/threads/${threadId}?api-version=7.1`,
    );
    const res = await this.request<{
      id: number;
      status?: string | number;
      comments?: { id: number; content?: string | null }[];
    }>(path);
    if (!res) throw new Error(`getThread(${threadId}) returned no body`);
    return res;
  }

  async listThreads(
    repo: string,
    prId: number,
  ): Promise<{ id: number; comments?: { content?: string }[] }[]> {
    const path = this.projPath(
      `_apis/git/repositories/${AdoRest.seg(repo)}/pullRequests/${prId}/threads?api-version=7.1`,
    );
    const res = await this.request<{ value?: { id: number; comments?: { content?: string }[] }[] }>(
      path,
    );
    return res?.value ?? [];
  }

  async listReviewers(
    repo: string,
    prId: number,
  ): Promise<{ id: string; vote: number; displayName?: string }[]> {
    const path = this.projPath(
      `_apis/git/repositories/${AdoRest.seg(repo)}/pullRequests/${prId}/reviewers?api-version=7.1`,
    );
    const res = await this.request<{
      value?: { id: string; vote: number; displayName?: string }[];
    }>(path);
    return res?.value ?? [];
  }

  /** Abandon a PR (status=abandoned). Best-effort cleanup. */
  async abandonPullRequest(repo: string, prId: number): Promise<void> {
    const path = this.projPath(
      `_apis/git/repositories/${AdoRest.seg(repo)}/pullRequests/${prId}?api-version=7.1`,
    );
    await this.request(path, { method: "PATCH", body: { status: "abandoned" }, allow404: true });
  }

  // ---- Wiki -------------------------------------------------------------

  async listWikis(): Promise<{ name: string; id: string; type?: string }[]> {
    const path = this.projPath(`_apis/wiki/wikis?api-version=7.1`);
    const res = await this.request<{ value?: { name: string; id: string; type?: string }[] }>(
      path,
    );
    return res?.value ?? [];
  }

  async getWikiPage(
    wiki: string,
    pagePath: string,
  ): Promise<{ content?: string; eTag?: string } | undefined> {
    const path = this.projPath(
      `_apis/wiki/wikis/${AdoRest.seg(wiki)}/pages?path=${encodeURIComponent(pagePath)}&includeContent=true&api-version=7.1`,
    );
    // Routed through authedFetch (not request()) because we need the raw
    // Response to read the ETag; auth + timeout stay centralised.
    const res = await this.authedFetch(path, { headers: { Accept: "application/json" } });
    if (res.status === 404) return undefined;
    if (!res.ok) {
      const text = await res.text().catch(() => "");
      throw new Error(`getWikiPage HTTP ${res.status}: ${text}`);
    }
    const eTag = res.headers.get("etag") ?? undefined;
    const json = (await res.json()) as { content?: string };
    return { content: json.content, eTag };
  }

  /**
   * Create or update a wiki page. ADO requires an `If-Match` ETag to overwrite
   * an EXISTING page (it returns HTTP 412 for an unconditional PUT to a page
   * that already exists); creating a new page must omit `If-Match`. Callers
   * updating a page should first {@link getWikiPage} and pass its `eTag`.
   */
  async putWikiPage(
    wiki: string,
    pagePath: string,
    content: string,
    eTag?: string,
  ): Promise<void> {
    const path = this.projPath(
      `_apis/wiki/wikis/${AdoRest.seg(wiki)}/pages?path=${encodeURIComponent(pagePath)}&api-version=7.1`,
    );
    await this.request(path, {
      method: "PUT",
      body: { content },
      headers: eTag ? { "If-Match": eTag } : undefined,
    });
  }

  async deleteWikiPage(wiki: string, pagePath: string): Promise<void> {
    const path = this.projPath(
      `_apis/wiki/wikis/${AdoRest.seg(wiki)}/pages?path=${encodeURIComponent(pagePath)}&api-version=7.1`,
    );
    await this.request(path, { method: "DELETE", allow404: true });
  }

  // ---- Builds -----------------------------------------------------------

  async getBuildTags(buildId: number): Promise<string[]> {
    const path = this.projPath(`_apis/build/builds/${buildId}/tags?api-version=7.1`);
    const res = await this.request<{ value?: string[] } | string[]>(path);
    if (Array.isArray(res)) return res;
    return res?.value ?? [];
  }

  async removeBuildTag(buildId: number, tag: string): Promise<void> {
    const path = this.projPath(
      `_apis/build/builds/${buildId}/tags/${AdoRest.seg(tag)}?api-version=7.1`,
    );
    await this.request(path, { method: "DELETE", allow404: true });
  }

  async getBuild(buildId: number): Promise<{ id: number; status?: string; result?: string }> {
    const path = this.projPath(`_apis/build/builds/${buildId}?api-version=7.1`);
    const res = await this.request<{ id: number; status?: string; result?: string }>(path);
    if (!res) throw new Error(`getBuild(${buildId}) returned no body`);
    return res;
  }

  async cancelBuild(buildId: number): Promise<void> {
    const path = this.projPath(`_apis/build/builds/${buildId}?api-version=7.1`);
    await this.request(path, { method: "PATCH", body: { status: "cancelling" }, allow404: true });
  }
}
