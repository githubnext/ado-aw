import { spawn } from "node:child_process";
import { createServer } from "node:http";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

const fixtureScript = resolve(
  dirname(fileURLToPath(import.meta.url)),
  "../../../../../tests/compiler-smoke-e2e/component-fixture/components/custom-build-tags/tag-build-script.js",
);

async function runFixture(
  orgUrl: string,
  proposal: unknown,
): Promise<{ status: number | null; stdout: string; stderr: string }> {
  const child = spawn(process.execPath, [fixtureScript], {
    env: {
      ...process.env,
      SYSTEM_COLLECTIONURI: orgUrl,
      SYSTEM_TEAMPROJECT: "AgentPlayground",
      BUILD_BUILDID: "42",
      SYSTEM_ACCESSTOKEN: "token",
    },
    stdio: ["pipe", "pipe", "pipe"],
  });
  child.stdin.end(JSON.stringify(proposal));

  let stdout = "";
  let stderr = "";
  child.stdout.setEncoding("utf8").on("data", (chunk) => {
    stdout += chunk;
  });
  child.stderr.setEncoding("utf8").on("data", (chunk) => {
    stderr += chunk;
  });
  const status = await new Promise<number | null>((resolveExit) => {
    child.on("close", resolveExit);
  });
  return { status, stdout, stderr };
}

describe("scripts-style build-tag component fixture", () => {
  it("adds the deterministic current-build tag and emits one JSON result", async () => {
    let method = "";
    let url = "";
    let authorization = "";
    const server = createServer((request, response) => {
      method = request.method ?? "";
      url = request.url ?? "";
      authorization = request.headers.authorization ?? "";
      response.writeHead(200, { "content-type": "application/json" });
      response.end("[]");
    });
    await new Promise<void>((resolveListen) =>
      server.listen(0, "127.0.0.1", resolveListen),
    );

    try {
      const address = server.address();
      if (!address || typeof address === "string") {
        throw new Error("test server did not expose a TCP address");
      }
      const outcome = await runFixture(
        `http://127.0.0.1:${address.port}/`,
        { proof: "candidate-smoke" },
      );
      expect(outcome.status).toBe(0);
      expect(outcome.stderr).toBe("");
      expect(outcome.stdout.trim().split("\n")).toHaveLength(1);
      expect(JSON.parse(outcome.stdout)).toEqual({
        status: "success",
        message: "added scripts-style build tag ado-aw-custom-script-42",
        data: { tag: "ado-aw-custom-script-42" },
      });
      expect(method).toBe("PUT");
      expect(url).toBe(
        "/AgentPlayground/_apis/build/builds/42/tags/ado-aw-custom-script-42?api-version=7.1",
      );
      expect(authorization).toBe(
        `Basic ${Buffer.from(":token", "utf8").toString("base64")}`,
      );
    } finally {
      await new Promise<void>((resolveClose, rejectClose) =>
        server.close((error) => (error ? rejectClose(error) : resolveClose())),
      );
    }
  });

  it("rejects an unexpected proof before making a request", async () => {
    const outcome = await runFixture("http://127.0.0.1:1/", {
      proof: "wrong",
    });
    expect(outcome.status).toBe(1);
    expect(outcome.stdout).toBe("");
    expect(outcome.stderr).toMatch(/proof must equal candidate-smoke/);
  });
});
