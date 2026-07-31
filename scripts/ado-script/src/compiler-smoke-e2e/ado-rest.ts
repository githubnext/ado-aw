/**
 * Minimal, self-contained Azure DevOps Build REST client for the
 * deterministic compiler-smoke E2E harness.
 *
 * Uses the global `fetch` (Node 20+) with Basic auth (empty user + token) —
 * same posture as `executor-e2e/ado-rest.ts` and `trigger-e2e`. Kept
 * self-contained (rather than importing the sibling harness's client) so
 * this directory has no cross-harness coupling; `fetchImpl`/`sleepImpl` are
 * injectable so every caller is testable without live network access.
 *
 * Test-harness module; not shipped in `ado-script.zip`.
 */
import { redact } from "./process.js";
import { sleep as defaultSleep } from "./process.js";

export interface AdoRestOptions {
  orgUrl: string;
  project: string;
  token: string;
  log?: (msg: string) => void;
  timeoutMs?: number;
  fetchImpl?: typeof fetch;
  sleepImpl?: (ms: number) => Promise<void>;
}

export interface BuildSummary {
  id: number;
  status?: string;
  result?: string;
  definition?: { id?: number };
  sourceBranch?: string;
  sourceVersion?: string;
  queueTime?: string;
  finishTime?: string;
}

export interface ArtifactInfo {
  name: string;
  resource?: { downloadUrl?: string; type?: string };
}

const DEFAULT_ARTIFACT_RETRIES = 5;
const DEFAULT_ARTIFACT_RETRY_DELAY_MS = 5_000;
const DEFAULT_TAG_RETRIES = 5;
const DEFAULT_TAG_RETRY_DELAY_MS = 2_000;

export class AdoRest {
  private readonly base: string;
  private readonly project: string;
  private readonly authHeader: string;
  private readonly log: (msg: string) => void;
  private readonly timeoutMs: number;
  private readonly fetchImpl: typeof fetch;
  private readonly sleepImpl: (ms: number) => Promise<void>;

  constructor(opts: AdoRestOptions) {
    this.base = opts.orgUrl.replace(/\/+$/, "");
    this.project = opts.project;
    this.authHeader = "Basic " + Buffer.from(":" + opts.token).toString("base64");
    this.log = opts.log ?? (() => {});
    this.timeoutMs = opts.timeoutMs ?? 30_000;
    this.fetchImpl = opts.fetchImpl ?? fetch;
    this.sleepImpl = opts.sleepImpl ?? defaultSleep;
  }

  private static seg(value: string): string {
    return encodeURIComponent(value);
  }

  private projPath(rest: string): string {
    return `${this.base}/${AdoRest.seg(this.project)}/${rest}`;
  }

  private async request<T>(
    path: string,
    opts: { method?: string; body?: unknown; allow404?: boolean } = {},
  ): Promise<T | undefined> {
    const headers: Record<string, string> = {
      Authorization: this.authHeader,
      Accept: "application/json",
    };
    let body: string | undefined;
    if (opts.body !== undefined) {
      body = JSON.stringify(opts.body);
      headers["Content-Type"] = "application/json";
    }
    const res = await this.fetchImpl(path, {
      method: opts.method ?? "GET",
      headers,
      body,
      signal: AbortSignal.timeout(this.timeoutMs),
    });
    if (res.status === 404 && opts.allow404) return undefined;
    if (!res.ok) {
      const text = await res.text().catch(() => "<no body>");
      throw new Error(`ADO ${opts.method ?? "GET"} ${path} -> HTTP ${res.status}: ${text}`);
    }
    if (res.status === 204) return undefined;
    const text = await res.text();
    if (!text) return undefined;
    return JSON.parse(text) as T;
  }

  /** Build a human-facing URL for a build (used in the final results table). */
  buildUrl(buildId: number): string {
    return `${this.base}/${AdoRest.seg(this.project)}/_build/results?buildId=${buildId}`;
  }

  /**
   * Confirm the producer build's configured artifact is visible before any
   * source/git work begins. Bounded retry: ADO can take a few seconds to
   * index a just-published artifact.
   */
  async getArtifact(
    buildId: number,
    artifactName: string,
    opts: { retries?: number; retryDelayMs?: number } = {},
  ): Promise<ArtifactInfo> {
    const retries = opts.retries ?? DEFAULT_ARTIFACT_RETRIES;
    const retryDelayMs = opts.retryDelayMs ?? DEFAULT_ARTIFACT_RETRY_DELAY_MS;
    const path = this.projPath(
      `_apis/build/builds/${buildId}/artifacts?artifactName=${AdoRest.seg(artifactName)}&api-version=7.1`,
    );
    let lastErr: unknown;
    for (let attempt = 1; attempt <= retries; attempt++) {
      try {
        const res = await this.request<ArtifactInfo>(path, { allow404: true });
        if (res) return res;
        lastErr = new Error(
          `artifact '${artifactName}' not found on build #${buildId} (attempt ${attempt}/${retries})`,
        );
      } catch (err) {
        lastErr = err;
      }
      if (attempt < retries) {
        this.log(
          `[artifact-visibility] attempt ${attempt}/${retries} failed, retrying in ${retryDelayMs}ms: ${
            (lastErr as Error).message
          }`,
        );
        await this.sleepImpl(retryDelayMs);
      }
    }
    throw new Error(
      `artifact '${artifactName}' not visible on build #${buildId} in project '${this.project}' after ${retries} attempts: ${
        (lastErr as Error)?.message ?? "unknown error"
      }`,
    );
  }

  async getBuild(buildId: number): Promise<BuildSummary> {
    const path = this.projPath(`_apis/build/builds/${buildId}?api-version=7.1`);
    const res = await this.request<BuildSummary>(path);
    if (!res) throw new Error(`getBuild(${buildId}) returned no body`);
    return res;
  }

  /** Read the observable tags on a completed child build. */
  async getBuildTags(
    buildId: number,
    opts: {
      retries?: number;
      retryDelayMs?: number;
      required?: readonly string[];
    } = {},
  ): Promise<string[]> {
    const retries = opts.retries ?? DEFAULT_TAG_RETRIES;
    const retryDelayMs = opts.retryDelayMs ?? DEFAULT_TAG_RETRY_DELAY_MS;
    const path = this.projPath(`_apis/build/builds/${buildId}/tags?api-version=7.1`);
    let lastErr: unknown;

    for (let attempt = 1; attempt <= retries; attempt++) {
      try {
        const response = await this.request<unknown>(path);
        const tags = Array.isArray(response)
          ? response
          : response &&
              typeof response === "object" &&
              Array.isArray((response as { value?: unknown }).value)
            ? (response as { value: unknown[] }).value
            : undefined;
        if (!tags || !tags.every((tag) => typeof tag === "string")) {
          throw new Error("ADO build-tags response was not a string array");
        }
        const stringTags = tags as string[];
        const missing = (opts.required ?? []).filter(
          (tag) => !stringTags.includes(tag),
        );
        if (missing.length > 0) {
          throw new Error(
            `required build tag(s) not visible yet: ${missing.join(", ")}; ` +
              `observed: ${stringTags.length > 0 ? stringTags.join(", ") : "<none>"}`,
          );
        }
        return stringTags;
      } catch (err) {
        lastErr = err;
      }

      if (attempt < retries) {
        this.log(
          `[build-tags] build #${buildId} attempt ${attempt}/${retries} failed, retrying in ${retryDelayMs}ms: ${
            (lastErr as Error).message
          }`,
        );
        await this.sleepImpl(retryDelayMs);
      }
    }

    throw new Error(
      `could not read tags for build #${buildId} after ${retries} attempts: ${
        (lastErr as Error)?.message ?? "unknown error"
      }`,
    );
  }

  async cancelBuild(buildId: number): Promise<void> {
    const path = this.projPath(`_apis/build/builds/${buildId}?api-version=7.1`);
    await this.request(path, { method: "PATCH", body: { status: "cancelling" } });
  }

  /**
   * Queue a build of `definitionId`, pointed at the staged candidate branch
   * + exact commit. Both are always supplied (never sourceBranch alone) so
   * a slow/racing ref update on the mirror repo can never cause ADO to
   * silently build the wrong commit.
   */
  async queueBuild(
    definitionId: number,
    opts: { sourceBranch: string; sourceVersion: string },
  ): Promise<{ id: number }> {
    const path = this.projPath(`_apis/build/builds?api-version=7.1`);
    const body = {
      definition: { id: definitionId },
      sourceBranch: opts.sourceBranch,
      sourceVersion: opts.sourceVersion,
    };
    const res = await this.request<{ id: number }>(path, { method: "POST", body });
    if (!res) throw new Error(`queueBuild(${definitionId}) returned no body`);
    return res;
  }

  /**
   * List every build of `definitionId` on the exact `branch` (a full ref,
   * e.g. `refs/heads/ado-aw-smoke-candidate/123`), regardless of status.
   *
   * Deliberately queries a single definition + exact branch and inspects
   * each build's own `status` client-side, rather than asking ADO's
   * `statusFilter` for a comma-separated set of "still running" states —
   * whether that filter reliably matches every non-terminal status across
   * ADO Build REST versions is not something this harness can assume.
   * Used by the stale-ref scanner to prove NO fixture child build is still
   * active on a candidate branch before it is deleted.
   */
  async listBuildsForDefinitionBranch(definitionId: number, branch: string): Promise<BuildSummary[]> {
    const path = this.projPath(
      `_apis/build/builds?definitions=${definitionId}&branchName=${AdoRest.seg(branch)}&api-version=7.1&$top=50`,
    );
    const res = await this.request<{ value?: BuildSummary[] }>(path);
    return res?.value ?? [];
  }
}

/** Redact the ADO token from a diagnostic string before logging/reporting. */
export function redactToken(text: string, token: string | undefined): string {
  return redact(text, [token]);
}
