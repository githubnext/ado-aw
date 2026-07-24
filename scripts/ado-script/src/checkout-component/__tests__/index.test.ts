import { describe, expect, it } from "vitest";

import type { GitResult } from "../../shared/git.js";
import { main, parseArgs, type GitRunners } from "../index.js";

const SHA = "a".repeat(40);
const OTHER = "b".repeat(40);

/**
 * Build a scriptable `GitRunners` pair. `present` decides, per invocation
 * count, whether `git cat-file -e <sha>^{commit}` reports the object present
 * (status 0) — so a test can model "absent until the Nth fetch". `checkout`
 * and `head` control the detach + rev-parse outcomes.
 */
function makeRunners(opts: {
  /** Return codes for successive `cat-file -e` probes (default: always 1/absent). */
  catFile?: number[];
  fetchStatus?: number;
  checkoutStatus?: number;
  head?: string;
}): { runners: GitRunners; calls: string[][] } {
  const calls: string[][] = [];
  let catIdx = 0;
  const runGit: GitRunners["runGit"] = (args) => {
    calls.push(args);
    let result: GitResult = { stdout: "", stderr: "", status: 0 };
    if (args[0] === "cat-file") {
      const seq = opts.catFile ?? [1];
      const status = (catIdx < seq.length ? seq[catIdx] : seq[seq.length - 1]) ?? 1;
      catIdx++;
      result = { stdout: "", stderr: "", status };
    } else if (args[0] === "fetch") {
      result = { stdout: "", stderr: "", status: opts.fetchStatus ?? 0 };
    } else if (args[0] === "checkout") {
      result = { stdout: "", stderr: "", status: opts.checkoutStatus ?? 0 };
    }
    return result;
  };
  const gitOk: GitRunners["gitOk"] = (args) => {
    if (args[0] === "rev-parse") return opts.head ?? SHA;
    return null;
  };
  return { runners: { runGit, gitOk }, calls };
}

describe("parseArgs", () => {
  it("parses --dir and --sha", () => {
    expect(parseArgs(["--dir", "/src/comp", "--sha", SHA])).toEqual({
      dir: "/src/comp",
      sha: SHA,
    });
  });

  it("defaults to empty strings when flags are absent", () => {
    expect(parseArgs([])).toEqual({ dir: "", sha: "" });
  });
});

describe("main", () => {
  const okEnv = { SYSTEM_ACCESSTOKEN: "tok" } as NodeJS.ProcessEnv;
  const noopChdir = () => {};

  it("rejects a non-40-char sha (fail closed)", () => {
    const { runners, calls } = makeRunners({});
    expect(main({ dir: "/c", sha: "main" }, okEnv, runners, noopChdir)).toBe(1);
    // Must not touch git for an invalid pin.
    expect(calls).toEqual([]);
  });

  it("requires --dir", () => {
    const { runners } = makeRunners({});
    expect(main({ dir: "", sha: SHA }, okEnv, runners, noopChdir)).toBe(1);
  });

  it("fails closed when the component dir cannot be entered", () => {
    const { runners } = makeRunners({});
    const throwingChdir = () => {
      throw new Error("no such dir");
    };
    expect(main({ dir: "/missing", sha: SHA }, okEnv, runners, throwingChdir)).toBe(1);
  });

  it("checks out and verifies when the sha is already present (no fetch)", () => {
    const { runners, calls } = makeRunners({ catFile: [0], head: SHA });
    expect(main({ dir: "/c", sha: SHA }, okEnv, runners, noopChdir)).toBe(0);
    expect(calls.some((c) => c[0] === "fetch")).toBe(false);
    expect(calls).toContainEqual(["checkout", "--detach", SHA]);
  });

  it("does a direct by-sha fetch when the sha is initially absent", () => {
    // absent, then present after the direct fetch.
    const { runners, calls } = makeRunners({ catFile: [1, 0], head: SHA });
    expect(main({ dir: "/c", sha: SHA }, okEnv, runners, noopChdir)).toBe(0);
    expect(calls).toContainEqual(["fetch", "--no-tags", "--depth", "1", "origin", SHA]);
  });

  it("falls back to progressive deepening when by-sha fetch does not yield the object", () => {
    // absent, absent after direct fetch, present after first deepen.
    const { runners, calls } = makeRunners({ catFile: [1, 1, 0], head: SHA });
    expect(main({ dir: "/c", sha: SHA }, okEnv, runners, noopChdir)).toBe(0);
    expect(calls).toContainEqual(["fetch", "--no-tags", "--depth=200", "origin"]);
  });

  it("fails closed when the sha can never be obtained", () => {
    const { runners, calls } = makeRunners({ catFile: [1] }); // always absent
    expect(main({ dir: "/c", sha: SHA }, okEnv, runners, noopChdir)).toBe(1);
    // Never attempts the checkout of an unavailable object.
    expect(calls.some((c) => c[0] === "checkout")).toBe(false);
  });

  it("fails closed when checkout fails", () => {
    const { runners } = makeRunners({ catFile: [0], checkoutStatus: 1 });
    expect(main({ dir: "/c", sha: SHA }, okEnv, runners, noopChdir)).toBe(1);
  });

  it("fails closed when HEAD does not equal the pin after checkout", () => {
    const { runners } = makeRunners({ catFile: [0], head: OTHER });
    expect(main({ dir: "/c", sha: SHA }, okEnv, runners, noopChdir)).toBe(1);
  });

  it("passes the bearer env to git fetch (never on argv)", () => {
    const seen: Array<Record<string, string> | undefined> = [];
    const runGit: GitRunners["runGit"] = (args, env) => {
      if (args[0] === "fetch") seen.push(env);
      // absent, then present after direct fetch.
      const status = args[0] === "cat-file" ? (seen.length === 0 ? 1 : 0) : 0;
      return { stdout: "", stderr: "", status };
    };
    const gitOk: GitRunners["gitOk"] = () => SHA;
    main({ dir: "/c", sha: SHA }, okEnv, { runGit, gitOk }, noopChdir);
    expect(seen.length).toBeGreaterThan(0);
    // Bearer is delivered via GIT_CONFIG_* env, and the token is never in argv.
    expect(seen[0]?.GIT_CONFIG_VALUE_0).toContain("bearer tok");
  });
});
