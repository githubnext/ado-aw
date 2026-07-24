import { describe, expect, it, vi, beforeEach } from "vitest";

const mockCalls: string[] = [];
const compiledFixturePaths: string[] = [];
let queuedFixtureNames: string[] = [];

const baseEnv = {
  SYSTEM_COLLECTIONURI: "https://dev.azure.com/org/",
  SYSTEM_TEAMPROJECT: "AgentPlayground",
  SYSTEM_ACCESSTOKEN: "secret-token",
  BUILD_BUILDID: "630001",
  BUILD_SOURCEBRANCH: "refs/heads/main",
  BUILD_SOURCEVERSION: "basecommit",
  BUILD_SOURCESDIRECTORY: "C:\\repo",
  SYSTEM_DEFINITIONID: "2560",
  COMPILER_SMOKE_ADO_AW_BIN: "C:\\bin\\ado-aw.exe",
  COMPILER_SMOKE_ARTIFACT_NAME: "ado-aw-candidate",
  COMPILER_SMOKE_MIRROR_REPO: "ado-aw-mirror",
  COMPILER_SMOKE_CANARY_DEFINITION_ID: "3001",
  COMPILER_SMOKE_AZURE_CLI_DEFINITION_ID: "3002",
  COMPILER_SMOKE_NOOP_TARGET_DEFINITION_ID: "3003",
  COMPILER_SMOKE_REPORTER_DEFINITION_ID: "3004",
  COMPILER_SMOKE_CUSTOM_SAFE_OUTPUT_DEFINITION_ID: "3005",
  COMPILER_SMOKE_CHILD_TIMEOUT_MS: "5000",
  COMPILER_SMOKE_POLL_MS: "1",
};

function specificRunYaml(agentReadToken: boolean): string {
  const readTokenEnv = agentReadToken
    ? "\n          AZURE_DEVOPS_EXT_PAT: $(SC_READ_TOKEN)"
    : "";
  return `
jobs:
  - job: Agent
    steps:
      - bash: copilot --allow-tool "shell(az)" --allow-tool "shell(head)"
        displayName: Run copilot (AWF network isolated)
        env:
          GITHUB_TOKEN: $(GITHUB_TOKEN)${readTokenEnv}
      - task: DownloadPipelineArtifact@2
        inputs:
          targetPath: in
          source: specific
          project: AgentPlayground
          pipeline: '2560'
          runVersion: specific
          runId: '630001'
          artifact: ado-aw-candidate
  - job: Detection
    steps:
      - bash: echo detection
        displayName: Run threat analysis (AWF network isolated)
        env:
          GITHUB_TOKEN: $(GITHUB_TOKEN)
`;
}

vi.mock("../ado-rest.js", () => {
  return {
    AdoRest: vi.fn().mockImplementation(function FakeAdoRest() {
      return {
        getArtifact: vi.fn(async () => {
          mockCalls.push("getArtifact");
          return { name: "ado-aw-candidate" };
        }),
        getBuild: vi.fn(async () => ({ status: "completed", result: "succeeded" })),
        getBuildTags: vi.fn(async (buildId: number) => [
          `ado-aw-custom-script-${buildId}`,
          `ado-aw-custom-job-${buildId}`,
        ]),
        queueBuild: vi.fn(async () => ({ id: 1 })),
        cancelBuild: vi.fn(async () => {}),
        buildUrl: (id: number) => `https://example/${id}`,
      };
    }),
    redactToken: (text: string) => text,
  };
});

vi.mock("../git.js", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../git.js")>();
  return {
    ...actual,
    mirrorRepoUrl: actual.mirrorRepoUrl,
    verifyLocalCommit: vi.fn(async () => {
      mockCalls.push("verifyLocalCommit");
    }),
    createDetachedWorktree: vi.fn(async () => {
      mockCalls.push("createDetachedWorktree");
    }),
    removeWorktree: vi.fn(async () => {
      mockCalls.push("removeWorktree");
    }),
    worktreeChangedFiles: vi.fn(async () => {
      mockCalls.push("worktreeChangedFiles");
      return [
        "tests/safe-outputs/canary.md",
        "tests/safe-outputs/canary.lock.yml",
        "tests/safe-outputs/azure-cli.md",
        "tests/safe-outputs/azure-cli.lock.yml",
        "tests/safe-outputs/noop-target.md",
        "tests/safe-outputs/noop-target.lock.yml",
        "tests/safe-outputs/smoke-failure-reporter.md",
        "tests/safe-outputs/smoke-failure-reporter.lock.yml",
        "tests/compiler-smoke-e2e/custom-safe-output.md",
        "tests/compiler-smoke-e2e/custom-safe-output.lock.yml",
        ".ado-aw/imports/.gitattributes",
        ".ado-aw/imports/AgentPlayground/ado-aw-e2e-fixture/aa711dd17c4dfcde492b2bfad62e5fb1baad71f6/components/custom-build-tags/component.md",
        ".ado-aw/imports/AgentPlayground/ado-aw-e2e-fixture/aa711dd17c4dfcde492b2bfad62e5fb1baad71f6/components/custom-build-tags/component.md.sha256",
      ];
    }),
    commitAll: vi.fn(async () => {
      mockCalls.push("commitAll");
      return "candidate-sha";
    }),
    pushCandidate: vi.fn(async () => {
      mockCalls.push("pushCandidate");
    }),
    verifyRemoteRef: vi.fn(async () => {
      mockCalls.push("verifyRemoteRef");
    }),
    deleteRemoteRef: vi.fn(async () => {
      mockCalls.push("deleteRemoteRef");
    }),
    listCandidateRefs: vi.fn(async () => {
      mockCalls.push("listCandidateRefs");
      return [];
    }),
  };
});

vi.mock("../compile-cli.js", () => ({
  compileAndCheck: vi.fn(async (opts: { relMd: string }) => {
    mockCalls.push("compileAndCheck");
    compiledFixturePaths.push(opts.relMd);
    return { ok: true, stdout: "", stderr: "" };
  }),
}));

vi.mock("../runner.js", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../runner.js")>();
  return {
    ...actual,
    runFixtures: vi.fn(async (_client: unknown, requests: { name: string }[]) => {
      mockCalls.push("runFixtures");
      queuedFixtureNames = requests.map((request) => request.name);
      return {
        ok: true,
        allTerminal: true,
        results: requests.map((r) => ({
          name: r.name,
          definitionId: 0,
          buildId: 1,
          url: "https://example/1",
          status: "succeeded" as const,
          result: "succeeded",
          durationMs: 1,
          terminalProven: true,
        })),
      };
    }),
  };
});

vi.mock("node:fs/promises", async (importOriginal) => {
  const actual = await importOriginal<typeof import("node:fs/promises")>();
  return {
    ...actual,
    mkdtemp: vi.fn(async () => "C:\\tmp\\compiler-smoke-xyz"),
    readFile: vi.fn(async (path: string) => {
      if (String(path).endsWith(".lock.yml")) {
        return specificRunYaml(!String(path).includes("custom-safe-output.lock.yml"));
      }
      return "---\nname: fixture\n---\nBody.\n";
    }),
    writeFile: vi.fn(async () => {}),
    rm: vi.fn(async () => {}),
  };
});

beforeEach(() => {
  mockCalls.length = 0;
  compiledFixturePaths.length = 0;
  queuedFixtureNames = [];
  vi.clearAllMocks();
});

describe("compiler-smoke-e2e index.main (happy path)", () => {
  it(
    "checks artifact visibility before any git work, and deletes the ref before removing the worktree",
    async () => {
      process.env = { ...process.env, ...baseEnv, VITEST: "true" };
      const { main } = await import("../index.js");
      const code = await main();
      expect(code).toBe(0);

      expect(mockCalls.indexOf("getArtifact")).toBeGreaterThanOrEqual(0);
      expect(mockCalls.indexOf("getArtifact")).toBeLessThan(mockCalls.indexOf("verifyLocalCommit"));
      expect(mockCalls.indexOf("verifyLocalCommit")).toBeLessThan(mockCalls.indexOf("createDetachedWorktree"));
      expect(mockCalls.indexOf("createDetachedWorktree")).toBeLessThan(mockCalls.indexOf("compileAndCheck"));
      expect(mockCalls.indexOf("compileAndCheck")).toBeLessThan(mockCalls.indexOf("worktreeChangedFiles"));
      expect(mockCalls.indexOf("worktreeChangedFiles")).toBeLessThan(mockCalls.indexOf("commitAll"));
      expect(mockCalls.indexOf("commitAll")).toBeLessThan(mockCalls.indexOf("pushCandidate"));
      expect(mockCalls.indexOf("pushCandidate")).toBeLessThan(mockCalls.indexOf("verifyRemoteRef"));
      expect(mockCalls.indexOf("verifyRemoteRef")).toBeLessThan(mockCalls.indexOf("runFixtures"));
      expect(compiledFixturePaths).toEqual([
        "tests/safe-outputs/canary.md",
        "tests/safe-outputs/azure-cli.md",
        "tests/safe-outputs/noop-target.md",
        "tests/safe-outputs/smoke-failure-reporter.md",
        "tests/compiler-smoke-e2e/custom-safe-output.md",
      ]);
      expect(compiledFixturePaths).not.toContain("tests/safe-outputs/janitor.md");
      expect(queuedFixtureNames).toEqual([
        "canary",
        "azure-cli",
        "noop-target",
        "smoke-failure-reporter",
        "custom-safe-output",
      ]);
      expect(queuedFixtureNames).not.toContain("janitor");

      // Cleanup ordering: the remote candidate ref must be deleted BEFORE the
      // local worktree is removed (never leave the ref hanging around).
      expect(mockCalls.indexOf("deleteRemoteRef")).toBeGreaterThanOrEqual(0);
      expect(mockCalls.indexOf("removeWorktree")).toBeGreaterThanOrEqual(0);
      expect(mockCalls.indexOf("deleteRemoteRef")).toBeLessThan(mockCalls.indexOf("removeWorktree"));
    },
    15_000,
  );
});

describe("compiler-smoke-e2e index.main (unexpected path guard)", () => {
  it("refuses to push and never deletes a ref that was never pushed, but still removes the worktree", async () => {
    const gitModule = await import("../git.js");
    vi.mocked(gitModule.worktreeChangedFiles).mockResolvedValueOnce([
      "tests/safe-outputs/canary.md",
      "src/main.rs", // unexpected — must abort before any commit/push
    ]);

    process.env = { ...process.env, ...baseEnv, VITEST: "true" };
    const { main } = await import("../index.js");
    const code = await main();
    expect(code).toBe(1);

    expect(mockCalls).not.toContain("commitAll");
    expect(mockCalls).not.toContain("pushCandidate");
    expect(mockCalls).not.toContain("deleteRemoteRef");
    expect(mockCalls).toContain("removeWorktree");
  });
});

describe("compiler-smoke-e2e index.main (stageFixtures reads from the worktree, not BUILD_SOURCESDIRECTORY)", () => {
  it("reads every fixture markdown source from the detached worktree — never from BUILD_SOURCESDIRECTORY (which may sit at a different commit when verifyLocalCommit falls back to the object-existence check)", async () => {
    process.env = { ...process.env, ...baseEnv, VITEST: "true" };
    const { main } = await import("../index.js");
    const fsModule = await import("node:fs/promises");
    const code = await main();
    expect(code).toBe(0);

    const mdReadPaths = vi
      .mocked(fsModule.readFile)
      .mock.calls.map((call) => String(call[0]))
      .filter((p) => p.endsWith(".md"));
    expect(mdReadPaths.length).toBeGreaterThan(0);
    for (const p of mdReadPaths) {
      // The worktree lives under the mocked mkdtemp() result, never under
      // BUILD_SOURCESDIRECTORY ("C:\repo").
      expect(p.startsWith("C:\\tmp\\compiler-smoke-xyz")).toBe(true);
      expect(p.startsWith("C:\\repo")).toBe(false);
    }
  });
});

describe("compiler-smoke-e2e index.main (PR base-ref regression — Fix #1)", () => {
  it("never fetches BUILD_SOURCEBRANCH from the mirror for a GitHub PR build; the worktree is based on the local BUILD_SOURCEVERSION", async () => {
    process.env = {
      ...process.env,
      ...baseEnv,
      BUILD_SOURCEBRANCH: "refs/pull/123/merge", // does not exist on the ADO mirror repo
      BUILD_SOURCEVERSION: "pr-head-sha",
      VITEST: "true",
    };
    const { main } = await import("../index.js");
    const gitModule = await import("../git.js");
    const code = await main();
    expect(code).toBe(0);

    // verifyLocalCommit must be asked to verify the LOCAL BUILD_SOURCEVERSION
    // — never the PR ref.
    expect(vi.mocked(gitModule.verifyLocalCommit).mock.calls[0]?.[0]).toMatchObject({
      cwd: "C:\\repo",
      expectedSha: "pr-head-sha",
    });
    // The worktree is created directly from that same local commit; it must
    // never receive `refs/pull/123/merge` as the commitish.
    const worktreeArgs = vi.mocked(gitModule.createDetachedWorktree).mock.calls[0]?.[0] as
      | { commitish?: string }
      | undefined;
    expect(worktreeArgs?.commitish).toBe("pr-head-sha");
    expect(worktreeArgs?.commitish).not.toBe("refs/pull/123/merge");
  });
});

describe("compiler-smoke-e2e index.main (unproven-terminal ref retention — Fix #3)", () => {
  it("retains (does not delete) the candidate ref when runFixtures cannot prove every build reached a terminal state", async () => {
    const runnerModule = await import("../runner.js");
    vi.mocked(runnerModule.runFixtures).mockResolvedValueOnce({
      ok: false,
      allTerminal: false,
      results: [
        {
          name: "canary",
          definitionId: 3001,
          buildId: 1,
          url: "https://example/1",
          status: "failed",
          message: "getBuild kept failing",
          durationMs: 1,
          terminalProven: false,
        },
      ],
    });

    process.env = { ...process.env, ...baseEnv, VITEST: "true" };
    const { main } = await import("../index.js");
    const code = await main();
    expect(code).toBe(1);

    // The push itself succeeded, but the ref must be RETAINED, not deleted,
    // because this run could not prove the build actually stopped.
    expect(mockCalls).toContain("pushCandidate");
    expect(mockCalls).not.toContain("deleteRemoteRef");
    expect(mockCalls).toContain("removeWorktree");
  });

  it("retains (does not delete) the candidate ref when runFixtures itself throws unexpectedly (never trusts the fail-closed default's absence of proof)", async () => {
    const runnerModule = await import("../runner.js");
    vi.mocked(runnerModule.runFixtures).mockImplementationOnce(async () => {
      mockCalls.push("runFixtures");
      throw new Error("runner crashed after queueing an unknown number of builds");
    });

    process.env = { ...process.env, ...baseEnv, VITEST: "true" };
    const { main } = await import("../index.js");
    const code = await main();
    expect(code).toBe(1);

    // The push succeeded and runFixtures was entered, but because it threw
    // instead of returning a proven outcome, `allChildrenTerminal` must
    // still be `false` (its pre-call fail-closed value) — the ref must be
    // retained, never deleted.
    expect(mockCalls).toContain("pushCandidate");
    expect(mockCalls).toContain("runFixtures");
    expect(mockCalls).not.toContain("deleteRemoteRef");
    expect(mockCalls).toContain("removeWorktree");
  });
});
