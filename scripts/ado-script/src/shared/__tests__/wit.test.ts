import { describe, it, expect, beforeEach, vi, afterEach } from "vitest";

const { mockGitApi, mockWitApi, mockWebApi, mockGetWebApi } = vi.hoisted(() => {
  const mockGitApi = {
    getPullRequestWorkItemRefs: vi.fn(),
  };
  const mockWitApi = {
    getWorkItem: vi.fn(),
    getComments: vi.fn(),
    queryByWiql: vi.fn(),
    createWorkItem: vi.fn(),
    addComment: vi.fn(),
  };
  const mockWebApi = {
    getGitApi: vi.fn().mockResolvedValue(mockGitApi),
    getWorkItemTrackingApi: vi.fn().mockResolvedValue(mockWitApi),
  };
  const mockGetWebApi = vi.fn().mockResolvedValue(mockWebApi);
  return { mockGitApi, mockWitApi, mockWebApi, mockGetWebApi };
});

vi.mock("../auth.js", () => ({
  getWebApi: mockGetWebApi,
  _resetCacheForTesting: vi.fn(),
}));

import {
  addWorkItemComment,
  createWorkItem,
  fileOrAppendWorkItem,
  findWorkItemByTitle,
  getWorkItem,
  getWorkItemComments,
  listPullRequestWorkItems,
  summariseRelations,
} from "../wit.js";

describe("shared/wit", () => {
  beforeEach(() => {
    mockGitApi.getPullRequestWorkItemRefs.mockReset();
    mockWitApi.getWorkItem.mockReset();
    mockWitApi.getComments.mockReset();
    mockWitApi.queryByWiql.mockReset();
    mockWitApi.createWorkItem.mockReset();
    mockWitApi.addComment.mockReset();
    mockWebApi.getGitApi.mockReset().mockResolvedValue(mockGitApi);
    mockWebApi.getWorkItemTrackingApi.mockReset().mockResolvedValue(mockWitApi);
    mockGetWebApi.mockReset().mockResolvedValue(mockWebApi);
    vi.spyOn(process.stdout, "write").mockImplementation(() => true);
  });
  afterEach(() => vi.restoreAllMocks());

  it("listPullRequestWorkItems calls SDK with (repoId, prId, project)", async () => {
    mockGitApi.getPullRequestWorkItemRefs.mockResolvedValue([
      { id: "1", url: "u/1" },
      { id: "2", url: "u/2" },
    ]);
    const result = await listPullRequestWorkItems("MyProject", "repo-id", 42);
    expect(mockGitApi.getPullRequestWorkItemRefs).toHaveBeenCalledWith(
      "repo-id",
      42,
      "MyProject",
    );
    expect(result).toHaveLength(2);
  });

  it("listPullRequestWorkItems returns empty array when PR has no linked WIs", async () => {
    mockGitApi.getPullRequestWorkItemRefs.mockResolvedValue([]);
    const result = await listPullRequestWorkItems("p", "r", 1);
    expect(result).toEqual([]);
  });

  it("getWorkItem fetches with WorkItemExpand.All", async () => {
    mockWitApi.getWorkItem.mockResolvedValue({ id: 4242, fields: { foo: "bar" } });
    const result = await getWorkItem("MyProject", 4242);
    expect(mockWitApi.getWorkItem).toHaveBeenCalledWith(
      4242,
      undefined, // fields
      undefined, // asOf
      4, // WorkItemExpand.All
      "MyProject",
    );
    expect(result.id).toBe(4242);
  });

  it("getWorkItemComments maps the SDK shape to a stable {text, createdBy, createdDate} format", async () => {
    mockWitApi.getComments.mockResolvedValue({
      comments: [
        {
          text: "hello",
          createdBy: { displayName: "Alice", id: "secret-id" },
          createdDate: new Date("2024-01-01T00:00:00Z"),
          // Extra SDK fields that we DON'T want leaking through:
          revisedDate: new Date("2024-02-01T00:00:00Z"),
        },
      ],
    });
    const r = await getWorkItemComments("p", 1);
    expect(r.comments).toHaveLength(1);
    expect(r.comments[0]?.text).toBe("hello");
    expect(r.comments[0]?.createdBy).toEqual({ displayName: "Alice" });
    // Confirms we DROPPED extra fields like revisedDate / id.
    expect(Object.keys(r.comments[0] ?? {})).toEqual([
      "text",
      "createdBy",
      "createdDate",
    ]);
  });

  it("getWorkItemComments handles missing comments array gracefully", async () => {
    mockWitApi.getComments.mockResolvedValue({});
    const r = await getWorkItemComments("p", 1);
    expect(r.comments).toEqual([]);
  });

  it("summariseRelations extracts rel + url + attributes", () => {
    const out = summariseRelations([
      { rel: "ArtifactLink", url: "u1", attributes: { name: "Build" } },
      { rel: "System.LinkTypes.Hierarchy-Reverse", url: "u2" },
    ]);
    expect(out).toEqual([
      { rel: "ArtifactLink", url: "u1", attributes: { name: "Build" } },
      { rel: "System.LinkTypes.Hierarchy-Reverse", url: "u2", attributes: undefined },
    ]);
  });

  it("summariseRelations handles undefined relations", () => {
    expect(summariseRelations(undefined)).toEqual([]);
  });

  it("findWorkItemByTitle runs WIQL with escaped quotes and returns the first id", async () => {
    mockWitApi.queryByWiql.mockResolvedValue({
      workItems: [{ id: 123 }, { id: 456 }],
    });

    const result = await findWorkItemByTitle("MyProject", "Bob's bug");

    expect(mockWitApi.queryByWiql).toHaveBeenCalledWith(
      {
        query:
          "SELECT [System.Id] FROM WorkItems " +
          "WHERE [System.Title] = 'Bob''s bug' " +
          "AND [System.TeamProject] = @project " +
          "AND [System.State] NOT IN ('Closed', 'Resolved', 'Done') " +
          "ORDER BY [System.ChangedDate] DESC",
      },
      { project: "MyProject" },
    );
    expect(result).toBe(123);
  });

  it("findWorkItemByTitle returns null when no matches exist", async () => {
    mockWitApi.queryByWiql.mockResolvedValue({ workItems: [] });
    await expect(findWorkItemByTitle("p", "title")).resolves.toBeNull();
  });

  it("createWorkItem builds a JsonPatch document and prefixes the type with $", async () => {
    mockWitApi.createWorkItem.mockResolvedValue({
      id: 99,
      _links: { html: { href: "https://example.test/wit/99" } },
    });

    const result = await createWorkItem("MyProject", "Task", {
      "System.Title": "Hello",
      "System.Description": "Body",
      "System.Tags": "one; two",
    });

    expect(mockWitApi.createWorkItem).toHaveBeenCalledWith(
      { "Content-Type": "application/json-patch+json" },
      [
        { op: "add", path: "/fields/System.Title", value: "Hello" },
        { op: "add", path: "/fields/System.Description", value: "Body" },
        { op: "add", path: "/fields/System.Tags", value: "one; two" },
        {
          op: "add",
          path: "/multilineFieldsFormat/System.Description",
          value: "Markdown",
        },
      ],
      "MyProject",
      "$Task",
    );
    expect(result).toEqual({ id: 99, url: "https://example.test/wit/99" });
  });

  it("addWorkItemComment posts a comment and returns its id", async () => {
    mockWitApi.addComment.mockResolvedValue({ id: 777 });

    const result = await addWorkItemComment("MyProject", 42, "hello");

    expect(mockWitApi.addComment).toHaveBeenCalledWith(
      { text: "hello" },
      "MyProject",
      42,
    );
    expect(result).toEqual({ commentId: 777 });
  });

  it("fileOrAppendWorkItem skips when disabled", async () => {
    const result = await fileOrAppendWorkItem(
      "MyProject",
      {
        enabled: false,
        workItemType: "Task",
        tags: [],
        includeStats: true,
      },
      "Default title",
      "Body",
    );

    expect(result).toEqual({
      action: "skipped",
      message: "Work-item filing disabled via enabled: false",
    });
  });

  it("fileOrAppendWorkItem appends to an existing active work item", async () => {
    mockWitApi.queryByWiql.mockResolvedValue({ workItems: [{ id: 51 }] });
    mockWitApi.addComment.mockResolvedValue({ id: 88 });

    const result = await fileOrAppendWorkItem(
      "MyProject",
      {
        enabled: true,
        title: "Existing title",
        workItemType: "Task",
        tags: [],
        includeStats: true,
      },
      "Default title",
      "Comment body",
    );

    expect(mockWitApi.createWorkItem).not.toHaveBeenCalled();
    expect(result).toEqual({
      action: "appended",
      workItemId: 51,
      commentId: 88,
      message: "Appended comment #88 to existing work item #51: Existing title",
    });
  });

  it("fileOrAppendWorkItem creates a new work item when no active title match exists", async () => {
    mockWitApi.queryByWiql.mockResolvedValue({ workItems: [] });
    mockWitApi.createWorkItem.mockResolvedValue({
      id: 64,
      _links: { html: { href: "https://example.test/wit/64" } },
    });

    const result = await fileOrAppendWorkItem(
      "MyProject",
      {
        enabled: true,
        workItemType: "Bug",
        areaPath: "Proj\\Area",
        iterationPath: "Proj\\Iteration",
        tags: ["tag-one", "tag-two"],
        includeStats: false,
      },
      "Default title",
      "Description body",
    );

    expect(mockWitApi.addComment).not.toHaveBeenCalled();
    expect(mockWitApi.createWorkItem).toHaveBeenCalledWith(
      { "Content-Type": "application/json-patch+json" },
      [
        { op: "add", path: "/fields/System.Title", value: "Default title" },
        {
          op: "add",
          path: "/fields/System.Description",
          value: "Description body",
        },
        { op: "add", path: "/fields/System.AreaPath", value: "Proj\\Area" },
        {
          op: "add",
          path: "/fields/System.IterationPath",
          value: "Proj\\Iteration",
        },
        {
          op: "add",
          path: "/fields/System.Tags",
          value: "tag-one; tag-two",
        },
        {
          op: "add",
          path: "/multilineFieldsFormat/System.Description",
          value: "Markdown",
        },
      ],
      "MyProject",
      "$Bug",
    );
    expect(result).toEqual({
      action: "created",
      workItemId: 64,
      message: "Created work item #64: Default title",
    });
  });
});
