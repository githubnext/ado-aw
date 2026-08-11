import { beforeEach, describe, expect, it, vi } from "vitest";

import type { ExecutedRecord, ScenarioContext } from "../scenario.js";
import { SkipError } from "../scenario.js";

vi.mock("../github-client.js", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../github-client.js")>();
  return {
    ...actual,
    addIssueAssignees: vi.fn(async () => {}),
    closeIssue: vi.fn(async () => {}),
    createIssueComment: vi.fn(async () => ({ id: 12, nodeId: "IC_12", body: "b", user: "u" })),
    createGitHubIssue: vi.fn(async () => "https://github.com/o/r/issues/123"),
    createMilestone: vi.fn(async () => ({ number: 9, title: "m" })),
    createRepoLabel: vi.fn(async () => {}),
    deleteIssueComment: vi.fn(async () => {}),
    deleteMilestone: vi.fn(async () => {}),
    deleteRepoLabel: vi.fn(async () => {}),
    diagnoseGitHubAuthFailure: vi.fn(async () => {}),
    findOpenIssueByTitle: vi.fn(async () => undefined),
    getAuthenticatedUser: vi.fn(async () => "octocat"),
    getCommentMinimization: vi.fn(async () => ({ isMinimized: true })),
    getIssue: vi.fn(async () => undefined),
    getIssueFieldValue: vi.fn(async () => undefined),
    getSubIssueParent: vi.fn(async () => undefined),
    listIssueComments: vi.fn(async () => []),
    listOrgIssueTypes: vi.fn(async () => []),
    listRepositoryIssueFields: vi.fn(async () => []),
    patchIssue: vi.fn(async () => ({ ok: false, status: 404, body: "" })),
    removeIssueAssignees: vi.fn(async () => {}),
    supportsGraphqlField: vi.fn(async () => true),
  };
});

const gh = await import("../github-client.js");
const {
  addGithubIssueLabels,
  assignGithubIssueMilestone,
  assignGithubIssueToUser,
  closeGithubIssue,
  commentOnGithubIssue,
  createGithubIssue,
  createGithubIssueLabelDenied,
  createGithubIssueTemporaryIdHandoff,
  githubIssueScenarios,
  hideGithubIssueComment,
  linkGithubSubIssue,
  recordForTool,
  removeGithubIssueLabels,
  resolveGithubIssueEnv,
  setGithubIssueField,
  setGithubIssueType,
  unassignGithubIssueFromUser,
  updateGithubIssue,
} = await import("../scenarios/github-issue.js");

const TEMPORARY_ID = "#aw_e2e1";
const REPO = "octo/scratch";

function fakeCtx(): ScenarioContext {
  return {
    orgUrl: "https://dev.azure.com/org/",
    project: "P",
    adoRepo: "agent-definitions",
    buildId: "77",
    token: "ado-token",
    adoAwBin: "ado-aw",
    workDir: "/tmp",
    rest: {} as ScenarioContext["rest"],
    log: () => {},
    prefix: (tool) => `ado-aw-det-77-${tool}`,
  };
}

function record(name: string, result: Record<string, unknown>): ExecutedRecord {
  return { name, status: "succeeded", result };
}

/** Records for a healthy handoff: create filed #501, set-type resolved #501. */
function handoffRecords(overrides: Record<string, unknown> = {}): ExecutedRecord[] {
  return [
    record("create_github_issue", {
      number: 501,
      url: "https://github.com/octo/scratch/issues/501",
      target_repo: REPO,
      temporary_id: TEMPORARY_ID,
    }),
    record("set_github_issue_type", {
      number: 501,
      target_repo: REPO,
      issue_type: "",
      ...overrides,
    }),
  ];
}

const goodEnv = {
  EXECUTOR_E2E_GITHUB_TOKEN: "tok",
  EXECUTOR_E2E_ISSUE_REPO: REPO,
} as NodeJS.ProcessEnv;

beforeEach(() => {
  vi.mocked(gh.getIssue).mockReset();
  vi.mocked(gh.getIssueFieldValue).mockReset();
  vi.mocked(gh.closeIssue).mockReset();
  vi.mocked(gh.findOpenIssueByTitle).mockReset();
  vi.mocked(gh.patchIssue).mockReset();
  vi.mocked(gh.listOrgIssueTypes).mockReset();
  vi.mocked(gh.listRepositoryIssueFields).mockReset();
  vi.mocked(gh.createGitHubIssue).mockReset();
  vi.mocked(gh.supportsGraphqlField).mockReset();
  vi.mocked(gh.getIssue).mockResolvedValue(undefined);
  vi.mocked(gh.getIssueFieldValue).mockResolvedValue(undefined);
  vi.mocked(gh.closeIssue).mockResolvedValue(undefined);
  vi.mocked(gh.findOpenIssueByTitle).mockResolvedValue(undefined);
  vi.mocked(gh.patchIssue).mockResolvedValue({ ok: false, status: 404, body: "" });
  vi.mocked(gh.listOrgIssueTypes).mockResolvedValue([]);
  vi.mocked(gh.listRepositoryIssueFields).mockResolvedValue([]);
  vi.mocked(gh.createGitHubIssue).mockResolvedValue("https://github.com/o/r/issues/123");
  vi.mocked(gh.supportsGraphqlField).mockResolvedValue(true);
});

describe("resolveGithubIssueEnv", () => {
  it("prefers the dedicated scenario repo over the failure-issue repo", () => {
    const env = resolveGithubIssueEnv("t", {
      EXECUTOR_E2E_GITHUB_TOKEN: "tok",
      EXECUTOR_E2E_SCENARIO_ISSUE_REPO: "octo/scenarios",
      EXECUTOR_E2E_ISSUE_REPO: "octo/failures",
    } as NodeJS.ProcessEnv);
    expect(env.repo).toBe("octo/scenarios");
  });

  it("falls back to the failure-issue repo", () => {
    expect(resolveGithubIssueEnv("t", goodEnv).repo).toBe(REPO);
  });

  it("skips when the token is missing", () => {
    expect(() =>
      resolveGithubIssueEnv("t", { EXECUTOR_E2E_ISSUE_REPO: REPO } as NodeJS.ProcessEnv),
    ).toThrow(SkipError);
  });

  it("treats an unexpanded ADO macro token as unset", () => {
    expect(() =>
      resolveGithubIssueEnv("t", {
        EXECUTOR_E2E_GITHUB_TOKEN: "$(EXECUTOR_E2E_GITHUB_TOKEN)",
        EXECUTOR_E2E_ISSUE_REPO: REPO,
      } as NodeJS.ProcessEnv),
    ).toThrow(SkipError);
  });

  it("skips rather than defaulting to a canonical repo when none is configured", () => {
    let thrown: unknown;
    try {
      resolveGithubIssueEnv("t", { EXECUTOR_E2E_GITHUB_TOKEN: "tok" } as NodeJS.ProcessEnv);
    } catch (err) {
      thrown = err;
    }
    expect(thrown).toBeInstanceOf(SkipError);
    expect((thrown as Error).message).not.toContain("githubnext/ado-aw");
  });

  it("treats an unexpanded repo macro as unset", () => {
    expect(() =>
      resolveGithubIssueEnv("t", {
        EXECUTOR_E2E_GITHUB_TOKEN: "tok",
        EXECUTOR_E2E_ISSUE_REPO: "$(EXECUTOR_E2E_ISSUE_REPO)",
      } as NodeJS.ProcessEnv),
    ).toThrow(SkipError);
  });
});

describe("registry", () => {
  it("registers the complete GitHub issue scenario family with unique ids", () => {
    const ids = githubIssueScenarios.map((s) => s.id ?? s.tool);
    expect(new Set(ids).size).toBe(20);
    expect(ids).toEqual([
      "create-github-issue",
      "create-github-issue-label-denied",
      "set-github-issue-type",
      "set-github-issue-type-clear",
      "create-github-issue-temporary-id-handoff",
      "comment-on-github-issue",
      "hide-github-issue-comment",
      "add-github-issue-labels",
      "remove-github-issue-labels",
      "close-github-issue",
      "update-github-issue",
      "set-github-issue-field",
      "assign-github-issue-milestone",
      "assign-github-issue-to-user",
      "unassign-github-issue-from-user",
      "link-github-sub-issue",
      "comment-on-github-issue-repo-denied",
      "add-github-issue-labels-blocked",
      "update-github-issue-filter-denied",
      "close-github-issue-state-denied",
    ]);
  });

  it("passes the harness token to the executor as ADO_AW_GITHUB_TOKEN", async () => {
    const state = { repo: REPO, token: "tok", gh: { token: "tok", repo: REPO }, title: "t" };
    const env = await createGithubIssue.env!(fakeCtx(), state);
    expect(env).toEqual({ ADO_AW_GITHUB_TOKEN: "tok" });
  });

  describe("new GitHub mutation contracts", () => {
    const base = {
      repo: REPO,
      token: "tok",
      gh: { token: "tok", repo: REPO },
      title: "scratch",
      issueNumber: 41,
    };

    it("registers exactly the signed-off eleven canonical tool names", () => {
      const names = [
        commentOnGithubIssue,
        hideGithubIssueComment,
        addGithubIssueLabels,
        removeGithubIssueLabels,
        closeGithubIssue,
        updateGithubIssue,
        setGithubIssueField,
        assignGithubIssueMilestone,
        assignGithubIssueToUser,
        unassignGithubIssueFromUser,
        linkGithubSubIssue,
      ].map((scenario) => scenario.tool);
      expect(names).toEqual([
        "comment-on-github-issue",
        "hide-github-issue-comment",
        "add-github-issue-labels",
        "remove-github-issue-labels",
        "close-github-issue",
        "update-github-issue",
        "set-github-issue-field",
        "assign-github-issue-milestone",
        "assign-github-issue-to-user",
        "unassign-github-issue-from-user",
        "link-github-sub-issue",
      ]);
    });

    it("uses the signed-off snake_case parameter objects", async () => {
      expect(await commentOnGithubIssue.ndjson(fakeCtx(), base)).toMatchObject({
        issue_number: 41,
        body: expect.any(String),
      });
      expect(
        await hideGithubIssueComment.ndjson(fakeCtx(), {
          ...base,
          commentId: 12,
          commentNodeId: "IC_12",
        }),
      ).toEqual({ comment_id: 12, reason: "spam", repository: REPO });
      expect(
        await addGithubIssueLabels.ndjson(fakeCtx(), { ...base, label: "e2e-label" }),
      ).toEqual({ issue_number: 41, labels: ["e2e-label"] });
      expect(
        await removeGithubIssueLabels.ndjson(fakeCtx(), { ...base, label: "e2e-label" }),
      ).toEqual({ issue_number: 41, labels: ["e2e-label"] });
      expect(await closeGithubIssue.ndjson(fakeCtx(), base)).toMatchObject({
        issue_number: 41,
        state_reason: "not_planned",
      });
      expect(
        await updateGithubIssue.ndjson(fakeCtx(), {
          ...base,
          updatedTitle: "new title",
          updatedBody: "new body",
        }),
      ).toEqual({
        issue_number: 41,
        title: "new title",
        body: "new body",
        operation: "replace",
      });
      expect(
        await setGithubIssueField.ndjson(fakeCtx(), {
          ...base,
          field: { id: "IF_1", name: "Priority", type: "IssueFieldText", options: [] },
          value: "high",
        }),
      ).toEqual({ issue_number: 41, field_name: "Priority", value: "high" });
      expect(
        await assignGithubIssueMilestone.ndjson(fakeCtx(), {
          ...base,
          milestoneNumber: 7,
          milestoneTitle: "m",
        }),
      ).toEqual({ issue_number: 41, milestone_number: 7 });
      expect(
        await assignGithubIssueToUser.ndjson(fakeCtx(), { ...base, assignee: "octocat" }),
      ).toEqual({ issue_number: 41, assignee: "octocat" });
      expect(
        await unassignGithubIssueFromUser.ndjson(fakeCtx(), { ...base, assignee: "octocat" }),
      ).toEqual({ issue_number: 41, assignee: "octocat" });
      expect(
        await linkGithubSubIssue.ndjson(fakeCtx(), {
          repo: REPO,
          token: "tok",
          gh: { token: "tok", repo: REPO },
          parentTitle: "parent",
          subTitle: "sub",
        }),
      ).toEqual({
        parent_issue_number: "#aw_parent",
        sub_issue_number: "#aw_sub",
      });
    });

    it("uses the signed-off kebab-case operator config", () => {
      expect(
        hideGithubIssueComment.config(fakeCtx(), {
          ...base,
          commentId: 12,
          commentNodeId: "IC_12",
        }),
      ).toMatchObject({ "target-repo": REPO, "allowed-reasons": ["SPAM"] });
      expect(
        closeGithubIssue.config(fakeCtx(), base),
      ).toMatchObject({ "allow-body": true, "allowed-state-reason": ["not_planned"] });
      expect(
        updateGithubIssue.config(fakeCtx(), {
          ...base,
          updatedTitle: "new",
          updatedBody: "body",
        }),
      ).toMatchObject({ title: true, body: true });
      expect(
        setGithubIssueField.config(fakeCtx(), {
          ...base,
          field: { id: "IF_1", name: "Priority", type: "IssueFieldText", options: [] },
          value: "high",
        }),
      ).toMatchObject({ "allowed-fields": ["Priority"] });
      expect(
        assignGithubIssueMilestone.config(fakeCtx(), {
          ...base,
          milestoneNumber: 7,
          milestoneTitle: "m",
        }),
      ).toMatchObject({ allowed: ["m"], "auto-create": false });
      expect(
        assignGithubIssueToUser.config(fakeCtx(), { ...base, assignee: "octocat" }),
      ).toMatchObject({ allowed: ["octocat"], blocked: [], "unassign-first": true });
    });

    it("stages both parent and child creates before link-github-sub-issue", async () => {
      const state = {
        repo: REPO,
        token: "tok",
        gh: { token: "tok", repo: REPO },
        parentTitle: "parent",
        subTitle: "sub",
      };
      const prior = await linkGithubSubIssue.priorEntries!(fakeCtx(), state);
      expect(prior.map((entry) => entry.entry.temporary_id)).toEqual([
        "#aw_parent",
        "#aw_sub",
      ]);
      expect(prior.every((entry) => entry.tool === "create-github-issue")).toBe(true);
      expect(prior.every((entry) => entry.config["require-temporary-id"] === true)).toBe(true);
    });

    it("skips preview GraphQL scenarios before creating issues when a field is unavailable", async () => {
      vi.stubEnv("EXECUTOR_E2E_GITHUB_TOKEN", "tok");
      vi.stubEnv("EXECUTOR_E2E_ISSUE_REPO", REPO);
      vi.mocked(gh.supportsGraphqlField).mockResolvedValue(false);
      await expect(hideGithubIssueComment.setup(fakeCtx())).rejects.toThrow(SkipError);
      expect(gh.createGitHubIssue).not.toHaveBeenCalled();
      vi.unstubAllEnvs();
    });
  });

  it("targets the configured repo explicitly rather than relying on resolution", () => {
    const state = {
      repo: REPO,
      token: "tok",
      gh: { token: "tok", repo: REPO },
      title: "t",
      label: "label",
      field: { id: "IF_1", name: "Priority", type: "IssueFieldText", options: [] },
      milestoneTitle: "milestone",
      assignee: "octocat",
    };
    for (const scenario of githubIssueScenarios) {
      const config = scenario.config(fakeCtx(), state as never);
      expect(config["target-repo"]).toBe(REPO);
    }
  });
});

describe("create-github-issue", () => {
  const state = () => ({
    repo: REPO,
    token: "tok",
    gh: { token: "tok", repo: REPO },
    title: "ado-aw-det-77-create-github-issue scratch issue",
    issueNumber: undefined as number | undefined,
  });

  it("applies the title prefix and merges static + allowed agent labels", () => {
    const config = createGithubIssue.config(fakeCtx(), state());
    expect(config["title-prefix"]).toBe("[executor-e2e] ");
    expect(config.labels).toEqual(["executor-e2e"]);
    expect(config["allowed-labels"]).toEqual(["executor-e2e-*"]);
  });

  it("asserts the prefixed title, footer marker and merged labels", async () => {
    const s = state();
    vi.mocked(gh.getIssue).mockResolvedValue({
      number: 123,
      title: `[executor-e2e] ${s.title}`,
      body: `Deterministic executor E2E exercising create-github-issue for build 77. Safe to delete.\n\n<!-- ado-aw -->`,
      state: "open",
      labels: ["executor-e2e", "executor-e2e-agent"],
    });
    await expect(
      createGithubIssue.assert(
        fakeCtx(),
        s,
        record("create_github_issue", { number: 123, target_repo: REPO }),
        [],
      ),
    ).resolves.toBeUndefined();
    expect(s.issueNumber).toBe(123);
  });

  it("fails when the executor drops the title prefix", async () => {
    const s = state();
    vi.mocked(gh.getIssue).mockResolvedValue({
      number: 123,
      title: s.title,
      body: "Deterministic executor E2E exercising create-github-issue for build 77. Safe to delete.\n\n<!-- ado-aw -->",
      state: "open",
      labels: ["executor-e2e", "executor-e2e-agent"],
    });
    await expect(
      createGithubIssue.assert(
        fakeCtx(),
        s,
        record("create_github_issue", { number: 123, target_repo: REPO }),
        [],
      ),
    ).rejects.toThrow(/issue title is/);
  });

  it("fails when a merged label is missing", async () => {
    const s = state();
    vi.mocked(gh.getIssue).mockResolvedValue({
      number: 123,
      title: `[executor-e2e] ${s.title}`,
      body: "Deterministic executor E2E exercising create-github-issue for build 77. Safe to delete.\n\n<!-- ado-aw -->",
      state: "open",
      labels: ["executor-e2e"],
    });
    await expect(
      createGithubIssue.assert(
        fakeCtx(),
        s,
        record("create_github_issue", { number: 123, target_repo: REPO }),
        [],
      ),
    ).rejects.toThrow(/missing 'executor-e2e-agent'/);
  });

  it("records the issue number before any fallible assertion so cleanup can close it", async () => {
    const s = state();
    // No issue returned -> the very next check throws, but the number must
    // already be captured for cleanup.
    vi.mocked(gh.getIssue).mockResolvedValue(undefined);
    await expect(
      createGithubIssue.assert(
        fakeCtx(),
        s,
        record("create_github_issue", { number: 456, target_repo: REPO }),
        [],
      ),
    ).rejects.toThrow(/was not created/);
    expect(s.issueNumber).toBe(456);
  });

  it("closes the issue it created during cleanup", async () => {
    const s = { ...state(), issueNumber: 99 };
    await createGithubIssue.cleanup(fakeCtx(), s);
    expect(gh.closeIssue).toHaveBeenCalledWith(s.gh, 99);
    expect(gh.findOpenIssueByTitle).not.toHaveBeenCalled();
  });

  it("falls back to the title marker when assert() never populated the number", async () => {
    const s = state();
    vi.mocked(gh.findOpenIssueByTitle).mockResolvedValue(321);
    await createGithubIssue.cleanup(fakeCtx(), s);
    expect(gh.findOpenIssueByTitle).toHaveBeenCalledWith(
      s.gh,
      `[executor-e2e] ado-aw-det-77-create-github-issue scratch issue`,
    );
    expect(gh.closeIssue).toHaveBeenCalledWith(s.gh, 321);
  });

  it("closes nothing when no marker-titled issue exists", async () => {
    await createGithubIssue.cleanup(fakeCtx(), state());
    expect(gh.closeIssue).not.toHaveBeenCalled();
  });
});

describe("create-github-issue allowed-labels rejection", () => {
  it("expects a default-deny rejection rather than a success", () => {
    expect(createGithubIssueLabelDenied.expectedFailure?.error.test(
      "Agent-supplied labels not in allowed-labels: definitely-not-allowed",
    )).toBe(true);
  });

  it("does NOT accept the 'no allowed-labels configured' message", () => {
    // That message means the executor never read the operator config, so
    // accepting it would make this scenario pass whether or not the
    // allowlist actually took effect.
    expect(createGithubIssueLabelDenied.expectedFailure?.error.test(
      'Agent-supplied labels rejected (no `allowed-labels` configured; set `allowed-labels: ["*"]` to permit any): x',
    )).toBe(false);
  });

  it("proposes a label outside the allowlist", async () => {
    const s = { repo: REPO, token: "tok", gh: { token: "tok", repo: REPO }, title: "t" };
    const entry = await createGithubIssueLabelDenied.ndjson(fakeCtx(), s);
    expect(entry.labels).toEqual(["definitely-not-allowed"]);
    const config = createGithubIssueLabelDenied.config(fakeCtx(), s);
    expect(config["allowed-labels"]).toEqual(["executor-e2e-*"]);
  });
});

describe("set-github-issue-type", () => {
  it("skips when the owner exposes no named issue types", async () => {
    vi.stubEnv("EXECUTOR_E2E_GITHUB_TOKEN", "tok");
    vi.stubEnv("EXECUTOR_E2E_ISSUE_REPO", REPO);
    vi.stubEnv("EXECUTOR_E2E_SCENARIO_ISSUE_REPO", "");
    vi.stubEnv("E2E_GITHUB_ISSUE_TYPE", "");
    vi.mocked(gh.listOrgIssueTypes).mockResolvedValue([]);
    await expect(setGithubIssueType.setup(fakeCtx())).rejects.toThrow(SkipError);
    // Nothing was created, so nothing can leak.
    expect(gh.createGitHubIssue).not.toHaveBeenCalled();
    vi.unstubAllEnvs();
  });

  it("fails when the executor targets a different issue", async () => {
    const s = {
      repo: REPO,
      token: "tok",
      gh: { token: "tok", repo: REPO },
      title: "t",
      issueNumber: 10,
      issueType: "Bug",
    };
    await expect(
      setGithubIssueType.assert(
        fakeCtx(),
        s,
        record("set_github_issue_type", { number: 11, target_repo: REPO, issue_type: "Bug" }),
        [],
      ),
    ).rejects.toThrow(/targeted issue #11, expected #10/);
  });
});

describe("set-github-issue-field", () => {
  const field = {
    id: "IF_1",
    name: "Estimate",
    type: "IssueFieldNumber",
    options: [],
  };
  const state = () => ({
    repo: REPO,
    token: "tok",
    gh: { token: "tok", repo: REPO },
    title: "ado-aw-det-77-set-github-issue-field scratch issue",
    issueNumber: 123,
    field,
    value: "42",
  });
  const output = () =>
    record("set_github_issue_field", {
      field_name: field.name,
      value: "42",
    });

  it("reads GitHub after execution and asserts the persisted field value and type", async () => {
    const s = state();
    vi.mocked(gh.getIssueFieldValue).mockResolvedValue({
      fieldId: field.id,
      fieldName: field.name,
      fieldType: field.type,
      valueType: "IssueFieldNumberValue",
      value: 42,
    });

    await expect(
      setGithubIssueField.assert(fakeCtx(), s, output(), []),
    ).resolves.toBeUndefined();
    expect(gh.getIssueFieldValue).toHaveBeenCalledWith(s.gh, s.issueNumber, field.id);
  });

  it("fails when executor output is healthy but GitHub did not persist the value", async () => {
    await expect(
      setGithubIssueField.assert(fakeCtx(), state(), output(), []),
    ).rejects.toThrow(/has no persisted value/);
  });

  it("fails when GitHub persisted a different field value type", async () => {
    vi.mocked(gh.getIssueFieldValue).mockResolvedValue({
      fieldId: field.id,
      fieldName: field.name,
      fieldType: field.type,
      valueType: "IssueFieldTextValue",
      value: "42",
    });

    await expect(
      setGithubIssueField.assert(fakeCtx(), state(), output(), []),
    ).rejects.toThrow(/value type is 'IssueFieldTextValue'/);
  });

  it("fails when GitHub persisted a different value", async () => {
    vi.mocked(gh.getIssueFieldValue).mockResolvedValue({
      fieldId: field.id,
      fieldName: field.name,
      fieldType: field.type,
      valueType: "IssueFieldNumberValue",
      value: 41,
    });

    await expect(
      setGithubIssueField.assert(fakeCtx(), state(), output(), []),
    ).rejects.toThrow(/persisted issue field value is 41, expected 42/);
  });

  it("closes the deterministic scratch issue during cleanup", async () => {
    const s = state();
    await setGithubIssueField.cleanup(fakeCtx(), s);
    expect(gh.closeIssue).toHaveBeenCalledWith(s.gh, s.issueNumber);
  });

  it("skips before creating an issue when the read-side preview API is unavailable", async () => {
    vi.stubEnv("EXECUTOR_E2E_GITHUB_TOKEN", "tok");
    vi.stubEnv("EXECUTOR_E2E_ISSUE_REPO", REPO);
    vi.mocked(gh.supportsGraphqlField).mockImplementation(
      async (_opts, type, name) => !(type === "Issue" && name === "issueFieldValues"),
    );
    vi.mocked(gh.listRepositoryIssueFields).mockResolvedValue([field]);

    await expect(setGithubIssueField.setup(fakeCtx())).rejects.toThrow(SkipError);
    expect(gh.createGitHubIssue).not.toHaveBeenCalled();
    vi.unstubAllEnvs();
  });
});

describe("temporary-ID handoff", () => {
  const state = () => ({
    repo: REPO,
    token: "tok",
    gh: { token: "tok", repo: REPO },
    title: "ado-aw-det-77-create-github-issue-temporary-id-handoff scratch issue",
    issueType: "",
    issueNumber: undefined as number | undefined,
  });

  function issueMatching(s: ReturnType<typeof state>, type?: string) {
    return { number: 501, title: s.title, body: "b", state: "open", labels: [], type };
  }

  it("stages create-github-issue ahead of the primary set-github-issue-type entry", async () => {
    const s = state();
    const prior = await createGithubIssueTemporaryIdHandoff.priorEntries!(fakeCtx(), s);
    expect(prior).toHaveLength(1);
    expect(prior[0]!.tool).toBe("create-github-issue");
    expect(prior[0]!.entry.temporary_id).toBe(TEMPORARY_ID);
    // require-temporary-id proves the producer is exercised on its strict path.
    expect(prior[0]!.config["require-temporary-id"]).toBe(true);

    // The primary entry CONSUMES the temporary id.
    expect(createGithubIssueTemporaryIdHandoff.tool).toBe("set-github-issue-type");
    const entry = await createGithubIssueTemporaryIdHandoff.ndjson(fakeCtx(), s);
    expect(entry.issue_number).toBe(TEMPORARY_ID);
  });

  it("passes when the temporary id resolves to the issue that was actually filed", async () => {
    const s = state();
    vi.mocked(gh.getIssue).mockResolvedValue(issueMatching(s));
    await expect(
      createGithubIssueTemporaryIdHandoff.assert(
        fakeCtx(),
        s,
        handoffRecords()[1]!,
        handoffRecords(),
      ),
    ).resolves.toBeUndefined();
    expect(s.issueNumber).toBe(501);
  });

  // ---- MUTATION CHECKS -----------------------------------------------------
  // These deliberately break the handoff. If the assertion were vacuous (e.g.
  // it only checked that the record existed) these would pass, which is exactly
  // the failure mode this scenario is meant to rule out.

  it("MUTATION: fails when the resolved issue number does not match the filed one", async () => {
    const s = state();
    vi.mocked(gh.getIssue).mockResolvedValue(issueMatching(s));
    const records = handoffRecords({ number: 999 });
    await expect(
      createGithubIssueTemporaryIdHandoff.assert(fakeCtx(), s, records[1]!, records),
    ).rejects.toThrow(/resolved to issue #999, but create-github-issue filed #501/);
  });

  it("MUTATION: fails when the resolved repository does not match the filed one", async () => {
    const s = state();
    vi.mocked(gh.getIssue).mockResolvedValue(issueMatching(s));
    const records = handoffRecords({ target_repo: "someone/else" });
    await expect(
      createGithubIssueTemporaryIdHandoff.assert(fakeCtx(), s, records[1]!, records),
    ).rejects.toThrow(/resolved to repository 'someone\/else'/);
  });

  it("MUTATION: fails when the producer echoes a different temporary id", async () => {
    const s = state();
    vi.mocked(gh.getIssue).mockResolvedValue(issueMatching(s));
    const records = handoffRecords();
    records[0]!.result!.temporary_id = "#aw_other";
    await expect(
      createGithubIssueTemporaryIdHandoff.assert(fakeCtx(), s, records[1]!, records),
    ).rejects.toThrow(/reported temporary_id '#aw_other'/);
  });

  it("MUTATION: fails when GitHub has no such issue, so a fabricated result cannot pass", async () => {
    const s = state();
    vi.mocked(gh.getIssue).mockResolvedValue(undefined);
    const records = handoffRecords();
    await expect(
      createGithubIssueTemporaryIdHandoff.assert(fakeCtx(), s, records[1]!, records),
    ).rejects.toThrow(/does not exist on/);
  });

  it("MUTATION: fails when the named type was not applied", async () => {
    const s = { ...state(), issueType: "Bug" };
    vi.mocked(gh.getIssue).mockResolvedValue(issueMatching(s, undefined));
    const records = handoffRecords({ issue_type: "Bug" });
    await expect(
      createGithubIssueTemporaryIdHandoff.assert(fakeCtx(), s, records[1]!, records),
    ).rejects.toThrow(/has type '\(none\)', expected 'Bug'/);
  });

  it("captures the filed issue number before any fallible check", async () => {
    const s = state();
    vi.mocked(gh.getIssue).mockResolvedValue(undefined);
    const records = handoffRecords({ number: 999 });
    await expect(
      createGithubIssueTemporaryIdHandoff.assert(fakeCtx(), s, records[1]!, records),
    ).rejects.toThrow();
    expect(s.issueNumber).toBe(501);
  });

  it("closes the filed issue during cleanup", async () => {
    const s = { ...state(), issueNumber: 501 };
    await createGithubIssueTemporaryIdHandoff.cleanup(fakeCtx(), s);
    expect(gh.closeIssue).toHaveBeenCalledWith(s.gh, 501);
  });

  it("falls back to the title marker when the run failed before assert()", async () => {
    const s = state();
    vi.mocked(gh.findOpenIssueByTitle).mockResolvedValue(777);
    await createGithubIssueTemporaryIdHandoff.cleanup(fakeCtx(), s);
    expect(gh.findOpenIssueByTitle).toHaveBeenCalledWith(s.gh, s.title);
    expect(gh.closeIssue).toHaveBeenCalledWith(s.gh, 777);
  });
});

describe("recordForTool", () => {
  it("maps kebab-case tool names onto snake_case record names", () => {
    const records = handoffRecords();
    expect(recordForTool(records, "create-github-issue").result!.number).toBe(501);
  });

  it("throws when the record is absent", () => {
    expect(() => recordForTool([], "create-github-issue")).toThrow(/no executed record/);
  });
});
