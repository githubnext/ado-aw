import { afterEach, describe, expect, it, vi } from "vitest";
import { AdoRest } from "../ado-rest.js";
import type { ScenarioContext } from "../scenario.js";
import { prApiContractScenarios } from "../scenarios/pr-api-contracts.js";

afterEach(() => vi.unstubAllGlobals());

function context(): ScenarioContext {
  return {
    orgUrl: "https://dev.azure.com/org",
    project: "P",
    adoRepo: "repo",
    buildId: "42",
    token: "test-token",
    adoAwBin: "unused",
    workDir: "unused",
    rest: new AdoRest({ orgUrl: "https://dev.azure.com/org", project: "P", token: "test-token" }),
    log: () => {},
    prefix: (tool) => `ado-aw-det-42-${tool}`,
  };
}

function probe(id: string) {
  const scenario = prApiContractScenarios.find((value) => value.id === id);
  if (!scenario) throw new Error(`missing scenario ${id}`);
  return scenario;
}

function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
}

describe("PR API contract probes", () => {
  it("identifies prerequisite checks separately from executor feature coverage", () => {
    expect(prApiContractScenarios.map((s) => s.id)).toEqual([
      "pr-api-draft-publication", "pr-api-label-replacement", "pr-api-owned-comments", "pr-api-push-concurrency",
    ]);
    expect(prApiContractScenarios.every((s) => s.tool === "noop")).toBe(true);
  });

  it.each([false, true])("requires persisted draft publication, not HTTP success (persisted=%s)", async (persisted) => {
    const before = { isDraft: true, status: "active", title: "original", description: "body", sourceRefName: "source", targetRefName: "target" };
    const fetchImpl = vi.fn<typeof fetch>()
      .mockResolvedValueOnce(json(before))
      .mockResolvedValueOnce(json({}))
      .mockResolvedValueOnce(json({ ...before, isDraft: !persisted }));
    vi.stubGlobal("fetch", fetchImpl);
    const ctx = context();
    const promise = probe("pr-api-draft-publication").assert(ctx,
      { repo: "repo", prId: 1, branch: ctx.prefix("probe") }, { name: "noop", status: "succeeded" }, []);
    if (persisted) await expect(promise).resolves.toBeUndefined();
    else await expect(promise).rejects.toThrow("not persisted");
    expect(fetchImpl.mock.calls[1]?.[1]?.body).toBe('{"isDraft":false}');
  });

  it("refuses API requests for a branch outside the scenario namespace", async () => {
    const fetchImpl = vi.fn<typeof fetch>();
    vi.stubGlobal("fetch", fetchImpl);
    await expect(probe("pr-api-draft-publication").assert(context(),
      { repo: "repo", prId: 1, branch: "main" }, { name: "noop", status: "succeeded" }, []))
      .rejects.toThrow("must own");
    expect(fetchImpl).not.toHaveBeenCalled();
  });

  it("does not remove the old label when both labels were not observed", async () => {
    const fetchImpl = vi.fn<typeof fetch>()
      .mockResolvedValueOnce(json({ id: "old-label" }))
      .mockResolvedValueOnce(json({ id: "new-label" }));
    vi.stubGlobal("fetch", fetchImpl);
    const ctx = context();
    vi.spyOn(ctx.rest, "listPullRequestLabels").mockResolvedValue([{ name: "ado-aw-probe-from" }]);
    await expect(probe("pr-api-label-replacement").assert(ctx,
      { repo: "repo", prId: 1, branch: ctx.prefix("probe") }, { name: "noop", status: "succeeded" }, []))
      .rejects.toThrow("must precede removal");
    expect(fetchImpl.mock.calls.map((call) => call[1]?.method)).toEqual(["POST", "POST"]);
  });

  it.each(["GitReferenceStaleException", "UnauthorizedException"])("requires a stale-ref error, not arbitrary rejection (%s)", async (typeKey) => {
    const fetchImpl = vi.fn<typeof fetch>()
      .mockResolvedValueOnce(json({ commits: [{ commitId: "b".repeat(40) }] }))
      .mockResolvedValueOnce(json({ typeKey }, 409));
    vi.stubGlobal("fetch", fetchImpl);
    const ctx = context();
    const refs = vi.spyOn(ctx.rest, "getRefObjectId")
      .mockResolvedValueOnce("a".repeat(40))
      .mockResolvedValue("b".repeat(40));
    const promise = probe("pr-api-push-concurrency").assert(ctx,
      { repo: "repo", prId: 1, branch: ctx.prefix("probe") }, { name: "noop", status: "succeeded" }, []);
    if (typeKey === "GitReferenceStaleException") {
      await expect(promise).resolves.toBeUndefined();
      expect(refs).toHaveBeenCalledTimes(3);
    } else await expect(promise).rejects.toThrow("Expected stale-ref rejection");
    const sent = JSON.parse(String(fetchImpl.mock.calls[1]?.[1]?.body));
    expect(sent.refUpdates[0].oldObjectId).toBe("a".repeat(40));
    expect(sent.commits[0].parents).toEqual(["a".repeat(40)]);
  });
});
