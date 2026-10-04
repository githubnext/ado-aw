import { renameSync, writeFileSync } from "node:fs";

export const SCHEMA_VERSION = 2;

const MODEL_PATTERN = /^[A-Za-z0-9._:-]+$/;
const COMMAND_PATTERN = /^[A-Za-z0-9._/-]+$/;
const REQUEST_KEYS = new Set([
  "schema_version",
  "document_kind",
  "role",
  "command",
  "prompt_path",
  "mcp_config_path",
  "args",
  "explicit_model",
]);
const PREPARED_KEYS = new Set([
  "schema_version",
  "document_kind",
  "role",
  "command",
  "prompt_path",
  "mcp_config_path",
  "args",
  "requested_model",
]);
const RESULT_KEYS = new Set([
  "schema_version",
  "document_kind",
  "role",
  "requested_model",
]);

export type InvocationRole = "agent" | "detection";

export interface InvocationRequest {
  schema_version: 2;
  document_kind: "request";
  role: InvocationRole;
  command: string;
  prompt_path: string;
  mcp_config_path: string | null;
  args: string[];
  explicit_model: string | null;
}

export interface PreparedInvocation {
  schema_version: 2;
  document_kind: "prepared";
  role: InvocationRole;
  command: string;
  prompt_path: string;
  mcp_config_path: string | null;
  args: string[];
  requested_model: string | null;
}

export interface InvocationResult {
  schema_version: 2;
  document_kind: "result";
  role: InvocationRole;
  requested_model: string | null;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function parseObject(raw: string, label: string): Record<string, unknown> {
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    throw new Error(`${label} is not valid JSON`);
  }
  if (!isRecord(parsed)) {
    throw new Error(`${label} must be a JSON object`);
  }
  return parsed;
}

function rejectUnknown(
  parsed: Record<string, unknown>,
  allowed: ReadonlySet<string>,
  label: string,
): void {
  const unknown = Object.keys(parsed).filter((key) => !allowed.has(key));
  if (unknown.length > 0) {
    throw new Error(`${label} contains unknown field '${unknown.sort()[0]}'`);
  }
}

function requireVersionAndKind(
  parsed: Record<string, unknown>,
  kind: "request" | "prepared" | "result",
  label: string,
): void {
  if (parsed.schema_version !== SCHEMA_VERSION) {
    throw new Error(
      `unsupported ${label} schema version '${String(parsed.schema_version)}'`,
    );
  }
  if (parsed.document_kind !== kind) {
    throw new Error(`${label} field 'document_kind' must be '${kind}'`);
  }
}

function parseRole(
  parsed: Record<string, unknown>,
  label: string,
): InvocationRole {
  if (parsed.role !== "agent" && parsed.role !== "detection") {
    throw new Error(`${label} field 'role' must be 'agent' or 'detection'`);
  }
  return parsed.role;
}

function requiredString(
  value: Record<string, unknown>,
  key: string,
  label: string,
  validator?: (input: string) => boolean,
): string {
  const candidate = value[key];
  if (typeof candidate !== "string" || candidate.length === 0) {
    throw new Error(`${label} field '${key}' must be a non-empty string`);
  }
  if (candidate.includes("\0") || (validator && !validator(candidate))) {
    throw new Error(`${label} field '${key}' is invalid`);
  }
  return candidate;
}

function nullableString(
  value: Record<string, unknown>,
  key: string,
  label: string,
  validator?: (input: string) => boolean,
): string | null {
  const candidate = value[key];
  if (candidate === null) return null;
  if (typeof candidate !== "string" || candidate.includes("\0")) {
    throw new Error(`${label} field '${key}' must be a string or null`);
  }
  if (validator && !validator(candidate)) {
    throw new Error(`${label} field '${key}' is invalid`);
  }
  return candidate;
}

function parseArgs(
  parsed: Record<string, unknown>,
  label: string,
): string[] {
  if (
    !Array.isArray(parsed.args) ||
    !parsed.args.every((arg) => typeof arg === "string")
  ) {
    throw new Error(`${label} field 'args' must be an array of strings`);
  }
  if (parsed.args.some((arg) => arg.includes("\0"))) {
    throw new Error(`${label} field 'args' contains an invalid NUL byte`);
  }
  return [...parsed.args];
}

function hasSafePathSegments(value: string, allowBareCommand: boolean): boolean {
  if (
    value.includes("\n") ||
    value.includes("\r") ||
    value.includes(":") ||
    value.endsWith("/")
  ) {
    return false;
  }
  if (allowBareCommand && !value.includes("/")) {
    return value !== "." && value !== "..";
  }
  if (!value.startsWith("/")) return false;
  const segments = value.slice(1).split("/");
  return (
    segments.length > 0 &&
    segments.every(
      (segment) => segment.length > 0 && segment !== "." && segment !== "..",
    )
  );
}

function isAbsoluteContainerPath(value: string): boolean {
  return hasSafePathSegments(value, false);
}

function isSafeCommand(value: string): boolean {
  return COMMAND_PATTERN.test(value) && hasSafePathSegments(value, true);
}

export function parseInvocationRequest(raw: string): InvocationRequest {
  const label = "invocation request";
  const parsed = parseObject(raw, label);
  rejectUnknown(parsed, REQUEST_KEYS, label);
  requireVersionAndKind(parsed, "request", label);
  return {
    schema_version: SCHEMA_VERSION,
    document_kind: "request",
    role: parseRole(parsed, label),
    command: requiredString(parsed, "command", label, isSafeCommand),
    prompt_path: requiredString(
      parsed,
      "prompt_path",
      label,
      isAbsoluteContainerPath,
    ),
    mcp_config_path: nullableString(
      parsed,
      "mcp_config_path",
      label,
      isAbsoluteContainerPath,
    ),
    args: parseArgs(parsed, label),
    explicit_model: nullableString(
      parsed,
      "explicit_model",
      label,
      (value) => MODEL_PATTERN.test(value),
    ),
  };
}

export function parsePreparedInvocation(raw: string): PreparedInvocation {
  const label = "prepared invocation";
  const parsed = parseObject(raw, label);
  rejectUnknown(parsed, PREPARED_KEYS, label);
  requireVersionAndKind(parsed, "prepared", label);
  return {
    schema_version: SCHEMA_VERSION,
    document_kind: "prepared",
    role: parseRole(parsed, label),
    command: requiredString(parsed, "command", label, isSafeCommand),
    prompt_path: requiredString(
      parsed,
      "prompt_path",
      label,
      isAbsoluteContainerPath,
    ),
    mcp_config_path: nullableString(
      parsed,
      "mcp_config_path",
      label,
      isAbsoluteContainerPath,
    ),
    args: parseArgs(parsed, label),
    requested_model: nullableString(
      parsed,
      "requested_model",
      label,
      (value) => MODEL_PATTERN.test(value),
    ),
  };
}

export function parseInvocationResult(raw: string): InvocationResult {
  const label = "invocation result";
  const parsed = parseObject(raw, label);
  rejectUnknown(parsed, RESULT_KEYS, label);
  requireVersionAndKind(parsed, "result", label);
  return {
    schema_version: SCHEMA_VERSION,
    document_kind: "result",
    role: parseRole(parsed, label),
    requested_model: nullableString(
      parsed,
      "requested_model",
      label,
      (value) => MODEL_PATTERN.test(value),
    ),
  };
}

function runtimeModelVariable(role: InvocationRole): string {
  return role === "agent"
    ? "ADO_AW_MODEL_AGENT_COPILOT"
    : "ADO_AW_MODEL_DETECTION_COPILOT";
}

function isUnresolvedAdoMacro(value: string, variable: string): boolean {
  return value === `$(${variable})`;
}

export function resolveRequestedModel(
  request: InvocationRequest,
  env: NodeJS.ProcessEnv,
): string | null {
  if (request.explicit_model !== null) {
    return request.explicit_model;
  }
  const specific = runtimeModelVariable(request.role);
  const candidates: Array<[string, string | undefined]> = [
    [specific, env[specific]],
    ["ADO_AW_DEFAULT_MODEL_COPILOT", env.ADO_AW_DEFAULT_MODEL_COPILOT],
  ];
  for (const [variable, candidate] of candidates) {
    if (
      candidate === undefined ||
      candidate.length === 0 ||
      isUnresolvedAdoMacro(candidate, variable)
    ) {
      continue;
    }
    if (!MODEL_PATTERN.test(candidate)) {
      throw new Error(
        `runtime Copilot model from ${specific}/ADO_AW_DEFAULT_MODEL_COPILOT contains invalid characters`,
      );
    }
    return candidate;
  }
  return null;
}

export function prepareInvocation(
  request: InvocationRequest,
  env: NodeJS.ProcessEnv,
): { prepared: PreparedInvocation; result: InvocationResult } {
  const requestedModel = resolveRequestedModel(request, env);
  return {
    prepared: {
      schema_version: SCHEMA_VERSION,
      document_kind: "prepared",
      role: request.role,
      command: request.command,
      prompt_path: request.prompt_path,
      mcp_config_path: request.mcp_config_path,
      args: [...request.args],
      requested_model: requestedModel,
    },
    result: {
      schema_version: SCHEMA_VERSION,
      document_kind: "result",
      role: request.role,
      requested_model: requestedModel,
    },
  };
}

export function buildCopilotArgs(
  document: PreparedInvocation,
  prompt: string,
): string[] {
  if (prompt.includes("\0")) {
    throw new Error("prompt contains an invalid NUL byte");
  }
  const args = [`--prompt=${prompt}`];
  if (document.mcp_config_path !== null) {
    args.push("--additional-mcp-config", `@${document.mcp_config_path}`);
  }
  args.push(...document.args);
  return args;
}

export function buildChildEnvironment(
  env: NodeJS.ProcessEnv,
  requestedModel: string | null,
): NodeJS.ProcessEnv {
  const childEnv = { ...env };
  delete childEnv.ADO_AW_MODEL_AGENT_COPILOT;
  delete childEnv.ADO_AW_MODEL_DETECTION_COPILOT;
  delete childEnv.ADO_AW_DEFAULT_MODEL_COPILOT;
  if (requestedModel === null) {
    delete childEnv.COPILOT_MODEL;
  } else {
    childEnv.COPILOT_MODEL = requestedModel;
  }
  return childEnv;
}

export function writeJsonAtomic(
  path: string,
  value: PreparedInvocation | InvocationResult,
): void {
  const temporary = `${path}.tmp-${process.pid}`;
  writeFileSync(temporary, `${JSON.stringify(value)}\n`, {
    encoding: "utf8",
    mode: 0o600,
  });
  renameSync(temporary, path);
}
