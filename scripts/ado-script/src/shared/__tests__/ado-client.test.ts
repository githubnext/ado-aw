import { describe, it, expect, beforeEach, vi, afterEach } from "vitest";
import { BuildStatus } from "azure-devops-node-api/interfaces/BuildInterfaces.js";

// Mock the auth module before importing ado-client
const { mockGitApi, mockBuildApi, mockWebApi, mockGetWebApi } = vi.hoisted(() => {
  const mockGitApi = {
    getPullRequestById: vi.fn(),
    getPullRequestIterations: vi.fn(),
    getPullRequestIterationChanges: vi.fn(),
  };
  const mockBuildApi = {
    updateBuild: vi.fn(),
  };
  const mockWebApi = {
    getGitApi: vi.fn().mockResolvedValue(mockGitApi),
    getBuildApi: vi.fn().mockResolvedValue(mockBuildApi),
  };
  const mockGetWebApi = vi.fn().mockResolvedValue(mockWebApi);
  return { mockGitApi, mockBuildApi, mockWebApi, mockGetWebApi };
});

vi.mock("../auth.js", () => ({
  getWebApi: mockGetWebApi,
  _resetCacheForTesting: vi.fn(),
}));

import {
  getPullRequestById,
  getPullRequestIterations,
  getIterationChanges,
  cancelBuild,
  withRetry,
} from "../ado-client.js";

describe("ado-client", () => {
  beforeEach(() => {
    mockGitApi.getPullRequestById.mockReset();
    mockGitApi.getPullRequestIterations.mockReset();
    mockGitApi.getPullRequestIterationChanges.mockReset();
    mockBuildApi.updateBuild.mockReset();
    mockWebApi.getGitApi.mockReset().mockResolvedValue(mockGitApi);
    mockWebApi.getBuildApi.mockReset().mockResolvedValue(mockBuildApi);
    mockGetWebApi.mockReset().mockResolvedValue(mockWebApi);
    vi.spyOn(process.stdout, "write").mockImplementation(() => true);
  });
  afterEach(() => vi.restoreAllMocks());

  it("getPullRequestById calls SDK with (prId, project)", async () => {
    mockGitApi.getPullRequestById.mockResolvedValue({ pullRequestId: 42 });
    const result = await getPullRequestById("p", "r", 42);
    expect(mockGitApi.getPullRequestById).toHaveBeenCalledWith(42, "p");
    expect(result).toEqual({ pullRequestId: 42 });
  });

  it("getPullRequestIterations calls SDK with (repoId, prId, project)", async () => {
    mockGitApi.getPullRequestIterations.mockResolvedValue([{ id: 1 }]);
    const result = await getPullRequestIterations("p", "r", 42);
    expect(mockGitApi.getPullRequestIterations).toHaveBeenCalledWith("r", 42, "p");
    expect(result).toEqual([{ id: 1 }]);
  });

  it("getIterationChanges calls SDK with (repoId, prId, iterationId, project, top, skip) and returns concatenated entries", async () => {
    mockGitApi.getPullRequestIterationChanges.mockResolvedValue({ changeEntries: [] });
    const result = await getIterationChanges("p", "r", 42, 7);
    expect(mockGitApi.getPullRequestIterationChanges).toHaveBeenCalledWith("r", 42, 7, "p", 100, 0);
    expect(result.changeEntries).toEqual([]);
  });

  it("getIterationChanges paginates with $skip until a short page is returned", async () => {
    const page1 = { changeEntries: Array.from({ length: 100 }, (_, i) => ({ item: { path: `/f${i}` } })) };
    const page2 = { changeEntries: Array.from({ length: 100 }, (_, i) => ({ item: { path: `/g${i}` } })) };
    const page3 = { changeEntries: [{ item: { path: "/last" } }] }; // short page → terminate
    mockGitApi.getPullRequestIterationChanges
      .mockResolvedValueOnce(page1)
      .mockResolvedValueOnce(page2)
      .mockResolvedValueOnce(page3);

    const result = await getIterationChanges("p", "r", 42, 7);
    expect(mockGitApi.getPullRequestIterationChanges).toHaveBeenCalledTimes(3);
    expect(mockGitApi.getPullRequestIterationChanges).toHaveBeenNthCalledWith(1, "r", 42, 7, "p", 100, 0);
    expect(mockGitApi.getPullRequestIterationChanges).toHaveBeenNthCalledWith(2, "r", 42, 7, "p", 100, 100);
    expect(mockGitApi.getPullRequestIterationChanges).toHaveBeenNthCalledWith(3, "r", 42, 7, "p", 100, 200);
    expect(result.changeEntries).toHaveLength(201);
  });

  it("getIterationChanges terminates on an exactly empty page", async () => {
    const fullPage = { changeEntries: Array.from({ length: 100 }, (_, i) => ({ item: { path: `/f${i}` } })) };
    const emptyPage = { changeEntries: [] };
    mockGitApi.getPullRequestIterationChanges
      .mockResolvedValueOnce(fullPage)
      .mockResolvedValueOnce(emptyPage);

    const result = await getIterationChanges("p", "r", 42, 7);
    expect(mockGitApi.getPullRequestIterationChanges).toHaveBeenCalledTimes(2);
    expect(result.changeEntries).toHaveLength(100);
  });

  it("cancelBuild calls updateBuild with status=Cancelling", async () => {
    mockBuildApi.updateBuild.mockResolvedValue({});
    await cancelBuild("p", 99);
    expect(mockBuildApi.updateBuild).toHaveBeenCalledTimes(1);
    const [patch, project, buildId] = mockBuildApi.updateBuild.mock.calls[0]!;
    expect(patch.status).toBe(BuildStatus.Cancelling);
    expect(project).toBe("p");
    expect(buildId).toBe(99);
  });

  it("withRetry retries once on a 5xx error", async () => {
    let calls = 0;
    const fn = async () => {
      calls++;
      if (calls === 1) {
        const err = new Error("server boom") as Error & { statusCode: number };
        err.statusCode = 503;
        throw err;
      }
      return "ok";
    };
    const result = await withRetry("test", fn);
    expect(result).toBe("ok");
    expect(calls).toBe(2);
  });

  it("withRetry does NOT retry non-transient errors", async () => {
    let calls = 0;
    const fn = async () => {
      calls++;
      const err = new Error("client") as Error & { statusCode: number };
      err.statusCode = 404;
      throw err;
    };
    await expect(withRetry("test", fn)).rejects.toThrow("client");
    expect(calls).toBe(1);
  });

  it("withRetry rethrows after the second failure", async () => {
    const fn = async () => {
      const err = new Error("still down") as Error & { statusCode: number };
      err.statusCode = 502;
      throw err;
    };
    await expect(withRetry("test", fn)).rejects.toThrow("still down");
  });

  it("withRetry times out a hung call and treats it as transient", async () => {
    process.env.ADO_API_TIMEOUT_MS = "50";
    try {
      let calls = 0;
      const fn = (): Promise<string> =>
        new Promise((resolve) => {
          calls++;
          if (calls === 2) {
            // second attempt resolves fast so the test doesn't hang on
            // the retry path itself
            setTimeout(() => resolve("ok"), 10);
            return;
          }
          // never resolves on first attempt — the timeout should fire
        });
      const result = await withRetry("hung", fn);
      expect(result).toBe("ok");
      expect(calls).toBe(2);
    } finally {
      delete process.env.ADO_API_TIMEOUT_MS;
    }
  });

  it("withRetry rejects when both attempts time out", async () => {
    process.env.ADO_API_TIMEOUT_MS = "30";
    try {
      const fn = (): Promise<string> => new Promise(() => { /* never resolves */ });
      await expect(withRetry("forever", fn)).rejects.toThrow(/timed out after 30ms/);
    } finally {
      delete process.env.ADO_API_TIMEOUT_MS;
    }
  });
});
