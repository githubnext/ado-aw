import { describe, expect, it, vi, beforeEach } from "vitest";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import type { FixtureBuildResult } from "../runner.js";

const mockCalls: string[] = [];
const compiledCasePaths: string[] = [];
const stagedWrites: { to: string; contents: string }[] = [];
let queuedCaseIds: string[] = [];
let queuedRequests: { caseId: string; lane: string; definitionId: number; sourceBranch: string }[] = [];
let deletedRefs: string[] = [];

const HERE = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = join(HERE, "..", "..", "..", "..", "..");
/** The REAL shipped manifest, so these tests fail if `cases.json` drifts. */
const REAL_MANIFEST = readFileSync(join(REPO_ROOT, "tests", "smoke", "cases.json"), "utf8");

const WORKTREE = "C:\\tmp\\ado-aw-smoke-xyz";

const baseEnv = {
  SYSTEM_COLLECTIONURI: "https://dev.azure.com/org/",
  SYSTEM_TEAMPROJECT: "AgentPlayground",
  SYSTEM_ACCESSTOKEN: "secret-token",
  BUILD_BUILDID: "630001",
  BUILD_SOURCEBRANCH: "refs/heads/main",
  BUILD_SOURCEVERSION: "basecommit",
  BUILD_SOURCESDIRECTORY: "C:\\repo",
  SYSTEM_DEFINITIONID: "2560",
  SMOKE_ADO_AW_BIN: "C:\\bin\\ado-aw.exe",
  SMOKE_ARTIFACT_NAME: "ado-aw-candidate",
  SMOKE_MIRROR_REPO: "ado-aw-mirror",
  SMOKE_COMPILER_SOURCE: "candidate",
  SMOKE_LANE_AGENTIC_DEFINITION_ID: "3001",
  SMOKE_LANE_INFRA_DEFINITION_ID: "3003",
  SMOKE_CHILD_TIMEOUT_MS: "5000",
  SMOKE_POLL_MS: "1",
};

/**
 * Compiled output shaped enough to satisfy every candidate-mode assertion.
 *
 * Carries explicit `trigger: none` / `pr: none`, because that is what
 * `ado-aw compile` emits once `prepareCaseSource` has stripped the `on:`
 * block — `on:` is the complete declaration of when a pipeline runs, so its
 * absence compiles to a manual / API-queued-only pipeline. The harness no
 * longer patches these keys in; `assertNoTriggers` verifies the compiler
 * produced them, so a compiler that regressed to omitting them (which ADO
 * reads as "CI on every branch") fails the run instead of silently
 * double-queueing the lane.
 */
function specificRunYaml(): string {
  return `
trigger: none
pr: none
jobs:
  - job: Agent
    steps:
      - bash: >-
          copilot --allow-tool "shell(az)" --allow-tool "shell(head)"
          --topology-attach "awmg-mcpg"
          --topology-attach "awmg-ado-proxy"
        displayName: Run copilot (AWF network isolated)
        env:
          GITHUB_TOKEN: $(GITHUB_TOKEN)
      - bash: |
          echo '"ADO_MCP_AUTH_TOKEN": "ado-proxy-injects-the-real-credential"'
          echo '"--network", "ado-aw-proxy-net",'
        displayName: Start ado-proxy policy engine
      - bash: echo peers running
        displayName: Verify trusted topology peers
      - bash: echo stop
        displayName: Stop ado-proxy
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
        // The real manifest has two cases with runtime tag proofs. Returning
        // both here keeps the generic build-id-only ADO mock independent of
        // which case is currently being verified.
        getBuildTags: vi.fn(async (buildId: number) => [
          `ado-aw-custom-job-${buildId}`,
          `ado-aw-proxy-${buildId}`,
        ]),
        queueBuild: vi.fn(async () => ({ id: 1 })),
        cancelBuild: vi.fn(async () => {}),
        addBuildTags: vi.fn(async () => {}),
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
    resetWorktree: vi.fn(async () => {
      mockCalls.push("resetWorktree");
    }),
    // Only the paths one case is allowed to touch — the harness resets
    // between cases, so a per-case commit never sees a sibling's changes.
    worktreeChangedFiles: vi.fn(async () => {
      mockCalls.push("worktreeChangedFiles");
      return [".smoke/pipeline.yml"];
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
    deleteRemoteRefs: vi.fn(async (opts: { refs: readonly string[] }) => {
      mockCalls.push("deleteRemoteRefs");
      deletedRefs.push(...opts.refs);
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
    compiledCasePaths.push(opts.relMd);
    return { ok: true, stdout: "", stderr: "" };
  }),
}));

vi.mock("../signals.js", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../signals.js")>();
  return {
    ...actual,
    verifyCandidateAudit: vi.fn(async (results: readonly FixtureBuildResult[]) => ({
      ok: true,
      results: results.map((result) => ({ ...result })),
    })),
  };
});

vi.mock("../runner.js", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../runner.js")>();
  return {
    ...actual,
    runFixtures: vi.fn(
      async (
        _client: unknown,
        requests: { caseId: string; lane: string; definitionId: number; sourceBranch: string }[],
      ) => {
        mockCalls.push("runFixtures");
        queuedCaseIds = requests.map((request) => request.caseId);
        queuedRequests = requests.map((r) => ({ ...r }));
        return {
          ok: true,
          allTerminal: true,
          results: requests.map((r) => ({
            caseId: r.caseId,
            lane: r.lane,
            definitionId: r.definitionId,
            buildId: 1,
            url: "https://example/1",
            status: "succeeded" as const,
            result: "succeeded",
            durationMs: 1,
            terminalProven: true,
          })),
        };
      },
    ),
  };
});

vi.mock("node:fs/promises", async (importOriginal) => {
  const actual = await importOriginal<typeof import("node:fs/promises")>();
  return {
    ...actual,
    mkdtemp: vi.fn(async () => WORKTREE),
    mkdir: vi.fn(async () => undefined),
    readFile: vi.fn(async (path: string) => {
      const p = String(path);
      // Serve the REAL manifest so these tests exercise the shipped cases.
      if (p.endsWith("cases.json")) return REAL_MANIFEST;
      if (p.endsWith(".lock.yml") || p.endsWith(".yml")) return specificRunYaml();
      return "---\nname: case\n---\nBody.\n";
    }),
    writeFile: vi.fn(async (path: string, contents: string) => {
      const p = String(path);
      if (p.endsWith("pipeline.yml")) stagedWrites.push({ to: p, contents: String(contents) });
    }),
    rm: vi.fn(async () => {}),
  };
});

beforeEach(() => {
  mockCalls.length = 0;
  compiledCasePaths.length = 0;
  stagedWrites.length = 0;
  queuedCaseIds = [];
  queuedRequests = [];
  deletedRefs = [];
  vi.clearAllMocks();
});

describe("smoke-e2e index.main (happy path, candidate mode)", () => {
  it("gates on artifact visibility, stages per case, and deletes refs before removing the worktree", async () => {
    process.env = { ...process.env, ...baseEnv, VITEST: "true" };
    const { main } = await import("../index.js");
    const code = await main();
    expect(code).toBe(0);

    expect(mockCalls.indexOf("getArtifact")).toBeGreaterThanOrEqual(0);
    expect(mockCalls.indexOf("getArtifact")).toBeLessThan(mockCalls.indexOf("verifyLocalCommit"));
    expect(mockCalls.indexOf("verifyLocalCommit")).toBeLessThan(
      mockCalls.indexOf("createDetachedWorktree"),
    );
    expect(mockCalls.indexOf("createDetachedWorktree")).toBeLessThan(
      mockCalls.indexOf("compileAndCheck"),
    );
    expect(mockCalls.indexOf("compileAndCheck")).toBeLessThan(
      mockCalls.indexOf("worktreeChangedFiles"),
    );
    expect(mockCalls.indexOf("worktreeChangedFiles")).toBeLessThan(mockCalls.indexOf("commitAll"));
    expect(mockCalls.indexOf("commitAll")).toBeLessThan(mockCalls.indexOf("pushCandidate"));
    expect(mockCalls.indexOf("pushCandidate")).toBeLessThan(mockCalls.indexOf("verifyRemoteRef"));
    expect(mockCalls.indexOf("verifyRemoteRef")).toBeLessThan(mockCalls.indexOf("runFixtures"));

    // Candidate mode runs exactly the cases the manifest declares for it.
    expect(queuedCaseIds).toEqual([
      "canary",
      "ado-proxy",
      "noop-target",
      "custom-safe-output",
      "multi-repo",
    ]);
    expect(queuedCaseIds).not.toContain("janitor");
    expect(compiledCasePaths).toEqual([
      "tests/safe-outputs/canary.md",
      "tests/smoke/ado-proxy.md",
      "tests/safe-outputs/noop-target.md",
      "tests/smoke/custom-safe-output.md",
      "tests/smoke/multi-repo.md",
    ]);

    // Cleanup ordering: remote refs deleted BEFORE the local worktree is removed.
    expect(mockCalls.indexOf("deleteRemoteRefs")).toBeGreaterThanOrEqual(0);
    expect(mockCalls.indexOf("deleteRemoteRefs")).toBeLessThan(mockCalls.indexOf("removeWorktree"));
  }, 60_000);

  it("gives every case its own ref and stages all of them to the one fixed path", async () => {
    process.env = { ...process.env, ...baseEnv, VITEST: "true" };
    const { main } = await import("../index.js");
    expect(await main()).toBe(0);

    expect(queuedRequests.map((r) => r.sourceBranch)).toEqual([
      "refs/heads/ado-aw-smoke-candidate/630001/canary",
      "refs/heads/ado-aw-smoke-candidate/630001/ado-proxy",
      "refs/heads/ado-aw-smoke-candidate/630001/noop-target",
      "refs/heads/ado-aw-smoke-candidate/630001/custom-safe-output",
      "refs/heads/ado-aw-smoke-candidate/630001/multi-repo",
    ]);
    // Every case is staged to the SAME path — the ref is what distinguishes them.
    expect(stagedWrites.length).toBe(5);
    for (const write of stagedWrites) {
      expect(write.to).toBe(join(WORKTREE, "candidate", ".smoke", "pipeline.yml"));
      // The compiler emits no trigger keys once `on:` is stripped, and a
      // MISSING `trigger:` means "CI on every branch" in ADO — which would
      // let this ref push queue the shared lane on top of the API-queued run.
      expect(write.contents).toMatch(/^trigger: none$/m);
      expect(write.contents).toMatch(/^pr: none$/m);
    }
    expect(deletedRefs).toEqual(queuedRequests.map((r) => r.sourceBranch));
  });

  it("routes every candidate case to its lane definition, not a per-case definition", async () => {
    process.env = { ...process.env, ...baseEnv, VITEST: "true" };
    const { main } = await import("../index.js");
    expect(await main()).toBe(0);

    for (const request of queuedRequests) {
      expect(request.lane).toBe("agentic");
      expect(request.definitionId).toBe(3001);
    }
  });

  it("resets the worktree between cases so each commit is a sibling of BUILD_SOURCEVERSION", async () => {
    process.env = { ...process.env, ...baseEnv, VITEST: "true" };
    const { main } = await import("../index.js");
    expect(await main()).toBe(0);

    const gitModule = await import("../git.js");
    const resets = vi.mocked(gitModule.resetWorktree).mock.calls;
    expect(resets.length).toBe(5);
    for (const call of resets) {
      expect(call[0]).toMatchObject({ commitish: "basecommit" });
    }
  });
});

describe("smoke-e2e index.main (released mode)", () => {
  it("skips the artifact gate and runs the released-mode case set", async () => {
    process.env = {
      ...process.env,
      ...baseEnv,
      SMOKE_COMPILER_SOURCE: "released",
      VITEST: "true",
    };
    const { main } = await import("../index.js");
    // Released mode asserts release URLs are PRESENT; the fake compiled YAML
    // has none, so staging is expected to fail closed rather than silently pass.
    const code = await main();
    expect(code).toBe(1);

    // The artifact-visibility gate is candidate-only: there is no candidate
    // artifact to gate on in released mode.
    expect(mockCalls).not.toContain("getArtifact");
    expect(mockCalls).toContain("createDetachedWorktree");
  });
});

describe("smoke-e2e index.main (unexpected path guard)", () => {
  it("refuses to push and never deletes a ref that was never pushed, but still removes the worktree", async () => {
    const gitModule = await import("../git.js");
    vi.mocked(gitModule.worktreeChangedFiles).mockResolvedValueOnce([
      ".smoke/pipeline.yml",
      "src/main.rs", // unexpected — must abort before any commit/push
    ]);

    process.env = { ...process.env, ...baseEnv, VITEST: "true" };
    const { main } = await import("../index.js");
    const code = await main();
    expect(code).toBe(1);

    expect(mockCalls).not.toContain("commitAll");
    expect(mockCalls).not.toContain("pushCandidate");
    expect(mockCalls).not.toContain("deleteRemoteRefs");
    expect(mockCalls).toContain("removeWorktree");
  });
});

describe("smoke-e2e index.main (sources read from the worktree, not BUILD_SOURCESDIRECTORY)", () => {
  it("reads every case source from the detached worktree", async () => {
    process.env = { ...process.env, ...baseEnv, VITEST: "true" };
    const { main } = await import("../index.js");
    const fsModule = await import("node:fs/promises");
    expect(await main()).toBe(0);

    const readPaths = vi
      .mocked(fsModule.readFile)
      .mock.calls.map((call) => String(call[0]))
      .filter((p) => p.endsWith(".md") || p.endsWith("cases.json"));
    expect(readPaths.length).toBeGreaterThan(0);
    for (const p of readPaths) {
      expect(p.startsWith(WORKTREE)).toBe(true);
      expect(p.startsWith("C:\\repo")).toBe(false);
    }
  });
});

describe("smoke-e2e index.main (PR base-ref regression)", () => {
  it("never fetches BUILD_SOURCEBRANCH from the mirror for a GitHub PR build", async () => {
    process.env = {
      ...process.env,
      ...baseEnv,
      BUILD_SOURCEBRANCH: "refs/pull/123/merge", // does not exist on the ADO mirror repo
      BUILD_SOURCEVERSION: "pr-head-sha",
      VITEST: "true",
    };
    const { main } = await import("../index.js");
    const gitModule = await import("../git.js");
    expect(await main()).toBe(0);

    expect(vi.mocked(gitModule.verifyLocalCommit).mock.calls[0]?.[0]).toMatchObject({
      cwd: "C:\\repo",
      expectedSha: "pr-head-sha",
    });
    const worktreeArgs = vi.mocked(gitModule.createDetachedWorktree).mock.calls[0]?.[0] as
      | { commitish?: string }
      | undefined;
    expect(worktreeArgs?.commitish).toBe("pr-head-sha");
    expect(worktreeArgs?.commitish).not.toBe("refs/pull/123/merge");
  });
});

describe("smoke-e2e index.main (per-case ref retention)", () => {
  it("retains only the unproven case's ref and still deletes the proven ones", async () => {
    const runnerModule = await import("../runner.js");
    vi.mocked(runnerModule.runFixtures).mockImplementationOnce(
      async (_client: unknown, requests: readonly { caseId: string; lane: string; definitionId: number }[]) => ({
        ok: false,
        allTerminal: false,
        results: requests.map((r, i) => ({
          caseId: r.caseId,
          lane: r.lane,
          definitionId: r.definitionId,
          buildId: i + 1,
          url: `https://example/${i + 1}`,
          status: (i === 1 ? "failed" : "succeeded") as "failed" | "succeeded",
          message: i === 1 ? "getBuild kept failing" : undefined,
          durationMs: 1,
          // Only the second case could not be proven terminal.
          terminalProven: i !== 1,
        })),
      }),
    );

    process.env = { ...process.env, ...baseEnv, VITEST: "true" };
    const { main } = await import("../index.js");
    expect(await main()).toBe(1);

    expect(mockCalls).toContain("pushCandidate");
    // All but one ref is provably safe to delete; the unproven one is kept
    // for the stale-ref scanner. Under the old shared-ref model, one unproven
    // build stranded every case's ref.
    expect(deletedRefs).toEqual([
      "refs/heads/ado-aw-smoke-candidate/630001/canary",
      "refs/heads/ado-aw-smoke-candidate/630001/noop-target",
      "refs/heads/ado-aw-smoke-candidate/630001/custom-safe-output",
      "refs/heads/ado-aw-smoke-candidate/630001/multi-repo",
    ]);
    expect(deletedRefs).not.toContain("refs/heads/ado-aw-smoke-candidate/630001/ado-proxy");
  });

  it("retains every pushed ref when runFixtures throws, because builds may already be queued", async () => {
    // Fail-closed regression: a throw out of runFixtures leaves no results at
    // all, which must NOT be mistaken for "nothing was queued". Deleting here
    // would pull refs out from under builds that may still be running.
    const runnerModule = await import("../runner.js");
    vi.mocked(runnerModule.runFixtures).mockRejectedValueOnce(new Error("runner exploded"));

    process.env = { ...process.env, ...baseEnv, VITEST: "true" };
    const { main } = await import("../index.js");
    expect(await main()).toBe(1);

    expect(mockCalls).toContain("pushCandidate");
    expect(deletedRefs).toEqual([]);
    expect(mockCalls).toContain("removeWorktree");
  });

  it("still deletes pushed refs when staging fails before any build is queued", async () => {
    // The mirror image: if we never reached runFixtures, no build can exist,
    // so retaining refs would just leak them.
    const gitModule = await import("../git.js");
    vi.mocked(gitModule.worktreeChangedFiles)
      .mockResolvedValueOnce([".smoke/pipeline.yml"])
      .mockResolvedValueOnce([".smoke/pipeline.yml", "src/main.rs"]);

    process.env = { ...process.env, ...baseEnv, VITEST: "true" };
    const { main } = await import("../index.js");
    expect(await main()).toBe(1);

    expect(mockCalls).not.toContain("runFixtures");
    // The first case was pushed before the second tripped the allowlist guard.
    expect(deletedRefs).toEqual(["refs/heads/ado-aw-smoke-candidate/630001/canary"]);
  });
});
