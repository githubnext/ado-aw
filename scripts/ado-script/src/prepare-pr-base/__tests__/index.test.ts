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

/** A `chdir` stub that records the dirs it was asked to enter. */
function recordingChdir(): { chdir: (dir: string) => void; dirs: string[] } {
  const dirs: string[] = [];
  return { chdir: (dir: string) => void dirs.push(dir), dirs };
}

describe("parseArgs", () => {
  it("pairs each --repo-dir with the following --target-branch", () => {
    const args = parseArgs([
      "--repo-dir",
      "/src",
      "--target-branch",
      "main",
      "--repo-dir",
      "/src/tools",
      "--target-branch",
      "release",
    ]);
    expect(args.repos).toEqual([
      { dir: "/src", target: "main" },
      { dir: "/src/tools", target: "release" },
    ]);
  });

  it("strips a leading refs/heads/ from targets", () => {
    const args = parseArgs([
      "--repo-dir",
      "/src",
      "--target-branch",
      "refs/heads/release/2.x",
    ]);
    expect(args.repos).toEqual([{ dir: "/src", target: "release/2.x" }]);
  });

  it("uses the fallback target for a --repo-dir with no following --target-branch", () => {
    // A leading global --target-branch sets the fallback; a bare trailing
    // --repo-dir inherits it.
    const args = parseArgs(["--target-branch", "develop", "--repo-dir", "/src"]);
    expect(args.fallbackTarget).toBe("develop");
    expect(args.repos).toEqual([{ dir: "/src", target: "develop" }]);
  });

  it("defaults the fallback target to main", () => {
    expect(parseArgs([]).fallbackTarget).toBe("main");
    expect(parseArgs([]).repos).toEqual([]);
  });
});

describe("prepare-pr-base main", () => {
  it("fetches origin/<target> and sets origin/HEAD on success", () => {
    const { runners, calls } = makeRunners({ fetchStatus: 0, mergeBase: SHA_M });
    const { chdir, dirs } = recordingChdir();
    const rc = main(
      { repos: [{ dir: "/src", target: "main" }], fallbackTarget: "main" },
      {},
      runners,
      chdir,
    );
    expect(rc).toBe(0);
    expect(dirs).toEqual(["/src"]);

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

  it("deepens each repo dir with its OWN target branch (meta-repo)", () => {
    const { runners, calls } = makeRunners({ fetchStatus: 0, mergeBase: SHA_M });
    const { chdir, dirs } = recordingChdir();
    const rc = main(
      {
        repos: [
          { dir: "/src", target: "main" },
          { dir: "/src/tools", target: "release" },
          { dir: "/src/docs", target: "gh-pages" },
        ],
        fallbackTarget: "main",
      },
      {},
      runners,
      chdir,
    );
    expect(rc).toBe(0);
    expect(dirs).toEqual(["/src", "/src/tools", "/src/docs"]);

    // Each dir fetched + set origin/HEAD to ITS target ref.
    const fetchRefspecs = calls
      .filter((c) => c[0] === "fetch")
      .map((c) => c.find((a) => a.startsWith("+refs/heads/")));
    expect(fetchRefspecs).toEqual([
      "+refs/heads/main:refs/remotes/origin/main",
      "+refs/heads/release:refs/remotes/origin/release",
      "+refs/heads/gh-pages:refs/remotes/origin/gh-pages",
    ]);
    const symTargets = calls.filter((c) => c[0] === "symbolic-ref").map((c) => c[2]);
    expect(symTargets).toEqual([
      "refs/remotes/origin/main",
      "refs/remotes/origin/release",
      "refs/remotes/origin/gh-pages",
    ]);
  });

  it("isolates a per-dir chdir failure — other dirs still processed", () => {
    const { runners, calls } = makeRunners({ fetchStatus: 0, mergeBase: SHA_M });
    const dirs: string[] = [];
    const chdir = (dir: string) => {
      dirs.push(dir);
      if (dir === "/src/broken") {
        throw new Error("no such directory");
      }
    };
    const rc = main(
      {
        repos: [
          { dir: "/src", target: "main" },
          { dir: "/src/broken", target: "release" },
          { dir: "/src/lib", target: "main" },
        ],
        fallbackTarget: "main",
      },
      {},
      runners,
      chdir,
    );
    expect(rc).toBe(0);
    expect(dirs).toEqual(["/src", "/src/broken", "/src/lib"]);
    // Only the two good dirs fetched + set origin/HEAD (broken skipped).
    expect(calls.filter((c) => c[0] === "fetch").length).toBe(2);
    expect(calls.filter((c) => c[0] === "symbolic-ref").length).toBe(2);
  });

  it("exits 0 without setting origin/HEAD when every fetch fails (benign)", () => {
    const { runners, calls } = makeRunners({ fetchStatus: 1, mergeBase: null });
    const { chdir } = recordingChdir();
    const rc = main(
      { repos: [{ dir: "/src", target: "main" }], fallbackTarget: "main" },
      {},
      runners,
      chdir,
    );
    // Non-fatal: the agent still runs; mcp.rs surfaces its own error if needed.
    expect(rc).toBe(0);
    expect(calls.some((c) => c[0] === "symbolic-ref")).toBe(false);
  });

  it("falls back to BUILD_SOURCESDIRECTORY + fallback target when no repos given", () => {
    const { runners, calls } = makeRunners({ fetchStatus: 0, mergeBase: SHA_M });
    const { chdir, dirs } = recordingChdir();
    main(
      { repos: [], fallbackTarget: "develop" },
      { BUILD_SOURCESDIRECTORY: "/agent/src" },
      runners,
      chdir,
    );
    expect(dirs).toEqual(["/agent/src"]);
    const fetch = calls.find((c) => c[0] === "fetch");
    expect(fetch).toContain("+refs/heads/develop:refs/remotes/origin/develop");
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
    const { chdir } = recordingChdir();
    main(
      { repos: [{ dir: "/src", target: "main" }], fallbackTarget: "main" },
      { SYSTEM_ACCESSTOKEN: "tok" },
      runners,
      chdir,
    );
    expect(seenEnvs[0]).toMatchObject({
      GIT_CONFIG_VALUE_0: "Authorization: bearer tok",
    });
  });
});
