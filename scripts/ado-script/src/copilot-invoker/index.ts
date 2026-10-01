import { spawn, type ChildProcess } from "node:child_process";
import { readFileSync, renameSync, writeFileSync } from "node:fs";
import { constants } from "node:os";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const SCHEMA_VERSION = 1;
const MODEL_PATTERN = /^[A-Za-z0-9._:-]+$/;
const COMMAND_PATTERN = /^[A-Za-z0-9._/-]+$/;
const DOCUMENT_KEYS = new Set([
  "schema_version",
  "role",
  "command",
  "prompt_path",
  "mcp_config_path",
  "args",
  "explicit_model",
  "result_path",
]);
const RESULT_KEYS = new Set(["schema_version", "role", "requested_model"]);

export type InvocationRole = "agent" | "detection";

export interface InvocationDocument {
  schema_version: 1;
  role: InvocationRole;
  command: string;
  prompt_path: string;
  mcp_config_path: string | null;
  args: string[];
  explicit_model: string | null;
  result_path: string;
}

export interface InvocationResult {
  schema_version: 1;
  role: InvocationRole;
  requested_model: string | null;
}

interface SpawnLike {
  (
    command: string,
    args: readonly string[],
    options: {
      env: NodeJS.ProcessEnv;
      stdio: "inherit";
    },
  ): ChildProcess;
}

export interface RunDependencies {
  spawn: SpawnLike;
  readFile(path: string): string;
  writeResult(path: string, result: InvocationResult): void;
}

const DEFAULT_DEPENDENCIES: RunDependencies = {
  spawn,
  readFile: (path) => readFileSync(path, "utf8"),
  writeResult: writeResultAtomic,
};

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function requiredString(
  value: Record<string, unknown>,
  key: string,
  validator?: (input: string) => boolean,
): string {
  const candidate = value[key];
  if (typeof candidate !== "string" || candidate.length === 0) {
    throw new Error(`invocation document field '${key}' must be a non-empty string`);
  }
  if (candidate.includes("\0") || (validator && !validator(candidate))) {
    throw new Error(`invocation document field '${key}' is invalid`);
  }
  return candidate;
}

function nullableString(
  value: Record<string, unknown>,
  key: string,
  validator?: (input: string) => boolean,
): string | null {
  const candidate = value[key];
  if (candidate === null) return null;
  if (typeof candidate !== "string" || candidate.includes("\0")) {
    throw new Error(`invocation document field '${key}' must be a string or null`);
  }
  if (validator && !validator(candidate)) {
    throw new Error(`invocation document field '${key}' is invalid`);
  }
  return candidate;
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
    segments.every((segment) => segment.length > 0 && segment !== "." && segment !== "..")
  );
}

function isAbsoluteContainerPath(value: string): boolean {
  return hasSafePathSegments(value, false);
}

function isSafeCommand(value: string): boolean {
  return COMMAND_PATTERN.test(value) && hasSafePathSegments(value, true);
}

export function parseInvocationDocument(raw: string): InvocationDocument {
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    throw new Error("invocation document is not valid JSON");
  }

  if (!isRecord(parsed)) {
    throw new Error("invocation document must be a JSON object");
  }
  const unknown = Object.keys(parsed).filter((key) => !DOCUMENT_KEYS.has(key));
  if (unknown.length > 0) {
    throw new Error(`invocation document contains unknown field '${unknown.sort()[0]}'`);
  }
  if (parsed.schema_version !== SCHEMA_VERSION) {
    throw new Error(
      `unsupported invocation schema version '${String(parsed.schema_version)}'`,
    );
  }
  if (parsed.role !== "agent" && parsed.role !== "detection") {
    throw new Error("invocation document field 'role' must be 'agent' or 'detection'");
  }
  if (!Array.isArray(parsed.args) || !parsed.args.every((arg) => typeof arg === "string")) {
    throw new Error("invocation document field 'args' must be an array of strings");
  }
  if (parsed.args.some((arg) => arg.includes("\0"))) {
    throw new Error("invocation document field 'args' contains an invalid NUL byte");
  }

  return {
    schema_version: SCHEMA_VERSION,
    role: parsed.role,
    command: requiredString(parsed, "command", isSafeCommand),
    prompt_path: requiredString(parsed, "prompt_path", isAbsoluteContainerPath),
    mcp_config_path: nullableString(parsed, "mcp_config_path", isAbsoluteContainerPath),
    args: [...parsed.args],
    explicit_model: nullableString(parsed, "explicit_model", (value) =>
      MODEL_PATTERN.test(value),
    ),
    result_path: requiredString(parsed, "result_path", isAbsoluteContainerPath),
  };
}

export function parseInvocationResult(raw: string): InvocationResult {
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    throw new Error("invocation result is not valid JSON");
  }
  if (!isRecord(parsed)) {
    throw new Error("invocation result must be a JSON object");
  }
  const unknown = Object.keys(parsed).filter((key) => !RESULT_KEYS.has(key));
  if (unknown.length > 0) {
    throw new Error(`invocation result contains unknown field '${unknown.sort()[0]}'`);
  }
  if (parsed.schema_version !== SCHEMA_VERSION) {
    throw new Error(
      `unsupported invocation result schema version '${String(parsed.schema_version)}'`,
    );
  }
  if (parsed.role !== "agent" && parsed.role !== "detection") {
    throw new Error("invocation result field 'role' must be 'agent' or 'detection'");
  }
  const requestedModel = nullableString(parsed, "requested_model", (value) =>
    MODEL_PATTERN.test(value),
  );
  return {
    schema_version: SCHEMA_VERSION,
    role: parsed.role,
    requested_model: requestedModel,
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
  document: InvocationDocument,
  env: NodeJS.ProcessEnv,
): string | null {
  if (document.explicit_model !== null) {
    return document.explicit_model;
  }
  const specific = runtimeModelVariable(document.role);
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

export function buildCopilotArgs(
  document: InvocationDocument,
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
  // AWF filters host-only credentials before launching the invoker. Preserve
  // the remaining environment because Copilot providers and MCPs consume it.
  const childEnv = { ...env };
  if (requestedModel === null) {
    delete childEnv.COPILOT_MODEL;
  } else {
    childEnv.COPILOT_MODEL = requestedModel;
  }
  return childEnv;
}

export function writeResultAtomic(path: string, result: InvocationResult): void {
  const temporary = `${path}.tmp-${process.pid}`;
  writeFileSync(temporary, `${JSON.stringify(result)}\n`, {
    encoding: "utf8",
    mode: 0o600,
  });
  renameSync(temporary, path);
}

function signalExitCode(signal: NodeJS.Signals): number {
  return 128 + (constants.signals[signal] ?? 0);
}

export async function runInvocation(
  document: InvocationDocument,
  env: NodeJS.ProcessEnv = process.env,
  dependencies: RunDependencies = DEFAULT_DEPENDENCIES,
): Promise<number> {
  const requestedModel = resolveRequestedModel(document, env);
  dependencies.writeResult(document.result_path, {
    schema_version: SCHEMA_VERSION,
    role: document.role,
    requested_model: requestedModel,
  });

  const prompt = dependencies.readFile(document.prompt_path);
  const args = buildCopilotArgs(document, prompt);
  const child = dependencies.spawn(document.command, args, {
    env: buildChildEnvironment(env, requestedModel),
    stdio: "inherit",
  });

  return await new Promise<number>((resolveExit) => {
    let settled = false;
    const settle = (code: number) => {
      if (settled) return;
      settled = true;
      for (const signal of forwardedSignals) {
        process.off(signal, handlers[signal]);
      }
      resolveExit(code);
    };
    const forwardedSignals: NodeJS.Signals[] = ["SIGINT", "SIGTERM", "SIGHUP"];
    const handlers = Object.fromEntries(
      forwardedSignals.map((signal) => [
        signal,
        () => {
          if (!child.killed) child.kill(signal);
        },
      ]),
    ) as Record<NodeJS.Signals, () => void>;
    for (const signal of forwardedSignals) {
      process.on(signal, handlers[signal]);
    }
    child.once("error", (error) => {
      console.error(`copilot-invoker: failed to start Copilot: ${error.message}`);
      settle(1);
    });
    child.once("close", (code, signal) => {
      settle(code ?? (signal ? signalExitCode(signal) : 1));
    });
  });
}

export async function main(argv: string[]): Promise<number> {
  if (argv[0] === "read-result" && argv.length === 3) {
    try {
      const result = parseInvocationResult(readFileSync(argv[1]!, "utf8"));
      if (result.role !== argv[2]) {
        throw new Error(
          `invocation result role '${result.role}' does not match expected role '${argv[2]}'`,
        );
      }
      await new Promise<void>((resolveWrite, rejectWrite) => {
        process.stdout.write(result.requested_model ?? "", (error) => {
          if (error) rejectWrite(error);
          else resolveWrite();
        });
      });
      return 0;
    } catch (error) {
      const message = error instanceof Error ? error.message : "unknown error";
      console.error(`copilot-invoker: ${message}`);
      return 1;
    }
  }
  if (argv[0] !== "run" || argv.length !== 2) {
    console.error(
      "usage: copilot-invoker run <invocation-document.json> | read-result <result.json> <role>",
    );
    return 2;
  }
  try {
    const document = parseInvocationDocument(readFileSync(argv[1]!, "utf8"));
    return await runInvocation(document);
  } catch (error) {
    const message = error instanceof Error ? error.message : "unknown error";
    console.error(`copilot-invoker: ${message}`);
    return 1;
  }
}

const invokedPath = process.argv[1] ? resolve(process.argv[1]) : "";
if (invokedPath === fileURLToPath(import.meta.url)) {
  void main(process.argv.slice(2))
    .then((code) => {
      process.exitCode = code;
    })
    .catch((error: unknown) => {
      const message = error instanceof Error ? error.message : "unknown error";
      console.error(`copilot-invoker: unexpected failure: ${message}`);
      process.exitCode = 1;
    });
}
