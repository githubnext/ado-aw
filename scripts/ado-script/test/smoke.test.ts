/**
 * End-to-end smoke tests of bundled ado-script programs.
 *
 * The gate smoke test validates the existing gate.js bundle.
 * The import smoke test builds import.js and verifies it expands
 * a prompt fixture in place.
 */
import { spawnSync } from "node:child_process";
import { randomUUID } from "node:crypto";
import { copyFileSync, existsSync, mkdirSync, readFileSync, rmSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";
import { describe, expect, it } from "vitest";

const __dirname = dirname(fileURLToPath(import.meta.url));
const workspaceDir = resolve(__dirname, "..");
const gateBundlePath = resolve(__dirname, "../gate.js");
const importBundlePath = resolve(__dirname, "../import.js");
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
