/**
 * End-to-end smoke test of the bundled gate.js.
 *
 * Spawns `node dist/gate/index.js` as a subprocess with a hand-rolled
 * GateSpec fixture and a known set of pipeline-style env vars. Verifies
 * the gate emits the expected `SHOULD_RUN` setvariable for both the
 * pass and fail cases. This validates the bundle, the env-var contract,
 * and the predicate evaluator end-to-end without touching the ADO REST
 * API.
 */
import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";
import { describe, expect, it } from "vitest";

const __dirname = dirname(fileURLToPath(import.meta.url));
const bundlePath = resolve(__dirname, "../dist/gate/index.js");
const fixturePath = resolve(
  __dirname,
  "fixtures/gate-spec-pr-title-match.json",
);

function runGate(extraEnv: Record<string, string>): {
  stdout: string;
  stderr: string;
  status: number | null;
} {
  const fixture = readFileSync(fixturePath, "utf8");
  const gateSpec = Buffer.from(fixture).toString("base64");
  const result = spawnSync(process.execPath, [bundlePath], {
    env: {
      // Wipe the parent env so leaked CI/system vars don't influence the gate.
      PATH: process.env.PATH ?? "",
      GATE_SPEC: gateSpec,
      ADO_BUILD_REASON: "PullRequest",
      SYSTEM_ACCESSTOKEN: "dummy",
      ADO_COLLECTION_URI: "https://example.invalid/",
      ADO_PROJECT: "p",
      ADO_BUILD_ID: "1",
      ...extraEnv,
    },
    encoding: "utf8",
  });
  return {
    stdout: result.stdout ?? "",
    stderr: result.stderr ?? "",
    status: result.status,
  };
}

describe("gate.js smoke", () => {
  it("emits SHOULD_RUN=true when pr_title matches the glob", () => {
    const { stdout, status } = runGate({ ADO_PR_TITLE: "fooBar" });
    expect(stdout).toContain(
      "##vso[task.setvariable variable=SHOULD_RUN;isOutput=true]true",
    );
    expect(status).toBe(0);
  });

  it("emits SHOULD_RUN=false when pr_title does not match the glob", () => {
    const { stdout } = runGate({ ADO_PR_TITLE: "barBar" });
    expect(stdout).toContain(
      "##vso[task.setvariable variable=SHOULD_RUN;isOutput=true]false",
    );
  });
});
