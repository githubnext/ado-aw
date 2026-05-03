import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import type { GateSpec, FactSpec } from "../shared/types.gen.js";
import type { PolicyTracker } from "../shared/policy.js";

const {
  mockReadEnvFact,
  mockIsPipelineVarFact,
  mockGetPullRequestById,
  mockGetPullRequestIterations,
  mockGetIterationChanges,
} = vi.hoisted(() => ({
  mockReadEnvFact: vi.fn(),
  mockIsPipelineVarFact: vi.fn(),
  mockGetPullRequestById: vi.fn(),
  mockGetPullRequestIterations: vi.fn(),
  mockGetIterationChanges: vi.fn(),
}));

vi.mock("../shared/env-facts.js", () => ({
  readEnvFact: mockReadEnvFact,
  isPipelineVarFact: mockIsPipelineVarFact,
}));

vi.mock("../shared/ado-client.js", () => ({
  getPullRequestById: mockGetPullRequestById,
  getPullRequestIterations: mockGetPullRequestIterations,
  getIterationChanges: mockGetIterationChanges,
}));

import { acquireFacts } from "./facts.js";

function fact(kind: string, failure_policy = "fail_closed"): FactSpec {
  return { kind, failure_policy, dependencies: [] };
}

function gateSpec(facts: FactSpec[]): GateSpec {
  return {
    facts,
    checks: [],
    context: {
      build_reason: "PullRequest",
      bypass_label: "ado-aw:bypass",
      step_name: "Gate",
      tag_prefix: "ado-aw:gate",
    },
  };
}

function makeTracker(initiallyUnavailable: string[] = []) {
  const unavailable = new Set(initiallyUnavailable);
  const recordFactFailure = vi.fn((kind: string) => {
    unavailable.add(kind);
    return "fail_closed";
  });
  const isUnavailableForAcquisition = vi.fn((kind: string) => unavailable.has(kind));
  const tracker = {
    recordFactFailure,
    isUnavailableForAcquisition,
  } as unknown as PolicyTracker;
  return { tracker, recordFactFailure, isUnavailableForAcquisition };
}

describe("acquireFacts", () => {
  let savedEnv: NodeJS.ProcessEnv;

  beforeEach(() => {
    savedEnv = { ...process.env };
    process.env.ADO_PROJECT = "project";
    process.env.ADO_REPO_ID = "repo";
    process.env.ADO_PR_ID = "42";

    mockReadEnvFact.mockReset();
    mockIsPipelineVarFact.mockReset().mockReturnValue(false);
    mockGetPullRequestById.mockReset();
    mockGetPullRequestIterations.mockReset();
    mockGetIterationChanges.mockReset();
  });

  afterEach(() => {
    process.env = savedEnv;
    vi.useRealTimers();
  });

  it("acquires pipeline-variable facts via env-facts", async () => {
    mockIsPipelineVarFact.mockImplementation((kind: string) => kind === "pr_title");
    mockReadEnvFact.mockReturnValue("Fix everything");
    const { tracker, recordFactFailure } = makeTracker();

    const facts = await acquireFacts(gateSpec([fact("pr_title")]), tracker);

    expect(mockReadEnvFact).toHaveBeenCalledWith("pr_title");
    expect(facts.get("pr_title")).toBe("Fix everything");
    expect(recordFactFailure).not.toHaveBeenCalled();
  });

  it("records a fact failure when a pipeline-variable env value is missing", async () => {
    mockIsPipelineVarFact.mockImplementation((kind: string) => kind === "author_email");
    mockReadEnvFact.mockReturnValue(undefined);
    const { tracker, recordFactFailure } = makeTracker();

    const facts = await acquireFacts(gateSpec([fact("author_email")]), tracker);

    expect(facts.has("author_email")).toBe(false);
    expect(recordFactFailure).toHaveBeenCalledWith(
      "author_email",
      "value undefined / missing env",
    );
  });

  it("acquires pr_metadata via the ADO client", async () => {
    const pr = { pullRequestId: 42, isDraft: false };
    mockGetPullRequestById.mockResolvedValue(pr);
    const { tracker, recordFactFailure } = makeTracker();

    const facts = await acquireFacts(gateSpec([fact("pr_metadata")]), tracker);

    expect(mockGetPullRequestById).toHaveBeenCalledWith("project", "repo", 42);
    expect(facts.get("pr_metadata")).toBe(pr);
    expect(recordFactFailure).not.toHaveBeenCalled();
  });

  it("records a fact failure when pr_metadata acquisition throws", async () => {
    mockGetPullRequestById.mockRejectedValue(new Error("SDK exploded"));
    const { tracker, recordFactFailure } = makeTracker();

    const facts = await acquireFacts(gateSpec([fact("pr_metadata")]), tracker);

    expect(facts.has("pr_metadata")).toBe(false);
    expect(recordFactFailure).toHaveBeenCalledWith("pr_metadata", "SDK exploded");
  });

  it("derives pr_is_draft from cached pr_metadata as true", async () => {
    mockGetPullRequestById.mockResolvedValue({ pullRequestId: 42, isDraft: true });
    const { tracker } = makeTracker();

    const facts = await acquireFacts(
      gateSpec([fact("pr_metadata"), fact("pr_is_draft")]),
      tracker,
    );

    expect(facts.get("pr_is_draft")).toBe("true");
  });

  it("derives pr_is_draft from cached pr_metadata as false", async () => {
    mockGetPullRequestById.mockResolvedValue({ pullRequestId: 42, isDraft: false });
    const { tracker } = makeTracker();

    const facts = await acquireFacts(
      gateSpec([fact("pr_metadata"), fact("pr_is_draft")]),
      tracker,
    );

    expect(facts.get("pr_is_draft")).toBe("false");
  });

  it("skips pr_is_draft acquisition when dependency propagation marks it unavailable", async () => {
    const { tracker, recordFactFailure, isUnavailableForAcquisition } = makeTracker([
      "pr_is_draft",
    ]);

    const facts = await acquireFacts(gateSpec([fact("pr_is_draft")]), tracker);

    expect(isUnavailableForAcquisition).toHaveBeenCalledWith("pr_is_draft");
    expect(facts.has("pr_is_draft")).toBe(false);
    expect(recordFactFailure).not.toHaveBeenCalled();
    expect(mockGetPullRequestById).not.toHaveBeenCalled();
  });

  it("derives pr_labels from cached pr_metadata", async () => {
    mockGetPullRequestById.mockResolvedValue({
      pullRequestId: 42,
      labels: [{ name: "ready" }, { name: "security" }, {}],
    });
    const { tracker } = makeTracker();

    const facts = await acquireFacts(
      gateSpec([fact("pr_metadata"), fact("pr_labels")]),
      tracker,
    );

    expect(facts.get("pr_labels")).toEqual(["ready", "security", ""]);
  });

  it("acquires changed_files from the last PR iteration and strips leading slashes", async () => {
    mockGetPullRequestIterations.mockResolvedValue([{ id: 3 }, { id: 7 }]);
    mockGetIterationChanges.mockResolvedValue({
      changeEntries: [
        { item: { path: "/src/main.ts" } },
        { item: { path: "docs/readme.md" } },
        { item: { path: "" } },
        {},
      ],
    });
    const { tracker } = makeTracker();

    const facts = await acquireFacts(gateSpec([fact("changed_files")]), tracker);

    expect(mockGetPullRequestIterations).toHaveBeenCalledWith("project", "repo", 42);
    expect(mockGetIterationChanges).toHaveBeenCalledWith("project", "repo", 42, 7);
    expect(facts.get("changed_files")).toEqual(["src/main.ts", "docs/readme.md"]);
  });

  it("returns an empty changed_files list when the PR has no iterations", async () => {
    mockGetPullRequestIterations.mockResolvedValue([]);
    const { tracker } = makeTracker();

    const facts = await acquireFacts(gateSpec([fact("changed_files")]), tracker);

    expect(facts.get("changed_files")).toEqual([]);
    expect(mockGetIterationChanges).not.toHaveBeenCalled();
  });

  it("derives changed_file_count from cached changed_files", async () => {
    mockGetPullRequestIterations.mockResolvedValue([{ id: 1 }]);
    mockGetIterationChanges.mockResolvedValue({
      changeEntries: [
        { item: { path: "/a.ts" } },
        { item: { path: "/b.ts" } },
        { item: { path: "/c.ts" } },
      ],
    });
    const { tracker } = makeTracker();

    const facts = await acquireFacts(
      gateSpec([fact("changed_files"), fact("changed_file_count")]),
      tracker,
    );

    expect(facts.get("changed_file_count")).toBe(3);
  });

  it("computes current_utc_minutes as a value in the current UTC day", async () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2024-01-02T12:34:56Z"));
    const { tracker } = makeTracker();

    const facts = await acquireFacts(gateSpec([fact("current_utc_minutes")]), tracker);

    const value = facts.get("current_utc_minutes");
    const now = new Date(Date.now());
    const expected = now.getUTCHours() * 60 + now.getUTCMinutes();
    expect(typeof value).toBe("number");
    expect(value).toBeGreaterThanOrEqual(0);
    expect(value).toBeLessThanOrEqual(1439);
    expect(value).toBe(expected);
  });
});
