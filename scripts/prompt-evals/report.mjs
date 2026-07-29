#!/usr/bin/env node

import { readFile, writeFile, mkdir } from "node:fs/promises";
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
    history: "history-exit-code.txt",
    runner: "runner-exit-code.txt",
    trend: "trend-exit-code.txt"
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

export function decideReportAction({
  eventName,
  trend = null,
  manualPublish = false
}) {
  if (eventName === "pull_request") {
    return {
      action: "pr-comment",
      reason: "pull-request comparison completed"
    };
  }
  if (eventName === "schedule") {
    if (trend?.alert?.started) {
      return {
        action: "discussion",
        reason: "new sustained regression"
      };
    }
    if (trend?.weekly_due) {
      return {
        action: "discussion",
        reason: trend.alert?.active
          ? "weekly digest with active sustained regression"
          : "weekly digest"
      };
    }
    return {
      action: "noop",
      reason: trend?.alert?.active
        ? "sustained regression already reported"
        : "nightly scorecard stored; no sustained regression"
    };
  }
  if (eventName === "workflow_dispatch") {
    return manualPublish
      ? {
          action: "discussion",
          reason: "manual calibration report requested"
        }
      : {
          action: "noop",
          reason: "manual calibration run stored without publication"
        };
  }
  return {
    action: "noop",
    reason: `unsupported reporting event ${eventName}`
  };
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

export function renderDiscussionReport(
  scorecard,
  trend,
  reason,
  infrastructure = null
) {
  const alert = trend?.alert;
  const lines = [
    "### Summary",
    "",
    "> [!NOTE]",
    "> Continuous prompt evaluation is advisory and uses synthetic fixtures only.",
    "",
    `**Report reason:** ${reason}`,
    ...infrastructureLines(infrastructure),
    "",
    `**Window:** ${trend?.windows?.latest_seven?.start_at ?? "insufficient history"} to ${trend?.windows?.latest_seven?.end_at ?? scorecard.completed_at} (UTC)`,
    "",
    "| Metric | Latest 7 | Previous 7 |",
    "|---|---:|---:|",
    `| Normalized rubric score | ${percent(trend?.windows?.latest_seven?.normalized_score)} | ${percent(trend?.windows?.previous_seven?.normalized_score)} |`,
    `| Artifact extraction | ${percent(trend?.windows?.latest_seven?.artifact_extraction_rate)} | ${percent(trend?.windows?.previous_seven?.artifact_extraction_rate)} |`,
    `| Compile success | ${percent(trend?.windows?.latest_seven?.compile_success_rate)} | ${percent(trend?.windows?.previous_seven?.compile_success_rate)} |`,
    `| Lint success | ${percent(trend?.windows?.latest_seven?.lint_success_rate)} | ${percent(trend?.windows?.previous_seven?.lint_success_rate)} |`,
    `| Safety/consent pass | ${percent(trend?.windows?.latest_seven?.safety_consent_pass_rate)} | ${percent(trend?.windows?.previous_seven?.safety_consent_pass_rate)} |`,
    `| Infrastructure failures | ${percent(trend?.windows?.latest_seven?.infrastructure_failure_rate)} | ${percent(trend?.windows?.previous_seven?.infrastructure_failure_rate)} |`
  ];

  if (alert?.active) {
    lines.push(
      "",
      "> [!WARNING]",
      "> A sustained regression is active across three comparable nightly runs.",
      "",
      "### Sustained regression"
    );
    for (const affected of alert.semantic?.affected_cases ?? []) {
      lines.push(
        `- \`${affected.case_id}\`: baseline ${percent(affected.baseline_median)}, recent ${affected.recent_scores.map(percent).join(", ")}`
      );
    }
    for (const hard of alert.hard_observables ?? []) {
      lines.push(
        `- \`${hard.case_id}\` ${hard.observable}: baseline ${percent(hard.baseline_rate)}, failed in all three recent runs`
      );
    }
  } else if (!alert?.eligible) {
    lines.push(
      "",
      "> [!NOTE]",
      `> Regression alerting is not yet eligible: ${alert?.reason ?? "insufficient comparable history"}.`
    );
  }

  lines.push(
    "",
    "### Per-prompt current state",
    "",
    "| Prompt | Score | Compile | Lint | Safety/consent | Inconclusive |",
    "|---|---:|---:|---:|---:|---:|"
  );
  for (const suite of ["create", "update", "debug"]) {
    const summary = scorecard.suites?.[suite];
    if (!summary) {
      continue;
    }
    lines.push(
      `| ${suite} | ${percent(summary.normalized_score)} | ${percent(summary.compile_success_rate)} | ${percent(summary.lint_success_rate)} | ${percent(summary.safety_consent_pass_rate)} | ${summary.inconclusive_cases}/${summary.case_count} |`
    );
  }

  lines.push(
    "",
    "<details>",
    "<summary>Cohort and case details</summary>",
    "",
    `- Cohort: \`${trend?.cohort?.key ?? "unavailable"}\``,
    `- Subject model: \`${scorecard.subject_model}\``,
    `- Judge model: \`${scorecard.judge_model}\``,
    `- Copilot CLI: \`${scorecard.copilot_cli_version}\``,
    `- Comparable previous runs: ${trend?.cohort?.comparable_previous_runs ?? 0}`,
    `- Excluded incomplete runs: ${trend?.history?.excluded_incomplete_runs ?? 0}`,
    "",
    "| Case | Prompt | Latest | 7-run average | Previous 7 |",
    "|---|---|---:|---:|---:|"
  );
  for (const entry of trend?.case_trends ?? []) {
    lines.push(
      `| \`${entry.case_id}\` | ${entry.suite} | ${percent(entry.latest_score)} | ${percent(entry.seven_run_average)} | ${percent(entry.previous_seven_run_average)} |`
    );
  }
  lines.push("", "</details>");
  const reference = runReference(scorecard);
  if (reference) {
    lines.push("", `**References:** ${reference}`);
  }
  return `${lines.join("\n")}\n`;
}

export async function buildReport({
  eventName,
  scorecard,
  trend,
  outputDir,
  manualPublish = false,
  infrastructure = null
}) {
  await mkdir(outputDir, { recursive: true });
  const decision = decideReportAction({ eventName, trend, manualPublish });
  let body = "";
  let title = "";
  if (decision.action === "pr-comment") {
    body = renderPrReport(scorecard, infrastructure);
  } else if (decision.action === "discussion") {
    body = renderDiscussionReport(
      scorecard,
      trend,
      decision.reason,
      infrastructure
    );
    title = trend?.alert?.started
      ? `Sustained regression - ${scorecard.completed_at.slice(0, 10)}`
      : `Weekly trends - ${scorecard.completed_at.slice(0, 10)}`;
  }
  const context = {
    schema_version: 1,
    event_name: eventName,
    action: decision.action,
    reason: decision.reason,
    report_path: body ? path.join(outputDir, "report.md") : null,
    title_path: title ? path.join(outputDir, "report-title.txt") : null
  };
  await writeFile(
    path.join(outputDir, "report-context.json"),
    `${JSON.stringify(context, null, 2)}\n`,
    "utf8"
  );
  await writeFile(
    path.join(outputDir, "report.md"),
    body,
    "utf8"
  );
  await writeFile(
    path.join(outputDir, "report-title.txt"),
    title,
    "utf8"
  );
  await writeFile(
    path.join(outputDir, "noop.txt"),
    `${decision.reason}\n`,
    "utf8"
  );
  return context;
}

export async function buildInfrastructureReport({
  eventName,
  manifest,
  outputDir
}) {
  await mkdir(outputDir, { recursive: true });
  const action = eventName === "pull_request" ? "pr-comment" : "noop";
  const reason =
    manifest?.error?.split(/\r?\n/, 1)[0] ??
    "prompt evaluation did not produce a scorecard";
  const body =
    action === "pr-comment"
      ? [
          "### Prompt evaluation",
          "",
          "> [!WARNING]",
          "> The advisory prompt evaluation could not produce a scorecard.",
          "",
          `**Infrastructure error:** ${tableCell(reason)}`,
          "",
          "The separate Prompt Contracts check remains the merge-blocking signal."
        ].join("\n") + "\n"
      : "";
  const context = {
    schema_version: 1,
    event_name: eventName,
    action,
    reason,
    report_path: body ? path.join(outputDir, "report.md") : null,
    title_path: null
  };
  await writeFile(
    path.join(outputDir, "report-context.json"),
    `${JSON.stringify(context, null, 2)}\n`,
    "utf8"
  );
  await writeFile(path.join(outputDir, "report.md"), body, "utf8");
  await writeFile(path.join(outputDir, "report-title.txt"), "", "utf8");
  await writeFile(path.join(outputDir, "noop.txt"), `${reason}\n`, "utf8");
  return context;
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  let scorecard = null;
  try {
    if (args.scorecard) {
      scorecard = JSON.parse(
        await readFile(path.resolve(args.scorecard), "utf8")
      );
    }
  } catch (error) {
    if (error.code !== "ENOENT") {
      throw error;
    }
  }
  if (!scorecard) {
    let manifest = null;
    try {
      if (args.manifest) {
        manifest = JSON.parse(
          await readFile(path.resolve(args.manifest), "utf8")
        );
      }
    } catch (error) {
      if (error.code !== "ENOENT") {
        throw error;
      }
    }
    await buildInfrastructureReport({
      eventName: args.event,
      manifest,
      outputDir: path.resolve(args.output)
    });
    return;
  }
  let trend = null;
  if (args.trend) {
    trend = JSON.parse(await readFile(path.resolve(args.trend), "utf8"));
  }
  const infrastructure = args["status-dir"]
    ? await readInfrastructureStatus(path.resolve(args["status-dir"]))
    : null;
  await buildReport({
    eventName: args.event,
    scorecard,
    trend,
    outputDir: path.resolve(args.output),
    manualPublish: args["manual-publish"] === "true",
    infrastructure
  });
}

if (path.resolve(process.argv[1] ?? "") === fileURLToPath(import.meta.url)) {
  main().catch((error) => {
    console.error(error.stack ?? error.message);
    process.exitCode = 1;
  });
}
