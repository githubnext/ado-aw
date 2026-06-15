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

const { runGit, gitOk } = vi.hoisted(() => ({
  runGit: vi.fn(),
  gitOk: vi.fn(),
}));

vi.mock("../../shared/git.js", () => ({ runGit, gitOk }));

import { main, successFragment } from "../index.js";

function makeWorkspace() {
  const root = mkdtempSync(join(tmpdir(), "exec-context-repo-test-"));
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

describe("successFragment", () => {
  it("includes branch + sha + tag + commits-since-tag", () => {
    const out = successFragment({
      branch: "main",
      sha: "abc",
      lastReleaseTag: "v1.2.3",
      commitsSinceTag: 7,
    });
    expect(out).toContain("## Repo context");
    expect(out).toContain("`main`");
    expect(out).toContain("`abc`");
    expect(out).toContain("`v1.2.3`");
    expect(out).toContain("7 commit(s) since");
  });

  it("emits the no-tags variant when tag is empty", () => {
    const out = successFragment({
      branch: "main",
      sha: "abc",
      lastReleaseTag: "",
      commitsSinceTag: 0,
    });
    expect(out).toContain("No release tags found");
  });
});

describe("main", () => {
  let ws: ReturnType<typeof makeWorkspace>;
  beforeEach(() => {
    ws = makeWorkspace();
    runGit.mockReset();
    gitOk.mockReset();
    vi.spyOn(process.stdout, "write").mockImplementation(() => true);
    vi.spyOn(process.stderr, "write").mockImplementation(() => true);
  });
  afterEach(() => {
    vi.restoreAllMocks();
    ws.cleanup();
  });

  it("stages branch/sha/tag/commits-since when a release tag exists", () => {
    gitOk.mockImplementation((args: string[]) => {
      if (args.join(" ") === "describe --tags --abbrev=0") return "v1.0.0";
      return null;
    });
    runGit.mockImplementation((args: string[]) => {
      if (args[0] === "log") return { stdout: "abc Foo\nbcd Bar\n", stderr: "", status: 0 };
      return { stdout: "", stderr: "", status: 1 };
    });

    const env: NodeJS.ProcessEnv = {
      BUILD_SOURCESDIRECTORY: ws.sourcesDir,
      AW_AGENT_PROMPT_FILE: ws.promptPath,
      BUILD_SOURCEVERSION: "abc123",
      BUILD_SOURCEBRANCH: "refs/heads/main",
    };
    const rc = main(env);
    expect(rc).toBe(0);
    const dir = join(ws.sourcesDir, "aw-context", "repo");
    expect(readFileSync(join(dir, "branch"), "utf8")).toBe("main"); // refs/heads/ stripped
    expect(readFileSync(join(dir, "sha"), "utf8")).toBe("abc123");
    expect(readFileSync(join(dir, "last-release-tag"), "utf8")).toBe("v1.0.0");
    expect(readFileSync(join(dir, "commits-since-tag.txt"), "utf8")).toContain(
      "abc Foo",
    );
    expect(readFileSync(ws.promptPath, "utf8")).toContain("## Repo context");
    // conventions.json must NOT be present without opt-in.
    expect(existsSync(join(dir, "conventions.json"))).toBe(false);
  });

  it("handles no-tags repo gracefully", () => {
    gitOk.mockReturnValue(null);
    runGit.mockReturnValue({ stdout: "", stderr: "", status: 1 });

    const env: NodeJS.ProcessEnv = {
      BUILD_SOURCESDIRECTORY: ws.sourcesDir,
      AW_AGENT_PROMPT_FILE: ws.promptPath,
      BUILD_SOURCEVERSION: "abc",
      BUILD_SOURCEBRANCH: "main",
    };
    main(env);
    const dir = join(ws.sourcesDir, "aw-context", "repo");
    expect(readFileSync(join(dir, "last-release-tag"), "utf8")).toBe("");
    expect(readFileSync(join(dir, "commits-since-tag.txt"), "utf8")).toBe("");
    expect(readFileSync(ws.promptPath, "utf8")).toContain("No release tags");
  });

  it("probes conventions when AW_REPO_CONVENTIONS=true", () => {
    gitOk.mockReturnValue(null);
    // Pre-create a CONTRIBUTING.md in the workspace.
    writeFileSync(
      join(ws.sourcesDir, "CONTRIBUTING.md"),
      "# Contributing\n\nLine 2\nLine 3\n",
      "utf8",
    );

    const env: NodeJS.ProcessEnv = {
      BUILD_SOURCESDIRECTORY: ws.sourcesDir,
      AW_AGENT_PROMPT_FILE: ws.promptPath,
      BUILD_SOURCEVERSION: "abc",
      BUILD_SOURCEBRANCH: "main",
      AW_REPO_CONVENTIONS: "true",
    };
    main(env);
    const dir = join(ws.sourcesDir, "aw-context", "repo");
    const conventions = JSON.parse(
      readFileSync(join(dir, "conventions.json"), "utf8"),
    );
    expect(conventions["CONTRIBUTING.md"].present).toBe(true);
    expect(conventions["CONTRIBUTING.md"].head).toContain("# Contributing");
    expect(conventions["CODEOWNERS"].present).toBe(false);
  });
});
