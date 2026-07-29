#!/usr/bin/env node

import { readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { buildTrend, loadHistoryScorecards } from "./lib/history.mjs";

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

async function main() {
  const args = parseArgs(process.argv.slice(2));
  const current = JSON.parse(await readFile(path.resolve(args.current), "utf8"));
  const config = JSON.parse(await readFile(path.resolve(args.config), "utf8"));
  const history = await loadHistoryScorecards(
    path.resolve(args.history),
    config.history.max_runs
  );
  const trend = buildTrend({
    currentScorecard: current,
    historyEntries: history.accepted,
    config: config.history
  });
  trend.history.rejected = history.rejected;
  if (args["history-status"]) {
    let historyStatus = 1;
    try {
      historyStatus = Number(
        (await readFile(path.resolve(args["history-status"]), "utf8")).trim()
      );
    } catch (error) {
      if (error.code !== "ENOENT") {
        throw error;
      }
    }
    trend.history.fetch_success = historyStatus === 0;
    if (historyStatus !== 0) {
      trend.alert = {
        ...trend.alert,
        active: false,
        started: false,
        recovered: false,
        eligible: false,
        reason: "history retrieval failed for this run"
      };
    }
  } else {
    trend.history.fetch_success = true;
  }
  await writeFile(
    path.resolve(args.output),
    `${JSON.stringify(trend, null, 2)}\n`,
    "utf8"
  );
}

if (path.resolve(process.argv[1] ?? "") === fileURLToPath(import.meta.url)) {
  main().catch((error) => {
    console.error(error.stack ?? error.message);
    process.exitCode = 1;
  });
}
