import { describe, expect, it } from "vitest";

import type { ExecutedRecord, ScenarioContext } from "../scenario.js";
import { SkipError } from "../scenario.js";
import {
  createPullRequestAddReviewers,
  createPullRequestAddReviewersGeneral,
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
  reviewerId: "01234567-89ab-cdef-0123-456789abcdef",
} as unknown as AddReviewersState;

const reviewerVariants = [
  {
    name: "resolved GUID",
    scenario: createPullRequestAddReviewers,
    id: "create-pull-request-add-reviewers",
    temporaryId: "#aw_prreviewers",
    submittedReviewer: "01234567-89ab-cdef-0123-456789abcdef",
  },
  {
    name: "raw configured email/name",
    scenario: createPullRequestAddReviewersGeneral,
    id: "create-pull-request-add-reviewers-general",
    temporaryId: "#aw_prreviewersgeneral",
    submittedReviewer: "requester@example.com",
  },
] as const;

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
  it("registers both reviewer variants with distinct live PR state", async () => {
    const ids = createPullRequestScenarios.map(
      (scenario) => scenario.id ?? scenario.tool,
    );
    expect(ids).toContain("create-pull-request-add-reviewers");
    expect(ids).toContain("create-pull-request-add-reviewers-general");

    const guidPrior = await createPullRequestAddReviewers.priorEntries!(
      ctx,
      state,
    );
    const generalState = {
      ...state,
      sourceBranch: "source-general",
      patchRelPath: "create-pr-add-reviewers-general.patch",
    } as unknown as AddReviewersState;
    const generalPrior =
      await createPullRequestAddReviewersGeneral.priorEntries!(
        ctx,
        generalState,
      );
    expect(guidPrior[0]?.entry).toMatchObject({
      temporary_id: "#aw_prreviewers",
      source_branch: "source",
      patch_file: "create-pr-add-reviewers.patch",
    });
    expect(generalPrior[0]?.entry).toMatchObject({
      temporary_id: "#aw_prreviewersgeneral",
      source_branch: "source-general",
      patch_file: "create-pr-add-reviewers-general.patch",
    });
  });

  it.each(reviewerVariants)(
    "configures and submits one $name reviewer",
    async ({ scenario, temporaryId, submittedReviewer }) => {
      expect(scenario.config(ctx, state)).toEqual({
        "allowed-operations": ["add-reviewers"],
        "allowed-repositories": ["agent-definitions"],
        "allowed-reviewers": [submittedReviewer],
        "max-reviewers": 1,
        max: 1,
      });
      await expect(scenario.ndjson(ctx, state)).resolves.toEqual({
        pull_request_id: temporaryId,
        operation: "add-reviewers",
        reviewers: [submittedReviewer],
      });
    },
  );

  it.each(reviewerVariants)(
    "asserts $name temporary-ID resolution and live membership by resolved ID",
    async ({ scenario, temporaryId, submittedReviewer }) => {
      const listReviewers = async () => [
        {
          id: "01234567-89AB-CDEF-0123-456789ABCDEF",
          vote: 0,
          displayName: "Requester",
        },
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
          temporary_id: temporaryId,
        },
      };
      const updated: ExecutedRecord = {
        name: "update_pr",
        status: "succeeded",
        result: {
          pull_request_id: 42,
          operation: "add-reviewers",
          added: [submittedReviewer.toUpperCase()],
          failed: [],
        },
      };

      await expect(
        scenario.assert(ctx, assertionState, updated, [created, updated]),
      ).resolves.toBeUndefined();
      expect(assertionState.prId).toBe(42);
    },
  );

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
        added: ["01234567-89ab-cdef-0123-456789abcdef"],
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
        added: ["01234567-89ab-cdef-0123-456789abcdef"],
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
        added: ["01234567-89ab-cdef-0123-456789abcdef"],
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
        failed: ["01234567-89ab-cdef-0123-456789abcdef (HTTP 403)"],
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
        added: ["aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"],
        failed: [],
      },
    },
  ])("rejects when $name", async ({ created, updated }) => {
    const assertionState = {
      ...state,
      rest: {
        listReviewers: async () => [
          {
            id: "01234567-89ab-cdef-0123-456789abcdef",
            vote: 0,
            displayName: "Requester",
          },
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
        added: ["01234567-89ab-cdef-0123-456789abcdef"],
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
