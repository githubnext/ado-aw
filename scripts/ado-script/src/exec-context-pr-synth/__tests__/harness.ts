/**
 * Test harness for the exec-context-pr-synth bundle.
 *
 * Provides:
 *   - `runMain(env)` — invokes the bundle's main() with a captured
 *     stdout buffer.
 *   - `makeEnv(overrides)` — returns a minimal env block populated
 *     with the required vars, easy to override per case.
 *   - `build_pr_synth_spec(spec)` — base64-encodes a PrSynthSpec JSON
 *     for the PR_SYNTH_SPEC env var.
 */
import { vi } from "vitest";

import { main } from "../index.js";
import { _resetCompletedForTesting } from "../../shared/vso-logger.js";

export interface RunResult {
  code: number;
  output: string;
}

export async function runMain(env: NodeJS.ProcessEnv): Promise<RunResult> {
  _resetCompletedForTesting();
  const chunks: string[] = [];
  const writeSpy = vi
    .spyOn(process.stdout, "write")
    .mockImplementation((c: any) => {
      chunks.push(typeof c === "string" ? c : c.toString());
      return true;
    });
  try {
    const code = await main(env);
    return { code, output: chunks.join("") };
  } finally {
    writeSpy.mockRestore();
  }
}

export function makeEnv(overrides: Record<string, string>): NodeJS.ProcessEnv {
  return {
    BUILD_REASON: "IndividualCI",
    BUILD_REPOSITORY_PROVIDER: "TfsGit",
    BUILD_SOURCEBRANCH: "refs/heads/feature/x",
    ADO_PROJECT: "MyProject",
    ADO_REPO_ID: "00000000-0000-0000-0000-000000000000",
    ...overrides,
  };
}

export function build_pr_synth_spec(
  spec: {
    branches?: { include: string[]; exclude: string[] };
    paths?: { include: string[]; exclude: string[] };
  } = {},
): string {
  const full = {
    branches: spec.branches ?? { include: ["main"], exclude: [] },
    paths: spec.paths ?? { include: [], exclude: [] },
  };
  return Buffer.from(JSON.stringify(full), "utf8").toString("base64");
}
