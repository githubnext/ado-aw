import { spawnSync } from "node:child_process";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { beforeAll, describe, expect, it } from "vitest";
import { parse as parseYaml } from "yaml";
import { main as renderApprovalSummary } from "../../approval-summary/index.js";

import { runExecute } from "../execute-cli.js";
import type {
  ExecutedRecord,
  PriorEntry,
  Scenario,
  ScenarioContext,
} from "../scenario.js";
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
    temporaryId: "#aw_prreviewgen",
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
      temporary_id: "#aw_prreviewgen",
      source_branch: "source-general",
      patch_file: "create-pr-add-reviewers-general.patch",
    });
  });

  it.each(reviewerVariants)(
    "configures and submits one $name reviewer",
    async ({ scenario, temporaryId, submittedReviewer }) => {
      expect(scenario.config(ctx, state)).toEqual({
        target: "*",
        "allowed-repositories": ["agent-definitions"],
        "allowed-reviewers": [submittedReviewer],
        "max-reviewers": 1,
        max: 1,
      });
      await expect(scenario.ndjson(ctx, state)).resolves.toEqual({
        pull_request_id: temporaryId,
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
        name: "add_pull_request_reviewers",
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
        name: "add_pull_request_reviewers",
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
      name: "add_pull_request_reviewers",
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

describe("Rust executor payload contract", () => {
  let adoAwBin: string;
  const cargoTimeoutMs = 10 * 60 * 1000;

  beforeAll(() => {
    const manifest = fileURLToPath(
      new URL("../../../../../Cargo.toml", import.meta.url),
    );
    const build = spawnSync(
      "cargo",
      [
        "build",
        "--manifest-path",
        manifest,
        "--bin",
        "ado-aw",
        "--message-format=json",
      ],
      { encoding: "utf8", timeout: cargoTimeoutMs, maxBuffer: 16 * 1024 * 1024 },
    );
    if (build.error) throw build.error;
    expect(build.status, `Failed to build the Rust executor:\n${build.stderr}`).toBe(0);

    for (const line of build.stdout.trim().split(/\r?\n/)) {
      const artifact = JSON.parse(line) as {
        reason: string;
        target?: { name: string };
        executable?: string | null;
      };
      if (
        artifact.reason === "compiler-artifact" &&
        artifact.target?.name === "ado-aw" &&
        artifact.executable
      ) {
        adoAwBin = artifact.executable;
      }
    }
    expect(adoAwBin, "Cargo must report the freshly built ado-aw executable").toBeTruthy();
  }, cargoTimeoutMs);

  function previewPolicyEnv(value: unknown): string | undefined {
    if (Array.isArray(value)) {
      for (const child of value) {
        const result = previewPolicyEnv(child);
        if (result !== undefined) return result;
      }
    } else if (value !== null && typeof value === "object") {
      const object = value as Record<string, unknown>;
      if (object.env !== null && typeof object.env === "object" && !Array.isArray(object.env)) {
        const env = object.env as Record<string, unknown>;
        if (typeof env.AW_PR_POLICIES === "string") return env.AW_PR_POLICIES;
      }
      for (const child of Object.values(object)) {
        const result = previewPolicyEnv(child);
        if (result !== undefined) return result;
      }
    }
    return undefined;
  }

  it.each([42, "42", "9007199254740993"])(
    "renders the Rust compiler's fixed target %s without falling back to trigger 7",
    async (target) => {
      const dir = await mkdtemp(join(tmpdir(), "ado-aw-preview-contract-"));
      try {
        const source = join(dir, "workflow.md");
        await writeFile(source, "---\n" + JSON.stringify({
          name: "preview-contract", description: "Compiler to preview target contract",
          "safe-outputs": {
            "update-pull-request": { target, "include-stats": false },
            "abandon-pull-request": { target, "include-stats": false },
          },
        }) + "\n---\nReview fixture.\n");
        const compiled = spawnSync(adoAwBin, ["compile", source], {
          cwd: dir, encoding: "utf8", timeout: 30000,
          env: { ...process.env, ADO_AW_LOG_DIR: join(dir, "logs"),
            ADO_AW_COMPILE_REMOTE_URL: "https://dev.azure.com/org/P/_git/repo" },
        });
        if (compiled.error) throw compiled.error;
        expect(compiled.status, compiled.stderr).toBe(0);
        const pipeline: unknown = parseYaml(await readFile(join(dir, "workflow.lock.yml"), "utf8"));
        const policies = previewPolicyEnv(pipeline);
        expect(policies, "compiler must emit preview policy").toBeDefined();
        const proposals = join(dir, "safe_outputs.ndjson");
        const summary = join(dir, "ado-aw-safe-outputs.md");
        await writeFile(proposals, [
          { name: "update-pull-request", body: "Update report." },
          { name: "abandon-pull-request", body: "Abandonment reason." },
        ].map((record) => JSON.stringify(record)).join("\n"));
        expect(renderApprovalSummary({
          AW_SAFE_OUTPUTS_NDJSON: proposals, AW_APPROVAL_SUMMARY_OUT: summary,
          AW_PR_POLICIES: policies, SYSTEM_PULLREQUEST_PULLREQUESTID: "7",
          BUILD_REASON: "PullRequest", BUILD_REPOSITORY_PROVIDER: "TfsGit",
          BUILD_REPOSITORY_ID: "11111111-2222-3333-4444-555555555555",
          BUILD_REPOSITORY_URI: "https://dev.azure.com/org/P/_git/repo",
          SYSTEM_COLLECTIONURI: "https://dev.azure.com/org/",
          SYSTEM_TEAMPROJECT: "P",
        })).toBe(0);
        const markdown = await readFile(summary, "utf8");
        expect(markdown.split("| PR | " + String(target) + " |")).toHaveLength(3);
        expect(markdown).not.toContain("| PR | 7 |");
        expect(markdown).not.toContain("9007199254740992");
      } finally {
        await rm(dir, { recursive: true, force: true });
      }
    },
  );

  async function parseScenario(
    scenario: Scenario<unknown>,
    mutate?: (prior: PriorEntry[], entry: Record<string, unknown>) => void,
  ) {
    const dir = await mkdtemp(join(tmpdir(), "ado-aw-payload-contract-"));
    try {
      // Use the live scenario's builders, but never its remote setup/assert/cleanup.
      const priorEntries = await scenario.priorEntries?.(ctx, state) ?? [];
      const entry = await scenario.ndjson(ctx, state);
      mutate?.(priorEntries, entry);
      return await runExecute({
        adoAwBin,
        dryRun: true,
        scenarioDir: dir,
        tool: scenario.tool,
        config: scenario.config(ctx, state),
        entry,
        priorEntries,
        adoRepo: state.repo,
        orgUrl: ctx.orgUrl,
        project: ctx.project,
        token: "",
        extraEnv: {
          ADO_AW_LOG_DIR: join(dir, "logs"),
          EXECUTOR_E2E_EXECUTE_TIMEOUT_MS: "15000",
        },
        log: () => {},
      });
    } finally {
      await rm(dir, { recursive: true, force: true });
    }
  }

  it.each(createPullRequestScenarios)(
    "accepts generated $id proposals without credentials or remote setup",
    async (scenario) => {
      const prior = await scenario.priorEntries?.(ctx, state) ?? [];
      const expectedNames = [...prior.map(({ tool }) => tool), scenario.tool]
        .map((name) => name.replaceAll("-", "_"));
      const result = await parseScenario(scenario);

      expect(result.exitCode, result.stdout + result.stderr).toBe(0);
      expect(result.records.map(({ name, status }) => ({ name, status }))).toEqual(
        expectedNames.map((name) => ({ name, status: "succeeded" })),
      );
      expect(result.stdout.match(/\[DRY-RUN\]/g)).toHaveLength(expectedNames.length);
    },
  );

  it.each([
    { target: "producer", tool: "create-pull-request", index: 0 },
    { target: "consumer", tool: "add-pull-request-reviewers", index: 1 },
  ] as const)(
    "rejects an overlong $target temporary ID through Rust deserialization",
    async ({ target, tool, index }) => {
      const result = await parseScenario(
        createPullRequestAddReviewersGeneral,
        (prior, entry) => {
          // Regression: this former fixture ID passed the string-equality tests.
          const invalidId = "#aw_prreviewersgeneral";
          if (target === "producer") {
            prior[0]!.entry.temporary_id = invalidId;
          } else {
            entry.pull_request_id = invalidId;
          }
        },
      );

      expect(result.exitCode, result.stdout + result.stderr).toBe(1);
      expect(result.records).toHaveLength(2);
      expect(result.records[index]?.status).toBe("failed");
      expect(result.records[index]?.error).toContain(`Failed to parse ${tool}:`);
      expect(result.records[index]?.error).toContain("3-12 ASCII alphanumeric/underscore");
      expect(result.records[1 - index]?.status).toBe("succeeded");
    },
  );

  it("rejects a producer missing its internal temporary_id", async () => {
    const result = await parseScenario(createPullRequestAddReviewers, (prior) => {
      delete prior[0]!.entry.temporary_id;
    });

    expect(result.exitCode, result.stdout + result.stderr).toBe(1);
    expect(result.records).toHaveLength(2);
    expect(result.records[0]?.status).toBe("failed");
    expect(result.records[0]?.error).toContain("Failed to parse create-pull-request:");
    expect(result.records[0]?.error).toContain("missing field `temporary_id`");
  });

  it("rejects a consumer with a non-array reviewers payload", async () => {
    const result = await parseScenario(
      createPullRequestAddReviewersGeneral,
      (_prior, entry) => {
        entry.reviewers = state.reviewer;
      },
    );

    expect(result.exitCode, result.stdout + result.stderr).toBe(1);
    expect(result.records).toHaveLength(2);
    expect(result.records[0]?.status).toBe("succeeded");
    expect(result.records[1]?.status).toBe("failed");
    expect(result.records[1]?.error).toContain("Failed to parse add-pull-request-reviewers:");
    expect(result.records[1]?.error).toContain("expected a sequence");
  });
});
