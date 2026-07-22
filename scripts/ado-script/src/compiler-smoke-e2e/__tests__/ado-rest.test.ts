import { describe, expect, it, vi } from "vitest";

import { AdoRest, redactToken } from "../ado-rest.js";

function jsonResponse(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  });
}

function emptyResponse(status: number): Response {
  return new Response(null, { status });
}

function makeRest(fetchImpl: typeof fetch, extra: Partial<ConstructorParameters<typeof AdoRest>[0]> = {}) {
  return new AdoRest({
    orgUrl: "https://dev.azure.com/org/",
    project: "AgentPlayground",
    token: "secret-token",
    fetchImpl,
    sleepImpl: async () => {},
    log: () => {},
    ...extra,
  });
}

describe("AdoRest.getArtifact", () => {
  it("returns the artifact on the first successful attempt", async () => {
    const fetchImpl = vi.fn(async () => jsonResponse(200, { name: "ado-aw-candidate" }));
    const rest = makeRest(fetchImpl as unknown as typeof fetch);
    const artifact = await rest.getArtifact(100, "ado-aw-candidate");
    expect(artifact.name).toBe("ado-aw-candidate");
    expect(fetchImpl).toHaveBeenCalledTimes(1);
  });

  it("retries a 404 up to the configured bound, then succeeds", async () => {
    let calls = 0;
    const fetchImpl = vi.fn(async () => {
      calls++;
      if (calls < 3) return emptyResponse(404);
      return jsonResponse(200, { name: "ado-aw-candidate" });
    });
    const rest = makeRest(fetchImpl as unknown as typeof fetch);
    const artifact = await rest.getArtifact(100, "ado-aw-candidate", { retries: 5, retryDelayMs: 1 });
    expect(artifact.name).toBe("ado-aw-candidate");
    expect(calls).toBe(3);
  });

  it("throws after exhausting all retries", async () => {
    const fetchImpl = vi.fn(async () => emptyResponse(404));
    const rest = makeRest(fetchImpl as unknown as typeof fetch);
    await expect(
      rest.getArtifact(100, "ado-aw-candidate", { retries: 2, retryDelayMs: 1 }),
    ).rejects.toThrow(/not visible/);
    expect(fetchImpl).toHaveBeenCalledTimes(2);
  });

  it("scopes the request to the exact same project as the producer build", async () => {
    const fetchImpl = vi.fn(async (input: RequestInfo | URL) => {
      expect(String(input)).toContain("/AgentPlayground/");
      return jsonResponse(200, { name: "a" });
    });
    const rest = makeRest(fetchImpl as unknown as typeof fetch);
    await rest.getArtifact(100, "a");
  });
});

describe("AdoRest.queueBuild", () => {
  it("always sends both sourceBranch and sourceVersion", async () => {
    let sentBody: unknown;
    const fetchImpl = vi.fn(async (_input: RequestInfo | URL, init?: RequestInit) => {
      sentBody = JSON.parse(String(init?.body));
      return jsonResponse(200, { id: 555 });
    });
    const rest = makeRest(fetchImpl as unknown as typeof fetch);
    const result = await rest.queueBuild(2560, {
      sourceBranch: "refs/heads/ado-aw-smoke-candidate/1",
      sourceVersion: "deadbeef",
    });
    expect(result.id).toBe(555);
    expect(sentBody).toMatchObject({
      definition: { id: 2560 },
      sourceBranch: "refs/heads/ado-aw-smoke-candidate/1",
      sourceVersion: "deadbeef",
    });
  });

  it("throws with a descriptive error on a non-2xx response", async () => {
    const fetchImpl = vi.fn(async () => new Response("nope", { status: 500 }));
    const rest = makeRest(fetchImpl as unknown as typeof fetch);
    await expect(
      rest.queueBuild(2560, { sourceBranch: "refs/heads/x", sourceVersion: "sha" }),
    ).rejects.toThrow(/HTTP 500/);
  });
});

describe("AdoRest.getBuild / cancelBuild", () => {
  it("getBuild returns the parsed build summary", async () => {
    const fetchImpl = vi.fn(async () => jsonResponse(200, { id: 1, status: "completed", result: "succeeded" }));
    const rest = makeRest(fetchImpl as unknown as typeof fetch);
    const build = await rest.getBuild(1);
    expect(build.status).toBe("completed");
    expect(build.result).toBe("succeeded");
  });

  describe("AdoRest.getBuildTags", () => {
    it("accepts the documented string-array response", async () => {
      const fetchImpl = vi.fn(async () =>
        jsonResponse(200, ["ado-aw-custom-script-10", "ado-aw-custom-job-10"]),
      );
      const rest = makeRest(fetchImpl as unknown as typeof fetch);
      await expect(rest.getBuildTags(10)).resolves.toEqual([
        "ado-aw-custom-script-10",
        "ado-aw-custom-job-10",
      ]);
    });

    it("accepts an ADO collection wrapper defensively", async () => {
      const fetchImpl = vi.fn(async () =>
        jsonResponse(200, { count: 1, value: ["tag-one"] }),
      );
      const rest = makeRest(fetchImpl as unknown as typeof fetch);
      await expect(rest.getBuildTags(10)).resolves.toEqual(["tag-one"]);
    });

    it("retries malformed responses and reports the final error", async () => {
      const fetchImpl = vi.fn(async () => jsonResponse(200, { value: [42] }));
      const rest = makeRest(fetchImpl as unknown as typeof fetch);
      await expect(
        rest.getBuildTags(10, { retries: 2, retryDelayMs: 1 }),
      ).rejects.toThrow(/not a string array/);
      expect(fetchImpl).toHaveBeenCalledTimes(2);
    });

    it("retries a successful response until every required tag is visible", async () => {
      let calls = 0;
      const fetchImpl = vi.fn(async () => {
        calls++;
        return jsonResponse(
          200,
          calls === 1
            ? ["ado-aw-custom-script-10"]
            : ["ado-aw-custom-script-10", "ado-aw-custom-job-10"],
        );
      });
      const rest = makeRest(fetchImpl as unknown as typeof fetch);
      await expect(
        rest.getBuildTags(10, {
          retries: 2,
          retryDelayMs: 1,
          required: [
            "ado-aw-custom-script-10",
            "ado-aw-custom-job-10",
          ],
        }),
      ).resolves.toEqual([
        "ado-aw-custom-script-10",
        "ado-aw-custom-job-10",
      ]);
      expect(fetchImpl).toHaveBeenCalledTimes(2);
    });
  });

  it("cancelBuild PATCHes the build to status=cancelling", async () => {
    let method: string | undefined;
    let body: unknown;
    const fetchImpl = vi.fn(async (_input: RequestInfo | URL, init?: RequestInit) => {
      method = init?.method;
      body = JSON.parse(String(init?.body));
      return emptyResponse(204);
    });
    const rest = makeRest(fetchImpl as unknown as typeof fetch);
    await rest.cancelBuild(1);
    expect(method).toBe("PATCH");
    expect(body).toEqual({ status: "cancelling" });
  });
});

describe("AdoRest.listBuildsForDefinitionBranch", () => {
  it("queries a single definition + exact branch and returns every build regardless of status", async () => {
    let requestedPath = "";
    const fetchImpl = vi.fn(async (input: RequestInfo | URL) => {
      requestedPath = String(input);
      return jsonResponse(200, {
        value: [
          { id: 1, status: "completed", result: "succeeded" },
          { id: 2, status: "inProgress" },
        ],
      });
    });
    const rest = makeRest(fetchImpl as unknown as typeof fetch);
    const builds = await rest.listBuildsForDefinitionBranch(3001, "refs/heads/ado-aw-smoke-candidate/1");
    expect(builds).toHaveLength(2);
    expect(builds[1]?.status).toBe("inProgress");
    // The query is scoped to exactly one definition id + the exact branch —
    // never a comma-separated statusFilter (status is inspected client-side
    // instead, per the stale-scan safety requirement).
    expect(requestedPath).toContain("definitions=3001");
    expect(requestedPath).toContain(encodeURIComponent("refs/heads/ado-aw-smoke-candidate/1"));
    expect(requestedPath).not.toContain("statusFilter");
  });

  it("returns an empty array when there are no builds on that branch", async () => {
    const fetchImpl = vi.fn(async () => jsonResponse(200, { value: [] }));
    const rest = makeRest(fetchImpl as unknown as typeof fetch);
    const builds = await rest.listBuildsForDefinitionBranch(3001, "refs/heads/ado-aw-smoke-candidate/2");
    expect(builds).toEqual([]);
  });
});

describe("AdoRest.buildUrl", () => {
  it("builds a human-facing build results URL", () => {
    const rest = makeRest((async () => emptyResponse(200)) as unknown as typeof fetch);
    expect(rest.buildUrl(123)).toBe(
      "https://dev.azure.com/org/AgentPlayground/_build/results?buildId=123",
    );
  });
});

describe("redactToken", () => {
  it("replaces the token with ***", () => {
    expect(redactToken("Bearer secret-token in header", "secret-token")).toBe(
      "Bearer *** in header",
    );
  });

  it("is a no-op for an undefined token", () => {
    expect(redactToken("nothing to redact", undefined)).toBe("nothing to redact");
  });
});
