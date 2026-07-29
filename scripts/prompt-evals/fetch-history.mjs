#!/usr/bin/env node

import { mkdir, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { runProcess } from "./lib/process.mjs";

function parseArgs(argv) {
  const result = {};
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    const key = arg.replace(/^--/, "");
    const value = argv[index + 1];
    if (!arg.startsWith("--") || !value || value.startsWith("--")) {
      throw new Error(`expected --key value, received ${arg}`);
    }
    result[key] = value;
    index += 1;
  }
  return result;
}

async function writeJson(filePath, value) {
  await writeFile(filePath, `${JSON.stringify(value, null, 2)}\n`, "utf8");
}

export async function fetchHistory({
  outputDir,
  workflow,
  limit,
  repository,
  currentRunId,
  env = process.env
}) {
  await mkdir(outputDir, { recursive: true });
  const list = await runProcess(
    "gh",
    [
      "run",
      "list",
      "--repo",
      repository,
      "--workflow",
      workflow,
      "--event",
      "schedule",
      "--limit",
      String(Math.max(limit + 10, limit)),
      "--json",
      "databaseId,conclusion,createdAt,headSha,url"
    ],
    {
      env,
      timeoutMs: 120_000,
      maxOutputBytes: 2_097_152
    }
  );
  if (!list.success) {
    throw new Error(`failed to list prompt evaluation history: ${list.stderr}`);
  }
  const runs = JSON.parse(list.stdout);
  const downloaded = [];
  const rejected = [];

  for (const run of runs) {
    if (downloaded.length >= limit) {
      break;
    }
    if (String(run.databaseId) === String(currentRunId)) {
      continue;
    }
    const runDir = path.join(outputDir, String(run.databaseId));
    await mkdir(runDir, { recursive: true });
    const download = await runProcess(
      "gh",
      [
        "run",
        "download",
        String(run.databaseId),
        "--repo",
        repository,
        "--name",
        "prompt-eval-results",
        "--dir",
        runDir
      ],
      {
        env,
        timeoutMs: 180_000,
        maxOutputBytes: 1_048_576
      }
    );
    if (download.success) {
      downloaded.push({ ...run, directory: runDir });
    } else {
      rejected.push({
        ...run,
        reason: download.stderr || "prompt-eval-results artifact unavailable"
      });
    }
  }
  const index = {
    schema_version: 1,
    workflow,
    repository,
    downloaded,
    rejected
  };
  await writeJson(path.join(outputDir, "history-index.json"), index);
  return index;
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  const outputDir = path.resolve(args.output);
  await fetchHistory({
    outputDir,
    workflow: args.workflow,
    limit: Number(args.limit ?? 30),
    repository: args.repository,
    currentRunId: args["current-run-id"],
    env: process.env
  });
}

if (path.resolve(process.argv[1] ?? "") === fileURLToPath(import.meta.url)) {
  main().catch((error) => {
    console.error(error.stack ?? error.message);
    process.exitCode = 1;
  });
}

