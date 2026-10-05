import { afterEach, describe, expect, it, vi } from "vitest";
import { AdoRest } from "../ado-rest.js";
import type { ScenarioContext } from "../scenario.js";
import { prScenarios } from "../scenarios/pr.js";
import { allScenarios } from "../scenarios/index.js";
import { prReviewerApiExperiments } from "../scenarios/pr-api-contracts.js";

afterEach(() => vi.restoreAllMocks());
const reviewer = "01234567-89ab-cdef-0123-456789abcdef";

function context(): ScenarioContext {
  return {
    orgUrl: "https://dev.azure.com/org", project: "P", adoRepo: "repo", token: "test-token",
    buildId: "42", adoAwBin: "unused", workDir: "unused", log: () => {},
    prefix: (id) => `ado-aw-det-42-${id}`,
    rest: new AdoRest({ orgUrl: "https://dev.azure.com/org", project: "P", token: "test-token" }),
  };
}

describe("existing reviewer executor evidence", () => {
  it("keeps failed atomicity hypotheses out of default scenario selection", () => {
    const ids = allScenarios.map((scenario) => scenario.id ?? scenario.tool);
    for (const experiment of prReviewerApiExperiments) expect(ids).not.toContain(experiment.id);
    expect(ids).toContain("create-pull-request-configured-reviewers");
  });

  it.each([
    { name: "required-negative", vote: -10, isRequired: true, isFlagged: true, hasDeclined: false },
    { name: "required-positive", vote: 5, isRequired: true, isFlagged: false, hasDeclined: false },
    { name: "optional", vote: 0, isRequired: false, isFlagged: true, hasDeclined: true },
  ])("requires persisted $name state and truthful no-op reporting", async (seed) => {
    const scenario = prScenarios.find((candidate) => candidate.id === `pr-reviewer-existing-${seed.name}`)!;
    expect(scenario.tool).toBe("add-pull-request-reviewers");
    const ctx = context();
    const state = { repo: "repo", prId: 42, branch: ctx.prefix("fixture"), reviewer };
    const read = vi.spyOn(ctx.rest, "listReviewers").mockResolvedValue([{ id: reviewer, ...seed }]);
    const record = { name: "add_pull_request_reviewers", status: "succeeded",
      result: { added: [], failed: [], already_present: [reviewer] } };
    await expect(scenario.assert(ctx, state, record, [record])).resolves.toBeUndefined();
    await expect(scenario.assert(ctx, state, { ...record, result: { ...record.result, added: [reviewer] } }, []))
      .rejects.toThrow("unchanged no-op");
    for (const field of ["vote", "isRequired", "isFlagged", "hasDeclined"] as const) {
      read.mockResolvedValue([{ id: reviewer, ...seed, [field]: field === "vote" ? 10 : !seed[field] }]);
      await expect(scenario.assert(ctx, state, record, [])).rejects.toThrow("changed vote");
    }
    expect(scenario.config(ctx, state)).not.toHaveProperty("allowed-events");
  });
});
