/**
 * End-to-end smoke tests of bundled ado-script programs.
 *
 * The gate smoke test validates the existing gate.js bundle.
 * The import smoke test builds import.js and verifies it expands
 * a prompt fixture in place.
 */
import { spawnSync } from "node:child_process";
import { randomUUID } from "node:crypto";
import { copyFileSync, existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { fileURLToPath, pathToFileURL } from "node:url";
import { dirname, resolve } from "node:path";
import { describe, expect, it } from "vitest";

const __dirname = dirname(fileURLToPath(import.meta.url));
const workspaceDir = resolve(__dirname, "..");
const gateBundlePath = resolve(__dirname, "../gate.js");
const importBundlePath = resolve(__dirname, "../import.js");
const execContextPrBundlePath = resolve(__dirname, "../exec-context-pr.js");
const preparePrBaseBundlePath = resolve(__dirname, "../prepare-pr-base.js");
const gateFixturePath = resolve(
  __dirname,
  "fixtures/gate-spec-pr-title-match.json",
);
const importFixtureDir = resolve(__dirname, "fixtures/import");
const smokeScratchRoot = resolve(__dirname, ".smoke-scratch");

function runGate(extraEnv: Record<string, string>): {
  stdout: string;
  stderr: string;
  status: number | null;
} {
  const fixture = readFileSync(gateFixturePath, "utf8");
  const gateSpec = Buffer.from(fixture).toString("base64");
  const result = spawnSync(process.execPath, [gateBundlePath], {
    env: {
      PATH: process.env.PATH ?? "",
      GATE_SPEC: gateSpec,
      ADO_BUILD_REASON: "PullRequest",
      SYSTEM_ACCESSTOKEN: "dummy",
      ADO_COLLECTION_URI: "https://example.invalid/",
      ADO_PROJECT: "p",
      ADO_BUILD_ID: "1",
      ...extraEnv,
    },
    encoding: "utf8",
  });
  return {
    stdout: result.stdout ?? "",
    stderr: result.stderr ?? "",
    status: result.status,
  };
}

function npmCommand(): string {
  return process.platform === "win32" ? "npm.cmd" : "npm";
}

function withSmokeScratchDir(label: string, run: (dir: string) => void): void {
  const dir = resolve(smokeScratchRoot, `${label}-${randomUUID()}`);
  mkdirSync(dir, { recursive: true });

  try {
    run(dir);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

describe("gate.js smoke", () => {
  it("emits SHOULD_RUN=true when pr_title matches the glob", () => {
    const { stdout, status } = runGate({ ADO_PR_TITLE: "fooBar" });
    expect(stdout).toContain(
      "##vso[task.setvariable variable=SHOULD_RUN;isOutput=true]true",
    );
    expect(status).toBe(0);
  });

  it("emits SHOULD_RUN=false when pr_title does not match the glob", () => {
    const { stdout } = runGate({ ADO_PR_TITLE: "barBar" });
    expect(stdout).toContain(
      "##vso[task.setvariable variable=SHOULD_RUN;isOutput=true]false",
    );
  });
});

describe("import.js smoke", () => {
  it("builds the bundle and expands the prompt fixture in place", () => {
    const build = spawnSync(npmCommand(), ["run", "build:import"], {
      cwd: workspaceDir,
      env: { ...process.env },
      encoding: "utf8",
      shell: process.platform === "win32",
    });

    expect(build.status).toBe(0);
    expect(existsSync(importBundlePath)).toBe(true);

    withSmokeScratchDir("import", (dir) => {
      const target = resolve(dir, "prompt.md");
      copyFileSync(resolve(importFixtureDir, "prompt.md"), target);
      copyFileSync(resolve(importFixtureDir, "snippet.md"), resolve(dir, "snippet.md"));

      const result = spawnSync(process.execPath, [importBundlePath, target], {
        env: { ...process.env },
        encoding: "utf8",
      });

      expect(result.status).toBe(0);
      expect(result.stdout).toBe("");
      expect(result.stderr).toBe("");

      const expanded = readFileSync(target, "utf8").replace(/\r\n/g, "\n");
      expect(expanded).toContain("smoke snippet");
      expect(expanded).not.toContain("{{#runtime-import");
      expect(expanded).toMatch(/^before\n/);
      expect(expanded).toMatch(/after\n$/);
    });
  }, 20000);
});

function runGitInRepo(repoDir: string, args: string[]): void {
  const result = spawnSync("git", args, {
    cwd: repoDir,
    encoding: "utf8",
    env: {
      ...process.env,
      // Deterministic identity for the test commits.
      GIT_AUTHOR_NAME: "smoke",
      GIT_AUTHOR_EMAIL: "smoke@example.invalid",
      GIT_COMMITTER_NAME: "smoke",
      GIT_COMMITTER_EMAIL: "smoke@example.invalid",
    },
  });
  if (result.status !== 0) {
    throw new Error(`git ${args.join(" ")} failed: ${result.stderr}`);
  }
}

/**
 * Run `git` in a repo and return its exit status WITHOUT throwing — for the
 * prepare-pr-base smoke test, which deliberately asserts a command fails
 * (before the fix) and later succeeds (after it).
 */
function gitStatusInRepo(repoDir: string, args: string[]): number | null {
  return spawnSync("git", args, {
    cwd: repoDir,
    encoding: "utf8",
    env: {
      ...process.env,
      GIT_AUTHOR_NAME: "smoke",
      GIT_AUTHOR_EMAIL: "smoke@example.invalid",
      GIT_COMMITTER_NAME: "smoke",
      GIT_COMMITTER_EMAIL: "smoke@example.invalid",
    },
  }).status;
}

/**
 * Build a fake checkout that looks like ADO's synthetic-merge PR
 * checkout: a merge commit on top of two parent branches.
 */
function makeSyntheticMergeRepo(repoDir: string): { baseSha: string; headSha: string } {
  mkdirSync(repoDir, { recursive: true });
  runGitInRepo(repoDir, ["init", "-q", "-b", "main"]);
  runGitInRepo(repoDir, ["commit", "--allow-empty", "-q", "-m", "root"]);
  // Diverge a feature branch from main, then advance main once more.
  runGitInRepo(repoDir, ["branch", "feature"]);
  runGitInRepo(repoDir, ["commit", "--allow-empty", "-q", "-m", "main advance"]);
  runGitInRepo(repoDir, ["checkout", "-q", "feature"]);
  runGitInRepo(repoDir, ["commit", "--allow-empty", "-q", "-m", "feature commit"]);
  // Compose a synthetic merge commit (-s ours simulates ADO's merge-into-target shape).
  runGitInRepo(repoDir, ["checkout", "-q", "main"]);
  runGitInRepo(repoDir, ["merge", "-q", "--no-ff", "-m", "synthetic merge", "feature"]);
  const baseSha = spawnSync("git", ["rev-parse", "HEAD^1"], {
    cwd: repoDir,
    encoding: "utf8",
  }).stdout.trim();
  const headSha = spawnSync("git", ["rev-parse", "HEAD^2"], {
    cwd: repoDir,
    encoding: "utf8",
  }).stdout.trim();
  return { baseSha, headSha };
}

describe("exec-context-pr.js smoke", () => {
  it("builds the bundle when missing", () => {
    expect(existsSync(execContextPrBundlePath)).toBe(true);
  });

  it("stages base.sha + head.sha and appends a success prompt fragment (synthetic merge)", () => {
    withSmokeScratchDir("exec-context-pr-success", (dir) => {
      const repoDir = resolve(dir, "repo");
      makeSyntheticMergeRepo(repoDir);

      // Compute expected SHAs directly from the synthetic repo so the
      // bundle's output can be cross-checked against git's own answer.
      // This guards against silent SHA-transposition / wrong-ref bugs
      // (e.g. swapping `HEAD^1` and `HEAD^2`, or using `HEAD^1` as
      // BASE_SHA instead of the true merge-base).
      const expectedHead = spawnSync("git", ["rev-parse", "HEAD^2"], {
        cwd: repoDir,
        encoding: "utf8",
      }).stdout.trim();
      const expectedBase = spawnSync("git", ["merge-base", "HEAD^1", "HEAD^2"], {
        cwd: repoDir,
        encoding: "utf8",
      }).stdout.trim();

      const awContext = resolve(repoDir, "aw-context");
      const agentPromptDir = resolve(dir, "awf-tools");
      mkdirSync(agentPromptDir, { recursive: true });
      const agentPromptPath = resolve(agentPromptDir, "agent-prompt.md");
      // Pre-seed the agent prompt the same way base.yml's "Prepare
      // agent prompt" step would.
      writeFileSync(agentPromptPath, "agent body\n", "utf8");

      const result = spawnSync(process.execPath, [execContextPrBundlePath], {
        cwd: repoDir,
        env: {
          PATH: process.env.PATH ?? "",
          BUILD_SOURCESDIRECTORY: repoDir,
          AW_AGENT_PROMPT_FILE: agentPromptPath,
          SYSTEM_PULLREQUEST_PULLREQUESTID: "4242",
          SYSTEM_PULLREQUEST_TARGETBRANCH: "refs/heads/main",
          SYSTEM_PULLREQUEST_SOURCEBRANCH: "refs/heads/feature",
          SYSTEM_TEAMPROJECT: "SmokeProject",
          BUILD_REPOSITORY_NAME: "smoke-repo",
        },
        encoding: "utf8",
      });

      // Bundle either succeeds (exit 0 with SHAs staged) or fails
      // softly (exit 0 with error.txt) — both are exit 0. A hard exit
      // 1 means infra (mkdir) failure.
      expect(result.status).toBe(0);

      const baseShaPath = resolve(awContext, "pr/base.sha");
      const headShaPath = resolve(awContext, "pr/head.sha");
      const errorPath = resolve(awContext, "pr/error.txt");

      if (existsSync(errorPath)) {
        throw new Error(
          `unexpected failure: error.txt=${readFileSync(errorPath, "utf8")}, stderr=${result.stderr}`,
        );
      }

      expect(existsSync(baseShaPath)).toBe(true);
      expect(existsSync(headShaPath)).toBe(true);

      const baseSha = readFileSync(baseShaPath, "utf8").trim();
      const headSha = readFileSync(headShaPath, "utf8").trim();
      // SHAs are 40 hex chars.
      expect(baseSha).toMatch(/^[a-f0-9]{40}$/);
      expect(headSha).toMatch(/^[a-f0-9]{40}$/);
      // Base != head (synthetic merge places them on different commits).
      expect(baseSha).not.toBe(headSha);
      // Cross-check against git's own answer: head must be HEAD^2 (the
      // PR head, not the target tip) and base must be the merge-base
      // of HEAD^1 + HEAD^2 (the true common ancestor, not HEAD^1).
      expect(headSha).toBe(expectedHead);
      expect(baseSha).toBe(expectedBase);

      // The agent prompt was appended with the success fragment.
      const promptContent = readFileSync(agentPromptPath, "utf8");
      expect(promptContent).toContain("agent body");
      expect(promptContent).toContain("## PR context");
      expect(promptContent).toContain("This is PR #4242 in project 'SmokeProject' / repository 'smoke-repo'.");
      expect(promptContent).toContain("repo_get_pull_request_by_id(project='SmokeProject'");
    });
  }, 30000);

  it("writes error.txt when PR identifier validation fails", () => {
    withSmokeScratchDir("exec-context-pr-fail", (dir) => {
      const repoDir = resolve(dir, "repo");
      makeSyntheticMergeRepo(repoDir);
      const agentPromptDir = resolve(dir, "awf-tools");
      mkdirSync(agentPromptDir, { recursive: true });
      const agentPromptPath = resolve(agentPromptDir, "agent-prompt.md");
      writeFileSync(agentPromptPath, "agent body\n", "utf8");

      const result = spawnSync(process.execPath, [execContextPrBundlePath], {
        cwd: repoDir,
        env: {
          PATH: process.env.PATH ?? "",
          BUILD_SOURCESDIRECTORY: repoDir,
          AW_AGENT_PROMPT_FILE: agentPromptPath,
          // Invalid PR id triggers validation failure.
          SYSTEM_PULLREQUEST_PULLREQUESTID: "not-a-number",
          SYSTEM_PULLREQUEST_TARGETBRANCH: "refs/heads/main",
          SYSTEM_TEAMPROJECT: "SmokeProject",
          BUILD_REPOSITORY_NAME: "smoke-repo",
        },
        encoding: "utf8",
      });

      expect(result.status).toBe(0);
      const errorPath = resolve(repoDir, "aw-context/pr/error.txt");
      expect(existsSync(errorPath)).toBe(true);
      const reason = readFileSync(errorPath, "utf8");
      expect(reason).toContain("PR_ID='not-a-number'");
      // No SHAs staged on failure.
      expect(existsSync(resolve(repoDir, "aw-context/pr/base.sha"))).toBe(false);
      expect(existsSync(resolve(repoDir, "aw-context/pr/head.sha"))).toBe(false);
      // Failure prompt appended.
      const promptContent = readFileSync(agentPromptPath, "utf8");
      expect(promptContent).toContain("context preparation failed");
      expect(promptContent).toContain("PR_ID='not-a-number'");
    });
  }, 30000);
});

describe("prepare-pr-base.js smoke", () => {
  it("makes origin/<target> available so the executor's worktree add succeeds on a shallow clone (issue #1453)", () => {
    // Build the bundle in place (mirrors the import smoke test's self-build) so
    // the smoke test is self-contained regardless of the build:* chain order.
    const build = spawnSync(npmCommand(), ["run", "build:prepare-pr-base"], {
      cwd: workspaceDir,
      env: { ...process.env },
      encoding: "utf8",
      shell: process.platform === "win32",
    });
    expect(build.status).toBe(0);
    expect(existsSync(preparePrBaseBundlePath)).toBe(true);

    withSmokeScratchDir("prepare-pr-base", (dir) => {
      // The create-pull-request target branch is a NON-default branch, so a
      // shallow single-branch clone of the default branch never fetches it —
      // exactly the shallow-default ADO SafeOutputs-job checkout in #1453.
      const target = "develop";

      // 1. A "remote" repo: default branch `main`, plus `develop` one commit
      //    ahead of it. HEAD is left on main (the default branch).
      const originDir = resolve(dir, "origin");
      mkdirSync(originDir, { recursive: true });
      runGitInRepo(originDir, ["init", "-q", "-b", "main"]);
      writeFileSync(resolve(originDir, "README.md"), "root\n", "utf8");
      runGitInRepo(originDir, ["add", "-A"]);
      runGitInRepo(originDir, ["commit", "-q", "-m", "root"]);
      runGitInRepo(originDir, ["checkout", "-q", "-b", target]);
      writeFileSync(resolve(originDir, "develop.txt"), "develop\n", "utf8");
      runGitInRepo(originDir, ["add", "-A"]);
      runGitInRepo(originDir, ["commit", "-q", "-m", "develop commit"]);
      runGitInRepo(originDir, ["checkout", "-q", "main"]);

      // 2. SHALLOW single-branch clone of `main` only. A `file://` URL (not a
      //    bare path) forces the transport that honours `--depth`, so the clone
      //    carries neither `origin/develop` nor enough history to reach it.
      const checkout = resolve(dir, "checkout");
      const fileUrl = pathToFileURL(originDir).href;
      runGitInRepo(dir, [
        "clone",
        "--depth",
        "1",
        "--single-branch",
        "--branch",
        "main",
        fileUrl,
        checkout,
      ]);

      // 3. Precondition — reproduce the exact #1453 failure. Before the prepare
      //    step, the executor's `git worktree add <wt> origin/develop` fails
      //    because `origin/develop` is an invalid reference in this checkout.
      expect(
        gitStatusInRepo(checkout, ["rev-parse", "--verify", `origin/${target}`]),
      ).not.toBe(0);
      expect(
        gitStatusInRepo(checkout, [
          "worktree",
          "add",
          resolve(dir, "wt-before"),
          `origin/${target}`,
        ]),
      ).not.toBe(0);

      // 4. Run the REAL prepare-pr-base bundle — the same step the compiler now
      //    emits in the SafeOutputs job before `ado-aw execute` (#1453 fix). A
      //    local `file://` remote needs no auth, so SYSTEM_ACCESSTOKEN is empty.
      const prep = spawnSync(
        process.execPath,
        [preparePrBaseBundlePath, "--repo-dir", checkout, "--target-branch", target],
        { env: { ...process.env, SYSTEM_ACCESSTOKEN: "" }, encoding: "utf8" },
      );
      expect(prep.status).toBe(0);

      // 5. The fix's effect: `origin/develop` now resolves and `origin/HEAD`
      //    points at it (so mcp.rs's symbolic-ref default-branch probe also
      //    resolves the right base).
      expect(
        gitStatusInRepo(checkout, ["rev-parse", "--verify", `origin/${target}`]),
      ).toBe(0);
      const originHead = spawnSync(
        "git",
        ["symbolic-ref", "refs/remotes/origin/HEAD"],
        { cwd: checkout, encoding: "utf8" },
      ).stdout.trim();
      expect(originHead).toBe(`refs/remotes/origin/${target}`);

      // 6. The executor's exact worktree operation now SUCCEEDS, checked out at
      //    the target branch's tip.
      const wtAfter = resolve(dir, "wt-after");
      expect(
        gitStatusInRepo(checkout, ["worktree", "add", wtAfter, `origin/${target}`]),
      ).toBe(0);
      expect(existsSync(resolve(wtAfter, "develop.txt"))).toBe(true);
    });
  }, 60000);
});
