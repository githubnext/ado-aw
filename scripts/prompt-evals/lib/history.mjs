import { createHash } from "node:crypto";
import { readFile, readdir, stat } from "node:fs/promises";
import path from "node:path";

function median(values) {
  const finite = values
    .filter((value) => typeof value === "number" && Number.isFinite(value))
    .sort((left, right) => left - right);
  if (finite.length === 0) {
    return null;
  }
  const middle = Math.floor(finite.length / 2);
  return finite.length % 2 === 0
    ? (finite[middle - 1] + finite[middle]) / 2
    : finite[middle];
}

function average(values) {
  const finite = values.filter(
    (value) => typeof value === "number" && Number.isFinite(value)
  );
  if (finite.length === 0) {
    return null;
  }
  return finite.reduce((sum, value) => sum + value, 0) / finite.length;
}

function rate(values) {
  if (values.length === 0) {
    return null;
  }
  return values.filter(Boolean).length / values.length;
}

function stableHash(value) {
  return createHash("sha256")
    .update(JSON.stringify(value))
    .digest("hex");
}

function cohortDescriptor(scorecard) {
  return {
    fixture_set_version: scorecard.fixture_set_version,
    fixture_set_digest: scorecard.fixture_set_digest,
    rubric_digest: scorecard.rubric_digest,
    evaluator_digest: scorecard.evaluator_digest,
    subject_model: scorecard.subject_model,
    judge_model: scorecard.judge_model,
    copilot_cli_version: scorecard.copilot_cli_version
  };
}

export function cohortKey(scorecard) {
  return stableHash(cohortDescriptor(scorecard));
}

function validateScorecard(scorecard, source) {
  if (!scorecard || typeof scorecard !== "object" || Array.isArray(scorecard)) {
    throw new Error(`${source}: scorecard must be an object`);
  }
  if (scorecard.schema_version !== 1) {
    throw new Error(`${source}: unsupported scorecard schema_version`);
  }
  if (!["nightly", "manual"].includes(scorecard.mode)) {
    throw new Error(`${source}: continuous history only accepts nightly/manual scorecards`);
  }
  if (!Array.isArray(scorecard.cases) || !scorecard.summary) {
    throw new Error(`${source}: scorecard is missing cases or summary`);
  }
  if (!scorecard.completed_at || Number.isNaN(Date.parse(scorecard.completed_at))) {
    throw new Error(`${source}: scorecard has invalid completed_at`);
  }
  return scorecard;
}

async function findFiles(root, name) {
  const found = [];
  let entries;
  try {
    entries = await readdir(root, { withFileTypes: true });
  } catch (error) {
    if (error.code === "ENOENT") {
      return found;
    }
    throw error;
  }
  for (const entry of entries) {
    const entryPath = path.join(root, entry.name);
    if (entry.isDirectory()) {
      found.push(...(await findFiles(entryPath, name)));
    } else if (entry.isFile() && entry.name === name) {
      found.push(entryPath);
    }
  }
  return found;
}

export async function loadHistoryScorecards(historyRoot, maxRuns = 30) {
  const scorecardPaths = await findFiles(historyRoot, "scorecard.json");
  const accepted = [];
  const rejected = [];
  for (const scorecardPath of scorecardPaths) {
    try {
      const scorecard = validateScorecard(
        JSON.parse(await readFile(scorecardPath, "utf8")),
        scorecardPath
      );
      accepted.push({
        scorecard,
        source: scorecardPath,
        cohort_key: cohortKey(scorecard)
      });
    } catch (error) {
      rejected.push({
        source: scorecardPath,
        reason: error.message
      });
    }
  }
  accepted.sort(
    (left, right) =>
      Date.parse(right.scorecard.completed_at) -
      Date.parse(left.scorecard.completed_at)
  );

  const seen = new Set();
  const deduplicated = [];
  for (const entry of accepted) {
    const key =
      entry.scorecard.run_id ??
      `${entry.scorecard.head_sha}:${entry.scorecard.completed_at}`;
    if (seen.has(key)) {
      rejected.push({
        source: entry.source,
        reason: `duplicate history record ${key}`
      });
      continue;
    }
    seen.add(key);
    deduplicated.push(entry);
  }
  return {
    accepted: deduplicated.slice(0, maxRuns),
    rejected
  };
}

function currentVariant(caseResult) {
  return caseResult?.variants?.current ?? null;
}

function caseScore(scorecard, caseId) {
  const caseResult = scorecard.cases.find((entry) => entry.case_id === caseId);
  const score = currentVariant(caseResult)?.score;
  return score?.status === "scored" ? score.normalized_score : null;
}

function observable(scorecard, caseId, name) {
  const caseResult = scorecard.cases.find((entry) => entry.case_id === caseId);
  const variant = currentVariant(caseResult);
  if (!caseResult || !variant) {
    return null;
  }
  if (name === "artifact") {
    return caseResult.expected.artifact_required
      ? variant.subject.artifact.found === true
      : null;
  }
  if (name === "compile") {
    return caseResult.expected.compile
      ? variant.subject.compiler?.compile.success === true
      : null;
  }
  if (name === "lint") {
    return caseResult.expected.lint
      ? variant.subject.compiler?.lint.success === true &&
          (variant.subject.compiler?.lint.summary?.errors ?? 0) === 0
      : null;
  }
  throw new Error(`unknown observable ${name}`);
}

export function isInfrastructureComplete(scorecard) {
  if (scorecard.summary?.case_count !== scorecard.summary?.scored_cases) {
    return false;
  }
  for (const caseResult of scorecard.cases) {
    const variant = currentVariant(caseResult);
    if (!variant?.subject?.execution?.success) {
      return false;
    }
  }
  return Object.values(scorecard.judges ?? {}).every(
    (judge) => judge?.success === true
  );
}

function aggregateWindow(entries) {
  if (entries.length === 0) {
    return null;
  }
  const summaries = entries.map((entry) => entry.scorecard.summary);
  return {
    run_count: entries.length,
    start_at: entries[0].scorecard.completed_at,
    end_at: entries.at(-1).scorecard.completed_at,
    normalized_score: average(
      summaries.map((summary) => summary.normalized_score)
    ),
    artifact_extraction_rate: average(
      summaries.map((summary) => summary.artifact_extraction_rate)
    ),
    compile_success_rate: average(
      summaries.map((summary) => summary.compile_success_rate)
    ),
    lint_success_rate: average(
      summaries.map((summary) => summary.lint_success_rate)
    ),
    safety_consent_pass_rate: average(
      summaries.map((summary) => summary.safety_consent_pass_rate)
    ),
    inconclusive_case_rate: average(
      summaries.map((summary) =>
        summary.case_count === 0
          ? null
          : summary.inconclusive_cases / summary.case_count
      )
    ),
    infrastructure_failure_rate: average(
      summaries.map((summary) => summary.infrastructure_failure_rate)
    ),
    average_duration_ms: average(
      summaries.map((summary) => summary.average_duration_ms)
    )
  };
}

function evaluateAlert(entries, config) {
  const recentCount = config.recent_window_runs;
  const minimumBaseline = config.minimum_baseline_runs;
  const recent = entries.slice(-recentCount);
  const baseline = entries.slice(
    Math.max(0, entries.length - recentCount - config.baseline_window_runs),
    Math.max(0, entries.length - recentCount)
  );
  if (recent.length < recentCount || baseline.length < minimumBaseline) {
    return {
      active: false,
      eligible: false,
      reason: "insufficient comparable history",
      recent_runs: recent.length,
      baseline_runs: baseline.length,
      semantic: null,
      hard_observables: []
    };
  }

  const baselineOverall = median(
    baseline.map((entry) => entry.scorecard.summary.normalized_score)
  );
  const semanticThreshold = config.semantic_decline_points / 100;
  const recentOverallDeclined =
    baselineOverall !== null &&
    recent.every((entry) => {
      const value = entry.scorecard.summary.normalized_score;
      return (
        typeof value === "number" &&
        value <= baselineOverall - semanticThreshold
      );
    });

  const caseIds = entries.at(-1).scorecard.cases.map((entry) => entry.case_id);
  const affectedCases = [];
  for (const caseId of caseIds) {
    const baselineCase = median(
      baseline.map((entry) => caseScore(entry.scorecard, caseId))
    );
    if (
      baselineCase !== null &&
      recent.every((entry) => {
        const value = caseScore(entry.scorecard, caseId);
        return (
          typeof value === "number" &&
          value <= baselineCase - semanticThreshold
        );
      })
    ) {
      affectedCases.push({
        case_id: caseId,
        baseline_median: baselineCase,
        recent_scores: recent.map((entry) =>
          caseScore(entry.scorecard, caseId)
        )
      });
    }
  }
  const semanticActive =
    recentOverallDeclined &&
    affectedCases.length >= config.minimum_affected_cases;

  const hardObservables = [];
  for (const caseId of caseIds) {
    for (const name of ["artifact", "compile", "lint"]) {
      const baselineValues = baseline
        .map((entry) => observable(entry.scorecard, caseId, name))
        .filter((value) => value !== null);
      const recentValues = recent.map((entry) =>
        observable(entry.scorecard, caseId, name)
      );
      const baselineRate = rate(baselineValues);
      if (
        baselineValues.length >= minimumBaseline &&
        baselineRate >= config.hard_observable_baseline_rate &&
        recentValues.every((value) => value === false)
      ) {
        hardObservables.push({
          case_id: caseId,
          observable: name,
          baseline_rate: baselineRate,
          recent_values: recentValues
        });
      }
    }
  }

  return {
    active: semanticActive || hardObservables.length > 0,
    eligible: true,
    reason: null,
    recent_runs: recent.length,
    baseline_runs: baseline.length,
    baseline_overall_median: baselineOverall,
    semantic: {
      active: semanticActive,
      threshold: semanticThreshold,
      affected_cases: affectedCases
    },
    hard_observables: hardObservables
  };
}

function caseTrends(entries) {
  const latest = entries.at(-1);
  if (!latest) {
    return [];
  }
  return latest.scorecard.cases.map((caseResult) => {
    const scores = entries.map((entry) =>
      caseScore(entry.scorecard, caseResult.case_id)
    );
    return {
      case_id: caseResult.case_id,
      suite: caseResult.suite,
      latest_score: scores.at(-1),
      seven_run_average: average(scores.slice(-7)),
      previous_seven_run_average: average(scores.slice(-14, -7)),
      compile_latest: observable(
        latest.scorecard,
        caseResult.case_id,
        "compile"
      ),
      lint_latest: observable(latest.scorecard, caseResult.case_id, "lint")
    };
  });
}

export function buildTrend({
  currentScorecard,
  historyEntries,
  config,
  now = new Date(),
  countCurrent = null
}) {
  validateScorecard(currentScorecard, "current scorecard");
  const shouldCountCurrent =
    countCurrent ?? currentScorecard.mode === "nightly";
  const currentCohort = cohortKey(currentScorecard);
  const previousSameCohort = historyEntries
    .filter((entry) => entry.cohort_key === currentCohort)
    .filter(
      (entry) =>
        entry.scorecard.run_id !== currentScorecard.run_id &&
        entry.scorecard.completed_at !== currentScorecard.completed_at
    )
    .sort(
      (left, right) =>
        Date.parse(left.scorecard.completed_at) -
        Date.parse(right.scorecard.completed_at)
    );
  const currentEntry = {
    scorecard: currentScorecard,
    source: "current",
    cohort_key: currentCohort
  };
  const allComparableCandidates = shouldCountCurrent
    ? [...previousSameCohort, currentEntry]
    : [...previousSameCohort];
  const comparable = allComparableCandidates.filter((entry) =>
    isInfrastructureComplete(entry.scorecard)
  );

  const currentAlert = evaluateAlert(comparable, config);
  const previousComparable = comparable.filter(
    (entry) => entry !== currentEntry
  );
  const previousAlert = evaluateAlert(previousComparable, config);
  const currentIncluded =
    shouldCountCurrent && comparable.includes(currentEntry);
  const alertActive = currentIncluded && currentAlert.active;
  const alertStarted = alertActive && !previousAlert.active;
  const alertRecovered =
    currentIncluded && !currentAlert.active && previousAlert.active;

  const latestSeven = comparable.slice(-7);
  const previousSeven = comparable.slice(-14, -7);
  const cohortBoundary =
    historyEntries.length > 0 &&
    historyEntries
      .sort(
        (left, right) =>
          Date.parse(right.scorecard.completed_at) -
          Date.parse(left.scorecard.completed_at)
      )[0]?.cohort_key !== currentCohort;

  return {
    schema_version: 1,
    generated_at: now.toISOString(),
    current_run: {
      run_id: currentScorecard.run_id,
      run_url: currentScorecard.run_url,
      completed_at: currentScorecard.completed_at,
      head_sha: currentScorecard.head_sha,
      infrastructure_complete: currentIncluded
    },
    cohort: {
      key: currentCohort,
      descriptor: cohortDescriptor(currentScorecard),
      comparable_previous_runs: previousSameCohort.length,
      comparable_complete_runs: comparable.length,
      boundary_from_previous_run: cohortBoundary
    },
    history: {
      loaded_runs: historyEntries.length,
      excluded_incomplete_runs:
        allComparableCandidates.length - comparable.length
    },
    windows: {
      latest_seven: aggregateWindow(latestSeven),
      previous_seven: aggregateWindow(previousSeven)
    },
    alert: {
      ...currentAlert,
      active: alertActive,
      started: alertStarted,
      recovered: alertRecovered,
      previous_active: previousAlert.active
    },
    counted_current_run: shouldCountCurrent,
    weekly_due: currentScorecard.mode === "nightly" && now.getUTCDay() === 1,
    case_trends: caseTrends(comparable)
  };
}

export async function fileExists(filePath) {
  try {
    await stat(filePath);
    return true;
  } catch (error) {
    if (error.code === "ENOENT") {
      return false;
    }
    throw error;
  }
}
