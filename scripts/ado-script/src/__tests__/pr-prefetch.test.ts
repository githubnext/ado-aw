import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { execFileSync, spawnSync } from "node:child_process";
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { parse } from "yaml";

const root = resolve(process.cwd(), "..", "..");
let directory: string;
let base: string;
let head: string;
let bash: string;

function git(cwd: string, args: string[]): string {
  return execFileSync("git", args, { cwd, encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] }).trim();
}

beforeAll(() => {
  directory = mkdtempSync(join(tmpdir(), "ado-aw-prefetch-test-"));
  const repo = join(directory, "fixture.git");
  mkdirSync(repo);
  git(repo, ["init", "--quiet", "--initial-branch=main"]);
  git(repo, ["config", "core.autocrlf", "false"]);
  writeFileSync(join(repo, "source.txt"), "base\n");
  git(repo, ["add", "."]);
  git(repo, ["-c", "user.name=Fixture", "-c", "user.email=fixture@example.test", "commit", "--quiet", "-m", "base"]);
  base = git(repo, ["rev-parse", "HEAD"]);
  writeFileSync(join(repo, "source.txt"), Array.from({ length: 21_010 }, (_, index) => `line ${index}\n`).join(""));
  for (const file of ["nested/workflow.lock.yml", "scripts/ado-script/bundle.js",
    "scripts/ado-script/test-bin/test.js", "src/model.gen.ts", "src/catalog.gen.json", "pkg/dist/output.js", "Cargo.lock"]) {
    mkdirSync(dirname(join(repo, file)), { recursive: true });
    writeFileSync(join(repo, file), "Generated noise.\n");
  }
  git(repo, ["add", "."]);
  git(repo, ["-c", "user.name=Fixture", "-c", "user.email=fixture@example.test", "commit", "--quiet", "-m", "large head"]);
  head = git(repo, ["rev-parse", "HEAD"]);
  bash = process.platform === "win32"
    ? resolve(git(repo, ["--exec-path"]), "..", "..", "..", "bin", "bash.exe")
    : "bash";
}, 60_000);

afterAll(() => {
  if (directory) rmSync(directory, { recursive: true, force: true });
});

function script(source: "prefetch" | "shared"): string {
  if (source === "prefetch") {
    const workflow = parse(readFileSync(join(root, ".github", "workflows", "pr-data-prefetch.yml"), "utf8"));
    return workflow.jobs.prefetch.steps[0].run;
  }
  const markdown = readFileSync(join(root, ".github", "workflows", "shared", "pr-diff-data-fetch.md"), "utf8");
  const frontMatter = markdown.split(/^---\r?$/m)[1];
  if (frontMatter === undefined) throw new Error("Shared prefetch source has no front matter");
  return parse(frontMatter)["pre-agent-steps"][0].run;
}

describe.each(["prefetch", "shared"] as const)("large PR %s diff fallback", (source) => {
  function run(error: string) {
    const output = join(directory, `${source}-${error === "diff exceeded the maximum number of lines" ? "large" : "denied"}`);
    mkdirSync(output);
    const full = script(source);
    const start = full.indexOf('if ! gh pr diff "$PR_NUMBER"');
    const end = full.indexOf("LINES=$(wc", start);
    expect(start).toBeGreaterThan(-1);
    expect(end).toBeGreaterThan(start);
    const commands = full.slice(start, end).replaceAll("/tmp/gh-aw/agent", "${PROBE_OUTPUT}");
    const probe = join(output, "probe.sh");
    writeFileSync(probe, [
      "set -euo pipefail",
      'PROBE_OUTPUT="$1"',
      'gh() { printf "%s\\n" "$PROBE_ERROR" >&2; return 1; }',
      commands,
    ].join("\n"));
    return {
      output,
      result: spawnSync(bash, [probe, output.replaceAll("\\", "/")], {
        cwd: directory,
        encoding: "utf8",
        timeout: 60_000,
        env: {
          ...process.env,
          GH_TOKEN: "synthetic-fixture-only",
          PR_NUMBER: "1",
          HEAD_SHA: head,
          BASE_SHA: base,
          TARGET_REPOSITORY: "fixture",
          EXPR_GITHUB_REPOSITORY: "fixture",
          GITHUB_SERVER_URL: pathToFileURL(directory).href,
          PROBE_ERROR: error,
          GIT_TERMINAL_PROMPT: "0",
        },
      }),
    };
  }

  it("produces a complete filtered diff beyond 20,000 lines without checking out PR code", () => {
    const { result, output } = run("diff exceeded the maximum number of lines");
    expect(result.error).toBeUndefined();
    expect(result.status, result.stderr).toBe(0);
    const diff = readFileSync(join(output, "pr-diff.patch"), "utf8");
    expect(diff.split("\n").length).toBeGreaterThan(20_000);
    expect(diff).toContain("+line 21009");
    expect(diff).not.toContain("Generated noise.");
    expect(diff).not.toContain("diff --git a/Cargo.lock");
    expect(script(source)).toContain("git init --bare");
    expect(script(source)).not.toContain("git checkout");
  });

  it("does not turn an unrelated API failure into empty or cached success", () => {
    const { result } = run("HTTP 403: forbidden");
    expect(result.status).not.toBe(0);
    expect(result.stderr).toContain("HTTP 403: forbidden");
    expect(result.stdout).not.toContain("generating the full filtered diff");
  });
});
