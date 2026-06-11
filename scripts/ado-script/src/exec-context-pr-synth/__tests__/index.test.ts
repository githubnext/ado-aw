import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("../../shared/ado-client.js", () => ({
  listActivePullRequestsBySourceRef: vi.fn(),
  getPullRequestIterations: vi.fn(),
  getIterationChanges: vi.fn(),
}));

import * as adoClient from "../../shared/ado-client.js";
import { build_pr_synth_spec, makeEnv, runMain } from "./harness.js";

const mocked = adoClient as unknown as {
  listActivePullRequestsBySourceRef: ReturnType<typeof vi.fn>;
  getPullRequestIterations: ReturnType<typeof vi.fn>;
  getIterationChanges: ReturnType<typeof vi.fn>;
};

describe("exec-context-pr-synth main", () => {
  beforeEach(() => {
    mocked.listActivePullRequestsBySourceRef.mockReset();
    mocked.getPullRequestIterations.mockReset();
    mocked.getIterationChanges.mockReset();
  });
  afterEach(() => vi.restoreAllMocks());

  it("no-ops on real PR builds (BUILD_REASON=PullRequest)", async () => {
    const { code, output } = await runMain(
      makeEnv({ BUILD_REASON: "PullRequest", PR_SYNTH_SPEC: build_pr_synth_spec() }),
    );
    expect(code).toBe(0);
    expect(mocked.listActivePullRequestsBySourceRef).not.toHaveBeenCalled();
    expect(output).not.toContain("AW_SYNTHETIC_PR_SKIP");
    expect(output).not.toContain("AW_SYNTHETIC_PR=true");
    expect(output).toContain("real PR build");
  });

  it("no-ops on GitHub-typed repos (BUILD_REPOSITORY_PROVIDER=GitHub)", async () => {
    const { code, output } = await runMain(
      makeEnv({
        BUILD_REPOSITORY_PROVIDER: "GitHub",
        PR_SYNTH_SPEC: build_pr_synth_spec(),
      }),
    );
    expect(code).toBe(0);
    expect(mocked.listActivePullRequestsBySourceRef).not.toHaveBeenCalled();
    expect(output).toContain("GitHub-typed repo");
  });

  it("returns 1 (hard fail) when PR_SYNTH_SPEC is missing", async () => {
    const env = makeEnv({});
    delete env.PR_SYNTH_SPEC;
    const { code, output } = await runMain(env);
    expect(code).toBe(1);
    expect(output).toContain("PR_SYNTH_SPEC env var is missing");
  });

  it("returns 1 when PR_SYNTH_SPEC is malformed base64", async () => {
    const { code, output } = await runMain(
      makeEnv({ PR_SYNTH_SPEC: "not!valid!base64!!!" }),
    );
    expect(code).toBe(1);
    expect(output).toContain("PR_SYNTH_SPEC");
  });

  it("skips when source branch has no active PR (per ADO API)", async () => {
    mocked.listActivePullRequestsBySourceRef.mockResolvedValue([]);
    const { code, output } = await runMain(
      makeEnv({
        BUILD_SOURCEBRANCH: "refs/heads/feature/unrelated",
        PR_SYNTH_SPEC: build_pr_synth_spec({ branches: { include: ["main"], exclude: [] } }),
      }),
    );
    expect(code).toBe(0);
    expect(output).toContain("AW_SYNTHETIC_PR_SKIP;isOutput=true]true");
    expect(mocked.listActivePullRequestsBySourceRef).toHaveBeenCalledOnce();
  });

  it("skips when a PR is active but its target branch is excluded by on.pr.branches", async () => {
    mocked.listActivePullRequestsBySourceRef.mockResolvedValue([
      {
        pullRequestId: 1,
        sourceRefName: "refs/heads/feature/x",
        targetRefName: "refs/heads/release/old",
      },
    ]);
    const spec = build_pr_synth_spec({ branches: { include: ["main"], exclude: [] } });
    const { code, output } = await runMain(makeEnv({ PR_SYNTH_SPEC: spec }));
    expect(code).toBe(0);
    expect(output).toContain("AW_SYNTHETIC_PR_SKIP;isOutput=true]true");
  });

  it("skips when >1 active PRs match (after target filter)", async () => {
    mocked.listActivePullRequestsBySourceRef.mockResolvedValue([
      { pullRequestId: 1, sourceRefName: "refs/heads/feature/x", targetRefName: "refs/heads/main" },
      { pullRequestId: 2, sourceRefName: "refs/heads/feature/x", targetRefName: "refs/heads/main" },
    ]);
    const { code, output } = await runMain(makeEnv({ PR_SYNTH_SPEC: build_pr_synth_spec() }));
    expect(code).toBe(0);
    expect(output).toContain("AW_SYNTHETIC_PR_SKIP;isOutput=true]true");
    expect(output).toContain("2 active PRs");
  });

  it("skips when path filter rejects all changed files", async () => {
    mocked.listActivePullRequestsBySourceRef.mockResolvedValue([
      { pullRequestId: 42, sourceRefName: "refs/heads/feature/x", targetRefName: "refs/heads/main" },
    ]);
    mocked.getPullRequestIterations.mockResolvedValue([{ id: 1 }]);
    mocked.getIterationChanges.mockResolvedValue({
      changeEntries: [{ item: { path: "/docs/readme.md" } }],
    });
    const spec = build_pr_synth_spec({
      branches: { include: ["main"], exclude: [] },
      paths: { include: ["src/**"], exclude: [] },
    });
    const { code, output } = await runMain(makeEnv({ PR_SYNTH_SPEC: spec }));
    expect(code).toBe(0);
    expect(output).toContain("AW_SYNTHETIC_PR_SKIP;isOutput=true]true");
    expect(output).toContain("no changed file");
  });

  it("emits AW_SYNTHETIC_PR=true + identifiers on the happy path", async () => {
    mocked.listActivePullRequestsBySourceRef.mockResolvedValue([
      {
        pullRequestId: 1234,
        sourceRefName: "refs/heads/feature/x",
        targetRefName: "refs/heads/main",
        isDraft: false,
      },
    ]);
    mocked.getPullRequestIterations.mockResolvedValue([{ id: 7 }]);
    mocked.getIterationChanges.mockResolvedValue({
      changeEntries: [{ item: { path: "/src/foo.rs" } }],
    });
    const spec = build_pr_synth_spec({
      branches: { include: ["main"], exclude: [] },
      paths: { include: ["src/**"], exclude: [] },
    });
    const { code, output } = await runMain(makeEnv({ PR_SYNTH_SPEC: spec }));
    expect(code).toBe(0);
    expect(output).not.toContain("AW_SYNTHETIC_PR_SKIP");
    // Each AW_SYNTHETIC_PR* variable is emitted TWICE: once as an
    // output (cross-job, consumed by the Agent job condition + the
    // Agent-job-level `variables:` hoist) and once as a regular
    // variable (same-job, consumed by the prGate step's env block
    // via `$[ coalesce(variables['AW_SYNTHETIC_PR_X'], ...) ]`).
    // See `setVar` in `shared/vso-logger.ts` for the rationale.
    expect(output).toContain("AW_SYNTHETIC_PR;isOutput=true]true");
    expect(output).toContain("AW_SYNTHETIC_PR_ID;isOutput=true]1234");
    expect(output).toContain("AW_SYNTHETIC_PR_TARGETBRANCH;isOutput=true]refs/heads/main");
    expect(output).toContain("AW_SYNTHETIC_PR_SOURCEBRANCH;isOutput=true]refs/heads/feature/x");
    expect(output).toContain("AW_SYNTHETIC_PR_IS_DRAFT;isOutput=true]false");
    // Regular-variable counterparts (no `isOutput`). Each line is a
    // separate ##vso command terminated by `]value`.
    expect(output).toContain("##vso[task.setvariable variable=AW_SYNTHETIC_PR]true");
    expect(output).toContain("##vso[task.setvariable variable=AW_SYNTHETIC_PR_ID]1234");
    expect(output).toContain(
      "##vso[task.setvariable variable=AW_SYNTHETIC_PR_TARGETBRANCH]refs/heads/main",
    );
    expect(output).toContain(
      "##vso[task.setvariable variable=AW_SYNTHETIC_PR_SOURCEBRANCH]refs/heads/feature/x",
    );
    expect(output).toContain("##vso[task.setvariable variable=AW_SYNTHETIC_PR_IS_DRAFT]false");
  });

  it("emits AW_SYNTHETIC_PR_IS_DRAFT=true when the PR is a draft", async () => {
    mocked.listActivePullRequestsBySourceRef.mockResolvedValue([
      {
        pullRequestId: 1,
        sourceRefName: "refs/heads/feature/x",
        targetRefName: "refs/heads/main",
        isDraft: true,
      },
    ]);
    const { code, output } = await runMain(makeEnv({ PR_SYNTH_SPEC: build_pr_synth_spec() }));
    expect(code).toBe(0);
    expect(output).toContain("AW_SYNTHETIC_PR_IS_DRAFT;isOutput=true]true");
  });

  it("skips path-filter API calls when paths.include and exclude are both empty", async () => {
    mocked.listActivePullRequestsBySourceRef.mockResolvedValue([
      {
        pullRequestId: 1,
        sourceRefName: "refs/heads/feature/x",
        targetRefName: "refs/heads/main",
      },
    ]);
    const { code, output } = await runMain(makeEnv({ PR_SYNTH_SPEC: build_pr_synth_spec() }));
    expect(code).toBe(0);
    expect(mocked.getPullRequestIterations).not.toHaveBeenCalled();
    expect(mocked.getIterationChanges).not.toHaveBeenCalled();
    expect(output).toContain("AW_SYNTHETIC_PR;isOutput=true]true");
  });
});
