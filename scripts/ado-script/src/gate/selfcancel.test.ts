import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";

const { cancelBuildMock } = vi.hoisted(() => ({
  cancelBuildMock: vi.fn(),
}));
vi.mock("../shared/ado-client.js", () => ({
  cancelBuild: cancelBuildMock,
}));

import { selfCancelIfRequested } from "./selfcancel.js";
import type { GateSpec } from "../shared/types.gen.js";

const baseSpec: GateSpec = {
  context: {
    build_reason: "PullRequest",
    tag_prefix: "pr-gate",
    step_name: "prGate",
    bypass_label: "Pull Request",
  },
  facts: [],
  checks: [],
};

describe("selfCancelIfRequested", () => {
  let writes: string[];
  let originalProject: string | undefined;
  let originalBuildId: string | undefined;

  beforeEach(() => {
    cancelBuildMock.mockReset();
    writes = [];
    originalProject = process.env.ADO_PROJECT;
    originalBuildId = process.env.ADO_BUILD_ID;
    vi.spyOn(process.stdout, "write").mockImplementation((c: any) => {
      writes.push(typeof c === "string" ? c : c.toString());
      return true;
    });
  });

  afterEach(() => {
    if (originalProject === undefined) {
      delete process.env.ADO_PROJECT;
    } else {
      process.env.ADO_PROJECT = originalProject;
    }
    if (originalBuildId === undefined) {
      delete process.env.ADO_BUILD_ID;
    } else {
      process.env.ADO_BUILD_ID = originalBuildId;
    }
    vi.restoreAllMocks();
  });

  it("emits skipped build tag and calls cancelBuild with project/buildId", async () => {
    process.env.ADO_PROJECT = "p";
    process.env.ADO_BUILD_ID = "42";
    cancelBuildMock.mockResolvedValue(undefined);
    await selfCancelIfRequested(baseSpec);
    expect(writes.join("")).toContain("##vso[build.addbuildtag]pr-gate:skipped");
    expect(cancelBuildMock).toHaveBeenCalledWith("p", 42);
  });

  it("logs warning and does NOT call cancelBuild when ADO_PROJECT missing", async () => {
    delete process.env.ADO_PROJECT;
    process.env.ADO_BUILD_ID = "42";
    await selfCancelIfRequested(baseSpec);
    expect(cancelBuildMock).not.toHaveBeenCalled();
    expect(writes.join("")).toContain("Cannot self-cancel");
  });

  it("logs warning and does NOT call cancelBuild when ADO_BUILD_ID missing", async () => {
    process.env.ADO_PROJECT = "p";
    delete process.env.ADO_BUILD_ID;
    await selfCancelIfRequested(baseSpec);
    expect(cancelBuildMock).not.toHaveBeenCalled();
    expect(writes.join("")).toContain("Cannot self-cancel");
  });

  it("swallows cancelBuild errors and emits warning", async () => {
    process.env.ADO_PROJECT = "p";
    process.env.ADO_BUILD_ID = "42";
    cancelBuildMock.mockRejectedValue(new Error("api boom"));
    await expect(selfCancelIfRequested(baseSpec)).resolves.toBeUndefined();
    expect(writes.join("")).toContain("Self-cancel failed: api boom");
  });
});
