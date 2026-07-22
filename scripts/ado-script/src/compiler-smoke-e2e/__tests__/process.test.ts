import { describe, expect, it } from "vitest";

import { redact, safeSpawn, sleep, stripTraceEnv } from "../process.js";

const isWindows = process.platform === "win32";
const nodeCmd = process.execPath;

function nodeEval(script: string): string[] {
  return ["-e", script];
}

describe("stripTraceEnv", () => {
  it("removes GIT_TRACE / GIT_TRACE_CURL / GIT_CURL_VERBOSE", () => {
    const env = stripTraceEnv({
      GIT_TRACE: "1",
      GIT_TRACE_CURL: "1",
      GIT_CURL_VERBOSE: "1",
      PATH: "/usr/bin",
    });
    expect(env.GIT_TRACE).toBeUndefined();
    expect(env.GIT_TRACE_CURL).toBeUndefined();
    expect(env.GIT_CURL_VERBOSE).toBeUndefined();
    expect(env.PATH).toBe("/usr/bin");
  });

  it("does not mutate the input object", () => {
    const input = { GIT_TRACE: "1" };
    stripTraceEnv(input);
    expect(input.GIT_TRACE).toBe("1");
  });
});

describe("redact", () => {
  it("replaces every occurrence of a secret with ***", () => {
    expect(redact("token=abc123 again abc123", ["abc123"])).toBe("token=*** again ***");
  });

  it("ignores empty/undefined secrets", () => {
    expect(redact("hello world", [undefined, "", "world"])).toBe("hello ***");
  });

  it("redacts multiple distinct secrets", () => {
    expect(redact("a=1 b=2", ["1", "2"])).toBe("a=*** b=***");
  });

  it("is a no-op when no secrets are given", () => {
    expect(redact("unchanged", [])).toBe("unchanged");
  });
});

describe("safeSpawn", () => {
  it("captures stdout and a zero exit status", async () => {
    const outcome = await safeSpawn({
      cmd: nodeCmd,
      args: nodeEval("process.stdout.write('hello')"),
      timeoutMs: 10_000,
    });
    expect(outcome.status).toBe(0);
    expect(outcome.stdout).toBe("hello");
    expect(outcome.timedOut).toBe(false);
  });

  it("captures a non-zero exit status", async () => {
    const outcome = await safeSpawn({
      cmd: nodeCmd,
      args: nodeEval("process.exit(3)"),
      timeoutMs: 10_000,
    });
    expect(outcome.status).toBe(3);
  });

  it("kills a child that exceeds timeoutMs and reports timedOut", async () => {
    const outcome = await safeSpawn({
      cmd: nodeCmd,
      args: nodeEval("setTimeout(() => {}, 60000)"),
      timeoutMs: 200,
    });
    expect(outcome.timedOut).toBe(true);
  }, 10_000);

  it("truncates stdout past maxOutputBytes and reports stdoutTruncated", async () => {
    const outcome = await safeSpawn({
      cmd: nodeCmd,
      args: nodeEval("process.stdout.write('x'.repeat(1000))"),
      timeoutMs: 10_000,
      maxOutputBytes: 10,
    });
    expect(outcome.stdout.length).toBeLessThanOrEqual(10);
    expect(outcome.stdoutTruncated).toBe(true);
  });

  it("strips ambient GIT_TRACE-family env vars from the child environment", async () => {
    const probe = isWindows
      ? "console.log(JSON.stringify({t: process.env.GIT_TRACE ?? null}))"
      : "console.log(JSON.stringify({t: process.env.GIT_TRACE ?? null}))";
    const outcome = await safeSpawn({
      cmd: nodeCmd,
      args: nodeEval(probe),
      env: { GIT_TRACE: "1" },
      timeoutMs: 10_000,
    });
    expect(JSON.parse(outcome.stdout.trim())).toEqual({ t: null });
  });

  it("merges caller-provided env on top of process.env", async () => {
    const outcome = await safeSpawn({
      cmd: nodeCmd,
      args: nodeEval("process.stdout.write(process.env.MY_CUSTOM_VAR ?? '')"),
      env: { MY_CUSTOM_VAR: "custom-value" },
      timeoutMs: 10_000,
    });
    expect(outcome.stdout).toBe("custom-value");
  });
});

describe("sleep", () => {
  it("resolves after roughly the requested delay", async () => {
    const start = Date.now();
    await sleep(20);
    expect(Date.now() - start).toBeGreaterThanOrEqual(10);
  });
});
