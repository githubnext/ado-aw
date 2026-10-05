import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

import {
  parseInvocationRequest,
  parseInvocationResult,
  prepareInvocation,
  writeJsonAtomic,
} from "../copilot-shared/protocol.js";

async function writeStdout(value: string): Promise<void> {
  await new Promise<void>((resolveWrite, rejectWrite) => {
    process.stdout.write(value, (error) => {
      if (error) rejectWrite(error);
      else resolveWrite();
    });
  });
}

export async function main(argv: string[]): Promise<number> {
  if (argv[0] === "prepare" && argv.length === 4) {
    try {
      const request = parseInvocationRequest(readFileSync(argv[1]!, "utf8"));
      const { prepared, result } = prepareInvocation(request, process.env);
      writeJsonAtomic(argv[3]!, result);
      writeJsonAtomic(argv[2]!, prepared);
      return 0;
    } catch (error) {
      const message = error instanceof Error ? error.message : "unknown error";
      console.error(`copilot-controller: ${message}`);
      return 1;
    }
  }
  if (argv[0] === "read-result" && argv.length === 3) {
    try {
      const result = parseInvocationResult(readFileSync(argv[1]!, "utf8"));
      if (result.role !== argv[2]) {
        throw new Error(
          `invocation result role '${result.role}' does not match expected role '${argv[2]}'`,
        );
      }
      await writeStdout(result.requested_model ?? "");
      return 0;
    } catch (error) {
      const message = error instanceof Error ? error.message : "unknown error";
      console.error(`copilot-controller: ${message}`);
      return 1;
    }
  }
  console.error(
    "usage: copilot-controller prepare <request.json> <prepared.json> <result.json> | read-result <result.json> <role>",
  );
  return 2;
}

const invokedPath = process.argv[1] ? resolve(process.argv[1]) : "";
if (invokedPath === fileURLToPath(import.meta.url)) {
  void main(process.argv.slice(2))
    .then((code) => {
      process.exitCode = code;
    })
    .catch((error: unknown) => {
      const message = error instanceof Error ? error.message : "unknown error";
      console.error(`copilot-controller: unexpected failure: ${message}`);
      process.exitCode = 1;
    });
}
