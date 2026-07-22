"use strict";

async function readStdin() {
  const chunks = [];
  for await (const chunk of process.stdin) {
    chunks.push(Buffer.from(chunk));
  }
  return Buffer.concat(chunks).toString("utf8");
}

function requireEnv(name) {
  const value = process.env[name]?.trim();
  if (!value) {
    throw new Error(`${name} is required`);
  }
  return value;
}

async function main() {
  const proposal = JSON.parse(await readStdin());
  if (proposal.proof !== "candidate-smoke") {
    throw new Error("proposal proof must equal candidate-smoke");
  }

  const orgUrl = requireEnv("SYSTEM_COLLECTIONURI").replace(/\/+$/, "");
  const project = requireEnv("SYSTEM_TEAMPROJECT");
  const buildId = requireEnv("BUILD_BUILDID");
  const token = requireEnv("SYSTEM_ACCESSTOKEN");
  if (!/^[1-9][0-9]*$/.test(buildId)) {
    throw new Error(`BUILD_BUILDID must be a positive integer, got '${buildId}'`);
  }

  const tag = `ado-aw-custom-script-${buildId}`;
  const url =
    `${orgUrl}/${encodeURIComponent(project)}/_apis/build/builds/` +
    `${buildId}/tags/${encodeURIComponent(tag)}?api-version=7.1`;
  const authorization = Buffer.from(`:${token}`, "utf8").toString("base64");
  const response = await fetch(url, {
    method: "PUT",
    headers: {
      Authorization: `Basic ${authorization}`,
      "Content-Length": "0",
    },
  });

  if (!response.ok) {
    const body = await response.text();
    throw new Error(`failed to add build tag (HTTP ${response.status}): ${body}`);
  }

  process.stdout.write(
    `${JSON.stringify({
      status: "success",
      message: `added scripts-style build tag ${tag}`,
      data: { tag },
    })}\n`,
  );
}

main().catch((error) => {
  const message = error instanceof Error ? error.message : String(error);
  process.stderr.write(`[candidate-script-build-tag] ${message}\n`);
  process.exitCode = 1;
});
