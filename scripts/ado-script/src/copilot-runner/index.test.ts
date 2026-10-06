import { EventEmitter } from "node:events";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it, vi } from "vitest";

import {
  main,
  runInvocation,
  type RunDependencies,
} from "./index.js";
import type { PreparedInvocation } from "../copilot-shared/protocol.js";

function prepared(
  overrides: Partial<PreparedInvocation> = {},
): PreparedInvocation {
  return {
    schema_version: 2,
    document_kind: "prepared",
    role: "agent",
    command: "/tmp/awf-tools/copilot",
    prompt_path: "/tmp/awf-tools/agent-prompt.md",
    mcp_config_path: null,
    args: ["--no-ask-user"],
    requested_model: "prepared-model",
    ...overrides,
  };
}

function childProcess() {
  const child = new EventEmitter() as EventEmitter & {
    killed: boolean;
    kill: ReturnType<typeof vi.fn>;
  };
  child.killed = false;
  child.kill = vi.fn();
  return child;
}

describe("copilot runner", () => {
  it("removes runner and prepared document before spawning", async () => {
    const events: string[] = [];
    const child = childProcess();
    let spawnedEnv: NodeJS.ProcessEnv | undefined;
    const dependencies: RunDependencies = {
      readFile: () => "prompt",
      removeFile: (path) => events.push(`remove:${path}`),
      spawn: ((
        _command: string,
        _args: readonly string[],
        options: { env: NodeJS.ProcessEnv; stdio: "inherit" },
      ) => {
        events.push("spawn");
        spawnedEnv = options.env;
        queueMicrotask(() => child.emit("close", 23, null));
        return child;
      }) as never,
    };

    await expect(
      runInvocation(
        prepared(),
        "/tmp/prepared.json",
        "/tmp/copilot-runner.js",
        {
          ADO_AW_MODEL_AGENT_COPILOT: "conflicting-model",
          ADO_AW_DEFAULT_MODEL_COPILOT: "conflicting-default",
        },
        dependencies,
      ),
    ).resolves.toBe(23);
    expect(events).toEqual([
      "remove:/tmp/prepared.json",
      "remove:/tmp/copilot-runner.js",
      "spawn",
    ]);
    expect(spawnedEnv).toEqual({ COPILOT_MODEL: "prepared-model" });
  });

  it("fails closed when self-removal fails", async () => {
    const spawn = vi.fn();
    await expect(
      runInvocation(
        prepared(),
        "/tmp/prepared.json",
        "/tmp/copilot-runner.js",
        {},
        {
          readFile: () => "prompt",
          removeFile: () => {
            throw new Error("EPERM");
          },
          spawn: spawn as never,
        },
      ),
    ).rejects.toThrow("EPERM");
    expect(spawn).not.toHaveBeenCalled();
  });

  it("returns deterministic spawn failure and forwards termination signals", async () => {
    const child = childProcess();
    const error = vi.spyOn(console, "error").mockImplementation(() => undefined);
    const failure = runInvocation(
      prepared(),
      "/tmp/prepared.json",
      "/tmp/copilot-runner.js",
      {},
      {
        readFile: () => "prompt",
        removeFile: () => undefined,
        spawn: (() => {
          queueMicrotask(() => child.emit("error", new Error("ENOENT")));
          return child;
        }) as never,
      },
    );
    await expect(failure).resolves.toBe(1);
    expect(error).toHaveBeenCalledWith(
      "copilot-runner: failed to start Copilot: ENOENT",
    );
    error.mockRestore();

    const signaledChild = childProcess();
    const listenersBefore = process.listenerCount("SIGTERM");
    signaledChild.kill = vi.fn((signal: NodeJS.Signals) => {
      queueMicrotask(() => signaledChild.emit("close", null, signal));
      return true;
    });
    const signaled = runInvocation(
      prepared(),
      "/tmp/prepared.json",
      "/tmp/copilot-runner.js",
      {},
      {
        readFile: () => "prompt",
        removeFile: () => undefined,
        spawn: (() => signaledChild) as never,
      },
    );
    process.emit("SIGTERM", "SIGTERM");
    await expect(signaled).resolves.toBe(143);
    expect(signaledChild.kill).toHaveBeenCalledWith("SIGTERM");
    expect(process.listenerCount("SIGTERM")).toBe(listenersBefore);
  });

  it("does not expose controller modes", async () => {
    const error = vi.spyOn(console, "error").mockImplementation(() => undefined);
    await expect(main(["prepare", "a", "b", "c"], "/tmp/runner.js")).resolves.toBe(2);
    await expect(main(["read-result", "a", "agent"], "/tmp/runner.js")).resolves.toBe(2);
    expect(error).toHaveBeenCalledWith(
      "usage: copilot-runner run <prepared-invocation.json>",
    );
    error.mockRestore();
  });

  it("reports unreadable and malformed prepared invocations", async () => {
    const directory = mkdtempSync(join(tmpdir(), "copilot-runner-"));
    const missingPath = join(directory, "missing.json");
    const malformedPath = join(directory, "malformed.json");
    writeFileSync(malformedPath, "not-json");
    const error = vi.spyOn(console, "error").mockImplementation(() => undefined);

    await expect(main(["run", missingPath], "/tmp/runner.js")).resolves.toBe(1);
    await expect(main(["run", malformedPath], "/tmp/runner.js")).resolves.toBe(1);
    expect(error).toHaveBeenCalledWith(
      expect.stringContaining("copilot-runner:"),
    );
    expect(error).toHaveBeenCalledWith(
      "copilot-runner: prepared invocation is not valid JSON",
    );
    error.mockRestore();
  });
});
