import { describe, it, expect, beforeEach, vi, afterEach } from "vitest";
import {
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { BuildResult } from "azure-devops-node-api/interfaces/BuildInterfaces.js";

const { listBuildsForPullRequest } = vi.hoisted(() => ({
  listBuildsForPullRequest: vi.fn(),
}));

vi.mock("../../shared/build.js", () => ({ listBuildsForPullRequest }));

import { failureFragment, main, successFragment, validateIdentifiers } from "../index.js";

function makeWorkspace() {
  const root = mkdtempSync(join(tmpdir(), "exec-context-pr-checks-test-"));
  const sourcesDir = join(root, "sources");
  mkdirSync(sourcesDir, { recursive: true });
  const promptPath = join(root, "agent-prompt.md");
  writeFileSync(promptPath, "# Agent prompt\n", "utf8");
  return {
    sourcesDir,
    promptPath,
    cleanup: () => rmSync(root, { recursive: true, force: true }),
  };
}

const validEnv = (overrides: NodeJS.ProcessEnv = {}): NodeJS.ProcessEnv => ({
  SYSTEM_TEAMPROJECT: "MyProject",
  SYSTEM_PULLREQUEST_PULLREQUESTID: "42",
  BUILD_BUILDID: "100",
  ...overrides,
});

describe("validateIdentifiers", () => {
  it("accepts well-formed env", () => {
    const r = validateIdentifiers(validEnv());
    expect(r.ok).toBe(true);
  });
  it("rejects non-numeric PR id", () => {
    expect(validateIdentifiers(validEnv({ SYSTEM_PULLREQUEST_PULLREQUESTID: "abc" })).ok).toBe(
      false,
    );
  });
});

describe("successFragment", () => {
  it("highlights failing count and tells agent how to read logs", () => {
    const out = successFragment({
      prId: 42,
      failingCount: 2,
      succeededCount: 3,
      failingNames: ["CI #99", "lint #50"],
    });
    expect(out).toContain("PR #42");
    expect(out).toContain("**2 failing**");
    expect(out).toContain("3 succeeded");
    expect(out).toContain("CI #99");
    expect(out).toContain("build_get_log");
  });

  it("emits all-green variant when nothing is failing", () => {
    const out = successFragment({
      prId: 42,
      failingCount: 0,
      succeededCount: 5,
      failingNames: [],
    });
    expect(out).toContain("All build validations are succeeding.");
  });
});

describe("failureFragment", () => {
  it("contains reason and do-not-invent instruction", () => {
    const out = failureFragment("403 Forbidden");
    expect(out).toContain("PR checks context preparation failed.");
    expect(out).toContain("do NOT invent");
  });
});

describe("main", () => {
  let ws: ReturnType<typeof makeWorkspace>;
  beforeEach(() => {
    ws = makeWorkspace();
    listBuildsForPullRequest.mockReset();
    vi.spyOn(process.stdout, "write").mockImplementation(() => true);
    vi.spyOn(process.stderr, "write").mockImplementation(() => true);
  });
  afterEach(() => {
    vi.restoreAllMocks();
    ws.cleanup();
  });

  it("partitions builds by result into failing.json + succeeded.json", async () => {
    listBuildsForPullRequest.mockResolvedValue([
      { id: 1, definition: { name: "CI" }, result: BuildResult.Succeeded },
      { id: 2, definition: { name: "Lint" }, result: BuildResult.Failed },
      {
        id: 3,
        definition: { name: "TestPart" },
        result: BuildResult.PartiallySucceeded,
      },
      { id: 4, definition: { name: "OldCI" }, result: BuildResult.Canceled },
    ]);

    const env = validEnv({
      BUILD_SOURCESDIRECTORY: ws.sourcesDir,
      AW_AGENT_PROMPT_FILE: ws.promptPath,
      SYSTEM_ACCESSTOKEN: "bearer-xyz",
    });
    const rc = await main(env);
    expect(rc).toBe(0);

    const dir = join(ws.sourcesDir, "aw-context", "pr", "checks");
    const failing = JSON.parse(readFileSync(join(dir, "failing.json"), "utf8"));
    const succeeded = JSON.parse(readFileSync(join(dir, "succeeded.json"), "utf8"));
    expect(failing).toHaveLength(3); // failed + partiallySucceeded + canceled
    expect(succeeded).toHaveLength(1);
    expect(succeeded[0].id).toBe(1);

    // Confirm REST helper was called with the right PR ref.
    expect(listBuildsForPullRequest).toHaveBeenCalledWith(
      "MyProject",
      "refs/pull/42/merge",
      100,
    );

    // Prompt fragment summarises.
    const prompt = readFileSync(ws.promptPath, "utf8");
    expect(prompt).toContain("3 failing");

    // Trust boundary: bearer must not appear in any staged file.
    for (const f of ["failing.json", "succeeded.json"]) {
      expect(readFileSync(join(dir, f), "utf8")).not.toContain("bearer-xyz");
    }
  });

  it("writes error.txt + failure fragment on REST failure", async () => {
    listBuildsForPullRequest.mockRejectedValue(new Error("403"));
    const env = validEnv({
      BUILD_SOURCESDIRECTORY: ws.sourcesDir,
      AW_AGENT_PROMPT_FILE: ws.promptPath,
    });
    await main(env);
    const dir = join(ws.sourcesDir, "aw-context", "pr", "checks");
    expect(readFileSync(join(dir, "error.txt"), "utf8")).toContain(
      "failed to list builds for PR #42",
    );
  });

  it("validation failure → no REST call, error.txt written", async () => {
    const env = validEnv({
      BUILD_SOURCESDIRECTORY: ws.sourcesDir,
      AW_AGENT_PROMPT_FILE: ws.promptPath,
      SYSTEM_PULLREQUEST_PULLREQUESTID: "evil",
    });
    await main(env);
    expect(listBuildsForPullRequest).not.toHaveBeenCalled();
  });
});
