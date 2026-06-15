/**
 * Tests for `shared/build.ts` — the ADO Build REST helpers shared by
 * the `pipeline`, `ci-push`, and `pr.checks` exec-context contributors.
 *
 * Mocks the same `azure-devops-node-api` surface as
 * `shared/__tests__/ado-client.test.ts`. The build.ts helpers
 * delegate to `withRetry` from `ado-client.ts` for transient-error
 * resilience, so the tests cover both the happy path and the
 * timeout / retry behaviour by reusing that wrapper directly.
 */
import { describe, it, expect, beforeEach, vi, afterEach } from "vitest";
import type { Build } from "azure-devops-node-api/interfaces/BuildInterfaces.js";

const { mockBuildApi, mockWebApi, mockGetWebApi } = vi.hoisted(() => {
  const mockBuildApi = {
    getBuild: vi.fn(),
    getArtifacts: vi.fn(),
  };
  const mockWebApi = {
    getBuildApi: vi.fn().mockResolvedValue(mockBuildApi),
  };
  const mockGetWebApi = vi.fn().mockResolvedValue(mockWebApi);
  return { mockBuildApi, mockWebApi, mockGetWebApi };
});

vi.mock("../auth.js", () => ({
  getWebApi: mockGetWebApi,
  _resetCacheForTesting: vi.fn(),
}));

import { getBuildById, listArtifacts } from "../build.js";

describe("shared/build", () => {
  beforeEach(() => {
    mockBuildApi.getBuild.mockReset();
    mockBuildApi.getArtifacts.mockReset();
    mockWebApi.getBuildApi.mockReset().mockResolvedValue(mockBuildApi);
    mockGetWebApi.mockReset().mockResolvedValue(mockWebApi);
    vi.spyOn(process.stdout, "write").mockImplementation(() => true);
  });
  afterEach(() => vi.restoreAllMocks());

  it("getBuildById calls SDK with (project, buildId) and returns the Build", async () => {
    const fakeBuild: Partial<Build> = {
      id: 42,
      sourceVersion: "abc123",
      sourceBranch: "refs/heads/main",
    };
    mockBuildApi.getBuild.mockResolvedValue(fakeBuild);
    const result = await getBuildById("MyProject", 42);
    expect(mockBuildApi.getBuild).toHaveBeenCalledWith("MyProject", 42);
    expect(result).toEqual(fakeBuild);
  });

  it("getBuildById retries once on a transient 5xx error", async () => {
    const err = new Error("503 Service Unavailable") as Error & {
      statusCode: number;
    };
    err.statusCode = 503;
    mockBuildApi.getBuild
      .mockRejectedValueOnce(err)
      .mockResolvedValue({ id: 42 });
    const result = await getBuildById("MyProject", 42);
    expect(mockBuildApi.getBuild).toHaveBeenCalledTimes(2);
    expect(result).toEqual({ id: 42 });
  });

  it("getBuildById rethrows on a non-transient (4xx) error", async () => {
    const err = new Error("404 Not Found") as Error & { statusCode: number };
    err.statusCode = 404;
    mockBuildApi.getBuild.mockRejectedValue(err);
    await expect(getBuildById("MyProject", 42)).rejects.toThrow(/404/);
    // No retry on 4xx.
    expect(mockBuildApi.getBuild).toHaveBeenCalledTimes(1);
  });

  it("listArtifacts calls SDK with (project, buildId) and returns the array", async () => {
    mockBuildApi.getArtifacts.mockResolvedValue([
      { id: 1, name: "drop", resource: { type: "Container" } },
      { id: 2, name: "logs", resource: { type: "FilePath" } },
    ]);
    const result = await listArtifacts("MyProject", 42);
    expect(mockBuildApi.getArtifacts).toHaveBeenCalledWith("MyProject", 42);
    expect(result).toHaveLength(2);
    expect(result[0]?.name).toBe("drop");
  });

  it("listArtifacts returns an empty array when the build has no artifacts", async () => {
    mockBuildApi.getArtifacts.mockResolvedValue([]);
    const result = await listArtifacts("MyProject", 42);
    expect(result).toEqual([]);
  });
});
