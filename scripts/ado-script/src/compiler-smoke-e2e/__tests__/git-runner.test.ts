import { describe, expect, it, vi } from "vitest";

const spawnCalls: { cmd: string; args: string[]; env?: NodeJS.ProcessEnv }[] = [];

vi.mock("../process.js", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../process.js")>();
  return {
    ...actual,
    safeSpawn: vi.fn(async (request: { cmd: string; args: string[]; env?: NodeJS.ProcessEnv }) => {
      spawnCalls.push({ cmd: request.cmd, args: request.args, env: request.env });
      return {
        status: 0,
        stdout: "",
        stderr: "",
        timedOut: false,
        stdoutTruncated: false,
        stderrTruncated: false,
      };
    }),
  };
});

const { defaultGitRunner } = await import("../git.js");

function reset(): void {
  spawnCalls.length = 0;
}

describe("defaultGitRunner", () => {
  it("forces GIT_TERMINAL_PROMPT=0 on every invocation so a bad token/unreachable mirror never hangs on a prompt", async () => {
    reset();
    await defaultGitRunner(["status"], { cwd: "/repo", timeoutMs: 1000 });
    expect(spawnCalls[0]?.cmd).toBe("git");
    expect(spawnCalls[0]?.env?.GIT_TERMINAL_PROMPT).toBe("0");
  });

  it("layers caller-supplied env (e.g. a bearer token) on top without it being shadowed", async () => {
    reset();
    await defaultGitRunner(["ls-remote", "https://example/_git/r"], {
      cwd: "/repo",
      timeoutMs: 1000,
      env: { GIT_CONFIG_COUNT: "1", GIT_CONFIG_KEY_0: "http.extraheader", GIT_CONFIG_VALUE_0: "Authorization: bearer t" },
    });
    expect(spawnCalls[0]?.env?.GIT_TERMINAL_PROMPT).toBe("0");
    expect(spawnCalls[0]?.env?.GIT_CONFIG_VALUE_0).toBe("Authorization: bearer t");
  });

  it("never lets a caller-supplied env override GIT_TERMINAL_PROMPT back on", async () => {
    reset();
    await defaultGitRunner(["fetch"], {
      cwd: "/repo",
      timeoutMs: 1000,
      env: { GIT_TERMINAL_PROMPT: "1" },
    });
    // GIT_TERMINAL_PROMPT=0 is a true floor: applied after any caller env,
    // so it can never be silently reopened.
    expect(spawnCalls[0]?.env?.GIT_TERMINAL_PROMPT).toBe("0");
  });
});
