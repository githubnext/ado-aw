import { describe, expect, it } from "vitest";

import type { GitResult } from "../../shared/git.js";
import type { GitRunners } from "../../shared/merge-base.js";
import { main, parseArgs } from "../index.js";

const SHA_M = "d".repeat(40);

/**
 * Build a `GitRunners` pair that records every `runGit` invocation and lets a
 * matcher decide the result. `gitOk` answers `merge-base` queries.
 */
function makeRunners(opts: {
  fetchStatus: number;
  symbolicStatus?: number;
  mergeBase?: string | null;
}): { runners: GitRunners; calls: string[][] } {
  const calls: string[][] = [];
  const runGit: GitRunners["runGit"] = (args) => {
    calls.push(args);
    let result: GitResult = { stdout: "", stderr: "", status: 1 };
    if (args[0] === "fetch") {
      result = { stdout: "", stderr: "", status: opts.fetchStatus };
    } else if (args[0] === "symbolic-ref") {
      result = { stdout: "", stderr: "", status: opts.symbolicStatus ?? 0 };
    }
    return result;
  };
  const gitOk: GitRunners["gitOk"] = (args) => {
    if (args[0] === "merge-base") {
      return opts.mergeBase === undefined ? SHA_M : opts.mergeBase;
    }
    return null;
  };
  return { runners: { runGit, gitOk }, calls };
}

describe("parseArgs", () => {
  it("defaults to main when --target-branch is absent", () => {
    expect(parseArgs([]).targetBranch).toBe("main");
  });

  it("reads the --target-branch value", () => {
    expect(parseArgs(["--target-branch", "develop"]).targetBranch).toBe("develop");
  });

  it("strips a leading refs/heads/ prefix", () => {
    expect(parseArgs(["--target-branch", "refs/heads/release/2.x"]).targetBranch).toBe(
      "release/2.x",
    );
  });

  it("falls back to main for an empty value", () => {
    expect(parseArgs(["--target-branch", ""]).targetBranch).toBe("main");
  });
});

describe("prepare-pr-base main", () => {
  it("fetches origin/<target> and sets origin/HEAD on success", () => {
    const { runners, calls } = makeRunners({ fetchStatus: 0, mergeBase: SHA_M });
    // No BUILD_SOURCESDIRECTORY → skip chdir (test runs in the repo cwd).
    const rc = main({ targetBranch: "main" }, {}, runners);
    expect(rc).toBe(0);

    const fetch = calls.find((c) => c[0] === "fetch");
    expect(fetch).toBeDefined();
    expect(fetch).toContain("+refs/heads/main:refs/remotes/origin/main");

    const sym = calls.find((c) => c[0] === "symbolic-ref");
    expect(sym).toEqual([
      "symbolic-ref",
      "refs/remotes/origin/HEAD",
      "refs/remotes/origin/main",
    ]);
  });

  it("threads a non-default target through the fetch refspec and origin/HEAD", () => {
    const { runners, calls } = makeRunners({ fetchStatus: 0, mergeBase: SHA_M });
    const rc = main({ targetBranch: "release/2.x" }, {}, runners);
    expect(rc).toBe(0);

    const fetch = calls.find((c) => c[0] === "fetch");
    expect(fetch).toContain("+refs/heads/release/2.x:refs/remotes/origin/release/2.x");

    const sym = calls.find((c) => c[0] === "symbolic-ref");
    expect(sym).toEqual([
      "symbolic-ref",
      "refs/remotes/origin/HEAD",
      "refs/remotes/origin/release/2.x",
    ]);
  });

  it("exits 0 without setting origin/HEAD when every fetch fails (benign)", () => {
    const { runners, calls } = makeRunners({ fetchStatus: 1, mergeBase: null });
    const rc = main({ targetBranch: "main" }, {}, runners);
    // Non-fatal: the agent still runs; mcp.rs surfaces its own error if needed.
    expect(rc).toBe(0);
    expect(calls.some((c) => c[0] === "symbolic-ref")).toBe(false);
  });

  it("passes the SYSTEM_ACCESSTOKEN bearer into the git fetch env", () => {
    const seenEnvs: Array<Record<string, string> | undefined> = [];
    const runners: GitRunners = {
      runGit: (args, env) => {
        if (args[0] === "fetch") seenEnvs.push(env);
        return { stdout: "", stderr: "", status: 0 };
      },
      gitOk: (args) => (args[0] === "merge-base" ? SHA_M : null),
    };
    main({ targetBranch: "main" }, { SYSTEM_ACCESSTOKEN: "tok" }, runners);
    expect(seenEnvs[0]).toMatchObject({
      GIT_CONFIG_VALUE_0: "Authorization: bearer tok",
    });
  });
});
