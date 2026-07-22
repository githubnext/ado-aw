import { describe, expect, it } from "vitest";

import type { GitRunner, GitRunOptions } from "../git.js";
import {
  commitAll,
  commitMessage,
  COMMIT_IDENTITY,
  createDetachedWorktree,
  deleteRemoteRef,
  disallowedChanges,
  listCandidateRefs,
  mirrorRepoUrl,
  parseCandidateBuildId,
  pushCandidate,
  removeWorktree,
  verifyLocalCommit,
  verifyRemoteRef,
  worktreeChangedFiles,
} from "../git.js";

function fakeRunner(
  handler: (
    args: string[],
    opts: GitRunOptions,
  ) => { status: number | null; stdout?: string; stderr?: string; timedOut?: boolean },
): { runner: GitRunner; calls: { args: string[]; opts: GitRunOptions }[] } {
  const calls: { args: string[]; opts: GitRunOptions }[] = [];
  const runner: GitRunner = async (args, opts) => {
    calls.push({ args, opts });
    const r = handler(args, opts);
    return {
      status: r.status,
      stdout: r.stdout ?? "",
      stderr: r.stderr ?? "",
      timedOut: r.timedOut ?? false,
      stdoutTruncated: false,
      stderrTruncated: false,
    };
  };
  return { runner, calls };
}

describe("mirrorRepoUrl", () => {
  it("builds the ADO git remote URL, percent-encoding project/repo", () => {
    expect(mirrorRepoUrl("https://dev.azure.com/org/", "My Project", "ado-aw-mirror")).toBe(
      "https://dev.azure.com/org/My%20Project/_git/ado-aw-mirror",
    );
  });

  it("strips a trailing slash from orgUrl", () => {
    expect(mirrorRepoUrl("https://dev.azure.com/org", "P", "R")).toBe(
      "https://dev.azure.com/org/P/_git/R",
    );
  });
});

describe("commitMessage", () => {
  it("matches the required exact format", () => {
    expect(commitMessage(42)).toBe("test(smoke): stage compiler candidate 42");
  });
});

describe("COMMIT_IDENTITY", () => {
  it("is a deterministic, non-empty identity", () => {
    expect(COMMIT_IDENTITY.name).toBeTruthy();
    expect(COMMIT_IDENTITY.email).toBeTruthy();
  });
});

describe("disallowedChanges", () => {
  it("returns an empty array when every changed path is allowed", () => {
    const allowed = new Set(["a.md", "b.lock.yml"]);
    expect(disallowedChanges(["a.md", "b.lock.yml"], allowed)).toEqual([]);
  });

  it("returns exactly the unexpected paths", () => {
    const allowed = new Set(["a.md"]);
    expect(disallowedChanges(["a.md", "unexpected.txt"], allowed)).toEqual(["unexpected.txt"]);
  });

  it("is order-preserving", () => {
    const allowed = new Set<string>();
    expect(disallowedChanges(["z", "a", "m"], allowed)).toEqual(["z", "a", "m"]);
  });
});

describe("parseCandidateBuildId", () => {
  it("parses the numeric build id from a well-formed candidate ref", () => {
    expect(parseCandidateBuildId("refs/heads/ado-aw-smoke-candidate/123")).toBe(123);
  });

  it("returns undefined for a ref with the wrong prefix", () => {
    expect(parseCandidateBuildId("refs/heads/main")).toBeUndefined();
  });

  it("returns undefined for a non-numeric suffix", () => {
    expect(parseCandidateBuildId("refs/heads/ado-aw-smoke-candidate/abc")).toBeUndefined();
  });

  it("returns undefined for a zero or negative-looking suffix", () => {
    expect(parseCandidateBuildId("refs/heads/ado-aw-smoke-candidate/0")).toBeUndefined();
    expect(parseCandidateBuildId("refs/heads/ado-aw-smoke-candidate/-5")).toBeUndefined();
  });
});

describe("worktreeChangedFiles", () => {
  it("parses git status --porcelain=v1 output, one path per line", async () => {
    const { runner } = fakeRunner(() => ({
      status: 0,
      stdout: " M tests/safe-outputs/canary.md\n?? tests/safe-outputs/canary.lock.yml\n",
    }));
    const files = await worktreeChangedFiles({ worktreeDir: "/wt", timeoutMs: 1000 }, runner);
    expect(files).toEqual([
      "tests/safe-outputs/canary.md",
      "tests/safe-outputs/canary.lock.yml",
    ]);
  });

  it("expands a rename line into both the old and new path", async () => {
    const { runner } = fakeRunner(() => ({
      status: 0,
      stdout: "R  old/path.md -> new/path.md\n",
    }));
    const files = await worktreeChangedFiles({ worktreeDir: "/wt", timeoutMs: 1000 }, runner);
    expect(files).toEqual(["old/path.md", "new/path.md"]);
  });

  it("returns an empty array for a clean worktree", async () => {
    const { runner } = fakeRunner(() => ({ status: 0, stdout: "" }));
    const files = await worktreeChangedFiles({ worktreeDir: "/wt", timeoutMs: 1000 }, runner);
    expect(files).toEqual([]);
  });
});

describe("verifyLocalCommit", () => {
  it("resolves without fetching anything when HEAD already matches expectedSha", async () => {
    const { runner, calls } = fakeRunner((args) => {
      if (args[0] === "rev-parse" && args[1] === "HEAD") return { status: 0, stdout: "deadbeef\n" };
      throw new Error(`unexpected args: ${args.join(" ")}`);
    });
    await expect(
      verifyLocalCommit({ cwd: "/repo", expectedSha: "deadbeef", timeoutMs: 1000 }, runner),
    ).resolves.toBeUndefined();
    expect(calls.map((c) => c.args[0])).toEqual(["rev-parse"]);
    expect(calls.every((c) => c.args[0] !== "fetch")).toBe(true);
  });

  it("falls back to an object-existence check when HEAD differs (e.g. a PR synthetic merge commit)", async () => {
    const { runner, calls } = fakeRunner((args) => {
      if (args[0] === "rev-parse" && args[1] === "HEAD") return { status: 0, stdout: "mergecommit\n" };
      if (args[0] === "cat-file") return { status: 0 };
      throw new Error(`unexpected args: ${args.join(" ")}`);
    });
    await expect(
      verifyLocalCommit({ cwd: "/repo", expectedSha: "prheadsha", timeoutMs: 1000 }, runner),
    ).resolves.toBeUndefined();
    expect(calls.some((c) => c.args[0] === "fetch")).toBe(false);
    expect(calls.some((c) => c.args.join(" ") === "cat-file -e prheadsha^{commit}")).toBe(true);
  });

  it("throws (never fetches from the mirror) when the commit is not present locally at all", async () => {
    const { runner, calls } = fakeRunner((args) => {
      if (args[0] === "rev-parse" && args[1] === "HEAD") return { status: 0, stdout: "mergecommit\n" };
      if (args[0] === "cat-file") return { status: 1 };
      throw new Error(`unexpected args: ${args.join(" ")}`);
    });
    await expect(
      verifyLocalCommit({ cwd: "/repo", expectedSha: "missingsha", timeoutMs: 1000 }, runner),
    ).rejects.toThrow(/not found as a commit object/);
    expect(calls.some((c) => c.args[0] === "fetch")).toBe(false);
  });

  it("never attempts to resolve a GitHub PR ref like refs/pull/<n>/merge against the mirror", async () => {
    // This is the regression this function exists to prevent: a PR build's
    // BUILD_SOURCEBRANCH (refs/pull/123/merge) does not exist on the ADO
    // mirror repo at all. verifyLocalCommit never even accepts a `ref` or
    // `mirrorUrl` parameter, so there is no way for a caller to pass one in.
    const { runner, calls } = fakeRunner((args) => {
      if (args[0] === "rev-parse" && args[1] === "HEAD") return { status: 0, stdout: "prmergecommit\n" };
      return { status: 0 };
    });
    await verifyLocalCommit({ cwd: "/repo", expectedSha: "prmergecommit", timeoutMs: 1000 }, runner);
    expect(calls.every((c) => !c.args.includes("refs/pull/123/merge"))).toBe(true);
  });
});

describe("createDetachedWorktree / removeWorktree", () => {
  it("adds a detached worktree at the given commitish", async () => {
    const { runner, calls } = fakeRunner(() => ({ status: 0 }));
    await createDetachedWorktree({ cwd: "/repo", worktreeDir: "/tmp/wt", commitish: "deadbeef", timeoutMs: 1000 }, runner);
    expect(calls[0]?.args).toEqual(["worktree", "add", "--detach", "/tmp/wt", "deadbeef"]);
  });

  it("force-removes the worktree", async () => {
    const { runner, calls } = fakeRunner(() => ({ status: 0 }));
    await removeWorktree({ cwd: "/repo", worktreeDir: "/tmp/wt", timeoutMs: 1000 }, runner);
    expect(calls[0]?.args).toEqual(["worktree", "remove", "--force", "/tmp/wt"]);
  });
});

describe("commitAll", () => {
  it("stages everything, commits with the deterministic identity/message, returns the new sha", async () => {
    const { runner, calls } = fakeRunner((args) => {
      if (args[0] === "add") return { status: 0 };
      if (args[0] === "-c") return { status: 0 };
      if (args[0] === "rev-parse") return { status: 0, stdout: "cafebabe\n" };
      throw new Error(`unexpected args: ${args.join(" ")}`);
    });
    const sha = await commitAll({ worktreeDir: "/wt", buildId: 42, timeoutMs: 1000 }, runner);
    expect(sha).toBe("cafebabe");
    expect(calls[0]?.args).toEqual(["add", "-A"]);
    const commitCall = calls[1]?.args ?? [];
    expect(commitCall).toContain(`user.name=${COMMIT_IDENTITY.name}`);
    expect(commitCall).toContain(`user.email=${COMMIT_IDENTITY.email}`);
    expect(commitCall).toContain("test(smoke): stage compiler candidate 42");
  });
});

describe("pushCandidate / verifyRemoteRef / deleteRemoteRef", () => {
  it("pushes HEAD to the ref without --force", async () => {
    const { runner, calls } = fakeRunner(() => ({ status: 0 }));
    await pushCandidate(
      { worktreeDir: "/wt", mirrorUrl: "https://example/_git/r", ref: "refs/heads/x/1", token: "t", timeoutMs: 1000 },
      runner,
    );
    expect(calls[0]?.args).toEqual(["push", "--porcelain", "https://example/_git/r", "HEAD:refs/heads/x/1"]);
    expect(calls[0]?.args).not.toContain("--force");
    expect(calls[0]?.args).not.toContain("-f");
  });

  it("verifyRemoteRef succeeds when ls-remote returns the expected sha", async () => {
    const { runner } = fakeRunner(() => ({ status: 0, stdout: "deadbeef\trefs/heads/x/1\n" }));
    await expect(
      verifyRemoteRef(
        {
          cwd: "/wt",
          mirrorUrl: "https://example/_git/r",
          ref: "refs/heads/x/1",
          expectedSha: "deadbeef",
          token: "secret-token",
          timeoutMs: 1000,
        },
        runner,
      ),
    ).resolves.toBeUndefined();
  });

  it("verifyRemoteRef authenticates ls-remote with a bearer env (private mirror reads need auth too)", async () => {
    const { runner, calls } = fakeRunner(() => ({ status: 0, stdout: "deadbeef\trefs/heads/x/1\n" }));
    await verifyRemoteRef(
      {
        cwd: "/wt",
        mirrorUrl: "https://example/_git/r",
        ref: "refs/heads/x/1",
        expectedSha: "deadbeef",
        token: "secret-token",
        timeoutMs: 1000,
      },
      runner,
    );
    expect(calls[0]?.args).toEqual(["ls-remote", "https://example/_git/r", "refs/heads/x/1"]);
    expect(calls[0]?.opts.env?.GIT_CONFIG_VALUE_0).toContain("secret-token");
  });

  it("verifyRemoteRef redacts the token from a thrown failure message", async () => {
    const { runner } = fakeRunner(() => ({ status: 1, stderr: "fatal: auth failed for secret-token" }));
    await expect(
      verifyRemoteRef(
        {
          cwd: "/wt",
          mirrorUrl: "https://example/_git/r",
          ref: "refs/heads/x/1",
          expectedSha: "deadbeef",
          token: "secret-token",
          timeoutMs: 1000,
        },
        runner,
      ),
    ).rejects.toThrow(/\*\*\*/);
  });

  it("verifyRemoteRef throws on a sha mismatch", async () => {
    const { runner } = fakeRunner(() => ({ status: 0, stdout: "other-sha\trefs/heads/x/1\n" }));
    await expect(
      verifyRemoteRef(
        {
          cwd: "/wt",
          mirrorUrl: "https://example/_git/r",
          ref: "refs/heads/x/1",
          expectedSha: "deadbeef",
          token: "t",
          timeoutMs: 1000,
        },
        runner,
      ),
    ).rejects.toThrow(/verification failed/);
  });

  it("verifyRemoteRef throws when the ref is missing entirely", async () => {
    const { runner } = fakeRunner(() => ({ status: 0, stdout: "" }));
    await expect(
      verifyRemoteRef(
        {
          cwd: "/wt",
          mirrorUrl: "https://example/_git/r",
          ref: "refs/heads/x/1",
          expectedSha: "deadbeef",
          token: "t",
          timeoutMs: 1000,
        },
        runner,
      ),
    ).rejects.toThrow();
  });

  it("deleteRemoteRef pushes a --delete for exactly the given ref", async () => {
    const { runner, calls } = fakeRunner(() => ({ status: 0 }));
    await deleteRemoteRef(
      { cwd: "/repo", mirrorUrl: "https://example/_git/r", ref: "refs/heads/x/1", token: "t", timeoutMs: 1000 },
      runner,
    );
    expect(calls[0]?.args).toEqual(["push", "--porcelain", "https://example/_git/r", "--delete", "refs/heads/x/1"]);
  });
});

describe("listCandidateRefs", () => {
  it("lists only refs under the exact candidate prefix", async () => {
    const { runner } = fakeRunner(() => ({
      status: 0,
      stdout: [
        "aaa1\trefs/heads/ado-aw-smoke-candidate/1",
        "bbb2\trefs/heads/ado-aw-smoke-candidate/2",
      ].join("\n"),
    }));
    const refs = await listCandidateRefs(
      { cwd: "/repo", mirrorUrl: "https://example/_git/r", token: "secret-token", timeoutMs: 1000 },
      runner,
    );
    expect(refs).toEqual([
      { ref: "refs/heads/ado-aw-smoke-candidate/1", sha: "aaa1" },
      { ref: "refs/heads/ado-aw-smoke-candidate/2", sha: "bbb2" },
    ]);
  });

  it("authenticates ls-remote with a bearer env so a private mirror's stale-ref scan actually works", async () => {
    const { runner, calls } = fakeRunner(() => ({ status: 0, stdout: "" }));
    await listCandidateRefs(
      { cwd: "/repo", mirrorUrl: "https://example/_git/r", token: "secret-token", timeoutMs: 1000 },
      runner,
    );
    expect(calls[0]?.opts.env?.GIT_CONFIG_VALUE_0).toContain("secret-token");
  });

  it("redacts the token from a thrown failure message", async () => {
    const { runner } = fakeRunner(() => ({ status: 1, stderr: "fatal: auth failed for secret-token" }));
    await expect(
      listCandidateRefs(
        { cwd: "/repo", mirrorUrl: "https://example/_git/r", token: "secret-token", timeoutMs: 1000 },
        runner,
      ),
    ).rejects.toThrow(/\*\*\*/);
  });

  it("excludes any ref whose glob match is not an exact-prefix match", async () => {
    const { runner } = fakeRunner(() => ({
      status: 0,
      stdout: [
        "aaa1\trefs/heads/ado-aw-smoke-candidate/1",
        // A pathological server match that merely CONTAINS the pattern but
        // does not start with the exact prefix must never be treated as ours.
        "ccc3\trefs/heads/other/ado-aw-smoke-candidate/3",
      ].join("\n"),
    }));
    const refs = await listCandidateRefs(
      { cwd: "/repo", mirrorUrl: "https://example/_git/r", token: "auth-token", timeoutMs: 1000 },
      runner,
    );
    expect(refs).toEqual([{ ref: "refs/heads/ado-aw-smoke-candidate/1", sha: "aaa1" }]);
  });

  it("returns an empty array when there are no candidate refs", async () => {
    const { runner } = fakeRunner(() => ({ status: 0, stdout: "" }));
    const refs = await listCandidateRefs(
      { cwd: "/repo", mirrorUrl: "https://example/_git/r", token: "auth-token", timeoutMs: 1000 },
      runner,
    );
    expect(refs).toEqual([]);
  });
});
