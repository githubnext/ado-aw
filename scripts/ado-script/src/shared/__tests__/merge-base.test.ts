import { describe, expect, it } from "vitest";

import type { GitResult } from "../git.js";
import {
  ensureExactMergeBaseFetched,
  ensureRefsForMergeBaseFetched,
  ensureTargetRefFetched,
  ensureTargetTipFetched,
  resolveMergeBase,
  type GitRunners,
} from "../merge-base.js";

// Real-shape SHAs (40-char lowercase hex) so the production
// SHA40 guard in resolveMergeBase accepts them. We keep the
// historical SHA_C/SHA_A/SHA_B/SHA_M naming via these aliases
// so the test bodies stay readable.
const SHA_C = "c".repeat(40);
const SHA_A = "a".repeat(40);
const SHA_B = "b".repeat(40);
// `m` isn't hex; use `d` (a valid hex digit) for the merge-base SHA.
const SHA_M = "d".repeat(40);

/** Build a `runGit` stub that matches arguments and returns canned results. */
function makeRunGit(handlers: Array<{ match: (args: string[]) => boolean; result: GitResult }>):
  GitRunners["runGit"] {
  return (args: string[]) => {
    for (const h of handlers) {
      if (h.match(args)) return h.result;
    }
    return { stdout: "", stderr: "no handler", status: 1 };
  };
}

function makeGitOk(handlers: Array<{ match: (args: string[]) => boolean; out: string | null }>):
  GitRunners["gitOk"] {
  return (args: string[]) => {
    for (const h of handlers) {
      if (h.match(args)) return h.out;
    }
    return null;
  };
}

describe("resolveMergeBase", () => {
  it("uses the synthetic-merge fast path when HEAD has 2+ parents", () => {
    const runGit = makeRunGit([
      {
        // rev-list --parents -n 1 HEAD returns 3 tokens (commit + 2 parents)
        match: (a) => a.join(" ") === "rev-list --parents -n 1 HEAD",
        result: { stdout: `${SHA_C} ${SHA_A} ${SHA_B}\n`, stderr: "", status: 0 },
      },
    ]);
    const gitOk = makeGitOk([
      { match: (a) => a.join(" ") === "rev-parse HEAD", out: SHA_C },
      { match: (a) => a.join(" ") === "rev-parse HEAD^1", out: SHA_A },
      { match: (a) => a.join(" ") === "rev-parse HEAD^2", out: SHA_B },
      { match: (a) => a.join(" ") === `merge-base ${SHA_A} ${SHA_B}`, out: SHA_M },
    ]);

    const result = resolveMergeBase("main", {}, { runGit, gitOk });
    expect(result.ok).toBe(true);
    if (result.ok) {
      expect(result.baseSha).toBe(SHA_M);
      expect(result.headSha).toBe(SHA_B);
    }
  });

  it("deepens target and source refs when synthetic-merge merge-base cannot initially resolve", () => {
    const depthArgsSeen: string[] = [];
    const refsSeen: string[] = [];
    const bearer = {
      GIT_CONFIG_COUNT: "1",
      GIT_CONFIG_KEY_0: "http.extraheader",
      GIT_CONFIG_VALUE_0: "Authorization: bearer test-token",
    };
    const runGit: GitRunners["runGit"] = (args, env) => {
      if (args.join(" ") === "rev-list --parents -n 1 HEAD") {
        return { stdout: `${SHA_C} ${SHA_A} ${SHA_B}\n`, stderr: "", status: 0 };
      }
      if (args[0] === "fetch") {
        expect(env).toEqual(bearer);
        depthArgsSeen.push(args.find((arg) => arg.startsWith("--depth=")) ?? "");
        refsSeen.push(...args.filter((arg) => arg.startsWith("+")));
        return { stdout: "", stderr: "", status: 0 };
      }
      return { stdout: "", stderr: "no handler", status: 1 };
    };
    let mergeBaseCalls = 0;
    const gitOk: GitRunners["gitOk"] = (args) => {
      if (args.join(" ") === "rev-parse HEAD") return SHA_C;
      if (args.join(" ") === "rev-parse HEAD^1") return SHA_A;
      if (args.join(" ") === "rev-parse HEAD^2") return SHA_B;
      if (args.join(" ") === `merge-base ${SHA_A} ${SHA_B}`) {
        mergeBaseCalls++;
        return mergeBaseCalls < 2 ? null : SHA_M;
      }
      return null;
    };

    const result = resolveMergeBase("main", bearer, { runGit, gitOk }, "feature/x");
    expect(result.ok).toBe(true);
    if (result.ok) {
      expect(result.baseSha).toBe(SHA_M);
      expect(result.headSha).toBe(SHA_B);
    }
    expect(depthArgsSeen).toEqual(["--depth=200"]);
    expect(refsSeen).toEqual([
      "+refs/heads/feature/x:refs/remotes/origin/ado-aw-prepare-source",
      "+refs/heads/main:refs/remotes/origin/main",
    ]);
  });

  it("fails closed when synthetic-merge merge-base cannot resolve after deepening", () => {
    let fetchCount = 0;
    const runGit: GitRunners["runGit"] = (args) => {
      if (args.join(" ") === "rev-list --parents -n 1 HEAD") {
        return { stdout: `${SHA_C} ${SHA_A} ${SHA_B}\n`, stderr: "", status: 0 };
      }
      if (args[0] === "fetch") {
        fetchCount++;
        return { stdout: "", stderr: "", status: 0 };
      }
      return { stdout: "", stderr: "no handler", status: 1 };
    };
    const gitOk = makeGitOk([
      { match: (a) => a.join(" ") === "rev-parse HEAD", out: SHA_C },
      { match: (a) => a.join(" ") === "rev-parse HEAD^1", out: SHA_A },
      { match: (a) => a.join(" ") === "rev-parse HEAD^2", out: SHA_B },
      { match: (a) => a.join(" ") === `merge-base ${SHA_A} ${SHA_B}`, out: null },
    ]);

    const result = resolveMergeBase("main", {}, { runGit, gitOk }, "feature/x");
    expect(result.ok).toBe(false);
    if (!result.ok) {
      expect(result.reason).toContain("Could not resolve base/head SHAs");
    }
    expect(fetchCount).toBe(3);
  });

  it("uses progressive deepening when HEAD has only 1 parent and stops on first resolution", () => {
    let fetchCount = 0;
    const runGit = makeRunGit([
      {
        match: (a) => a.join(" ") === "rev-list --parents -n 1 HEAD",
        result: { stdout: `${SHA_C} ${SHA_A}\n`, stderr: "", status: 0 }, // 2 tokens = 1 parent
      },
      {
        match: (a) => a[0] === "fetch",
        // All fetches succeed
        result: { stdout: "", stderr: "", status: 0 },
      },
    ]);
    // Custom runGit increments a counter on fetch
    const runGitTracking: GitRunners["runGit"] = (args) => {
      if (args[0] === "fetch") fetchCount++;
      return runGit(args);
    };
    const gitOk = makeGitOk([
      { match: (a) => a.join(" ") === "rev-parse HEAD", out: SHA_C },
      { match: (a) => a.join(" ") === "merge-base --all origin/main HEAD", out: SHA_M },
    ]);

    const result = resolveMergeBase("main", {}, { runGit: runGitTracking, gitOk });
    expect(result.ok).toBe(true);
    if (result.ok) {
      expect(result.baseSha).toBe(SHA_M);
      expect(result.headSha).toBe(SHA_C);
    }
    expect(fetchCount).toBe(1); // stopped on first successful resolution
  });

  it("retries with deeper fetches when earlier merge-base fails", () => {
    let mergeBaseCalls = 0;
    const runGit = makeRunGit([
      {
        match: (a) => a.join(" ") === "rev-list --parents -n 1 HEAD",
        result: { stdout: `${SHA_C} ${SHA_A}\n`, stderr: "", status: 0 },
      },
      {
        match: (a) => a[0] === "fetch",
        result: { stdout: "", stderr: "", status: 0 },
      },
    ]);
    const gitOk: GitRunners["gitOk"] = (args) => {
      if (args.join(" ") === "rev-parse HEAD") return SHA_C;
      if (args.join(" ") === "merge-base --all origin/main HEAD") {
        mergeBaseCalls++;
        // First two attempts fail; third succeeds
        return mergeBaseCalls < 3 ? null : SHA_M;
      }
      return null;
    };

    const result = resolveMergeBase("main", {}, { runGit, gitOk });
    expect(result.ok).toBe(true);
    if (result.ok) {
      expect(result.baseSha).toBe(SHA_M);
    }
    expect(mergeBaseCalls).toBe(3);
  });

  it("returns failure when no depth resolves the merge-base", () => {
    const runGit = makeRunGit([
      {
        match: (a) => a.join(" ") === "rev-list --parents -n 1 HEAD",
        result: { stdout: `${SHA_C} ${SHA_A}\n`, stderr: "", status: 0 },
      },
      {
        match: (a) => a[0] === "fetch",
        result: { stdout: "", stderr: "", status: 0 },
      },
    ]);
    const gitOk = makeGitOk([
      { match: (a) => a.join(" ") === "rev-parse HEAD", out: SHA_C },
      // No merge-base handler — always returns null
    ]);

    const result = resolveMergeBase("main", {}, { runGit, gitOk });
    expect(result.ok).toBe(false);
    if (!result.ok) {
      expect(result.reason).toContain("Could not resolve base/head SHAs");
      expect(result.reason).toContain("'main'");
      expect(result.reason).toContain(`HEAD=${SHA_C}`);
    }
  });

  it("skips bounded depths where fetch fails", () => {
    let fetchAttempts = 0;
    let mergeBaseAttempts = 0;
    const runGit: GitRunners["runGit"] = (args) => {
      if (args.join(" ") === "rev-list --parents -n 1 HEAD") {
        return { stdout: `${SHA_C} ${SHA_A}\n`, stderr: "", status: 0 };
      }
      if (args[0] === "fetch") {
        fetchAttempts++;
        // First two fetches fail, third succeeds
        return { stdout: "", stderr: "fail", status: fetchAttempts < 3 ? 128 : 0 };
      }
      return { stdout: "", stderr: "no handler", status: 1 };
    };
    const gitOk: GitRunners["gitOk"] = (args) => {
      if (args.join(" ") === "rev-parse HEAD") return SHA_C;
      if (args.join(" ") === "merge-base --all origin/main HEAD") {
        mergeBaseAttempts++;
        return SHA_M;
      }
      return null;
    };

    const result = resolveMergeBase("main", {}, { runGit, gitOk });
    expect(result.ok).toBe(true);
    expect(fetchAttempts).toBe(3); // tried 3 depths
    expect(mergeBaseAttempts).toBe(1); // only called once, on first success
  });

  it("passes bearer env to git fetch", () => {
    let observedEnv: Record<string, string> | undefined;
    const runGit: GitRunners["runGit"] = (args, env) => {
      if (args.join(" ") === "rev-list --parents -n 1 HEAD") {
        return { stdout: `${SHA_C} ${SHA_A}\n`, stderr: "", status: 0 };
      }
      if (args[0] === "fetch") {
        observedEnv = env;
        return { stdout: "", stderr: "", status: 0 };
      }
      return { stdout: "", stderr: "", status: 1 };
    };
    const gitOk = makeGitOk([
      { match: (a) => a.join(" ") === "rev-parse HEAD", out: SHA_C },
      { match: (a) => a.join(" ") === "merge-base --all origin/main HEAD", out: SHA_M },
    ]);

    const bearer = {
      GIT_CONFIG_COUNT: "1",
      GIT_CONFIG_KEY_0: "http.extraheader",
      GIT_CONFIG_VALUE_0: "Authorization: bearer test-token",
    };
    resolveMergeBase("main", bearer, { runGit, gitOk });

    expect(observedEnv).toEqual(bearer);
  });

  it("returns failure when resolved SHAs are not 40-char hex (defensive guard)", () => {
    // Simulate a misconfigured git (e.g. `core.abbrev = 7` or some
    // unusual hook) returning abbreviated output. resolveMergeBase
    // must NOT stage these — the agent's `git diff $BASE..$HEAD`
    // would then error out in-sandbox with a confusing message.
    const runGit = makeRunGit([
      {
        match: (a) => a.join(" ") === "rev-list --parents -n 1 HEAD",
        result: { stdout: `${SHA_C} ${SHA_A} ${SHA_B}\n`, stderr: "", status: 0 },
      },
    ]);
    const gitOk = makeGitOk([
      { match: (a) => a.join(" ") === "rev-parse HEAD", out: SHA_C },
      { match: (a) => a.join(" ") === "rev-parse HEAD^1", out: SHA_A },
      { match: (a) => a.join(" ") === "rev-parse HEAD^2", out: SHA_B },
      // merge-base returns an abbreviated 7-char SHA
      { match: (a) => a.join(" ") === `merge-base ${SHA_A} ${SHA_B}`, out: "abc1234" },
    ]);

    const result = resolveMergeBase("main", {}, { runGit, gitOk });
    expect(result.ok).toBe(false);
    if (!result.ok) {
      expect(result.reason).toContain("not 40-char hex");
      expect(result.reason).toContain("baseSha='abc1234'");
    }
  });

  it("returns failure when resolved SHA contains non-hex characters", () => {
    // Unlikely in practice but the guard must also reject e.g. a
    // multi-line / whitespace-laden return that slipped past the
    // outer non-empty check.
    const runGit = makeRunGit([
      {
        match: (a) => a.join(" ") === "rev-list --parents -n 1 HEAD",
        result: { stdout: `${SHA_C} ${SHA_A} ${SHA_B}\n`, stderr: "", status: 0 },
      },
    ]);
    const gitOk = makeGitOk([
      { match: (a) => a.join(" ") === "rev-parse HEAD", out: SHA_C },
      { match: (a) => a.join(" ") === "rev-parse HEAD^1", out: SHA_A },
      // rev-parse HEAD^2 returns a value of correct length but with
      // a non-hex character.
      { match: (a) => a.join(" ") === "rev-parse HEAD^2", out: "z".repeat(40) },
      { match: (a) => a.join(" ") === `merge-base ${SHA_A} z${"z".repeat(39)}`, out: SHA_M },
    ]);

    const result = resolveMergeBase("main", {}, { runGit, gitOk });
    expect(result.ok).toBe(false);
    if (!result.ok) {
      expect(result.reason).toContain("not 40-char hex");
    }
  });
});

describe("ensureTargetRefFetched", () => {
  it("resolves at the first depth and reports the merge base", () => {
    const depthArgsSeen: string[] = [];
    const runGit: GitRunners["runGit"] = (args) => {
      if (args[0] === "fetch") {
        depthArgsSeen.push(args.find((arg) => arg.startsWith("--depth=")) ?? "");
      }
      return { stdout: "", stderr: "", status: 0 };
    };
    const gitOk = makeGitOk([
      { match: (a) => a.join(" ") === "merge-base --all origin/main HEAD", out: SHA_M },
    ]);

    const result = ensureTargetRefFetched("main", {}, { runGit, gitOk });
    expect(result.ok).toBe(true);
    if (result.ok) expect(result.baseSha).toBe(SHA_M);
    // Stops after the first depth (does not keep deepening once resolved).
    expect(depthArgsSeen).toEqual(["--depth=200"]);
  });

  it("keeps deepening until merge-base resolves, then stops", () => {
    const depthArgsSeen: string[] = [];
    const runGit: GitRunners["runGit"] = (args) => {
      if (args[0] === "fetch") {
        depthArgsSeen.push(args.find((arg) => arg.startsWith("--depth=")) ?? "");
      }
      return { stdout: "", stderr: "", status: 0 };
    };
    let calls = 0;
    const gitOk: GitRunners["gitOk"] = (a) => {
      if (a.join(" ") === "merge-base --all origin/main HEAD") {
        calls++;
        // Resolves only on the 3rd depth (--depth=2000).
        return calls >= 3 ? SHA_M : null;
      }
      return null;
    };

    const result = ensureTargetRefFetched("main", {}, { runGit, gitOk });
    expect(result.ok).toBe(true);
    expect(depthArgsSeen).toEqual(["--depth=200", "--depth=500", "--depth=2000"]);
  });

  it("returns ok:false when no depth resolves the merge base", () => {
    const runGit: GitRunners["runGit"] = () => ({ stdout: "", stderr: "", status: 0 });
    const gitOk: GitRunners["gitOk"] = () => null;

    const result = ensureTargetRefFetched("main", {}, { runGit, gitOk });
    expect(result.ok).toBe(false);
    if (!result.ok) expect(result.reason).toContain("origin/main");
  });
});

describe("targeted shallow history helpers", () => {
  it("fetches exact source/target SHAs at count-derived depths and verifies the base", () => {
    const calls: string[][] = [];
    const runners: GitRunners = {
      runGit: (args) => {
        calls.push(args);
        return { stdout: "", stderr: "", status: 0 };
      },
      gitOk: (args) =>
        args.join(" ") === "merge-base --all origin/main HEAD" ? SHA_M : null,
    };
    const result = ensureExactMergeBaseFetched(
      "main",
      {
        commonCommit: SHA_M,
        aheadCount: 7,
        behindCount: 5,
        sourceCommit: SHA_A,
        targetCommit: SHA_B,
      },
      {},
      runners,
    );
    expect(result).toEqual({ ok: true, baseSha: SHA_M });
    expect(calls[0]).toContain("--depth=8");
    expect(calls[0]).toContain(
      `+${SHA_A}:refs/remotes/origin/ado-aw-prepare-source`,
    );
    expect(calls[1]).toContain("--depth=6");
    expect(calls[1]).toContain(`+${SHA_B}:refs/remotes/origin/main`);
  });

  it("rejects an API-directed fetch beyond the automatic safety limit", () => {
    let fetches = 0;
    const runners: GitRunners = {
      runGit: () => {
        fetches++;
        return { stdout: "", stderr: "", status: 0 };
      },
      gitOk: () => null,
    };
    const result = ensureExactMergeBaseFetched(
      "main",
      {
        commonCommit: SHA_M,
        aheadCount: 10_000,
        behindCount: 0,
        sourceCommit: SHA_A,
        targetCommit: SHA_B,
      },
      {},
      runners,
    );
    expect(result.ok).toBe(false);
    expect(fetches).toBe(0);
  });

  it("fetches both refs in one bounded command and never unshallows", () => {
    const calls: string[][] = [];
    const runners: GitRunners = {
      runGit: (args) => {
        calls.push(args);
        return { stdout: "", stderr: "", status: 0 };
      },
      gitOk: (args) =>
        args.join(" ") === "merge-base --all origin/main HEAD" ? SHA_M : null,
    };
    const result = ensureRefsForMergeBaseFetched(
      "refs/heads/feature",
      "main",
      {},
      runners,
    );
    expect(result.ok).toBe(true);
    expect(calls).toHaveLength(1);
    expect(calls[0]).toContain("--depth=200");
    expect(calls[0]).toContain(
      "+refs/heads/feature:refs/remotes/origin/ado-aw-prepare-source",
    );
    expect(calls[0]).toContain("+refs/heads/main:refs/remotes/origin/main");
    expect(calls.flat()).not.toContain("--unshallow");
  });

  it("fetches only the target tip for SafeOutputs", () => {
    const calls: string[][] = [];
    const runners: GitRunners = {
      runGit: (args) => {
        calls.push(args);
        return { stdout: "", stderr: "", status: 0 };
      },
      gitOk: (args) =>
        args.join(" ") === "rev-parse origin/main" ? SHA_B : null,
    };
    expect(ensureTargetTipFetched("main", {}, runners)).toEqual({
      ok: true,
      baseSha: SHA_B,
    });
    expect(calls).toEqual([
      [
        "fetch",
        "--no-tags",
        "--no-recurse-submodules",
        "--depth=1",
        "origin",
        "+refs/heads/main:refs/remotes/origin/main",
      ],
    ]);
  });

  it("preserves a full checkout by omitting depth-limited fetch arguments", () => {
    const calls: string[][] = [];
    const runners: GitRunners = {
      runGit: (args) => {
        calls.push(args);
        return { stdout: "", stderr: "", status: 0 };
      },
      gitOk: (args) => {
        if (args.join(" ") === "rev-parse --is-shallow-repository") return "false";
        if (args.join(" ") === "merge-base --all origin/main HEAD") return SHA_M;
        if (args.join(" ") === "rev-parse origin/main") return SHA_B;
        return null;
      },
    };
    expect(
      ensureExactMergeBaseFetched(
        "main",
        {
          commonCommit: SHA_M,
          aheadCount: 7,
          behindCount: 5,
          sourceCommit: SHA_A,
          targetCommit: SHA_B,
        },
        {},
        runners,
      ),
    ).toEqual({ ok: true, baseSha: SHA_M });
    expect(calls).toEqual([
      [
        "fetch",
        "--no-tags",
        "--no-recurse-submodules",
        "origin",
        `+${SHA_B}:refs/remotes/origin/main`,
      ],
    ]);

    calls.length = 0;
    expect(ensureTargetTipFetched("main", {}, runners).ok).toBe(true);
    expect(calls[0]).toEqual([
      "fetch",
      "--no-tags",
      "--no-recurse-submodules",
      "origin",
      "+refs/heads/main:refs/remotes/origin/main",
    ]);
  });

  it("accepts valid Git ref punctuation and Unicode without shell parsing", () => {
    const calls: string[][] = [];
    const runners: GitRunners = {
      runGit: (args) => {
        calls.push(args);
        return { stdout: "", stderr: "", status: 0 };
      },
      gitOk: (args) =>
        args.join(" ") === "merge-base --all origin/release+次 HEAD"
          ? SHA_M
          : null,
    };
    const result = ensureRefsForMergeBaseFetched(
      "refs/heads/team's+候補@x",
      "release+次",
      {},
      runners,
    );
    expect(result.ok).toBe(true);
    expect(calls[0]).toContain(
      "+refs/heads/team's+候補@x:refs/remotes/origin/ado-aw-prepare-source",
    );
    expect(calls[0]).toContain(
      "+refs/heads/release+次:refs/remotes/origin/release+次",
    );
  });

  it("never shortens an existing shallow history depth", () => {
    const calls: string[][] = [];
    const runners: GitRunners = {
      runGit: (args) => {
        calls.push(args);
        return { stdout: "", stderr: "", status: 0 };
      },
      gitOk: (args) => {
        if (args.join(" ") === "rev-list --count HEAD") return "500";
        if (args.join(" ") === "merge-base --all origin/main HEAD") return SHA_M;
        if (args.join(" ") === "rev-parse origin/main") return SHA_B;
        return null;
      },
    };
    expect(ensureTargetTipFetched("main", {}, runners).ok).toBe(true);
    expect(calls[0]).toContain("--depth=500");
    expect(calls[0]).toContain("--no-recurse-submodules");
  });
});
