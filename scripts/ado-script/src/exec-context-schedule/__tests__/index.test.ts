import { describe, it, expect, beforeEach, vi, afterEach } from "vitest";
import {
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
  existsSync,
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

vi.mock("../../shared/build.js", () => ({ listLastSuccessfulBuildOnBranch }));
vi.mock("../../shared/git.js", () => ({ runGit, gitOk, bearerEnv }));

import { failureFragment, main, successFragment, validateIdentifiers } from "../index.js";

function makeWorkspace() {
  const root = mkdtempSync(join(tmpdir(), "exec-context-schedule-test-"));
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

const validEnv = (overrides: NodeJS.ProcessEnv = {}): NodeJS.ProcessEnv => ({
  SYSTEM_TEAMPROJECT: "MyProject",
  SYSTEM_DEFINITIONID: "10",
  BUILD_BUILDID: "42",
  BUILD_SOURCEVERSION: SHA_A,
  BUILD_SOURCEBRANCH: "refs/heads/main",
  ...overrides,
});

describe("validateIdentifiers", () => {
  it("accepts well-formed env", () => {
    const r = validateIdentifiers(validEnv());
    expect(r.ok).toBe(true);
  });
  it("rejects non-hex source version", () => {
    const r = validateIdentifiers(validEnv({ BUILD_SOURCEVERSION: "abc" }));
    expect(r.ok).toBe(false);
  });
});

describe("successFragment", () => {
  it("interpolates current/previous SHAs + previous-run-time when present", () => {
    const out = successFragment({
      currentSha: SHA_A,
      previousSha: SHA_B,
      branchRef: "refs/heads/main",
      commitsCount: 3,
      changedFilesCount: 7,
      previousRunTime: "2024-01-15T09:00:00.000Z",
    });
    expect(out).toContain("## Schedule context");
    expect(out).toContain(SHA_A);
    expect(out).toContain(SHA_B);
    expect(out).toContain("2024-01-15T09:00:00.000Z");
    expect(out).toContain("3 new commit(s)");
  });
  it("omits the time clause when previous-run-time is undefined", () => {
    const out = successFragment({
      currentSha: SHA_A,
      previousSha: SHA_B,
      branchRef: "main",
      commitsCount: 0,
      changedFilesCount: 0,
      previousRunTime: undefined,
    });
    expect(out).not.toContain("at `2024-");
    expect(out).toContain("at SHA");
  });
});

describe("failureFragment", () => {
  it("contains reason and do-not-claim-empty instruction", () => {
    const out = failureFragment("no previous green run found");
    expect(out).toContain("Schedule context preparation failed.");
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
    bearerEnv.mockReturnValue({});
    vi.spyOn(process.stdout, "write").mockImplementation(() => true);
    vi.spyOn(process.stderr, "write").mockImplementation(() => true);
  });
  afterEach(() => {
    vi.restoreAllMocks();
    ws.cleanup();
  });

  it("happy path: stages all files and writes previous-run-time", async () => {
    listLastSuccessfulBuildOnBranch.mockResolvedValue({
      id: 41,
      sourceVersion: SHA_B,
      finishTime: new Date("2024-01-15T09:00:00.000Z"),
    });
    gitOk.mockImplementation((args: string[]) =>
      args[0] === "cat-file" ? "" : null,
    );
    runGit.mockImplementation((args: string[]) => {
      if (args[0] === "log") {
        return { stdout: "abc Add foo\n", stderr: "", status: 0 };
      }
      if (args[0] === "diff") {
        return { stdout: "A\tnew.txt\n", stderr: "", status: 0 };
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
    const dir = join(ws.sourcesDir, "aw-context", "schedule");
    expect(readFileSync(join(dir, "current-sha"), "utf8")).toBe(SHA_A);
    expect(readFileSync(join(dir, "previous-run-sha"), "utf8")).toBe(SHA_B);
    expect(readFileSync(join(dir, "previous-run-time"), "utf8")).toBe(
      "2024-01-15T09:00:00.000Z",
    );
    expect(readFileSync(ws.promptPath, "utf8")).toContain("## Schedule context");
    // Trust boundary: bearer must not leak into staged artefacts.
    for (const f of ["current-sha", "previous-run-sha", "previous-run-time", "commits.txt", "changed-files.txt"]) {
      expect(readFileSync(join(dir, f), "utf8")).not.toContain("bearer-xyz");
    }
  });

  it("no previous green run → failure fragment", async () => {
    listLastSuccessfulBuildOnBranch.mockResolvedValue(null);
    const env = validEnv({
      BUILD_SOURCESDIRECTORY: ws.sourcesDir,
      AW_AGENT_PROMPT_FILE: ws.promptPath,
    });
    await main(env);
    const dir = join(ws.sourcesDir, "aw-context", "schedule");
    expect(readFileSync(join(dir, "error.txt"), "utf8")).toMatch(
      /no previous successful build/,
    );
    expect(existsSync(join(dir, "current-sha"))).toBe(false);
  });
});
