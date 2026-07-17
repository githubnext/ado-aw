import { afterEach, describe, expect, it, vi } from "vitest";

import type { CommitDiffMetadata } from "../../shared/ado-client.js";
import type { GitResult } from "../../shared/git.js";
import type { GitRunners } from "../../shared/merge-base.js";
import {
  main,
  parseArgs,
  type PrepareArgs,
  type PrepareDependencies,
} from "../index.js";

const HEAD = "a".repeat(40);
const TARGET = "b".repeat(40);
const BASE = "c".repeat(40);

function metadata(overrides: Partial<CommitDiffMetadata> = {}): CommitDiffMetadata {
  return {
    commonCommit: BASE,
    aheadCount: 7,
    behindCount: 5,
    sourceCommit: HEAD,
    targetCommit: TARGET,
    ...overrides,
  };
}

function dependencies(opts: {
  remote?: string;
  fetchStatus?: number;
  mergeBase?: string | null;
  targetSha?: string | null;
  metadata?: CommitDiffMetadata;
  metadataError?: unknown;
  chdir?: (dir: string) => void;
} = {}): {
  deps: PrepareDependencies;
  calls: Array<{ args: string[]; env?: Record<string, string> }>;
  dirs: string[];
} {
  const calls: Array<{ args: string[]; env?: Record<string, string> }> = [];
  const dirs: string[] = [];
  const runners: GitRunners = {
    runGit: (args, env) => {
      calls.push({ args, env });
      let result: GitResult = { stdout: "", stderr: "", status: 1 };
      if (args[0] === "fetch") {
        result = {
          stdout: "",
          stderr: opts.fetchStatus === 0 || opts.fetchStatus === undefined ? "" : "fetch failed",
          status: opts.fetchStatus ?? 0,
        };
      } else if (args[0] === "symbolic-ref") {
        result = { stdout: "", stderr: "", status: 0 };
      }
      return result;
    },
    gitOk: (args) => {
      const command = args.join(" ");
      if (command === "rev-parse HEAD") return HEAD;
      if (command === "remote get-url origin") {
        return opts.remote ?? "https://dev.azure.com/org/project/_git/repo";
      }
      if (command.startsWith("merge-base --all ")) {
        return opts.mergeBase === undefined ? BASE : opts.mergeBase;
      }
      if (command.startsWith("rev-parse origin/")) {
        return opts.targetSha === undefined ? TARGET : opts.targetSha;
      }
      return null;
    },
  };
  const getCommitDiffMetadata = vi.fn(async () => {
    if (opts.metadataError !== undefined) throw opts.metadataError;
    return opts.metadata ?? metadata();
  });
  const chdir = opts.chdir ?? ((dir: string) => void dirs.push(dir));
  return {
    deps: { runners, chdir, getCommitDiffMetadata },
    calls,
    dirs,
  };
}

function patchArgs(): PrepareArgs {
  return {
    mode: "patch-base",
    repos: [
      {
        dir: "/src",
        sourceRef: "refs/heads/feature",
        target: "main",
      },
    ],
    fallbackTarget: "main",
  };
}

afterEach(() => vi.restoreAllMocks());

describe("parseArgs", () => {
  it("parses mode and per-repository source/target triples", () => {
    expect(
      parseArgs([
        "--mode",
        "patch-base",
        "--repo-dir",
        "/src",
        "--source-ref",
        "refs/heads/feature",
        "--target-branch",
        "refs/heads/main",
      ]),
    ).toEqual(patchArgs());
  });

  it("allows target-worktree entries without a source ref", () => {
    expect(
      parseArgs([
        "--mode",
        "target-worktree",
        "--repo-dir",
        "/src",
        "--target-branch",
        "develop",
      ]),
    ).toEqual({
      mode: "target-worktree",
      repos: [{ dir: "/src", target: "develop", sourceRef: undefined }],
      fallbackTarget: "main",
      fallbackSourceRef: undefined,
    });
  });

  it("rejects unknown modes", () => {
    expect(() => parseArgs(["--mode", "everything"])).toThrow(/Unsupported/);
  });
});

describe("prepare-pr-base main", () => {
  it("uses ADO metadata to fetch exact shallow source and target ranges", async () => {
    vi.spyOn(process.stdout, "write").mockImplementation(() => true);
    const { deps, calls, dirs } = dependencies();
    const rc = await main(
      patchArgs(),
      {
        SYSTEM_COLLECTIONURI: "https://dev.azure.com/org/",
        SYSTEM_ACCESSTOKEN: "token",
      },
      deps,
    );
    expect(rc).toBe(0);
    expect(dirs).toEqual(["/src"]);
    const fetches = calls.filter((call) => call.args[0] === "fetch");
    expect(fetches).toHaveLength(2);
    expect(fetches[0]!.args).toContain("--depth=8");
    expect(fetches[0]!.args).toContain(
      `+${HEAD}:refs/remotes/origin/ado-aw-prepare-source`,
    );
    expect(fetches[1]!.args).toContain("--depth=6");
    expect(fetches[1]!.args).toContain(
      `+${TARGET}:refs/remotes/origin/main`,
    );
    expect(fetches[0]!.env).toMatchObject({
      GIT_CONFIG_VALUE_0: "Authorization: bearer token",
    });
    expect(calls.some((call) => call.args[0] === "symbolic-ref")).toBe(true);
  });

  it("falls back to bounded dual-ref fetch when ADO REST is unavailable", async () => {
    vi.spyOn(process.stdout, "write").mockImplementation(() => true);
    const error = Object.assign(new Error("forbidden"), { statusCode: 403 });
    const { deps, calls } = dependencies({ metadataError: error });
    await main(
      patchArgs(),
      { SYSTEM_COLLECTIONURI: "https://dev.azure.com/org/" },
      deps,
    );
    const fetches = calls.filter((call) => call.args[0] === "fetch");
    expect(fetches).toHaveLength(1);
    expect(fetches[0]!.args).toContain("--depth=200");
    expect(fetches[0]!.args).toContain(
      "+refs/heads/feature:refs/remotes/origin/ado-aw-prepare-source",
    );
    expect(fetches[0]!.args).toContain(
      "+refs/heads/main:refs/remotes/origin/main",
    );
  });

  it("target-worktree fetches only the target tip at depth one", async () => {
    vi.spyOn(process.stdout, "write").mockImplementation(() => true);
    const { deps, calls } = dependencies();
    await main(
      {
        mode: "target-worktree",
        repos: [{ dir: "/src", target: "develop" }],
        fallbackTarget: "main",
      },
      {},
      deps,
    );
    const fetches = calls.filter((call) => call.args[0] === "fetch");
    expect(fetches).toHaveLength(1);
    expect(fetches[0]!.args).toEqual([
      "fetch",
      "--no-tags",
      "--depth=1",
      "origin",
      "+refs/heads/develop:refs/remotes/origin/develop",
    ]);
  });

  it("does not send the ADO bearer to a non-Azure origin", async () => {
    vi.spyOn(process.stdout, "write").mockImplementation(() => true);
    const { deps, calls } = dependencies({
      remote: "https://github.com/example/repo.git",
    });
    await main(
      {
        mode: "target-worktree",
        repos: [{ dir: "/src", target: "main" }],
        fallbackTarget: "main",
      },
      { SYSTEM_ACCESSTOKEN: "secret-token" },
      deps,
    );
    const fetch = calls.find((call) => call.args[0] === "fetch");
    expect(fetch?.env).toEqual({});
  });

  it("isolates a checkout-directory failure and processes later repos", async () => {
    vi.spyOn(process.stdout, "write").mockImplementation(() => true);
    const visited: string[] = [];
    const { deps, calls } = dependencies({
      chdir: (dir) => {
        visited.push(dir);
        if (dir === "/broken") throw new Error("missing");
      },
    });
    await main(
      {
        mode: "target-worktree",
        repos: [
          { dir: "/broken", target: "main" },
          { dir: "/good", target: "main" },
        ],
        fallbackTarget: "main",
      },
      {},
      deps,
    );
    expect(visited).toEqual(["/broken", "/good"]);
    expect(calls.filter((call) => call.args[0] === "fetch")).toHaveLength(1);
  });

  it("uses BUILD_SOURCESDIRECTORY and BUILD_SOURCEBRANCH for legacy no-arg calls", async () => {
    vi.spyOn(process.stdout, "write").mockImplementation(() => true);
    const { deps, dirs } = dependencies({ remote: "https://github.com/org/repo" });
    await main(
      {
        mode: "patch-base",
        repos: [],
        fallbackTarget: "develop",
      },
      {
        BUILD_SOURCESDIRECTORY: "/agent/src",
        BUILD_SOURCEBRANCH: "refs/heads/feature",
      },
      deps,
    );
    expect(dirs).toEqual(["/agent/src"]);
  });
});
