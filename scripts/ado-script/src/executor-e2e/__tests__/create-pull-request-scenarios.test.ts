import { describe, expect, it } from "vitest";

import type { ExecutedRecord, ScenarioContext } from "../scenario.js";
import { SkipError } from "../scenario.js";
import {
  createPullRequestAddReviewers,
  createPullRequestScenarios,
  resolveExecutorE2eReviewer,
} from "../scenarios/create-pull-request.js";

const ctx = {
  orgUrl: "https://dev.azure.com/org/",
  project: "P",
  adoRepo: "agent-definitions",
  buildId: "77",
  token: "ado-token",
  adoAwBin: "ado-aw",
  workDir: "work",
  rest: {},
  log: () => {},
  prefix: (tool: string) => `ado-aw-det-77-${tool}`,
} as unknown as ScenarioContext;

type AddReviewersState = Parameters<
  typeof createPullRequestAddReviewers.config
>[1];

const state = {
  repo: "agent-definitions",
  sourceBranch: "source",
  targetBranch: "main",
  baseCommit: "a".repeat(40),
  patchRelPath: "create-pr-add-reviewers.patch",
  patchSha256: "b".repeat(64),
  patchContent: "patch",
  sourcesDir: "sources",
  checkoutDir: "checkout",
  rest: {},
  executorToken: "token",
  repositorySelector: "agent-definitions",
  reviewer: "requester@example.com",
  reviewerId: "reviewer-id",
} as unknown as AddReviewersState;

describe("resolveExecutorE2eReviewer", () => {
  it("trims the dedicated reviewer environment value", () => {
    expect(
      resolveExecutorE2eReviewer({
        EXECUTOR_E2E_REVIEWER: "  requester@example.com  ",
      }),
    ).toBe("requester@example.com");
  });

  it.each([{}, { EXECUTOR_E2E_REVIEWER: "  " }, {
    EXECUTOR_E2E_REVIEWER: "$(Build.RequestedForEmail)",
  }])("skips unavailable or unexpanded values", (env) => {
    expect(() => resolveExecutorE2eReviewer(env)).toThrow(SkipError);
  });
});

describe("create-pull-request add-reviewers handoff", () => {
  it("is registered with the constrained reviewer policy", () => {
    const ids = createPullRequestScenarios.map(
      (scenario) => scenario.id ?? scenario.tool,
    );
    expect(ids).toContain("create-pull-request-add-reviewers");
    expect(createPullRequestAddReviewers.config(ctx, state)).toEqual({
      "allowed-operations": ["add-reviewers"],
      "allowed-repositories": ["agent-definitions"],
      "allowed-reviewers": ["requester@example.com"],
      "max-reviewers": 1,
      max: 1,
    });
  });

  it("stages create first and submits the reviewer against its temporary ID", async () => {
    const prior = await createPullRequestAddReviewers.priorEntries!(ctx, state);
    expect(prior).toEqual([
      expect.objectContaining({
        tool: "create-pull-request",
        entry: expect.objectContaining({
          temporary_id: "#aw_prreviewers",
          source_branch: "source",
        }),
      }),
    ]);
    await expect(
      createPullRequestAddReviewers.ndjson(ctx, state),
    ).resolves.toEqual({
      pull_request_id: "#aw_prreviewers",
      operation: "add-reviewers",
      reviewers: ["requester@example.com"],
    });
  });

  it("asserts temporary-ID resolution and live reviewer membership by identity ID", async () => {
    const listReviewers = async () => [
      { id: "REVIEWER-ID", vote: 0, displayName: "Requester" },
    ];
    const assertionState = {
      ...state,
      rest: { listReviewers },
    } as unknown as AddReviewersState;
    const created: ExecutedRecord = {
      name: "create_pull_request",
      status: "succeeded",
      result: {
        pull_request_id: 42,
        temporary_id: "#aw_prreviewers",
      },
    };
    const updated: ExecutedRecord = {
      name: "update_pr",
      status: "succeeded",
      result: {
        pull_request_id: 42,
        operation: "add-reviewers",
        added: ["REQUESTER@example.com"],
        failed: [],
      },
    };

    await expect(
      createPullRequestAddReviewers.assert(
        ctx,
        assertionState,
        updated,
        [created, updated],
      ),
    ).resolves.toBeUndefined();
    expect(assertionState.prId).toBe(42);
  });

  it.each([
    {
      name: "the producer temporary ID differs",
      created: {
        pull_request_id: 42,
        temporary_id: "#aw_wrong",
      },
      updated: {
        pull_request_id: 42,
        operation: "add-reviewers",
        added: ["requester@example.com"],
        failed: [],
      },
    },
    {
      name: "the consumer resolves a different PR",
      created: {
        pull_request_id: 42,
        temporary_id: "#aw_prreviewers",
      },
      updated: {
        pull_request_id: 43,
        operation: "add-reviewers",
        added: ["requester@example.com"],
        failed: [],
      },
    },
    {
      name: "the operation differs",
      created: {
        pull_request_id: 42,
        temporary_id: "#aw_prreviewers",
      },
      updated: {
        pull_request_id: 42,
        operation: "update-description",
        added: ["requester@example.com"],
        failed: [],
      },
    },
    {
      name: "a reviewer fails",
      created: {
        pull_request_id: 42,
        temporary_id: "#aw_prreviewers",
      },
      updated: {
        pull_request_id: 42,
        operation: "add-reviewers",
        added: [],
        failed: ["requester@example.com (HTTP 403)"],
      },
    },
    {
      name: "the configured reviewer is absent from added",
      created: {
        pull_request_id: 42,
        temporary_id: "#aw_prreviewers",
      },
      updated: {
        pull_request_id: 42,
        operation: "add-reviewers",
        added: ["someone@example.com"],
        failed: [],
      },
    },
  ])("rejects when $name", async ({ created, updated }) => {
    const assertionState = {
      ...state,
      rest: {
        listReviewers: async () => [
          { id: "reviewer-id", vote: 0, displayName: "Requester" },
        ],
      },
    } as unknown as AddReviewersState;
    const records: ExecutedRecord[] = [
      {
        name: "create_pull_request",
        status: "succeeded",
        result: created,
      },
      {
        name: "update_pr",
        status: "succeeded",
        result: updated,
      },
    ];

    await expect(
      createPullRequestAddReviewers.assert(
        ctx,
        assertionState,
        records[1]!,
        records,
      ),
    ).rejects.toThrow();
  });

  it("rejects when the reviewer is absent from live ADO state", async () => {
    const assertionState = {
      ...state,
      rest: { listReviewers: async () => [] },
    } as unknown as AddReviewersState;
    const created: ExecutedRecord = {
      name: "create_pull_request",
      status: "succeeded",
      result: {
        pull_request_id: 42,
        temporary_id: "#aw_prreviewers",
      },
    };
    const updated: ExecutedRecord = {
      name: "update_pr",
      status: "succeeded",
      result: {
        pull_request_id: 42,
        operation: "add-reviewers",
        added: ["requester@example.com"],
        failed: [],
      },
    };

    await expect(
      createPullRequestAddReviewers.assert(
        ctx,
        assertionState,
        updated,
        [created, updated],
      ),
    ).rejects.toThrow("does not contain reviewer identity");
  });
});
