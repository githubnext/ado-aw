import { describe, expect, it, vi } from "vitest";

import {
  loadMirrorSyncConfig,
  mirrorRepoUrl,
  runMirrorSyncPreflight,
  type MirrorGitRequest,
  type MirrorGitResult,
  type MirrorGitRunner,
} from "../mirror.js";

const HEAD = "1111111111111111111111111111111111111111";

function baseEnv(): NodeJS.ProcessEnv {
  return {
    TRIGGER_E2E_SYNC_MIRROR: "true",
    TRIGGER_E2E_VICTIM_REPO: "ado-aw-mirror",
    SYSTEM_COLLECTIONURI: "https://dev.azure.com/msazuresphere/",
    SYSTEM_TEAMPROJECT: "Agent Playground",
    SYSTEM_ACCESSTOKEN: "secret-token",
    BUILD_SOURCESDIRECTORY: "/src",
    BUILD_SOURCEBRANCH: "refs/heads/main",
    BUILD_SOURCEVERSION: HEAD,
  };
}

function successRunner(calls: MirrorGitRequest[]): MirrorGitRunner {
  return async (request): Promise<MirrorGitResult> => {
    calls.push(request);
    if (request.args[0] === "rev-parse" && request.args[1] === "--is-shallow-repository") {
      return { status: 0, stdout: "false\n", stderr: "" };
    }
    if (request.args[0] === "rev-parse") {
      return { status: 0, stdout: `${HEAD}\n`, stderr: "" };
    }
    if (request.args[0] === "ls-remote") {
      return { status: 0, stdout: `${HEAD}\trefs/heads/main\n`, stderr: "" };
    }
    return { status: 0, stdout: "", stderr: "" };
  };
}

describe("trigger mirror sync", () => {
  it("is disabled unless explicitly opted in", async () => {
    const runner = vi.fn<MirrorGitRunner>();
    const result = await runMirrorSyncPreflight({}, () => {}, runner);
    expect(result).toBeUndefined();
    expect(runner).not.toHaveBeenCalled();
  });

  it("builds an encoded ADO Git URL", () => {
    expect(
      mirrorRepoUrl(
        "https://dev.azure.com/msazuresphere/",
        "Agent Playground",
        "ado aw/mirror",
      ),
    ).toBe(
      "https://dev.azure.com/msazuresphere/Agent%20Playground/_git/ado%20aw%2Fmirror",
    );
  });

  it("pushes main fast-forward-only and verifies the remote SHA", async () => {
    const calls: MirrorGitRequest[] = [];
    const result = await runMirrorSyncPreflight(baseEnv(), () => {}, successRunner(calls));

    expect(result?.ok).toBe(true);
    expect(calls.map((call) => call.args[0])).toEqual([
      "rev-parse",
      "rev-parse",
      "push",
      "ls-remote",
    ]);
    const push = calls[2];
    expect(push?.args).toContain("HEAD:refs/heads/main");
    expect(push?.args).not.toContain("--force");
    expect(push?.args.join(" ")).not.toContain("secret-token");
    expect(push?.env.GIT_CONFIG_VALUE_0).toBe("Authorization: bearer secret-token");
    expect(push?.cwd).toBe("/src");
  });

  it("rejects a non-main orchestrator checkout before invoking git", async () => {
    const runner = vi.fn<MirrorGitRunner>();
    const result = await runMirrorSyncPreflight(
      { ...baseEnv(), BUILD_SOURCEBRANCH: "refs/heads/feature" },
      () => {},
      runner,
    );

    expect(result?.ok).toBe(false);
    expect(result?.message).toContain("refs/heads/main");
    expect(runner).not.toHaveBeenCalled();
  });

  it("rejects a shallow checkout", async () => {
    const runner: MirrorGitRunner = async () => ({
      status: 0,
      stdout: "true\n",
      stderr: "",
    });
    const result = await runMirrorSyncPreflight(baseEnv(), () => {}, runner);

    expect(result?.ok).toBe(false);
    expect(result?.message).toContain("fetchDepth: 0");
  });

  it("fails closed on a non-fast-forward push", async () => {
    const calls: MirrorGitRequest[] = [];
    const runner: MirrorGitRunner = async (request) => {
      calls.push(request);
      if (request.args[0] === "rev-parse" && request.args[1] === "--is-shallow-repository") {
        return { status: 0, stdout: "false\n", stderr: "" };
      }
      if (request.args[0] === "rev-parse") {
        return { status: 0, stdout: `${HEAD}\n`, stderr: "" };
      }
      return { status: 1, stdout: "", stderr: "! [rejected] non-fast-forward" };
    };

    const result = await runMirrorSyncPreflight(baseEnv(), () => {}, runner);
    expect(result?.ok).toBe(false);
    expect(result?.message).toContain("non-fast-forward");
    expect(calls.some((call) => call.args[0] === "ls-remote")).toBe(false);
  });

  it("fails when the verified remote SHA differs", async () => {
    const runner: MirrorGitRunner = async (request) => {
      if (request.args[0] === "rev-parse" && request.args[1] === "--is-shallow-repository") {
        return { status: 0, stdout: "false\n", stderr: "" };
      }
      if (request.args[0] === "rev-parse") {
        return { status: 0, stdout: `${HEAD}\n`, stderr: "" };
      }
      if (request.args[0] === "ls-remote") {
        return {
          status: 0,
          stdout: "2222222222222222222222222222222222222222\trefs/heads/main\n",
          stderr: "",
        };
      }
      return { status: 0, stdout: "", stderr: "" };
    };

    const result = await runMirrorSyncPreflight(baseEnv(), () => {}, runner);
    expect(result?.ok).toBe(false);
    expect(result?.message).toContain("mirror verification failed");
  });

  it("preserves the bypass-only baseline when the victim repo is unset", async () => {
    const runner = vi.fn<MirrorGitRunner>();
    const logs: string[] = [];
    const env = {
      ...baseEnv(),
      TRIGGER_E2E_VICTIM_REPO: "",
    };

    expect(loadMirrorSyncConfig(env)).toBeUndefined();
    expect(await runMirrorSyncPreflight(env, (message) => logs.push(message), runner)).toBeUndefined();
    expect(logs).toContain(
      "[mirror-sync] skipped: TRIGGER_E2E_VICTIM_REPO is not configured",
    );
    expect(runner).not.toHaveBeenCalled();
  });
});
