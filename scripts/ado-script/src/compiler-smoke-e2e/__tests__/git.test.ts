import { describe, expect, it } from "vitest";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import type { GitRunner, GitRunOptions } from "../git.js";
import {
  commitAll,
  commitMessage,
  COMMIT_IDENTITY,
  createDetachedWorktree,
  deleteRemoteRef,
  deleteRemoteRefs,
  defaultGitRunner,
  disallowedChanges,
  listCandidateRefs,
  mirrorRepoUrl,
  parseCandidateRef,
  parseBoundaryTargetRef,
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
    expect(commitMessage(42, "canary")).toBe("test(smoke): stage canary for candidate 42");
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

describe("parseCandidateRef", () => {
  it("parses the build id and case id from a well-formed candidate ref", () => {
    expect(parseCandidateRef("refs/heads/ado-aw-smoke-candidate/123/canary")).toEqual({ buildId: 123, caseId: "canary" });
  });

  it("returns undefined for a ref with the wrong prefix", () => {
    expect(parseCandidateRef("refs/heads/main")).toBeUndefined();
  });

  it("returns undefined for a non-numeric suffix", () => {
    expect(parseCandidateRef("refs/heads/ado-aw-smoke-candidate/abc")).toBeUndefined();
  });

  it("returns undefined for a zero or negative-looking suffix", () => {
    expect(parseCandidateRef("refs/heads/ado-aw-smoke-candidate/0")).toBeUndefined();
    expect(parseCandidateRef("refs/heads/ado-aw-smoke-candidate/-5")).toBeUndefined();
  });
});

describe("worktreeChangedFiles", () => {
  it("parses git status --porcelain=v1 output, one path per line", async () => {
    const { runner, calls } = fakeRunner(() => ({
      status: 0,
      stdout: " M tests/safe-outputs/canary.md\n?? tests/safe-outputs/canary.lock.yml\n",
    }));
    const files = await worktreeChangedFiles({ worktreeDir: "/wt", timeoutMs: 1000 }, runner);
    expect(files).toEqual([
      "tests/safe-outputs/canary.md",
      "tests/safe-outputs/canary.lock.yml",
    ]);
    expect(calls[0]?.args).toEqual([
      "status",
      "--porcelain=v1",
      "--untracked-files=all",
    ]);
  });

  it("receives untracked import-cache files individually instead of a collapsed directory", async () => {
    const { runner } = fakeRunner(() => ({
      status: 0,
      stdout: [
        "?? .ado-aw/imports/.gitattributes",
        "?? .ado-aw/imports/owner/repo/sha/component.md",
        "?? .ado-aw/imports/owner/repo/sha/component.md.sha256",
      ].join("\n"),
    }));
    await expect(
      worktreeChangedFiles({ worktreeDir: "/wt", timeoutMs: 1000 }, runner),
    ).resolves.toEqual([
      ".ado-aw/imports/.gitattributes",
      ".ado-aw/imports/owner/repo/sha/component.md",
      ".ado-aw/imports/owner/repo/sha/component.md.sha256",
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
    const sha = await commitAll({ worktreeDir: "/wt", buildId: 42, caseId: "canary", timeoutMs: 1000 }, runner);
    expect(sha).toBe("cafebabe");
    expect(calls[0]?.args).toEqual(["add", "-A"]);
    const commitCall = calls[1]?.args ?? [];
    expect(commitCall).toContain(`user.name=${COMMIT_IDENTITY.name}`);
    expect(commitCall).toContain(`user.email=${COMMIT_IDENTITY.email}`);
    expect(commitCall).toContain("test(smoke): stage canary for candidate 42");
  });
});

describe("pushCandidate / verifyRemoteRef / deleteRemoteRef", () => {
  it("preserves a genuinely advanced remote ref and deletes only its observed tip", async () => {
    const root = await mkdtemp(join(tmpdir(), "ado-smoke-lease-"));
    try {
      const work = join(root, "work");
      const remote = join(root, "remote.git");
      const runGit = async (args: string[], cwd = root) => {
        const result = await defaultGitRunner(args, { cwd, timeoutMs: 10_000 });
        expect(result.status, result.stderr).toBe(0);
        return result.stdout.trim();
      };
      await runGit(["init", "--bare", remote]);
      await runGit(["init", work]);
      const commit = async (message: string) => {
        await runGit(["-c", "user.name=Smoke Test", "-c", "user.email=smoke@example.test",
          "commit", "--allow-empty", "-m", message], work);
        return runGit(["rev-parse", "HEAD"], work);
      };
      const original = await commit("original");
      const ref = "refs/heads/ado-aw-smoke-candidate/42/check";
      await runGit(["push", remote, `HEAD:${ref}`], work);
      const advanced = await commit("advanced");
      await runGit(["push", remote, `HEAD:${ref}`], work);
      const opts = { cwd: work, mirrorUrl: remote, ref, token: "opaque-test-token", timeoutMs: 10_000 };
      await expect(deleteRemoteRef({ ...opts, sha: original })).rejects.toThrow("retained");
      expect(await runGit(["--git-dir", remote, "rev-parse", ref])).toBe(advanced);
      await deleteRemoteRef({ ...opts, sha: advanced });
      expect(await runGit(["ls-remote", "--heads", remote, ref], work)).toBe("");
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  }, 30_000);

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

  it("deleteRemoteRef deletes exactly the owned ref with its observed SHA lease", async () => {
    const { runner, calls } = fakeRunner(() => ({ status: 0 }));
    const ref = "refs/heads/ado-aw-smoke-candidate/1/canary";
    const sha = "a".repeat(40);
    await deleteRemoteRef(
      { cwd: "/repo", mirrorUrl: "https://example/_git/r", ref, sha, token: "t", timeoutMs: 1000 },
      runner,
    );
    expect(calls[0]?.args).toEqual([
      "push", "--porcelain", `--force-with-lease=${ref}:${sha}`, "https://example/_git/r", `:${ref}`,
    ]);
  });

  it("does not retry a failed lease and reports partial deletion from read-back", async () => {
    const source = { ref: "refs/heads/ado-aw-smoke-candidate/1/canary", sha: "a".repeat(40) };
    const target = { ref: "refs/heads/ado-aw-smoke-boundary-target/1/canary", sha: "b".repeat(40) };
    const { runner, calls } = fakeRunner((args) => args[0] === "push"
      ? { status: 1, stderr: "stale info" }
      : { status: 0, stdout: `${"c".repeat(40)}\t${source.ref}\n` });
    await expect(deleteRemoteRefs({
      cwd: "/repo", mirrorUrl: "https://example/_git/r", refs: [source, target], token: "opaque-test-token", timeoutMs: 1000,
    }, runner)).rejects.toThrow(`retained: ${source.ref}; confirmed absent: ${target.ref}`);
    expect(calls.filter((call) => call.args[0] === "push")).toHaveLength(1);
    expect(calls[0]?.args).toContain(`--force-with-lease=${source.ref}:${source.sha}`);
    expect(calls[0]?.args).toContain(`--force-with-lease=${target.ref}:${target.sha}`);
  });

  it("accepts confirmed absence after a lost deletion response without replay", async () => {
    const { runner, calls } = fakeRunner((args) => args[0] === "push"
      ? { status: 1, stderr: "connection lost" } : { status: 0, stdout: "" });
    await expect(deleteRemoteRefs({
      cwd: "/repo", mirrorUrl: "https://example/_git/r", token: "opaque-test-token", timeoutMs: 1000,
      refs: [{ ref: "refs/heads/ado-aw-smoke-candidate/1/canary", sha: "a".repeat(40) }],
    }, runner)).resolves.toBeUndefined();
    expect(calls.filter((call) => call.args[0] === "push")).toHaveLength(1);
  });

  it("requires an exact owned name and SHA before deletion", async () => {
    const { runner, calls } = fakeRunner(() => ({ status: 0 }));
    for (const ref of ["refs/heads/main", "refs/heads/ado-aw-smoke-candidate/0/canary"]) {
      await expect(deleteRemoteRef({
        cwd: "/repo", mirrorUrl: "https://example/_git/r", ref, sha: "a".repeat(40), token: "t", timeoutMs: 1000,
      }, runner)).rejects.toThrow("owned ref");
    }
    expect(calls).toEqual([]);
  });
});

describe("listCandidateRefs", () => {
  it("discovers the distinct target namespace without confusing -target case names", async () => {
    const source = "refs/heads/ado-aw-smoke-candidate/1/real-target";
    const target = "refs/heads/ado-aw-smoke-boundary-target/1/real-target";
    const { runner, calls } = fakeRunner(() => ({
      status: 0, stdout: `${"a".repeat(40)}\t${source}\n${"b".repeat(40)}\t${target}\n`,
    }));
    const refs = await listCandidateRefs({
      cwd: "/repo", mirrorUrl: "https://example/_git/r", token: "opaque-test-token", timeoutMs: 1000,
    }, runner);
    expect(refs.map((entry) => entry.ref)).toEqual([source, target]);
    expect(parseCandidateRef(source)).toEqual({ buildId: 1, caseId: "real-target" });
    expect(parseBoundaryTargetRef(source)).toBeUndefined();
    expect(parseBoundaryTargetRef(target)).toEqual({ buildId: 1, caseId: "real-target" });
    expect(parseCandidateRef(target)).toBeUndefined();
    expect(calls[0]?.args).toContain("refs/heads/ado-aw-smoke-boundary-target/**");
  });
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
