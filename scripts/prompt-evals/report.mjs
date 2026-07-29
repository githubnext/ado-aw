#!/usr/bin/env node

import { mkdir, readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

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

function percent(value) {
  return typeof value === "number" && Number.isFinite(value)
    ? `${(value * 100).toFixed(1)}%`
    : "n/a";
}

function delta(value) {
  if (typeof value !== "number" || !Number.isFinite(value)) {
    return "n/a";
  }
  const points = value * 100;
  return `${points > 0 ? "+" : ""}${points.toFixed(1)} pp`;
}

function tableCell(value) {
  return String(value ?? "")
    .replaceAll("|", "\\|")
    .replace(/\r?\n/g, " ")
    .trim();
}

function shortSha(value) {
  return value ? String(value).slice(0, 12) : "n/a";
}

function runReference(scorecard) {
  if (!scorecard.run_url) {
    return null;
  }
  return scorecard.run_id
    ? `[§${scorecard.run_id}](${scorecard.run_url})`
    : scorecard.run_url;
}

export async function readInfrastructureStatus(statusDir) {
  const checks = {
    build: "build-exit-code.txt",
    copilot_install: "copilot-install-exit-code.txt",
    runner: "runner-exit-code.txt"
  };
  const results = {};
  for (const [name, fileName] of Object.entries(checks)) {
    try {
      const raw = await readFile(path.join(statusDir, fileName), "utf8");
      const code = Number(raw.trim());
      results[name] = {
        present: true,
        code,
        success: code === 0
      };
    } catch (error) {
      if (error.code !== "ENOENT") {
        throw error;
      }
      results[name] = {
        present: false,
        code: null,
        success: null
      };
    }
  }
  return {
    checks: results,
    degraded: Object.values(results).some(
      (result) => result.present && result.success === false
    )
  };
}

function infrastructureLines(infrastructure) {
  if (!infrastructure?.degraded) {
    return [];
  }
  const failed = Object.entries(infrastructure.checks)
    .filter(([, result]) => result.present && result.success === false)
    .map(([name]) => name.replaceAll("_", " "));
  return [
    "",
    "> [!WARNING]",
    `> Evaluator infrastructure was degraded: ${failed.join(", ")}. Treat missing or inconclusive scores cautiously.`
  ];
}

function criterionRegressions(caseResult) {
  const base = caseResult.variants?.base?.score;
  const head = caseResult.variants?.head?.score;
  if (base?.status !== "scored" || head?.status !== "scored") {
    return [];
  }
  const baseCriteria = new Map(
    base.criteria.map((criterion) => [criterion.id, criterion])
  );
  return head.criteria
    .filter(
      (criterion) =>
        baseCriteria.has(criterion.id) &&
        criterion.score < baseCriteria.get(criterion.id).score
    )
    .map((criterion) => ({
      id: criterion.id,
      base_score: baseCriteria.get(criterion.id).score,
      head_score: criterion.score,
      evidence: criterion.evidence,
      reason: criterion.reason
    }));
}

export function renderPrReport(scorecard, infrastructure = null) {
  const lines = [
    "### Prompt evaluation",
    "",
    "> [!NOTE]",
    "> Semantic results are advisory. Only the separate Prompt Contracts check is merge-blocking.",
    "",
    "| Base | Candidate | Subject model | Judge model | Copilot CLI |",
    "|---|---|---|---|---|",
    `| \`${shortSha(scorecard.base_sha)}\` | \`${shortSha(scorecard.head_sha)}\` | \`${tableCell(scorecard.subject_model)}\` | \`${tableCell(scorecard.judge_model)}\` | \`${tableCell(scorecard.copilot_cli_version)}\` |`,
    ...infrastructureLines(infrastructure),
    "",
    "### Summary",
    "",
    "| Prompt | Cases | Improved | Unchanged | Regressed | Inconclusive |",
    "|---|---:|---:|---:|---:|---:|"
  ];
  for (const suite of ["create", "update", "debug"]) {
    const summary = scorecard.suites?.[suite];
    if (!summary) {
      continue;
    }
    lines.push(
      `| ${suite} | ${summary.case_count} | ${summary.improved} | ${summary.unchanged} | ${summary.regressed} | ${summary.inconclusive} |`
    );
  }

  const regressions = scorecard.cases.filter(
    (caseResult) => caseResult.comparison?.classification === "regressed"
  );
  lines.push("", "### Potential regressions", "");
  if (regressions.length === 0) {
    lines.push("No candidate regression was identified in the sampled cases.");
  } else {
    for (const caseResult of regressions) {
      lines.push(
        `#### \`${caseResult.case_id}\` (${delta(caseResult.comparison.delta)})`,
        ""
      );
      const criteria = criterionRegressions(caseResult);
      if (criteria.length === 0) {
        lines.push(
          "The aggregate score declined, but no single criterion decrease was available."
        );
      } else {
        for (const criterion of criteria) {
          lines.push(
            `- **${criterion.id}**: ${criterion.base_score} -> ${criterion.head_score}. ${tableCell(criterion.reason)} Evidence: ${tableCell(criterion.evidence)}`
          );
        }
      }
      lines.push("");
    }
  }

  lines.push(
    "<details>",
    "<summary>Per-case scores</summary>",
    "",
    "| Case | Prompt | Base | Candidate | Delta | Result |",
    "|---|---|---:|---:|---:|---|"
  );
  for (const caseResult of scorecard.cases) {
    lines.push(
      `| \`${caseResult.case_id}\` | ${caseResult.suite} | ${percent(caseResult.variants?.base?.score?.normalized_score)} | ${percent(caseResult.variants?.head?.score?.normalized_score)} | ${delta(caseResult.comparison?.delta)} | ${caseResult.comparison?.classification ?? "inconclusive"} |`
    );
  }
  lines.push("", "</details>");
  const reference = runReference(scorecard);
  if (reference) {
    lines.push("", `**References:** ${reference}`);
  }
  return `${lines.join("\n")}\n`;
}

export function renderInfrastructureReport(manifest) {
  const reason =
    manifest?.error?.split(/\r?\n/, 1)[0] ??
    "prompt evaluation did not produce a scorecard";
  return [
    "### Prompt evaluation",
    "",
    "> [!WARNING]",
    "> The advisory prompt evaluation could not produce a scorecard.",
    "",
    `**Infrastructure error:** ${tableCell(reason)}`,
    "",
    "The separate Prompt Contracts check remains the merge-blocking signal.",
    ""
  ].join("\n");
}

export async function buildPrReport({
  scorecard,
  manifest,
  outputDir,
  infrastructure
}) {
  await mkdir(outputDir, { recursive: true });
  const body = scorecard
    ? renderPrReport(scorecard, infrastructure)
    : renderInfrastructureReport(manifest);
  await writeFile(path.join(outputDir, "report.md"), body, "utf8");
}

async function readJsonIfPresent(filePath) {
  try {
    return JSON.parse(await readFile(filePath, "utf8"));
  } catch (error) {
    if (error.code === "ENOENT") {
      return null;
    }
    throw error;
  }
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  const scorecard = await readJsonIfPresent(path.resolve(args.scorecard));
  const manifest = await readJsonIfPresent(path.resolve(args.manifest));
  const infrastructure = args["status-dir"]
    ? await readInfrastructureStatus(path.resolve(args["status-dir"]))
    : null;
  await buildPrReport({
    scorecard,
    manifest,
    outputDir: path.resolve(args.output),
    infrastructure
  });
}

if (path.resolve(process.argv[1] ?? "") === fileURLToPath(import.meta.url)) {
  main().catch((error) => {
    console.error(error.stack ?? error.message);
    process.exitCode = 1;
  });
}
