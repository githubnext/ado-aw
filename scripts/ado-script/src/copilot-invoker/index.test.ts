import { EventEmitter } from "node:events";
import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it, vi } from "vitest";

import {
  buildChildEnvironment,
  buildCopilotArgs,
  main,
  parseInvocationDocument,
  parseInvocationResult,
  resolveRequestedModel,
  runInvocation,
  type InvocationDocument,
  type InvocationResult,
} from "./index.js";

function document(overrides: Partial<InvocationDocument> = {}): InvocationDocument {
  return {
    schema_version: 1,
    role: "agent",
    command: "/tmp/awf-tools/copilot",
    prompt_path: "/tmp/awf-tools/agent-prompt.md",
    mcp_config_path: "/tmp/awf-tools/mcp-config.json",
    args: ["--disable-builtin-mcps", "--allow-tool", "shell(cat *)"],
    explicit_model: null,
    result_path: "/tmp/awf-tools/copilot-invocation-result.json",
    ...overrides,
  };
}

describe("copilot invoker document", () => {
  it("parses a strict versioned document", () => {
    expect(parseInvocationDocument(JSON.stringify(document()))).toEqual(document());
  });

  describe("copilot invoker result", () => {
    it("parses a strict result document", () => {
      expect(
        parseInvocationResult(
          JSON.stringify({
            schema_version: 1,
            role: "detection",
            requested_model: "detector",
          }),
        ),
      ).toEqual({
        schema_version: 1,
        role: "detection",
        requested_model: "detector",
      });
    });

    it.each([
      [{ schema_version: 2, role: "agent", requested_model: null }, "unsupported"],
      [
        { schema_version: 1, role: "agent", requested_model: null, extra: true },
        "unknown field",
      ],
      [{ schema_version: 1, role: "other", requested_model: null }, "must be"],
      [{ schema_version: 1, role: "agent", requested_model: "bad model" }, "invalid"],
    ])("rejects malformed results %#", (value, message) => {
      expect(() => parseInvocationResult(JSON.stringify(value))).toThrow(message);
    });
  });

  it.each([
    [{ ...document(), schema_version: 2 }, "unsupported invocation schema"],
    [{ ...document(), unexpected: true }, "unknown field 'unexpected'"],
    [{ ...document(), role: "other" }, "must be 'agent' or 'detection'"],
    [{ ...document(), command: "copilot;sh" }, "field 'command' is invalid"],
    [{ ...document(), command: ".." }, "field 'command' is invalid"],
    [{ ...document(), command: "bin/copilot" }, "field 'command' is invalid"],
    [{ ...document(), command: "/tmp/../copilot" }, "field 'command' is invalid"],
    [{ ...document(), command: "/tmp//copilot" }, "field 'command' is invalid"],
    [{ ...document(), command: "/tmp/copilot/" }, "field 'command' is invalid"],
    [{ ...document(), prompt_path: "relative.md" }, "field 'prompt_path' is invalid"],
    [{ ...document(), prompt_path: "/tmp/../prompt.md" }, "field 'prompt_path' is invalid"],
    [{ ...document(), result_path: "/" }, "field 'result_path' is invalid"],
    [{ ...document(), args: [1] }, "field 'args' must be an array of strings"],
    [{ ...document(), explicit_model: "bad model" }, "field 'explicit_model' is invalid"],
  ])("rejects malformed input %#", (value, message) => {
    expect(() => parseInvocationDocument(JSON.stringify(value))).toThrow(message);
  });
});

describe("model resolution", () => {
  it("keeps an explicit model authoritative", () => {
    expect(
      resolveRequestedModel(document({ explicit_model: "frontmatter-model" }), {
        ADO_AW_MODEL_AGENT_COPILOT: "role-model",
        ADO_AW_DEFAULT_MODEL_COPILOT: "default-model",
      }),
    ).toBe("frontmatter-model");
  });

  it("uses role-specific then shared values", () => {
    expect(
      resolveRequestedModel(document(), {
        ADO_AW_MODEL_AGENT_COPILOT: "role-model",
        ADO_AW_DEFAULT_MODEL_COPILOT: "default-model",
      }),
    ).toBe("role-model");
    expect(
      resolveRequestedModel(document({ role: "detection" }), {
        ADO_AW_MODEL_DETECTION_COPILOT: "",
        ADO_AW_DEFAULT_MODEL_COPILOT: "default-model",
      }),
    ).toBe("default-model");
  });

  it("treats unresolved ADO macros as absent", () => {
    expect(
      resolveRequestedModel(document(), {
        ADO_AW_MODEL_AGENT_COPILOT: "$(ADO_AW_MODEL_AGENT_COPILOT)",
        ADO_AW_DEFAULT_MODEL_COPILOT: "$(ADO_AW_DEFAULT_MODEL_COPILOT)",
      }),
    ).toBeNull();
  });

  it("rejects invalid runtime values without printing them", () => {
    let message = "";
    try {
      resolveRequestedModel(document(), {
        ADO_AW_MODEL_AGENT_COPILOT: "secret value",
      });
    } catch (error) {
      message = error instanceof Error ? error.message : String(error);
    }
    expect(message).toContain("contains invalid characters");
    expect(message).not.toContain("secret value");
  });
});

describe("argv and child environment", () => {
  it("passes prompt, MCP config, and authored values as distinct argv elements", () => {
    expect(buildCopilotArgs(document(), "line one\nline two")).toEqual([
      "--prompt=line one\nline two",
      "--additional-mcp-config",
      "@/tmp/awf-tools/mcp-config.json",
      "--disable-builtin-mcps",
      "--allow-tool",
      "shell(cat *)",
    ]);
  });

  it("rejects NUL bytes in prompt content", () => {
    expect(() => buildCopilotArgs(document(), "before\0after")).toThrow(
      "prompt contains an invalid NUL byte",
    );
  });

  it("sets or removes only the child COPILOT_MODEL", () => {
    const original = { KEEP: "yes", COPILOT_MODEL: "old" };
    expect(buildChildEnvironment(original, "selected")).toEqual({
      KEEP: "yes",
      COPILOT_MODEL: "selected",
    });
    expect(buildChildEnvironment(original, null)).toEqual({ KEEP: "yes" });
    expect(original.COPILOT_MODEL).toBe("old");
  });
});

describe("read-result command", () => {
  it("writes the requested model after validating the result", async () => {
    const directory = mkdtempSync(join(tmpdir(), "copilot-invoker-result-"));
    const resultPath = join(directory, "result.json");
    writeFileSync(
      resultPath,
      JSON.stringify({
        schema_version: 1,
        role: "agent",
        requested_model: "gpt-test",
      }),
    );
    const write = vi.spyOn(process.stdout, "write").mockImplementation(
      ((_chunk: unknown, callback?: (error?: Error | null) => void) => {
        callback?.();
        return true;
      }) as typeof process.stdout.write,
    );

    await expect(main(["read-result", resultPath, "agent"])).resolves.toBe(0);
    expect(write).toHaveBeenCalledWith("gpt-test", expect.any(Function));
    write.mockRestore();
  });

  it("rejects a result for the wrong role", async () => {
    const directory = mkdtempSync(join(tmpdir(), "copilot-invoker-result-"));
    const resultPath = join(directory, "result.json");
    writeFileSync(
      resultPath,
      JSON.stringify({
        schema_version: 1,
        role: "agent",
        requested_model: "gpt-test",
      }),
    );
    const error = vi.spyOn(console, "error").mockImplementation(() => undefined);

    await expect(main(["read-result", resultPath, "detection"])).resolves.toBe(1);
    expect(error).toHaveBeenCalledWith(
      "copilot-invoker: invocation result role 'agent' does not match expected role 'detection'",
    );
    error.mockRestore();
  });

  it("reports a missing or malformed result", async () => {
    const directory = mkdtempSync(join(tmpdir(), "copilot-invoker-result-"));
    const resultPath = join(directory, "missing.json");
    const error = vi.spyOn(console, "error").mockImplementation(() => undefined);

    await expect(main(["read-result", resultPath, "agent"])).resolves.toBe(1);
    expect(error).toHaveBeenCalledWith(expect.stringContaining("copilot-invoker:"));
    error.mockRestore();
  });

  it("reports stdout write failures", async () => {
    const directory = mkdtempSync(join(tmpdir(), "copilot-invoker-result-"));
    const resultPath = join(directory, "result.json");
    writeFileSync(
      resultPath,
      JSON.stringify({
        schema_version: 1,
        role: "agent",
        requested_model: "gpt-test",
      }),
    );
    const write = vi.spyOn(process.stdout, "write").mockImplementation(
      ((_chunk: unknown, callback?: (error?: Error | null) => void) => {
        callback?.(new Error("EPIPE"));
        return false;
      }) as typeof process.stdout.write,
    );
    const error = vi.spyOn(console, "error").mockImplementation(() => undefined);

    await expect(main(["read-result", resultPath, "agent"])).resolves.toBe(1);
    expect(error).toHaveBeenCalledWith("copilot-invoker: EPIPE");
    write.mockRestore();
    error.mockRestore();
  });
});

describe("process lifecycle", () => {
  it("publishes the result before spawning and preserves the child exit code", async () => {
    const events: string[] = [];
    const child = new EventEmitter() as EventEmitter & {
      killed: boolean;
      kill: ReturnType<typeof vi.fn>;
    };
    child.killed = false;
    child.kill = vi.fn();
    const results: InvocationResult[] = [];
    let spawnedArgs: readonly string[] | undefined;
    const spawn = vi.fn((_command: string, args: readonly string[]) => {
      spawnedArgs = args;
      events.push("spawn");
      queueMicrotask(() => child.emit("close", 23, null));
      return child;
    });

    const exit = await runInvocation(
      document(),
      { ADO_AW_MODEL_AGENT_COPILOT: "runtime-model" },
      {
        readFile: () => "prompt",
        writeResult: (_path, result) => {
          events.push("result");
          results.push(result);
        },
        spawn: spawn as never,
      },
    );

    expect(events).toEqual(["result", "spawn"]);
    expect(results).toEqual([
      { schema_version: 1, role: "agent", requested_model: "runtime-model" },
    ]);
    expect(exit).toBe(23);
    expect(spawnedArgs).toEqual([
      "--prompt=prompt",
      "--additional-mcp-config",
      "@/tmp/awf-tools/mcp-config.json",
      "--disable-builtin-mcps",
      "--allow-tool",
      "shell(cat *)",
    ]);
  });

  it("writes results atomically with private permissions", async () => {
    const directory = mkdtempSync(join(tmpdir(), "copilot-invoker-"));
    const resultPath = join(directory, "result.json").replaceAll("\\", "/");
    const child = new EventEmitter() as EventEmitter & {
      killed: boolean;
      kill: ReturnType<typeof vi.fn>;
    };
    child.killed = false;
    child.kill = vi.fn();
    const invocation = document({ result_path: resultPath });
    const promise = runInvocation(invocation, {}, {
      readFile: () => "prompt",
      writeResult: (await import("./index.js")).writeResultAtomic,
      spawn: (() => {
        queueMicrotask(() => child.emit("close", 0, null));
        return child;
      }) as never,
    });
    await expect(promise).resolves.toBe(0);
    expect(JSON.parse(readFileSync(resultPath, "utf8"))).toEqual({
      schema_version: 1,
      role: "agent",
      requested_model: null,
    });
  });

  it("returns a deterministic failure when Copilot cannot start", async () => {
    const child = new EventEmitter() as EventEmitter & {
      killed: boolean;
      kill: ReturnType<typeof vi.fn>;
    };
    child.killed = false;
    child.kill = vi.fn();
    const error = vi.spyOn(console, "error").mockImplementation(() => undefined);
    const promise = runInvocation(document(), {}, {
      readFile: () => "prompt",
      writeResult: () => undefined,
      spawn: (() => {
        queueMicrotask(() => child.emit("error", new Error("ENOENT")));
        return child;
      }) as never,
    });
    await expect(promise).resolves.toBe(1);
    expect(error).toHaveBeenCalledWith(
      "copilot-invoker: failed to start Copilot: ENOENT",
    );
    error.mockRestore();
  });

  it("forwards termination signals and maps a signaled exit", async () => {
    const child = new EventEmitter() as EventEmitter & {
      killed: boolean;
      kill: ReturnType<typeof vi.fn>;
    };
    child.killed = false;
    child.kill = vi.fn((signal: NodeJS.Signals) => {
      queueMicrotask(() => child.emit("close", null, signal));
      return true;
    });
    const promise = runInvocation(document(), {}, {
      readFile: () => "prompt",
      writeResult: () => undefined,
      spawn: (() => child) as never,
    });
    process.emit("SIGTERM", "SIGTERM");
    await expect(promise).resolves.toBe(143);
    expect(child.kill).toHaveBeenCalledWith("SIGTERM");
  });
});
