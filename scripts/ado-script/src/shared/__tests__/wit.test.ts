import { describe, it, expect, beforeEach, vi, afterEach } from "vitest";

const { mockGitApi, mockWitApi, mockWebApi, mockGetWebApi } = vi.hoisted(() => {
  const mockGitApi = {
    getPullRequestWorkItemRefs: vi.fn(),
  };
  const mockWitApi = {
    getWorkItem: vi.fn(),
    getComments: vi.fn(),
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
});
