import { describe, expect, it, vi } from "vitest";

const spawnCalls: { cmd: string; args: string[] }[] = [];
let scriptedOutcomes: Array<{ status: number | null; stdout?: string; stderr?: string; timedOut?: boolean }> = [];

vi.mock("../process.js", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../process.js")>();
  return {
    ...actual,
    safeSpawn: vi.fn(async (request: { cmd: string; args: string[] }) => {
      spawnCalls.push({ cmd: request.cmd, args: request.args });
      const next = scriptedOutcomes.shift();
      if (!next) throw new Error("no scripted outcome left");
      return {
        status: next.status,
        stdout: next.stdout ?? "",
        stderr: next.stderr ?? "",
        timedOut: next.timedOut ?? false,
        stdoutTruncated: false,
        stderrTruncated: false,
      };
    }),
  };
});

const { compileAndCheck } = await import("../compile-cli.js");

function reset(outcomes: typeof scriptedOutcomes): void {
  spawnCalls.length = 0;
  scriptedOutcomes = [...outcomes];
}

describe("compileAndCheck", () => {
  it("runs 'compile --force <md>' then 'check <lock>' in order and reports success", async () => {
    reset([{ status: 0, stdout: "compiled\n" }, { status: 0, stdout: "checked\n" }]);
    const result = await compileAndCheck({
      adoAwBin: "C:\\bin\\ado-aw.exe",
      worktreeDir: "/wt",
      relMd: "tests/safe-outputs/canary.md",
      relLock: "tests/safe-outputs/canary.lock.yml",
      timeoutMs: 1000,
    });
    expect(result.ok).toBe(true);
    expect(spawnCalls).toEqual([
      { cmd: "C:\\bin\\ado-aw.exe", args: ["compile", "--force", "tests/safe-outputs/canary.md"] },
      { cmd: "C:\\bin\\ado-aw.exe", args: ["check", "tests/safe-outputs/canary.lock.yml"] },
    ]);
  });

  it("reports phase 'compile' and never runs check when compile fails", async () => {
    reset([{ status: 1, stderr: "parse error" }]);
    const result = await compileAndCheck({
      adoAwBin: "ado-aw",
      worktreeDir: "/wt",
      relMd: "tests/safe-outputs/canary.md",
      relLock: "tests/safe-outputs/canary.lock.yml",
      timeoutMs: 1000,
    });
    expect(result.ok).toBe(false);
    expect(result.phase).toBe("compile");
    expect(result.message).toMatch(/exited 1/);
    expect(spawnCalls).toHaveLength(1);
  });

  it("reports phase 'check' when compile succeeds but check fails", async () => {
    reset([{ status: 0, stdout: "compiled\n" }, { status: 2, stderr: "check failed" }]);
    const result = await compileAndCheck({
      adoAwBin: "ado-aw",
      worktreeDir: "/wt",
      relMd: "tests/safe-outputs/canary.md",
      relLock: "tests/safe-outputs/canary.lock.yml",
      timeoutMs: 1000,
    });
    expect(result.ok).toBe(false);
    expect(result.phase).toBe("check");
    expect(spawnCalls).toHaveLength(2);
  });

  it("reports a timeout as a compile-phase failure without throwing", async () => {
    reset([{ status: null, timedOut: true }]);
    const result = await compileAndCheck({
      adoAwBin: "ado-aw",
      worktreeDir: "/wt",
      relMd: "tests/safe-outputs/canary.md",
      relLock: "tests/safe-outputs/canary.lock.yml",
      timeoutMs: 1000,
    });
    expect(result.ok).toBe(false);
    expect(result.phase).toBe("compile");
    expect(result.message).toMatch(/timed out/);
  });

  it("redacts configured secrets from captured stdout/stderr", async () => {
    reset([{ status: 1, stderr: "token=super-secret-value leaked" }]);
    const result = await compileAndCheck({
      adoAwBin: "ado-aw",
      worktreeDir: "/wt",
      relMd: "tests/safe-outputs/canary.md",
      relLock: "tests/safe-outputs/canary.lock.yml",
      timeoutMs: 1000,
      secrets: ["super-secret-value"],
    });
    expect(result.stderr).not.toContain("super-secret-value");
    expect(result.stderr).toContain("***");
  });
});
