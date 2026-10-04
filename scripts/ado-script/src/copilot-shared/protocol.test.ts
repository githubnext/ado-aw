import { mkdtempSync, readdirSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

import {
  buildChildEnvironment,
  buildCopilotArgs,
  parseInvocationRequest,
  parseInvocationResult,
  parsePreparedInvocation,
  prepareInvocation,
  resolveRequestedModel,
  writeJsonAtomic,
  type InvocationRequest,
  type PreparedInvocation,
} from "./protocol.js";

function request(
  overrides: Partial<InvocationRequest> = {},
): InvocationRequest {
  return {
    schema_version: 2,
    document_kind: "request",
    role: "agent",
    command: "/tmp/awf-tools/copilot",
    prompt_path: "/tmp/awf-tools/agent-prompt.md",
    mcp_config_path: "/tmp/awf-tools/mcp-config.json",
    args: ["--disable-builtin-mcps", "--allow-tool", "shell(cat *)"],
    explicit_model: null,
    ...overrides,
  };
}

function prepared(
  overrides: Partial<PreparedInvocation> = {},
): PreparedInvocation {
  return {
    schema_version: 2,
    document_kind: "prepared",
    role: "agent",
    command: "/tmp/awf-tools/copilot",
    prompt_path: "/tmp/awf-tools/agent-prompt.md",
    mcp_config_path: "/tmp/awf-tools/mcp-config.json",
    args: ["--disable-builtin-mcps", "--allow-tool", "shell(cat *)"],
    requested_model: null,
    ...overrides,
  };
}

describe("Copilot invocation protocol", () => {
  it("parses strict request, prepared, and result documents", () => {
    expect(parseInvocationRequest(JSON.stringify(request()))).toEqual(request());
    expect(parsePreparedInvocation(JSON.stringify(prepared()))).toEqual(prepared());
    expect(
      parseInvocationResult(
        JSON.stringify({
          schema_version: 2,
          document_kind: "result",
          role: "detection",
          requested_model: "detector",
        }),
      ),
    ).toEqual({
      schema_version: 2,
      document_kind: "result",
      role: "detection",
      requested_model: "detector",
    });
  });

  it.each([
    [{ ...request(), schema_version: 1 }, "unsupported invocation request schema"],
    [{ ...request(), document_kind: "prepared" }, "must be 'request'"],
    [{ ...request(), unexpected: true }, "unknown field 'unexpected'"],
    [{ ...request(), role: "other" }, "must be 'agent' or 'detection'"],
    [{ ...request(), command: "copilot;sh" }, "field 'command' is invalid"],
    [{ ...request(), command: "bin/copilot" }, "field 'command' is invalid"],
    [{ ...request(), command: "/tmp/../copilot" }, "field 'command' is invalid"],
    [{ ...request(), command: "/tmp//copilot" }, "field 'command' is invalid"],
    [{ ...request(), command: "/tmp/copilot/" }, "field 'command' is invalid"],
    [{ ...request(), prompt_path: "relative.md" }, "field 'prompt_path' is invalid"],
    [{ ...request(), args: [1] }, "field 'args' must be an array of strings"],
    [{ ...request(), explicit_model: "bad model" }, "field 'explicit_model' is invalid"],
  ])("rejects malformed requests %#", (value, message) => {
    expect(() => parseInvocationRequest(JSON.stringify(value))).toThrow(message);
  });

  it("rejects request/prepared document-kind confusion", () => {
    expect(() => parsePreparedInvocation(JSON.stringify(request()))).toThrow(
      "unknown field 'explicit_model'",
    );
    expect(() => parseInvocationRequest(JSON.stringify(prepared()))).toThrow(
      "unknown field 'requested_model'",
    );
  });

  it.each([
    [{ ...prepared(), schema_version: 1 }, "unsupported prepared invocation schema"],
    [{ ...prepared(), document_kind: "request" }, "must be 'prepared'"],
    [{ ...prepared(), unexpected: true }, "unknown field 'unexpected'"],
    [{ ...prepared(), requested_model: "bad model" }, "field 'requested_model' is invalid"],
  ])("rejects malformed prepared invocations %#", (value, message) => {
    expect(() => parsePreparedInvocation(JSON.stringify(value))).toThrow(message);
  });

  it.each([
    [
      {
        schema_version: 1,
        document_kind: "result",
        role: "agent",
        requested_model: null,
      },
      "unsupported invocation result schema",
    ],
    [
      {
        schema_version: 2,
        document_kind: "prepared",
        role: "agent",
        requested_model: null,
      },
      "must be 'result'",
    ],
    [
      {
        schema_version: 2,
        document_kind: "result",
        role: "agent",
        requested_model: null,
        unexpected: true,
      },
      "unknown field 'unexpected'",
    ],
  ])("rejects malformed invocation results %#", (value, message) => {
    expect(() => parseInvocationResult(JSON.stringify(value))).toThrow(message);
  });

  it("writes JSON atomically without leaving a temporary sibling", () => {
    const directory = mkdtempSync(join(tmpdir(), "copilot-protocol-"));
    const path = join(directory, "result.json");
    writeJsonAtomic(path, {
      schema_version: 2,
      document_kind: "result",
      role: "agent",
      requested_model: null,
    });
    expect(JSON.parse(readFileSync(path, "utf8"))).toMatchObject({
      document_kind: "result",
      role: "agent",
    });
    expect(readdirSync(directory)).toEqual(["result.json"]);
  });
});

describe("model preparation", () => {
  it("keeps explicit model authoritative", () => {
    expect(
      resolveRequestedModel(request({ explicit_model: "frontmatter-model" }), {
        ADO_AW_MODEL_AGENT_COPILOT: "role-model",
        ADO_AW_DEFAULT_MODEL_COPILOT: "default-model",
      }),
    ).toBe("frontmatter-model");
  });

  it("uses role-specific then shared values and ignores unresolved macros", () => {
    expect(
      resolveRequestedModel(request(), {
        ADO_AW_MODEL_AGENT_COPILOT: "role-model",
        ADO_AW_DEFAULT_MODEL_COPILOT: "default-model",
      }),
    ).toBe("role-model");
    expect(
      resolveRequestedModel(request({ role: "detection" }), {
        ADO_AW_MODEL_DETECTION_COPILOT: "",
        ADO_AW_DEFAULT_MODEL_COPILOT: "default-model",
      }),
    ).toBe("default-model");
    expect(
      resolveRequestedModel(request(), {
        ADO_AW_MODEL_AGENT_COPILOT: "$(ADO_AW_MODEL_AGENT_COPILOT)",
        ADO_AW_DEFAULT_MODEL_COPILOT: "$(ADO_AW_DEFAULT_MODEL_COPILOT)",
      }),
    ).toBeNull();
  });

  it("rejects invalid runtime values without printing them", () => {
    expect(() =>
      resolveRequestedModel(request(), {
        ADO_AW_MODEL_AGENT_COPILOT: "secret value",
      }),
    ).toThrow("contains invalid characters");
    try {
      resolveRequestedModel(request(), {
        ADO_AW_MODEL_AGENT_COPILOT: "secret value",
      });
    } catch (error) {
      expect(String(error)).not.toContain("secret value");
    }
  });

  it("emits identical requested models in prepared and result documents", () => {
    const { prepared: execution, result } = prepareInvocation(request(), {
      ADO_AW_MODEL_AGENT_COPILOT: "runtime-model",
    });
    expect(execution.requested_model).toBe("runtime-model");
    expect(result.requested_model).toBe(execution.requested_model);
    expect(execution).not.toHaveProperty("explicit_model");
    expect(execution).not.toHaveProperty("result_path");
  });
});

describe("argv and child environment", () => {
  it("preserves prompt, MCP config, and authored values as argv elements", () => {
    expect(buildCopilotArgs(prepared(), "line one\nline two")).toEqual([
      "--prompt=line one\nline two",
      "--additional-mcp-config",
      "@/tmp/awf-tools/mcp-config.json",
      "--disable-builtin-mcps",
      "--allow-tool",
      "shell(cat *)",
    ]);
  });

  it("rejects NUL bytes in prompt content", () => {
    expect(() => buildCopilotArgs(prepared(), "before\0after")).toThrow(
      "prompt contains an invalid NUL byte",
    );
  });

  it("uses only the prepared model and removes runtime selector variables", () => {
    const original = {
      KEEP: "yes",
      COPILOT_MODEL: "old",
      ADO_AW_MODEL_AGENT_COPILOT: "conflicting",
      ADO_AW_DEFAULT_MODEL_COPILOT: "conflicting-default",
    };
    expect(buildChildEnvironment(original, "selected")).toEqual({
      KEEP: "yes",
      COPILOT_MODEL: "selected",
    });
    expect(original.COPILOT_MODEL).toBe("old");
  });
});
