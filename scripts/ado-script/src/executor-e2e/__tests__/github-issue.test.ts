import { describe, expect, it, vi } from "vitest";

import {
  ISSUE_TITLE_PREFIX,
  buildIssueTitle,
  fileFailureIssue,
  loadIssueEnv,
  renderIssueBody,
} from "../github-issue.js";
import type { ScenarioResult } from "../scenario.js";

function result(partial: Partial<ScenarioResult> & { tool: string }): ScenarioResult {
  return { ok: true, durationMs: 1, ...partial };
}

describe("buildIssueTitle", () => {
  it("keys the title on the sorted failing tool set", () => {
    const title = buildIssueTitle([
      result({ tool: "update-pr", ok: false }),
      result({ tool: "add-pr-comment", ok: false }),
    ]);
    expect(title).toBe(`${ISSUE_TITLE_PREFIX}add-pr-comment, update-pr`);
  });

  it("dedupes repeated tools", () => {
    const title = buildIssueTitle([
      result({ tool: "create-branch", ok: false }),
      result({ tool: "create-branch", ok: false }),
    ]);
    expect(title).toBe(`${ISSUE_TITLE_PREFIX}create-branch`);
  });
});

describe("renderIssueBody", () => {
  it("includes a failure table, run stats, and skipped section", () => {
    const results: ScenarioResult[] = [
      result({ tool: "create-work-item" }),
      result({ tool: "add-pr-comment", ok: false, phase: "assert", message: "no thread" }),
      result({ tool: "queue-build", ok: true, skipped: true, message: "no pipeline id" }),
    ];
    const body = renderIssueBody(results, {
      repo: "githubnext/ado-aw",
      labels: [],
      project: "AgentPlayground",
      buildId: "42",
      buildUrl: "https://example/build/42",
    });
    expect(body).toContain("| `add-pr-comment` | assert | no thread |");
    expect(body).toContain("Passed: 1 | Failed: 1 | Skipped: 1");
    expect(body).toContain("`queue-build`: no pipeline id");
    expect(body).toContain("https://example/build/42");
  });
});

describe("loadIssueEnv", () => {
  it("prefers EXECUTOR_E2E_GITHUB_TOKEN and defaults repo/labels", () => {
    const env = loadIssueEnv({
      EXECUTOR_E2E_GITHUB_TOKEN: "tok",
      ADO_AW_DEBUG_GITHUB_TOKEN: "other",
      SYSTEM_TEAMPROJECT: "P",
    } as NodeJS.ProcessEnv);
    expect(env.token).toBe("tok");
    expect(env.repo).toBe("githubnext/ado-aw");
    expect(env.labels).toContain("executor-e2e-failure");
  });

  it("falls back to ADO_AW_DEBUG_GITHUB_TOKEN", () => {
    const env = loadIssueEnv({ ADO_AW_DEBUG_GITHUB_TOKEN: "fallback" } as NodeJS.ProcessEnv);
    expect(env.token).toBe("fallback");
  });
});

describe("fileFailureIssue", () => {
  const failing: ScenarioResult[] = [result({ tool: "create-branch", ok: false, phase: "assert" })];

  it("no-ops when there are no failures", async () => {
    const out = await fileFailureIssue(
      [result({ tool: "ok-tool" })],
      { repo: "r", labels: [], token: "t" },
      () => {},
    );
    expect(out.filed).toBe(false);
    expect(out.reason).toBe("no failures");
  });

  it("no-ops when no token is configured", async () => {
    const out = await fileFailureIssue(failing, { repo: "r", labels: [] }, () => {});
    expect(out.filed).toBe(false);
    expect(out.reason).toBe("no token");
  });

  it("dedupes to an existing open issue", async () => {
    const title = buildIssueTitle(failing);
    const fetchMock = vi.fn(async (url: string) => {
      expect(url).toContain("search/issues");
      return new Response(JSON.stringify({ items: [{ number: 7, title }] }), { status: 200 });
    }) as unknown as typeof fetch;
    const out = await fileFailureIssue(
      failing,
      { repo: "r", labels: [], token: "t" },
      () => {},
      fetchMock,
    );
    expect(out.filed).toBe(false);
    expect(out.reason).toBe("deduped to #7");
  });

  it("creates a new issue when none matches", async () => {
    const calls: string[] = [];
    const fetchMock = vi.fn(async (url: string, init?: RequestInit) => {
      calls.push(`${init?.method ?? "GET"} ${url}`);
      if (url.includes("search/issues")) {
        return new Response(JSON.stringify({ items: [] }), { status: 200 });
      }
      return new Response(JSON.stringify({ html_url: "https://github.com/x/y/issues/9" }), {
        status: 201,
      });
    }) as unknown as typeof fetch;
    const out = await fileFailureIssue(
      failing,
      { repo: "githubnext/ado-aw", labels: ["l"], token: "t" },
      () => {},
      fetchMock,
    );
    expect(out.filed).toBe(true);
    expect(out.url).toBe("https://github.com/x/y/issues/9");
    expect(calls.some((c) => c.startsWith("POST"))).toBe(true);
  });

  it("diagnoses a 403 create failure as authenticated-but-missing-Issues:write", async () => {
    const logs: string[] = [];
    const fetchMock = vi.fn(async (url: string) => {
      if (url.includes("search/issues")) {
        return new Response(JSON.stringify({ items: [] }), { status: 200 });
      }
      if (url.endsWith("/user")) {
        return new Response(JSON.stringify({ login: "octocat" }), {
          status: 200,
          headers: { "x-accepted-github-permissions": "issues=write" },
        });
      }
      // create issue
      return new Response(JSON.stringify({ message: "Resource not accessible" }), { status: 403 });
    }) as unknown as typeof fetch;
    await expect(
      fileFailureIssue(
        failing,
        { repo: "some/repo", labels: [], token: "t" },
        (m) => logs.push(m),
        fetchMock,
      ),
    ).rejects.toThrow(/HTTP 403/);
    const diag = logs.find((l) => l.includes("token diagnosis"));
    expect(diag).toContain("authenticated as 'octocat'");
    expect(diag).toContain("some/repo");
    expect(diag).toContain("Issues:write");
  });

  it("diagnoses a 401 search failure as an invalid/revoked token", async () => {
    const logs: string[] = [];
    const fetchMock = vi.fn(async (url: string) => {
      if (url.includes("search/issues")) {
        return new Response(JSON.stringify({ message: "Bad credentials" }), { status: 401 });
      }
      if (url.endsWith("/user")) {
        return new Response(JSON.stringify({ message: "Bad credentials" }), { status: 401 });
      }
      return new Response("{}", { status: 201 });
    }) as unknown as typeof fetch;
    await expect(
      fileFailureIssue(
        failing,
        { repo: "some/repo", labels: [], token: "t" },
        (m) => logs.push(m),
        fetchMock,
      ),
    ).rejects.toThrow(/HTTP 401/);
    const diag = logs.find((l) => l.includes("token diagnosis"));
    expect(diag).toContain("REVOKED");
    expect(diag).toContain("some/repo");
  });
});
