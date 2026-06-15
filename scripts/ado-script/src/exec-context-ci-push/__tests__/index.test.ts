/**
 * Tests for the exec-context-ci-push bundle.
 *
 * Mocks the shared/build.ts REST helper and the shared/git.ts
 * git invocations so the bundle can be exercised end-to-end
 * without an ADO connection or a real git workspace.
 */
import { describe, it, expect, beforeEach, vi, afterEach } from "vitest";
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const { listLastSuccessfulBuildOnBranch } = vi.hoisted(() => ({
  listLastSuccessfulBuildOnBranch: vi.fn(),
}));
const { runGit, gitOk, bearerEnv } = vi.hoisted(() => ({
  runGit: vi.fn(),
  gitOk: vi.fn(),
  bearerEnv: vi.fn(),
}));

vi.mock("../../shared/build.js", () => ({
  listLastSuccessfulBuildOnBranch,
}));
vi.mock("../../shared/git.js", () => ({
  runGit,
  gitOk,
  bearerEnv,
}));

import {
  failureFragment,
  main,
  successFragment,
  validateIdentifiers,
} from "../index.js";

function makeWorkspace(): {
  sourcesDir: string;
  promptPath: string;
  cleanup: () => void;
} {
  const root = mkdtempSync(join(tmpdir(), "exec-context-ci-push-test-"));
  const sourcesDir = join(root, "sources");
  mkdirSync(sourcesDir, { recursive: true });
  const promptPath = join(root, "agent-prompt.md");
  writeFileSync(promptPath, "# Agent prompt\n", "utf8");
  return {
    sourcesDir,
    promptPath,
    cleanup: () => rmSync(root, { recursive: true, force: true }),
  };
}

const SHA_A = "a".repeat(40);
const SHA_B = "b".repeat(40);
const SHA_C = "c".repeat(40);

const validEnv = (overrides: NodeJS.ProcessEnv = {}): NodeJS.ProcessEnv => ({
  SYSTEM_TEAMPROJECT: "MyProject",
  SYSTEM_DEFINITIONID: "10",
  BUILD_BUILDID: "42",
  BUILD_SOURCEVERSION: SHA_A,
  BUILD_SOURCEBRANCH: "refs/heads/main",
  ...overrides,
});

describe("validateIdentifiers", () => {
  it("accepts a well-formed env block", () => {
    const r = validateIdentifiers(validEnv());
    expect(r.ok).toBe(true);
    if (r.ok) {
      expect(r.project).toBe("MyProject");
      expect(r.definitionId).toBe(10);
      expect(r.currentSha).toBe(SHA_A);
    }
  });

  for (const [overridesDesc, overrides, reasonRegex] of [
    ["missing project", { SYSTEM_TEAMPROJECT: "" }, /SYSTEM_TEAMPROJECT/],
    [
      "non-numeric definition id",
      { SYSTEM_DEFINITIONID: "evil; rm -rf /" },
      /SYSTEM_DEFINITIONID/,
    ],
    [
      "non-hex source version",
      { BUILD_SOURCEVERSION: "abc" },
      /BUILD_SOURCEVERSION/,
    ],
    ["empty branch", { BUILD_SOURCEBRANCH: "" }, /BUILD_SOURCEBRANCH/],
  ] as const) {
    it(`rejects ${overridesDesc}`, () => {
      const r = validateIdentifiers(validEnv(overrides));
      expect(r.ok).toBe(false);
      if (!r.ok) expect(r.reason).toMatch(reasonRegex);
    });
  }
});

describe("successFragment", () => {
  it("includes current, previous, base SHAs and counts", () => {
    const out = successFragment({
      currentSha: SHA_A,
      previousSha: SHA_B,
      baseSha: SHA_C,
      branchRef: "refs/heads/main",
      commitsCount: 5,
      changedFilesCount: 12,
    });
    expect(out).toContain("## CI-push context");
    expect(out).toContain(SHA_A);
    expect(out).toContain(SHA_B);
    expect(out).toContain(SHA_C);
    expect(out).toContain("`refs/heads/main`");
    expect(out).toContain("5 new commit(s)");
    expect(out).toContain("12 change(s)");
  });

  it("sanitises a hostile branch ref", () => {
    const out = successFragment({
      currentSha: SHA_A,
      previousSha: SHA_B,
      baseSha: SHA_C,
      branchRef: "evil\n## Injected\n",
      commitsCount: 0,
      changedFilesCount: 0,
    });
    expect(out).not.toContain("\n## Injected\n");
  });
});

describe("failureFragment", () => {
  it("contains the reason and a do-not-claim-empty instruction", () => {
    const out = failureFragment("no previous successful build found");
    expect(out).toContain("CI-push context preparation failed.");
    expect(out).toContain("no previous successful build found");
    expect(out).toContain("Do NOT claim the diff is empty");
  });
});

describe("main", () => {
  let ws: ReturnType<typeof makeWorkspace>;

  beforeEach(() => {
    ws = makeWorkspace();
    listLastSuccessfulBuildOnBranch.mockReset();
    runGit.mockReset();
    gitOk.mockReset();
    bearerEnv.mockReset();
    bearerEnv.mockReturnValue({ GIT_CONFIG_COUNT: "0" });
    vi.spyOn(process.stdout, "write").mockImplementation(() => true);
    vi.spyOn(process.stderr, "write").mockImplementation(() => true);
  });
  afterEach(() => {
    vi.restoreAllMocks();
    ws.cleanup();
  });

  it("stages all 5 files and appends success fragment on the happy path", async () => {
    listLastSuccessfulBuildOnBranch.mockResolvedValue({
      id: 41,
      sourceVersion: SHA_B,
    });
    // cat-file -e SHA_B already reachable; same for SHA_A.
    gitOk.mockImplementation((args: string[]) => {
      if (args[0] === "cat-file") return ""; // truthy → reachable
      if (args[0] === "merge-base") return SHA_C;
      return null;
    });
    runGit.mockImplementation((args: string[]) => {
      if (args[0] === "log") {
        return {
          stdout: "abc Add foo\nbcd Add bar\n",
          stderr: "",
          status: 0,
        };
      }
      if (args[0] === "diff") {
        return {
          stdout: "A\tnew.txt\nM\texisting.txt\n",
          stderr: "",
          status: 0,
        };
      }
      return { stdout: "", stderr: "", status: 1 };
    });

    const env = validEnv({
      BUILD_SOURCESDIRECTORY: ws.sourcesDir,
      AW_AGENT_PROMPT_FILE: ws.promptPath,
      SYSTEM_ACCESSTOKEN: "bearer-xyz",
    });
    const rc = await main(env);
    expect(rc).toBe(0);

    const dir = join(ws.sourcesDir, "aw-context", "ci-push");
    expect(readFileSync(join(dir, "current-sha"), "utf8")).toBe(SHA_A);
    expect(readFileSync(join(dir, "previous-sha"), "utf8")).toBe(SHA_B);
    expect(readFileSync(join(dir, "base.sha"), "utf8")).toBe(SHA_C);
    expect(readFileSync(join(dir, "commits.txt"), "utf8")).toContain("abc Add foo");
    expect(readFileSync(join(dir, "changed-files.txt"), "utf8")).toContain(
      "A\tnew.txt",
    );

    const prompt = readFileSync(ws.promptPath, "utf8");
    expect(prompt).toContain("## CI-push context");
    expect(prompt).toContain(SHA_A);

    // Trust boundary: bearer MUST NOT appear in any staged artefact
    // or the prompt fragment.
    for (const f of [
      "current-sha",
      "previous-sha",
      "base.sha",
      "commits.txt",
      "changed-files.txt",
    ]) {
      expect(readFileSync(join(dir, f), "utf8")).not.toContain("bearer-xyz");
    }
    expect(prompt).not.toContain("bearer-xyz");
  });

  it("writes failure fragment when no previous green build exists", async () => {
    listLastSuccessfulBuildOnBranch.mockResolvedValue(null);

    const env = validEnv({
      BUILD_SOURCESDIRECTORY: ws.sourcesDir,
      AW_AGENT_PROMPT_FILE: ws.promptPath,
    });
    const rc = await main(env);
    expect(rc).toBe(0);
    const dir = join(ws.sourcesDir, "aw-context", "ci-push");
    expect(readFileSync(join(dir, "error.txt"), "utf8")).toMatch(
      /no previous successful build/,
    );
    expect(existsSync(join(dir, "current-sha"))).toBe(false);
    expect(readFileSync(ws.promptPath, "utf8")).toContain(
      "CI-push context preparation failed.",
    );
  });

  it("writes failure fragment when the previous SHA cannot be fetched (depth exhausted)", async () => {
    listLastSuccessfulBuildOnBranch.mockResolvedValue({
      id: 41,
      sourceVersion: SHA_B,
    });
    // cat-file -e always returns null → SHA never reachable.
    gitOk.mockReturnValue(null);
    runGit.mockReturnValue({ stdout: "", stderr: "fetch failed", status: 1 });

    const env = validEnv({
      BUILD_SOURCESDIRECTORY: ws.sourcesDir,
      AW_AGENT_PROMPT_FILE: ws.promptPath,
    });
    const rc = await main(env);
    expect(rc).toBe(0);
    const dir = join(ws.sourcesDir, "aw-context", "ci-push");
    expect(readFileSync(join(dir, "error.txt"), "utf8")).toMatch(
      /depth-budget exhausted/,
    );
  });

  it("writes failure fragment when the REST call fails", async () => {
    listLastSuccessfulBuildOnBranch.mockRejectedValue(new Error("503"));
    const env = validEnv({
      BUILD_SOURCESDIRECTORY: ws.sourcesDir,
      AW_AGENT_PROMPT_FILE: ws.promptPath,
    });
    const rc = await main(env);
    expect(rc).toBe(0);
    const dir = join(ws.sourcesDir, "aw-context", "ci-push");
    expect(readFileSync(join(dir, "error.txt"), "utf8")).toMatch(
      /failed to query last successful build/,
    );
  });

  it("writes failure fragment when identifier validation fails", async () => {
    const env = validEnv({
      BUILD_SOURCESDIRECTORY: ws.sourcesDir,
      AW_AGENT_PROMPT_FILE: ws.promptPath,
      BUILD_SOURCEVERSION: "not-a-sha",
    });
    const rc = await main(env);
    expect(rc).toBe(0);
    const dir = join(ws.sourcesDir, "aw-context", "ci-push");
    expect(readFileSync(join(dir, "error.txt"), "utf8")).toMatch(
      /BUILD_SOURCEVERSION/,
    );
    expect(listLastSuccessfulBuildOnBranch).not.toHaveBeenCalled();
  });
});
