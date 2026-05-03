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

vi.mock("./auth.js", () => ({
  getWebApi: mockGetWebApi,
  _resetCacheForTesting: vi.fn(),
}));

import {
  getPullRequestById,
  getPullRequestIterations,
  getIterationChanges,
  cancelBuild,
  withRetry,
} from "./ado-client.js";

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

  it("getIterationChanges calls SDK with (repoId, prId, iterationId, project)", async () => {
    mockGitApi.getPullRequestIterationChanges.mockResolvedValue({ changeEntries: [] });
    const result = await getIterationChanges("p", "r", 42, 7);
    expect(mockGitApi.getPullRequestIterationChanges).toHaveBeenCalledWith("r", 42, 7, "p");
    expect(result).toEqual({ changeEntries: [] });
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
});
