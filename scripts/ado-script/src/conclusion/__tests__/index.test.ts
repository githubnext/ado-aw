import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { FileOrAppendResult, WorkItemReportConfig } from "../../shared/wit.js";

const {
  originalEnv,
  fileOrAppendWorkItem,
  findWorkItemByTitle,
  addWorkItemComment,
  createWorkItem,
  logInfo,
  logWarning,
  logError,
  existsSync,
  readFileSync,
} = vi.hoisted(() => {
  const originalEnv = new Map<string, string | undefined>(
    [
      "AW_AGENT_RESULT",
      "AW_DETECTION_RESULT",
      "AW_SAFEOUTPUTS_RESULT",
      "AW_SAFEOUTPUTS_REVIEWED_RESULT",
      "AW_REPORT_FAILURE_AS_WORK_ITEM",
      "AW_SAFE_OUTPUT_DIR",
      "AW_PIPELINE_NAME",
      "AW_NOOP_REPORT_AS_WORK_ITEM",
      "AW_NOOP_TITLE_PREFIX",
      "AW_NOOP_WORK_ITEM_TYPE",
      "AW_NOOP_AREA_PATH",
      "AW_NOOP_ITERATION_PATH",
      "AW_NOOP_TAGS",
      "AW_MISSING_TOOL_REPORT_AS_WORK_ITEM",
      "AW_MISSING_TOOL_TITLE_PREFIX",
      "AW_MISSING_TOOL_WORK_ITEM_TYPE",
      "AW_MISSING_TOOL_AREA_PATH",
      "AW_MISSING_TOOL_ITERATION_PATH",
      "AW_MISSING_TOOL_TAGS",
      "AW_MISSING_DATA_REPORT_AS_WORK_ITEM",
      "AW_MISSING_DATA_TITLE_PREFIX",
      "AW_MISSING_DATA_WORK_ITEM_TYPE",
      "AW_MISSING_DATA_AREA_PATH",
      "AW_MISSING_DATA_ITERATION_PATH",
      "AW_MISSING_DATA_TAGS",
      "SYSTEM_TEAMPROJECT",
      "BUILD_BUILDURI",
      "BUILD_BUILDID",
    ].map((key) => [key, process.env[key]]),
  );
  process.env.AW_AGENT_RESULT = "Succeeded";
  process.env.AW_DETECTION_RESULT = "Succeeded";
  process.env.AW_SAFEOUTPUTS_RESULT = "Succeeded";
  process.env.AW_REPORT_FAILURE_AS_WORK_ITEM = "false";
  vi.spyOn(process, "exit").mockImplementation((() => undefined) as never);

  return {
    originalEnv,
    fileOrAppendWorkItem: vi.fn(),
    findWorkItemByTitle: vi.fn(),
    addWorkItemComment: vi.fn(),
    createWorkItem: vi.fn(),
    logInfo: vi.fn(),
    logWarning: vi.fn(),
    logError: vi.fn(),
    existsSync: vi.fn(),
    readFileSync: vi.fn(),
  };
});

vi.mock("../../shared/wit.js", () => ({
  fileOrAppendWorkItem,
  findWorkItemByTitle,
  addWorkItemComment,
  createWorkItem,
}));

vi.mock("../../shared/vso-logger.js", () => ({
  logInfo,
  logWarning,
  logError,
}));

vi.mock("node:fs", () => ({
  existsSync,
  readFileSync,
}));

import { main } from "../index.js";

const trackedEnvKeys = [
  "AW_AGENT_RESULT",
  "AW_DETECTION_RESULT",
  "AW_SAFEOUTPUTS_RESULT",
  "AW_REPORT_FAILURE_AS_WORK_ITEM",
  "AW_SAFEOUTPUTS_REVIEWED_RESULT",
  "AW_SAFE_OUTPUT_DIR",
  "AW_PIPELINE_NAME",
  "AW_NOOP_REPORT_AS_WORK_ITEM",
  "AW_NOOP_TITLE_PREFIX",
  "AW_NOOP_WORK_ITEM_TYPE",
  "AW_NOOP_AREA_PATH",
  "AW_NOOP_ITERATION_PATH",
  "AW_NOOP_TAGS",
  "AW_MISSING_TOOL_REPORT_AS_WORK_ITEM",
  "AW_MISSING_TOOL_TITLE_PREFIX",
  "AW_MISSING_TOOL_WORK_ITEM_TYPE",
  "AW_MISSING_TOOL_AREA_PATH",
  "AW_MISSING_TOOL_ITERATION_PATH",
  "AW_MISSING_TOOL_TAGS",
  "AW_MISSING_DATA_REPORT_AS_WORK_ITEM",
  "AW_MISSING_DATA_TITLE_PREFIX",
  "AW_MISSING_DATA_WORK_ITEM_TYPE",
  "AW_MISSING_DATA_AREA_PATH",
  "AW_MISSING_DATA_ITERATION_PATH",
  "AW_MISSING_DATA_TAGS",
  "SYSTEM_TEAMPROJECT",
  "BUILD_BUILDURI",
  "BUILD_BUILDID",
] as const;

function baseEnv(): Record<(typeof trackedEnvKeys)[number], string> {
  return {
    AW_AGENT_RESULT: "Succeeded",
    AW_DETECTION_RESULT: "Succeeded",
    AW_SAFEOUTPUTS_RESULT: "Succeeded",
    AW_REPORT_FAILURE_AS_WORK_ITEM: "true",
    AW_SAFE_OUTPUT_DIR: "C:\\software\\ado-aw-feature-reporter\\scripts\\ado-script\\src\\conclusion\\__tests__\\fixtures",
    AW_PIPELINE_NAME: "feature reporter",
    AW_NOOP_REPORT_AS_WORK_ITEM: "true",
    AW_NOOP_TITLE_PREFIX: "[ado-aw] Agent noop",
    AW_NOOP_WORK_ITEM_TYPE: "Bug",
    AW_NOOP_AREA_PATH: "MyProject\\Automation",
    AW_NOOP_ITERATION_PATH: "",
    AW_NOOP_TAGS: "[\"pipeline-failure\",\"automated\"]",
    AW_MISSING_TOOL_REPORT_AS_WORK_ITEM: "",
    AW_MISSING_TOOL_TITLE_PREFIX: "",
    AW_MISSING_TOOL_WORK_ITEM_TYPE: "",
    AW_MISSING_TOOL_AREA_PATH: "",
    AW_MISSING_TOOL_ITERATION_PATH: "",
    AW_MISSING_TOOL_TAGS: "",
    AW_MISSING_DATA_REPORT_AS_WORK_ITEM: "",
    AW_MISSING_DATA_TITLE_PREFIX: "",
    AW_MISSING_DATA_WORK_ITEM_TYPE: "",
    AW_MISSING_DATA_AREA_PATH: "",
    AW_MISSING_DATA_ITERATION_PATH: "",
    AW_MISSING_DATA_TAGS: "",
    SYSTEM_TEAMPROJECT: "MyProject",
    BUILD_BUILDURI: "https://dev.azure.com/org/project/_build/results?buildId=321",
    BUILD_BUILDID: "321",
  };
}

function applyEnv(overrides: Partial<Record<(typeof trackedEnvKeys)[number], string | undefined>> = {}) {
  const next = { ...baseEnv(), ...overrides };
  for (const key of trackedEnvKeys) {
    const value = next[key];
    if (value === undefined) {
      delete process.env[key];
      continue;
    }
    process.env[key] = value;
  }
}

function setManifestEntries(entries: readonly Record<string, unknown>[]) {
  existsSync.mockReturnValue(true);
  readFileSync.mockReturnValue(entries.map((entry) => JSON.stringify(entry)).join("\n"));
}

function installDedupAwareWitMock() {
  fileOrAppendWorkItem.mockImplementation(
    async (
    project: string,
    config: WorkItemReportConfig,
    defaultTitle: string,
    body: string,
    ): Promise<FileOrAppendResult> => {
    if (!config.enabled) {
      return {
        action: "skipped",
        message: "Work-item filing disabled via enabled: false",
      };
    }

    const title = config.title ?? defaultTitle;
    const existingId = await findWorkItemByTitle(project, title);
    if (existingId !== null) {
      const { commentId } = await addWorkItemComment(project, existingId, body);
      return {
        action: "appended",
        workItemId: existingId,
        commentId,
        message: `Appended comment #${commentId} to existing work item #${existingId}: ${title}`,
      };
    }

    const created = await createWorkItem(project, config.workItemType, {
      "System.Title": title,
      "System.Description": body,
    });
    return {
      action: "created",
      workItemId: created.id,
      message: `Created work item #${created.id}: ${title}`,
    };
    },
  );
}

describe("conclusion/main", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    applyEnv();
    existsSync.mockReturnValue(false);
    readFileSync.mockReturnValue("");
    fileOrAppendWorkItem.mockResolvedValue({
      action: "created",
      workItemId: 42,
      message: "Created",
    });
    findWorkItemByTitle.mockResolvedValue(null);
    addWorkItemComment.mockResolvedValue({ commentId: 88 });
    createWorkItem.mockResolvedValue({ id: 99, url: "https://example.test/wit/99" });
  });

  afterEach(() => {
    for (const [key, value] of originalEnv.entries()) {
      if (value === undefined) {
        delete process.env[key];
      } else {
        process.env[key] = value;
      }
    }
    vi.restoreAllMocks();
  });

  it("files a pipeline-failure work item when an upstream job failed", async () => {
    applyEnv({ AW_AGENT_RESULT: "Failed" });

    await expect(main()).resolves.toBe(0);

    expect(fileOrAppendWorkItem).toHaveBeenCalledTimes(1);
    expect(fileOrAppendWorkItem).toHaveBeenCalledWith(
      "MyProject",
      expect.objectContaining({
        enabled: true,
        workItemType: "Task",
      }),
      "[ado-aw] Pipeline failure: feature reporter",
      expect.stringContaining("Agent (Failed)"),
    );
  });

  it("files a pipeline-failure work item when the reviewed job failed", async () => {
    applyEnv({ AW_SAFEOUTPUTS_REVIEWED_RESULT: "Failed" });

    await expect(main()).resolves.toBe(0);

    expect(fileOrAppendWorkItem).toHaveBeenCalledTimes(1);
    expect(fileOrAppendWorkItem).toHaveBeenCalledWith(
      "MyProject",
      expect.objectContaining({ enabled: true, workItemType: "Task" }),
      "[ado-aw] Pipeline failure: feature reporter",
      expect.stringContaining("SafeOutputs_Reviewed (Failed)"),
    );
  });

  it("files a noop work item when the manifest contains noop", async () => {
    setManifestEntries([{ name: "noop", context: "nothing to do" }]);

    await main();

    expect(fileOrAppendWorkItem).toHaveBeenCalledTimes(1);
    expect(fileOrAppendWorkItem).toHaveBeenCalledWith(
      "MyProject",
      expect.objectContaining({
        enabled: true,
        title: "[ado-aw] Agent noop feature reporter",
        workItemType: "Bug",
        areaPath: "MyProject\\Automation",
        tags: ["pipeline-failure", "automated"],
      }),
      "[ado-aw] Agent reported no operation: feature reporter",
      expect.stringContaining("nothing to do"),
    );
  });

  it("truncates the work-item title to ADO's 255-char limit", async () => {
    // A very long prefix + pipeline name must not exceed System.Title's cap.
    const longPrefix = "X".repeat(300);
    applyEnv({ AW_NOOP_TITLE_PREFIX: longPrefix });
    setManifestEntries([{ name: "noop", context: "nothing to do" }]);

    await main();

    expect(fileOrAppendWorkItem).toHaveBeenCalledTimes(1);
    const firstCall = (fileOrAppendWorkItem as ReturnType<typeof vi.fn>).mock
      .calls[0];
    const titleArg = (firstCall?.[1] as { title?: string } | undefined)?.title;
    expect(titleArg).toBeDefined();
    expect(titleArg).toHaveLength(255);
    expect(titleArg?.startsWith("XXX")).toBe(true);
  });

  it("files a missing-tool work item when the manifest contains missing_tool", async () => {
    setManifestEntries([{ name: "missing_tool", tool_name: "gh", context: "tool_name: gh" }]);

    await main();

    expect(fileOrAppendWorkItem).toHaveBeenCalledTimes(1);
    expect(fileOrAppendWorkItem).toHaveBeenCalledWith(
      "MyProject",
      expect.objectContaining({ enabled: true }),
      "[ado-aw] Agent encountered missing tool: feature reporter",
      expect.stringContaining("- gh"),
    );
  });

  it("files a missing-data work item when the manifest contains missing_data", async () => {
    setManifestEntries([
      {
        name: "missing_data",
        data_type: "pull_request",
        reason: "PR metadata not available",
        context: "data_type: pull_request",
      },
    ]);

    await main();

    expect(fileOrAppendWorkItem).toHaveBeenCalledTimes(1);
    expect(fileOrAppendWorkItem).toHaveBeenCalledWith(
      "MyProject",
      expect.objectContaining({ enabled: true }),
      "[ado-aw] Agent reported missing data: feature reporter",
      expect.stringContaining("PR metadata not available"),
    );
  });

  it("appends a comment to an existing work item instead of creating a duplicate", async () => {
    applyEnv({ AW_AGENT_RESULT: "Failed" });
    installDedupAwareWitMock();
    findWorkItemByTitle.mockResolvedValue(77);

    await main();

    expect(findWorkItemByTitle).toHaveBeenCalledWith(
      "MyProject",
      "[ado-aw] Pipeline failure: feature reporter",
    );
    expect(addWorkItemComment).toHaveBeenCalledTimes(1);
    expect(addWorkItemComment).toHaveBeenCalledWith(
      "MyProject",
      77,
      expect.stringContaining("upstream agentic-pipeline failure"),
    );
    expect(createWorkItem).not.toHaveBeenCalled();
  });

  it("skips all filing when work-item reporting is disabled", async () => {
    applyEnv({
      AW_AGENT_RESULT: "Failed",
      AW_REPORT_FAILURE_AS_WORK_ITEM: "false",
    });

    await main();

    expect(fileOrAppendWorkItem).not.toHaveBeenCalled();
    expect(logInfo).toHaveBeenCalledWith(
      "Conclusion work-item filing disabled via AW_REPORT_FAILURE_AS_WORK_ITEM=false",
    );
  });

  it("does not throw and logs a warning when the manifest file is missing", async () => {
    applyEnv({ AW_SAFE_OUTPUT_DIR: "C:\\missing-manifest" });
    existsSync.mockReturnValue(false);

    await expect(main()).resolves.toBe(0);

    expect(logWarning).toHaveBeenCalledWith(
      expect.stringContaining("Conclusion manifest not found:"),
    );
  });

  it("does nothing when there are no failures or diagnostic signals", async () => {
    applyEnv({ AW_SAFE_OUTPUT_DIR: undefined });

    await main();

    expect(fileOrAppendWorkItem).not.toHaveBeenCalled();
    expect(logInfo).toHaveBeenCalledWith("Conclusion reporting found no failure or diagnostic signals");
  });
});
