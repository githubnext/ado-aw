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

  // ── Real-PR path ─────────────────────────────────────────────────
  //
  // On a real PR build, ADO populates `SYSTEM_PULLREQUEST_*` env vars
  // directly. The bundle propagates them into the canonical `AW_PR_*`
  // namespace so downstream consumers can read a single name regardless
  // of the build's reason. No API call is made; no `AW_SYNTHETIC_PR`
  // flag is emitted (this is not a synth-promotion).

  it("propagates SYSTEM_PULLREQUEST_* to AW_PR_* on real PR builds", async () => {
    const { code, output } = await runMain(
      makeEnv({
        BUILD_REASON: "PullRequest",
        SYSTEM_PULLREQUEST_PULLREQUESTID: "4242",
        SYSTEM_PULLREQUEST_TARGETBRANCH: "refs/heads/main",
        SYSTEM_PULLREQUEST_SOURCEBRANCH: "refs/heads/feature/x",
        SYSTEM_PULLREQUEST_ISDRAFT: "false",
        PR_SYNTH_SPEC: build_pr_synth_spec(),
      }),
    );
    expect(code).toBe(0);
    expect(mocked.listActivePullRequestsBySourceRef).not.toHaveBeenCalled();
    // AW_PR_* are emitted as BOTH output (cross-job) and var (same-job).
    expect(output).toContain("AW_PR_ID;isOutput=true]4242");
    expect(output).toContain("##vso[task.setvariable variable=AW_PR_ID]4242");
    expect(output).toContain("AW_PR_TARGETBRANCH;isOutput=true]refs/heads/main");
    expect(output).toContain(
      "##vso[task.setvariable variable=AW_PR_TARGETBRANCH]refs/heads/main",
    );
    expect(output).toContain("AW_PR_SOURCEBRANCH;isOutput=true]refs/heads/feature/x");
    expect(output).toContain(
      "##vso[task.setvariable variable=AW_PR_SOURCEBRANCH]refs/heads/feature/x",
    );
    expect(output).toContain("AW_PR_IS_DRAFT;isOutput=true]false");
    // No synth-promotion flag on a real PR build.
    expect(output).not.toContain("AW_SYNTHETIC_PR;isOutput=true]true");
    expect(output).not.toContain("AW_SYNTHETIC_PR_SKIP");
    expect(output).toContain("real PR build");
  });

  it("detects real PR build by SYSTEM_PULLREQUEST_PULLREQUESTID, not BUILD_REASON", async () => {
    // Defensive: even on builds where BUILD_REASON isn't "PullRequest"
    // for some reason (e.g. a manual re-queue of a PR build), the
    // presence of a SYSTEM_PULLREQUEST_PULLREQUESTID is the authoritative
    // signal — that's the value we need to propagate.
    const { code, output } = await runMain(
      makeEnv({
        BUILD_REASON: "Manual",
        SYSTEM_PULLREQUEST_PULLREQUESTID: "99",
        SYSTEM_PULLREQUEST_TARGETBRANCH: "refs/heads/main",
        PR_SYNTH_SPEC: build_pr_synth_spec(),
      }),
    );
    expect(code).toBe(0);
    expect(mocked.listActivePullRequestsBySourceRef).not.toHaveBeenCalled();
    expect(output).toContain("AW_PR_ID;isOutput=true]99");
  });

  // ── GitHub repo path ────────────────────────────────────────────

  it("skips with empty AW_PR_* on GitHub-typed repos (CI builds)", async () => {
    const { code, output } = await runMain(
      makeEnv({
        BUILD_REPOSITORY_PROVIDER: "GitHub",
        PR_SYNTH_SPEC: build_pr_synth_spec(),
      }),
    );
    expect(code).toBe(0);
    expect(mocked.listActivePullRequestsBySourceRef).not.toHaveBeenCalled();
    expect(output).toContain("GitHub-typed repo");
    // SKIP marker tells the Agent job's condition to opt out cleanly.
    expect(output).toContain("AW_SYNTHETIC_PR_SKIP;isOutput=true]true");
    // Empty AW_PR_* so same-job consumers see stable defined variables.
    expect(output).toContain("##vso[task.setvariable variable=AW_PR_ID]");
    expect(output).not.toContain("AW_PR_ID;isOutput=true]4242");
  });

  // ── Hard failures ────────────────────────────────────────────────

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

  // ── Soft skips (CI build, ADO repo, no matching PR) ─────────────
  //
  // Every soft-skip path emits empty AW_PR_* via setVar+setOutput so
  // downstream consumers see stable defined variables (rather than the
  // literal `$(AW_PR_ID)` string that ADO leaves when a macro is
  // undefined). The SKIP marker gates the Agent job's `condition:`.

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
    // Empty defaults for AW_PR_*.
    expect(output).toContain("##vso[task.setvariable variable=AW_PR_ID]");
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
    expect(output).toContain("##vso[task.setvariable variable=AW_PR_ID]");
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

  // ── Happy path: synth-promote a CI build with a matching PR ─────

  it("emits AW_PR_* + AW_SYNTHETIC_PR=true on the synth happy path", async () => {
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
    // AW_PR_* emitted TWICE: once as output (cross-job, hoisted into the
    // Agent job's `variables:` block) and once as a regular variable
    // (same-job, consumed by the Setup-job gate step's `env:` via
    // `$(AW_PR_*)` macros). See `setVar` in `shared/vso-logger.ts`.
    expect(output).toContain("AW_PR_ID;isOutput=true]1234");
    expect(output).toContain("##vso[task.setvariable variable=AW_PR_ID]1234");
    expect(output).toContain("AW_PR_TARGETBRANCH;isOutput=true]refs/heads/main");
    expect(output).toContain(
      "##vso[task.setvariable variable=AW_PR_TARGETBRANCH]refs/heads/main",
    );
    expect(output).toContain("AW_PR_SOURCEBRANCH;isOutput=true]refs/heads/feature/x");
    expect(output).toContain(
      "##vso[task.setvariable variable=AW_PR_SOURCEBRANCH]refs/heads/feature/x",
    );
    expect(output).toContain("AW_PR_IS_DRAFT;isOutput=true]false");
    // Synth-promotion flag (only on this path, not real PR or skip).
    expect(output).toContain("AW_SYNTHETIC_PR;isOutput=true]true");
    expect(output).toContain("##vso[task.setvariable variable=AW_SYNTHETIC_PR]true");
  });

  it("emits AW_PR_IS_DRAFT=true when the matched synth PR is a draft", async () => {
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
    expect(output).toContain("AW_PR_IS_DRAFT;isOutput=true]true");
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
