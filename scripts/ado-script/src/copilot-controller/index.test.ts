import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it, vi } from "vitest";

import { main } from "./index.js";

function writeRequest(directory: string): string {
  const path = join(directory, "request.json");
  writeFileSync(
    path,
    JSON.stringify({
      schema_version: 2,
      document_kind: "request",
      role: "agent",
      command: "/tmp/awf-tools/copilot",
      prompt_path: "/tmp/awf-tools/agent-prompt.md",
      mcp_config_path: null,
      args: ["--no-ask-user"],
      explicit_model: null,
    }),
  );
  return path;
}

afterEach(() => {
  vi.unstubAllEnvs();
  vi.restoreAllMocks();
});

describe("copilot controller", () => {
  it("prepares sandbox execution and host result from the same model decision", async () => {
    const directory = mkdtempSync(join(tmpdir(), "copilot-controller-"));
    const requestPath = writeRequest(directory);
    const preparedPath = join(directory, "prepared.json");
    const resultPath = join(directory, "result.json");
    vi.stubEnv("ADO_AW_MODEL_AGENT_COPILOT", "runtime-model");

    await expect(
      main(["prepare", requestPath, preparedPath, resultPath]),
    ).resolves.toBe(0);
    const prepared = JSON.parse(readFileSync(preparedPath, "utf8"));
    const result = JSON.parse(readFileSync(resultPath, "utf8"));
    expect(prepared).toMatchObject({
      schema_version: 2,
      document_kind: "prepared",
      requested_model: "runtime-model",
    });
    expect(result).toEqual({
      schema_version: 2,
      document_kind: "result",
      role: "agent",
      requested_model: prepared.requested_model,
    });
  });

  it("commits the trusted result before exposing a prepared sandbox document", async () => {
    const directory = mkdtempSync(join(tmpdir(), "copilot-controller-"));
    const requestPath = writeRequest(directory);
    const preparedPath = join(directory, "missing", "prepared.json");
    const resultPath = join(directory, "result.json");
    const error = vi.spyOn(console, "error").mockImplementation(() => undefined);

    await expect(
      main(["prepare", requestPath, preparedPath, resultPath]),
    ).resolves.toBe(1);
    expect(JSON.parse(readFileSync(resultPath, "utf8"))).toMatchObject({
      document_kind: "result",
      role: "agent",
    });
    expect(error).toHaveBeenCalledWith(
      expect.stringContaining("copilot-controller:"),
    );
  });

  it("reads only a matching strict result", async () => {
    const directory = mkdtempSync(join(tmpdir(), "copilot-controller-"));
    const resultPath = join(directory, "result.json");
    writeFileSync(
      resultPath,
      JSON.stringify({
        schema_version: 2,
        document_kind: "result",
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
  });

  it("rejects role mismatch, missing results, and stdout failures", async () => {
    const directory = mkdtempSync(join(tmpdir(), "copilot-controller-"));
    const resultPath = join(directory, "result.json");
    writeFileSync(
      resultPath,
      JSON.stringify({
        schema_version: 2,
        document_kind: "result",
        role: "agent",
        requested_model: "gpt-test",
      }),
    );
    const error = vi.spyOn(console, "error").mockImplementation(() => undefined);
    await expect(main(["read-result", resultPath, "detection"])).resolves.toBe(1);
    await expect(
      main(["read-result", join(directory, "missing.json"), "agent"]),
    ).resolves.toBe(1);

    vi.spyOn(process.stdout, "write").mockImplementation(
      ((_chunk: unknown, callback?: (error?: Error | null) => void) => {
        callback?.(new Error("EPIPE"));
        return false;
      }) as typeof process.stdout.write,
    );
    await expect(main(["read-result", resultPath, "agent"])).resolves.toBe(1);
    expect(error).toHaveBeenCalledWith("copilot-controller: EPIPE");
  });

  it("does not expose sandbox run mode", async () => {
    const error = vi.spyOn(console, "error").mockImplementation(() => undefined);
    await expect(main(["run", "/tmp/prepared.json"])).resolves.toBe(2);
    expect(error).toHaveBeenCalledWith(expect.stringContaining("usage:"));
  });
});
