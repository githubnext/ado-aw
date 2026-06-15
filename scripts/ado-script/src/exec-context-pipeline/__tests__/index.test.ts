/**
 * Tests for the exec-context-pipeline bundle entry point.
 *
 * Mocks the shared/build.ts REST helpers so the bundle can be
 * exercised end-to-end without an ADO connection.
 */
import { describe, it, expect, beforeEach, vi, afterEach } from "vitest";
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { BuildResult } from "azure-devops-node-api/interfaces/BuildInterfaces.js";

const { getBuildById, listArtifacts } = vi.hoisted(() => ({
  getBuildById: vi.fn(),
  listArtifacts: vi.fn(),
}));

vi.mock("../../shared/build.js", () => ({
  getBuildById,
  listArtifacts,
}));

import {
  failureFragment,
  main,
  successFragment,
  validateIdentifiers,
} from "../index.js";

function makeWorkspace(): {
  sourcesDir: string;
  promptPath: string;
  cleanup: () => void;
} {
  const root = mkdtempSync(join(tmpdir(), "exec-context-pipeline-test-"));
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

describe("validateIdentifiers", () => {
  it("accepts a well-formed env block", () => {
    const result = validateIdentifiers({
      BUILD_TRIGGEREDBY_BUILDID: "42",
      BUILD_TRIGGEREDBY_PROJECTID: "00000000-0000-0000-0000-000000000001",
      BUILD_TRIGGEREDBY_DEFINITIONNAME: "upstream",
    });
    expect(result.ok).toBe(true);
    if (result.ok) {
      expect(result.buildId).toBe(42);
      expect(result.projectId).toBe("00000000-0000-0000-0000-000000000001");
      expect(result.definitionName).toBe("upstream");
    }
  });

  it("rejects a non-numeric build id", () => {
    const result = validateIdentifiers({
      BUILD_TRIGGEREDBY_BUILDID: "evil; rm -rf /",
      BUILD_TRIGGEREDBY_PROJECTID: "00000000-0000-0000-0000-000000000001",
    });
    expect(result.ok).toBe(false);
    if (!result.ok) {
      expect(result.reason).toMatch(/BUILD_TRIGGEREDBY_BUILDID/);
    }
  });

  it("rejects a non-GUID project id", () => {
    const result = validateIdentifiers({
      BUILD_TRIGGEREDBY_BUILDID: "42",
      BUILD_TRIGGEREDBY_PROJECTID: "evil; rm -rf /",
    });
    expect(result.ok).toBe(false);
    if (!result.ok) {
      expect(result.reason).toMatch(/BUILD_TRIGGEREDBY_PROJECTID/);
    }
  });

  it("rejects an empty build id", () => {
    const result = validateIdentifiers({
      BUILD_TRIGGEREDBY_BUILDID: "",
      BUILD_TRIGGEREDBY_PROJECTID: "00000000-0000-0000-0000-000000000001",
    });
    expect(result.ok).toBe(false);
  });
});

describe("successFragment", () => {
  it("includes build id, definition name, branch, sha, and status", () => {
    const out = successFragment({
      buildId: 42,
      definitionName: "upstream-ci",
      sourceBranch: "refs/heads/main",
      sourceSha: "abc123",
      status: "succeeded",
      artifactCount: 3,
    });
    expect(out).toContain("## Pipeline-completion context");
    expect(out).toContain("**upstream-ci**");
    expect(out).toContain("build #42");
    expect(out).toContain("`refs/heads/main`");
    expect(out).toContain("`abc123`");
    expect(out).toContain("status: `succeeded`");
    expect(out).toContain("3 artifact(s)");
    expect(out).toContain("Upstream succeeded — proceed");
  });

  it("nudges the agent to surface failures when upstream did not succeed", () => {
    const out = successFragment({
      buildId: 42,
      definitionName: "upstream",
      sourceBranch: "main",
      sourceSha: "abc",
      status: "failed",
      artifactCount: 0,
    });
    expect(out).toContain("Surface the failure");
    expect(out).not.toContain("Upstream succeeded");
  });

  it("sanitises a hostile pipeline name", () => {
    const out = successFragment({
      buildId: 42,
      definitionName: "evil\n## Injected heading\n",
      sourceBranch: "main",
      sourceSha: "abc",
      status: "succeeded",
      artifactCount: 0,
    });
    expect(out).not.toContain("\n## Injected heading\n");
  });
});

describe("failureFragment", () => {
  it("contains the reason and a do-not-invent instruction", () => {
    const out = failureFragment("REST call returned 404");
    expect(out).toContain("Pipeline-completion context preparation failed.");
    expect(out).toContain("REST call returned 404");
    expect(out).toContain("do NOT invent");
  });
});

describe("main", () => {
  let ws: ReturnType<typeof makeWorkspace>;

  beforeEach(() => {
    ws = makeWorkspace();
    getBuildById.mockReset();
    listArtifacts.mockReset();
    vi.spyOn(process.stdout, "write").mockImplementation(() => true);
    vi.spyOn(process.stderr, "write").mockImplementation(() => true);
  });
  afterEach(() => {
    vi.restoreAllMocks();
    ws.cleanup();
  });

  it("stages all upstream-* files and appends success fragment on the happy path", async () => {
    getBuildById.mockResolvedValue({
      id: 42,
      sourceVersion: "abc123",
      sourceBranch: "refs/heads/main",
      result: BuildResult.Succeeded,
      definition: { name: "upstream-ci" },
    });
    listArtifacts.mockResolvedValue([
      { id: 1, name: "drop", source: "src", resource: { type: "Container" } },
    ]);

    const env: NodeJS.ProcessEnv = {
      BUILD_SOURCESDIRECTORY: ws.sourcesDir,
      AW_AGENT_PROMPT_FILE: ws.promptPath,
      SYSTEM_ACCESSTOKEN: "bearer-xyz",
      BUILD_TRIGGEREDBY_BUILDID: "42",
      BUILD_TRIGGEREDBY_PROJECTID: "00000000-0000-0000-0000-000000000001",
      BUILD_TRIGGEREDBY_DEFINITIONNAME: "upstream-ci",
    };
    const rc = await main(env);
    expect(rc).toBe(0);

    const dir = join(ws.sourcesDir, "aw-context", "pipeline");
    expect(readFileSync(join(dir, "upstream-build-id"), "utf8")).toBe("42");
    expect(readFileSync(join(dir, "upstream-source-sha"), "utf8")).toBe("abc123");
    expect(readFileSync(join(dir, "upstream-source-branch"), "utf8")).toBe(
      "refs/heads/main",
    );
    expect(readFileSync(join(dir, "upstream-status"), "utf8")).toBe("succeeded");
    expect(readFileSync(join(dir, "upstream-definition"), "utf8")).toBe(
      "upstream-ci",
    );

    const artifacts = JSON.parse(
      readFileSync(join(dir, "upstream-artifacts.json"), "utf8"),
    );
    expect(artifacts).toEqual([
      { id: 1, name: "drop", source: "src", resource: { type: "Container" } },
    ]);

    const prompt = readFileSync(ws.promptPath, "utf8");
    expect(prompt).toContain("## Pipeline-completion context");
    expect(prompt).toContain("upstream-ci");

    // Trust boundary: bearer MUST NOT appear in any staged artefact
    // or the prompt fragment.
    for (const f of [
      "upstream-build-id",
      "upstream-source-sha",
      "upstream-source-branch",
      "upstream-status",
      "upstream-definition",
      "upstream-artifacts.json",
    ]) {
      expect(readFileSync(join(dir, f), "utf8")).not.toContain("bearer-xyz");
    }
    expect(readFileSync(ws.promptPath, "utf8")).not.toContain("bearer-xyz");
  });

  it("writes error.txt + failure fragment when the build lookup fails", async () => {
    getBuildById.mockRejectedValue(new Error("404 Not Found"));

    const env: NodeJS.ProcessEnv = {
      BUILD_SOURCESDIRECTORY: ws.sourcesDir,
      AW_AGENT_PROMPT_FILE: ws.promptPath,
      BUILD_TRIGGEREDBY_BUILDID: "999",
      BUILD_TRIGGEREDBY_PROJECTID: "00000000-0000-0000-0000-000000000001",
      BUILD_TRIGGEREDBY_DEFINITIONNAME: "upstream",
    };
    const rc = await main(env);
    // Soft fail: rc 0 + error.txt + failure fragment.
    expect(rc).toBe(0);

    const dir = join(ws.sourcesDir, "aw-context", "pipeline");
    expect(readFileSync(join(dir, "error.txt"), "utf8")).toContain(
      "failed to fetch upstream build 999",
    );
    // None of the upstream-* success files should be present on failure.
    expect(existsSync(join(dir, "upstream-status"))).toBe(false);
    expect(readFileSync(ws.promptPath, "utf8")).toContain(
      "Pipeline-completion context preparation failed.",
    );

    // listArtifacts should not be called when the initial getBuildById fails.
    expect(listArtifacts).not.toHaveBeenCalled();
  });

  it("writes error.txt when identifiers fail validation", async () => {
    const env: NodeJS.ProcessEnv = {
      BUILD_SOURCESDIRECTORY: ws.sourcesDir,
      AW_AGENT_PROMPT_FILE: ws.promptPath,
      BUILD_TRIGGEREDBY_BUILDID: "evil; rm -rf /",
      BUILD_TRIGGEREDBY_PROJECTID: "00000000-0000-0000-0000-000000000001",
    };
    const rc = await main(env);
    expect(rc).toBe(0);
    const dir = join(ws.sourcesDir, "aw-context", "pipeline");
    expect(readFileSync(join(dir, "error.txt"), "utf8")).toMatch(
      /BUILD_TRIGGEREDBY_BUILDID/,
    );
    // No REST calls when validation fails.
    expect(getBuildById).not.toHaveBeenCalled();
    expect(listArtifacts).not.toHaveBeenCalled();
  });

  it("removes stale artefacts from a prior run", async () => {
    const dir = join(ws.sourcesDir, "aw-context", "pipeline");
    mkdirSync(dir, { recursive: true });
    writeFileSync(join(dir, "upstream-status"), "STALE-failed", "utf8");
    writeFileSync(join(dir, "error.txt"), "stale error", "utf8");

    getBuildById.mockResolvedValue({
      id: 1,
      sourceVersion: "fresh",
      sourceBranch: "refs/heads/main",
      result: BuildResult.Succeeded,
      definition: { name: "u" },
    });
    listArtifacts.mockResolvedValue([]);

    const env: NodeJS.ProcessEnv = {
      BUILD_SOURCESDIRECTORY: ws.sourcesDir,
      AW_AGENT_PROMPT_FILE: ws.promptPath,
      BUILD_TRIGGEREDBY_BUILDID: "1",
      BUILD_TRIGGEREDBY_PROJECTID: "00000000-0000-0000-0000-000000000001",
      BUILD_TRIGGEREDBY_DEFINITIONNAME: "u",
    };
    await main(env);

    expect(readFileSync(join(dir, "upstream-status"), "utf8")).toBe("succeeded");
    expect(existsSync(join(dir, "error.txt"))).toBe(false);
  });
});
