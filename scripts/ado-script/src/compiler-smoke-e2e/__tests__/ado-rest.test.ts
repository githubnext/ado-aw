import { describe, expect, it, vi } from "vitest";

import { AdoHttpError, AdoRest, redactToken, transientReadRetryAfter, type OwnedBoundaryPr } from "../ado-rest.js";

function jsonResponse(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  });
}

function emptyResponse(status: number): Response {
  return new Response(null, { status });
}

function makeRest(
  fetchImpl: typeof fetch,
  extra: Partial<ConstructorParameters<typeof AdoRest>[0]> = {},
) {
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
    const fetchImpl = vi.fn(async () =>
      jsonResponse(200, { name: "ado-aw-candidate" }),
    );
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
    const artifact = await rest.getArtifact(100, "ado-aw-candidate", {
      retries: 5,
      retryDelayMs: 1,
    });
    expect(artifact.name).toBe("ado-aw-candidate");
    expect(calls).toBe(3);
  });

  it("throws after exhausting all retries", async () => {
    const fetchImpl = vi.fn(async () => emptyResponse(404));
    const rest = makeRest(fetchImpl as unknown as typeof fetch);
    await expect(
      rest.getArtifact(100, "ado-aw-candidate", {
        retries: 2,
        retryDelayMs: 1,
      }),
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

describe("build status read failures", () => {
  it.each([408, 429, 500, 502, 503, 504])("preserves transient HTTP %i and Retry-After without retrying the request", async (status) => {
    const fetchImpl = vi.fn(async () => new Response("try later", {
      status, headers: { "retry-after": "2" },
    }));
    const error = await makeRest(fetchImpl).getBuild(1).catch((failure: unknown) => failure);
    expect(error).toBeInstanceOf(AdoHttpError);
    expect(error).toMatchObject({ status, retryAfterMs: 2_000 });
    expect(transientReadRetryAfter(error)).toBe(2_000);
    expect(fetchImpl).toHaveBeenCalledTimes(1);
  });

  it.each([400, 401, 403, 404, 409, 422])("never retries permanent HTTP %i", async (status) => {
    const error = await makeRest(vi.fn(async () => new Response("rejected", {
      status, headers: { "retry-after": "2" },
    }))).getBuild(1).catch((failure: unknown) => failure);
    expect(error).toBeInstanceOf(AdoHttpError);
    expect(transientReadRetryAfter(error)).toBeUndefined();
  });

  it("distinguishes socket errors from programming and malformed-response errors", () => {
    for (const error of [
      new TypeError("fetch failed", { cause: Object.assign(new Error("reset"), { code: "ECONNRESET" }) }),
      new DOMException("request timed out", "TimeoutError"),
    ]) expect(transientReadRetryAfter(error)).toBe(0);
    for (const error of [
      new TypeError("fetch failed"),
      new TypeError("bad URL", { cause: Object.assign(new Error("bad URL"), { code: "ERR_INVALID_URL" }) }),
      new SyntaxError("truncated JSON"),
      new Error("transient network error"),
    ]) expect(transientReadRetryAfter(error)).toBeUndefined();
  });

  it.each([{}, [], "invalid", { id: 2, status: "completed" }, { id: 1 }, { id: 1, status: "unknown" }])("rejects malformed build summaries %j", async (body) => {
    const error = await makeRest(vi.fn(async () => jsonResponse(200, body)))
      .getBuild(1).catch((failure: unknown) => failure);
    expect(error).toBeInstanceOf(Error);
    expect(transientReadRetryAfter(error)).toBeUndefined();
  });

  it("leaves malformed JSON non-retryable", async () => {
    const fetchImpl = vi.fn(async () => new Response('{"id":1,', { status: 200 }));
    const error = await makeRest(fetchImpl).getBuild(1).catch((failure: unknown) => failure);
    expect(error).toBeInstanceOf(SyntaxError);
    expect(transientReadRetryAfter(error)).toBeUndefined();
    expect(fetchImpl).toHaveBeenCalledTimes(1);
  });

  it("does not retry a mutating request even when its HTTP error is transient", async () => {
    const fetchImpl = vi.fn(async () => new Response("unavailable", { status: 503 }));
    await expect(makeRest(fetchImpl).queueBuild(1, {
      sourceBranch: "refs/heads/x", sourceVersion: "sha",
    })).rejects.toThrow(AdoHttpError);
    expect(fetchImpl).toHaveBeenCalledTimes(1);
  });
});

describe("AdoRest.queueBuild", () => {
  it("always sends both sourceBranch and sourceVersion", async () => {
    let sentBody: unknown;
    const fetchImpl = vi.fn(
      async (_input: RequestInfo | URL, init?: RequestInit) => {
        sentBody = JSON.parse(String(init?.body));
        return jsonResponse(200, { id: 555 });
      },
    );
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
      rest.queueBuild(2560, {
        sourceBranch: "refs/heads/x",
        sourceVersion: "sha",
      }),
    ).rejects.toThrow(/HTTP 500/);
  });
});

describe("AdoRest.getBuild / cancelBuild", () => {
  it("getBuild returns the parsed build summary", async () => {
    const fetchImpl = vi.fn(async () =>
      jsonResponse(200, { id: 1, status: "completed", result: "succeeded" }),
    );
    const rest = makeRest(fetchImpl as unknown as typeof fetch);
    const build = await rest.getBuild(1);
    expect(build.status).toBe("completed");
    expect(build.result).toBe("succeeded");
  });

  describe("AdoRest.getBuildTags", () => {
    it("accepts the documented string-array response", async () => {
      const fetchImpl = vi.fn(async () =>
        jsonResponse(200, ["ado-aw-custom-job-10"]),
      );
      const rest = makeRest(fetchImpl as unknown as typeof fetch);
      await expect(rest.getBuildTags(10)).resolves.toEqual([
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
        return jsonResponse(200, calls === 1 ? [] : ["ado-aw-custom-job-10"]);
      });
      const rest = makeRest(fetchImpl as unknown as typeof fetch);
      await expect(
        rest.getBuildTags(10, {
          retries: 2,
          retryDelayMs: 1,
          required: ["ado-aw-custom-job-10"],
        }),
      ).resolves.toEqual(["ado-aw-custom-job-10"]);
      expect(fetchImpl).toHaveBeenCalledTimes(2);
    });
  });

  it("cancelBuild PATCHes the build to status=cancelling", async () => {
    let method: string | undefined;
    let body: unknown;
    const fetchImpl = vi.fn(
      async (_input: RequestInfo | URL, init?: RequestInit) => {
        method = init?.method;
        body = JSON.parse(String(init?.body));
        return emptyResponse(204);
      },
    );
    const rest = makeRest(fetchImpl as unknown as typeof fetch);
    await rest.cancelBuild(1);
    expect(method).toBe("PATCH");
    expect(body).toEqual({ status: "cancelling" });
  });
});

describe("AdoRest.listBuildsForBranch", () => {
  it.each([
    {},
    { value: null },
    { value: [{ id: 1, status: "completed" }] },
    { value: [{ id: 1, status: "completed", definition: { id: 3002 }, sourceBranch: "refs/heads/wrong" }] },
    { value: Array.from({ length: 50 }, (_, i) => ({
      id: i + 1, status: "completed", definition: { id: 3001 }, sourceBranch: "refs/heads/case",
    })) },
  ])("rejects malformed, mismatched or possibly truncated child metadata", async (body) => {
    const rest = makeRest(vi.fn(async () => jsonResponse(200, body)));
    await expect(rest.listBuildsForBranch("refs/heads/case")).rejects.toThrow("incomplete");
  });

  it("queries every definition on the exact branch, regardless of status", async () => {
    let requestedPath = "";
    const fetchImpl = vi.fn(async (input: RequestInfo | URL) => {
      requestedPath = String(input);
      return jsonResponse(200, {
        value: [
          { id: 1, status: "completed", result: "succeeded", definition: { id: 3001 }, sourceBranch: "refs/heads/ado-aw-smoke-candidate/1" },
          { id: 2, status: "inProgress", definition: { id: 3999 }, sourceBranch: "refs/heads/ado-aw-smoke-candidate/1" },
        ],
      });
    });
    const rest = makeRest(fetchImpl as unknown as typeof fetch);
    const builds = await rest.listBuildsForBranch(
      "refs/heads/ado-aw-smoke-candidate/1",
    );
    expect(builds).toHaveLength(2);
    expect(builds[1]?.status).toBe("inProgress");
    expect(requestedPath).not.toContain("definitions=");
    expect(requestedPath).toContain(
      encodeURIComponent("refs/heads/ado-aw-smoke-candidate/1"),
    );
    expect(requestedPath).not.toContain("statusFilter");
  });

  it("returns an empty array when there are no builds on that branch", async () => {
    const fetchImpl = vi.fn(async () => jsonResponse(200, { value: [] }));
    const rest = makeRest(fetchImpl as unknown as typeof fetch);
    const builds = await rest.listBuildsForBranch(
      "refs/heads/ado-aw-smoke-candidate/2",
    );
    expect(builds).toEqual([]);
  });
});

describe("owned boundary PR recovery", () => {
  const source = "refs/heads/ado-aw-smoke-candidate/42/check";
  const pr: OwnedBoundaryPr = {
    pullRequestId: 7, status: "active",
    title: "ado-aw-boundary-original-42-check",
    sourceRefName: source, targetRefName: "refs/heads/ado-aw-smoke-boundary-target/42/check",
    repository: { name: "mirror", project: { name: "AgentPlayground" } },
  };

  it.each([pr.targetRefName, `${source}-target`])("recovers exact source/target/marker identity (%s)", async (targetRefName) => {
    const fetch = vi.fn<typeof globalThis.fetch>(async (input) => {
      const url = new URL(String(input));
      expect(url.searchParams.get("searchCriteria.sourceRefName")).toBe(source);
      expect(url.searchParams.get("searchCriteria.status")).toBe("all");
      expect(url.searchParams.get("$top")).toBe("2");
      return jsonResponse(200, { value: [{ ...pr, targetRefName }] });
    });
    await expect(makeRest(fetch).findBoundaryPr("mirror", source)).resolves.toMatchObject({ targetRefName });
  });

  it.each([
    {},
    { value: [null] },
    { value: [pr, pr] },
    { value: [{ ...pr, sourceRefName: `${source}-other` }] },
    { value: [{ ...pr, targetRefName: "refs/heads/main" }] },
    { value: [{ ...pr, title: "human PR" }] },
    { value: [{ ...pr, repository: { name: "other", project: { name: "AgentPlayground" } } }] },
    { value: [{ ...pr, repository: { name: "mirror", project: { name: "other" } } }] },
    { value: [{ ...pr, forkSource: {} }] },
  ])("rejects incomplete or contradictory ownership evidence", async (body) => {
    await expect(makeRest(vi.fn(async () => jsonResponse(200, body))).findBoundaryPr("mirror", source))
      .rejects.toThrow();
  });

  it("does not treat a continuation as complete discovery", async () => {
    const fetch = vi.fn(async () => new Response(JSON.stringify({ value: [] }), {
      headers: { "content-type": "application/json", "x-ms-continuationtoken": "more" },
    }));
    await expect(makeRest(fetch).findBoundaryPr("mirror", source)).rejects.toThrow("Incomplete");
  });

  it.each(["abandoned", "completed", "active", "wrong-target", "read-error"])(
    "reconciles a lost abandonment response without retrying (%s)", async (outcome) => {
      let writes = 0;
      const fetch = vi.fn<typeof globalThis.fetch>(async (_input, init) => {
        if (init?.method === "PATCH") { writes += 1; throw new Error("lost response"); }
        if (writes === 0) return jsonResponse(200, pr);
        if (outcome === "read-error") return new Response("forbidden", { status: 403 });
        return jsonResponse(200, outcome === "wrong-target"
          ? { ...pr, status: "abandoned", targetRefName: "refs/heads/main" }
          : { ...pr, status: outcome });
      });
      const operation = makeRest(fetch).abandonBoundaryPr("mirror", pr);
      if (outcome === "abandoned") await expect(operation).resolves.toBeUndefined();
      else await expect(operation).rejects.toThrow();
      expect(writes).toBe(1);
    },
  );

  it("retains an ordinary boundary PR that unexpectedly completed", async () => {
    const fetch = vi.fn(async () => jsonResponse(200, { ...pr, status: "completed" }));
    await expect(makeRest(fetch).abandonBoundaryPr("mirror", pr)).rejects.toThrow("unexpectedly completed");
    expect(fetch).toHaveBeenCalledTimes(1);
  });
});

describe("AdoRest.buildUrl", () => {
  it("builds a human-facing build results URL", () => {
    const rest = makeRest((async () =>
      emptyResponse(200)) as unknown as typeof fetch);
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
    expect(redactToken("nothing to redact", undefined)).toBe(
      "nothing to redact",
    );
  });
});

describe("AdoRest.addBuildTags", () => {
  // Regression: tags were sent one-per-request as PUT .../tags/<tag>, putting
  // the tag in the URL PATH. ADO's ASP.NET front end validates the *decoded*
  // path, so `smoke-case:canary` was rejected with HTTP 400 "A potentially
  // dangerous Request.Path value was detected from the client (:)" even
  // correctly encoded as %3A — which is every tag this harness writes.
  // Observed live on build 629522's children.
  it("sends tags in the request body, never in the URL path", async () => {
    const calls: { url: string; method?: string; body?: unknown }[] = [];
    const fetchImpl = vi.fn(async (url: string, init?: RequestInit) => {
      calls.push({ url, method: init?.method, body: init?.body });
      return jsonResponse(200, { count: 2, value: ["smoke-case:canary"] });
    });
    const rest = makeRest(fetchImpl as unknown as typeof fetch);

    await rest.addBuildTags(4242, ["smoke-case:canary", "smoke-candidate:99"]);

    expect(calls).toHaveLength(1);
    expect(calls[0]!.method).toBe("POST");
    expect(calls[0]!.url).toContain("/build/builds/4242/tags?api-version=");
    // The colon-bearing values must not reach the path in any encoding.
    expect(calls[0]!.url).not.toContain("smoke-case");
    expect(calls[0]!.url).not.toContain("%3A");
    expect(JSON.parse(String(calls[0]!.body))).toEqual([
      "smoke-case:canary",
      "smoke-candidate:99",
    ]);
  });

  it("makes no request when there are no tags", async () => {
    const fetchImpl = vi.fn(async () => jsonResponse(200, {}));
    const rest = makeRest(fetchImpl as unknown as typeof fetch);
    await rest.addBuildTags(1, []);
    expect(fetchImpl).not.toHaveBeenCalled();
  });
});
