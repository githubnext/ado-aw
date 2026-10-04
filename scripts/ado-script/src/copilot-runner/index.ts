import { spawn, type ChildProcess } from "node:child_process";
import { readFileSync, unlinkSync } from "node:fs";
import { constants } from "node:os";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

import {
  buildChildEnvironment,
  buildCopilotArgs,
  parsePreparedInvocation,
  type PreparedInvocation,
} from "../copilot-shared/protocol.js";

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
  removeFile(path: string): void;
}

const DEFAULT_DEPENDENCIES: RunDependencies = {
  spawn,
  readFile: (path) => readFileSync(path, "utf8"),
  removeFile: unlinkSync,
};

function signalExitCode(signal: NodeJS.Signals): number {
  return 128 + (constants.signals[signal] ?? 0);
}

export async function runInvocation(
  document: PreparedInvocation,
  preparedPath: string,
  runnerPath: string,
  env: NodeJS.ProcessEnv = process.env,
  dependencies: RunDependencies = DEFAULT_DEPENDENCIES,
): Promise<number> {
  const prompt = dependencies.readFile(document.prompt_path);
  const args = buildCopilotArgs(document, prompt);
  dependencies.removeFile(preparedPath);
  dependencies.removeFile(runnerPath);
  const child = dependencies.spawn(document.command, args, {
    env: buildChildEnvironment(env, document.requested_model),
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
      console.error(`copilot-runner: failed to start Copilot: ${error.message}`);
      settle(1);
    });
    child.once("close", (code, signal) => {
      settle(code ?? (signal ? signalExitCode(signal) : 1));
    });
  });
}

export async function main(
  argv: string[],
  runnerPath = fileURLToPath(import.meta.url),
): Promise<number> {
  if (argv[0] !== "run" || argv.length !== 2) {
    console.error("usage: copilot-runner run <prepared-invocation.json>");
    return 2;
  }
  try {
    const document = parsePreparedInvocation(readFileSync(argv[1]!, "utf8"));
    return await runInvocation(document, argv[1]!, runnerPath);
  } catch (error) {
    const message = error instanceof Error ? error.message : "unknown error";
    console.error(`copilot-runner: ${message}`);
    return 1;
  }
}

const invokedPath = process.argv[1] ? resolve(process.argv[1]) : "";
if (invokedPath === fileURLToPath(import.meta.url)) {
  void main(process.argv.slice(2), invokedPath)
    .then((code) => {
      process.exitCode = code;
    })
    .catch((error: unknown) => {
      const message = error instanceof Error ? error.message : "unknown error";
      console.error(`copilot-runner: unexpected failure: ${message}`);
      process.exitCode = 1;
    });
}
