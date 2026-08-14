import { describe, expect, it, vi } from "vitest";

import {
  createRepoLabel,
  getIssue,
  getIssueFieldValue,
  getSubIssueParent,
  githubGraphql,
  listRepositoryIssueFields,
  supportsGraphqlField,
  unlinkSubIssue,
} from "../github-client.js";

const base = { token: "token", repo: "octo/scratch" };

describe("executor E2E GitHub client", () => {
  it("parses mutation-relevant issue fields", async () => {
    const fetchImpl = vi.fn(async () =>
      new Response(
        JSON.stringify({
          number: 7,
          node_id: "I_7",
          title: "title",
          body: "body",
          state: "closed",
          state_reason: "not_planned",
          labels: [{ name: "bug" }],
          assignees: [{ login: "octocat" }],
          milestone: { number: 3, title: "v1" },
        }),
        { status: 200 },
      ),
    ) as unknown as typeof fetch;

    await expect(getIssue({ ...base, fetchImpl }, 7)).resolves.toEqual({
      number: 7,
      nodeId: "I_7",
      title: "title",
      body: "body",
      state: "closed",
      stateReason: "not_planned",
      labels: ["bug"],
      assignees: ["octocat"],
      milestone: { number: 3, title: "v1" },
      type: undefined,
    });
  });

  it("probes preview fields through GraphQL introspection", async () => {
    const fetchImpl = vi.fn(async (_url: string, init?: RequestInit) => {
      const body = JSON.parse(String(init?.body)) as { variables: { type: string } };
      expect(body.variables.type).toBe("Mutation");
      return new Response(
        JSON.stringify({
          data: { __type: { fields: [{ name: "minimizeComment" }] } },
        }),
        { status: 200 },
      );
    }) as unknown as typeof fetch;

    await expect(
      supportsGraphqlField({ ...base, fetchImpl }, "Mutation", "minimizeComment"),
    ).resolves.toBe(true);
    expect(fetchImpl).toHaveBeenCalledWith(
      "https://api.github.com/graphql",
      expect.objectContaining({ method: "POST" }),
    );
  });

  it("surfaces GraphQL product errors instead of treating them as unsupported", async () => {
    const fetchImpl = vi.fn(async () =>
      new Response(
        JSON.stringify({ errors: [{ message: "Resource not accessible by integration" }] }),
        { status: 200 },
      ),
    ) as unknown as typeof fetch;

    await expect(
      githubGraphql({ ...base, fetchImpl }, "query { viewer { login } }"),
    ).rejects.toThrow(/Resource not accessible/);
  });

  it("discovers supported repository issue fields", async () => {
    const fetchImpl = vi.fn(async () =>
      new Response(
        JSON.stringify({
          data: {
            repository: {
              issueFields: {
                nodes: [
                  { id: "IF_1", name: "Priority", __typename: "IssueFieldSingleSelect",
                    options: [{ id: "O_1", name: "High" }] },
                  { id: null, name: "invalid", __typename: "IssueFieldText" },
                ],
              },
            },
          },
        }),
        { status: 200 },
      ),
    ) as unknown as typeof fetch;

    await expect(listRepositoryIssueFields({ ...base, fetchImpl })).resolves.toEqual([
      {
        id: "IF_1",
        name: "Priority",
        type: "IssueFieldSingleSelect",
        options: [{ id: "O_1", name: "High" }],
      },
    ]);
  });

  it("reads persisted repository issue field values with their concrete types", async () => {
    const fetchImpl = vi.fn(async (_url: string, init?: RequestInit) => {
      const body = JSON.parse(String(init?.body)) as {
        query: string;
        variables: Record<string, unknown>;
      };
      expect(body.query).toContain("issueFieldValues");
      expect(body.variables).toEqual({ owner: "octo", repo: "scratch", number: 7 });
      return new Response(
        JSON.stringify({
          data: {
            repository: {
              issue: {
                issueFieldValues: {
                  nodes: [
                    {
                      __typename: "IssueFieldTextValue",
                      value: "persisted text",
                      field: {
                        id: "IF_TEXT",
                        name: "Details",
                        __typename: "IssueFieldText",
                      },
                    },
                    {
                      __typename: "IssueFieldNumberValue",
                      value: 42,
                      field: {
                        id: "IF_NUMBER",
                        name: "Estimate",
                        __typename: "IssueFieldNumber",
                      },
                    },
                    {
                      __typename: "IssueFieldSingleSelectValue",
                      name: "High",
                      field: {
                        id: "IF_SELECT",
                        name: "Priority",
                        __typename: "IssueFieldSingleSelect",
                      },
                    },
                  ],
                },
              },
            },
          },
        }),
        { status: 200 },
      );
    }) as unknown as typeof fetch;

    await expect(
      getIssueFieldValue({ ...base, fetchImpl }, 7, "IF_NUMBER"),
    ).resolves.toEqual({
      fieldId: "IF_NUMBER",
      fieldName: "Estimate",
      fieldType: "IssueFieldNumber",
      valueType: "IssueFieldNumberValue",
      value: 42,
    });
    await expect(
      getIssueFieldValue({ ...base, fetchImpl }, 7, "IF_SELECT"),
    ).resolves.toEqual({
      fieldId: "IF_SELECT",
      fieldName: "Priority",
      fieldType: "IssueFieldSingleSelect",
      valueType: "IssueFieldSingleSelectValue",
      value: "High",
    });
  });

  it("returns undefined when the selected issue field has no persisted value", async () => {
    const fetchImpl = vi.fn(async () =>
      new Response(
        JSON.stringify({
          data: {
            repository: { issue: { issueFieldValues: { nodes: [] } } },
          },
        }),
        { status: 200 },
      ),
    ) as unknown as typeof fetch;

    await expect(
      getIssueFieldValue({ ...base, fetchImpl }, 7, "IF_MISSING"),
    ).resolves.toBeUndefined();
  });

  it("reads a sub-issue's parent for independent assertion", async () => {
    const fetchImpl = vi.fn(async () =>
      new Response(
        JSON.stringify({
          data: { repository: { issue: { parent: { number: 11 } } } },
        }),
        { status: 200 },
      ),
    ) as unknown as typeof fetch;

    await expect(getSubIssueParent({ ...base, fetchImpl }, 12)).resolves.toBe(11);
  });

  it("treats a deterministic label left by a same-build retry as already set up", async () => {
    const fetchImpl = vi.fn(async () =>
      new Response(JSON.stringify({ errors: [{ code: "already_exists" }] }), {
        status: 422,
      }),
    ) as unknown as typeof fetch;

    await expect(
      createRepoLabel({ ...base, fetchImpl }, "executor-e2e-77-add"),
    ).resolves.toBeUndefined();
  });

  it("unlinks scratch sub-issues during cleanup", async () => {
    const fetchImpl = vi.fn(async (url: string, init?: RequestInit) => {
      if (url.endsWith("/issues/11")) {
        return new Response(JSON.stringify({ number: 11, node_id: "I_parent" }), { status: 200 });
      }
      if (url.endsWith("/issues/12")) {
        return new Response(JSON.stringify({ number: 12, node_id: "I_sub" }), { status: 200 });
      }
      const payload = JSON.parse(String(init?.body)) as {
        query: string;
        variables: Record<string, string>;
      };
      expect(payload.query).toContain("removeSubIssue");
      expect(payload.variables).toEqual({ parentId: "I_parent", subIssueId: "I_sub" });
      return new Response(
        JSON.stringify({ data: { removeSubIssue: { clientMutationId: null } } }),
        { status: 200 },
      );
    }) as unknown as typeof fetch;

    await expect(unlinkSubIssue({ ...base, fetchImpl }, 11, 12)).resolves.toBeUndefined();
  });
});
