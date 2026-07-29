import assert from "node:assert/strict";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import {
  buildTrend,
  cohortKey,
  isInfrastructureComplete,
  loadHistoryScorecards
} from "../lib/history.mjs";

const CONFIG = {
  max_runs: 30,
  minimum_baseline_runs: 7,
  baseline_window_runs: 14,
  recent_window_runs: 3,
  semantic_decline_points: 10,
  minimum_affected_cases: 2,
  hard_observable_baseline_rate: 0.8
};

function caseResult(id, score, { compile = true, lint = true } = {}) {
  const criteria = [
    "task_completion",
    "grounding",
    "safety_and_consent",
    "clarity_and_done_criteria"
  ].map((criterion) => ({
    id: criterion,
    score: Math.round(score * 2),
    weight: 1,
    evidence: "synthetic",
    reason: "synthetic"
  }));
  return {
    case_id: id,
    suite: "create",
    expected: {
      artifact_required: true,
      compile: true,
      lint: true
    },
    variants: {
      current: {
        subject: {
          execution: { success: true, duration_ms: 100 },
          artifact: { found: true },
          compiler: {
            compile: { success: compile },
            lint: { success: lint, summary: { errors: lint ? 0 : 1 } }
          }
        },
        score: {
          status: "scored",
          normalized_score: score,
          criteria
        }
      }
    }
  };
}

function scorecard(index, score, options = {}) {
  const completed = new Date(Date.UTC(2026, 6, index + 1)).toISOString();
  const cases = [
    caseResult("case-a", score, options),
    caseResult("case-b", score, options),
    caseResult("case-c", score, options)
  ];
  return {
    schema_version: 1,
    mode: "nightly",
    run_id: String(index),
    run_url: `https://example.invalid/runs/${index}`,
    head_sha: `sha-${index}`,
    completed_at: completed,
    fixture_set_version: 1,
    fixture_set_digest: options.fixtureDigest ?? "fixtures-v1",
    rubric_digest: "rubrics-v1",
    evaluator_digest: options.evaluatorDigest ?? "evaluator-v1",
    subject_model: "subject-v1",
    judge_model: options.judgeModel ?? "judge-v1",
    copilot_cli_version: "1.0.0",
    cases,
    summary: {
      case_count: 3,
      scored_cases: options.infrastructureFailure ? 2 : 3,
      inconclusive_cases: options.infrastructureFailure ? 1 : 0,
      normalized_score: score,
      artifact_extraction_rate: 1,
      compile_success_rate: options.compile === false ? 0 : 1,
      lint_success_rate: options.lint === false ? 0 : 1,
      safety_consent_pass_rate: score >= 1 ? 1 : 0,
      infrastructure_failure_rate: options.infrastructureFailure ? 1 / 3 : 0,
      average_duration_ms: 100
    },
    judges: {
      create: { success: !options.infrastructureFailure }
    }
  };
}

function entries(scorecards) {
  return scorecards.map((value) => ({
    scorecard: value,
    source: `run-${value.run_id}`,
    cohort_key: cohortKey(value)
  }));
}

test("starts an alert only on the third sustained decline", () => {
  const baseline = Array.from({ length: 7 }, (_, index) =>
    scorecard(index, 1)
  );
  const firstTwoLow = [
    scorecard(7, 0.7),
    scorecard(8, 0.7)
  ];
  const current = scorecard(9, 0.7);
  const trend = buildTrend({
    currentScorecard: current,
    historyEntries: entries([...baseline, ...firstTwoLow]),
    config: CONFIG,
    now: new Date("2026-07-27T02:00:00Z")
  });
  assert.equal(trend.alert.active, true);
  assert.equal(trend.alert.started, true);
  assert.equal(trend.alert.semantic.affected_cases.length, 3);
});

test("does not repost an already-active alert", () => {
  const baseline = Array.from({ length: 7 }, (_, index) =>
    scorecard(index, 1)
  );
  const priorLow = [
    scorecard(7, 0.7),
    scorecard(8, 0.7),
    scorecard(9, 0.7)
  ];
  const current = scorecard(10, 0.7);
  const trend = buildTrend({
    currentScorecard: current,
    historyEntries: entries([...baseline, ...priorLow]),
    config: CONFIG,
    now: new Date("2026-07-28T02:00:00Z")
  });
  assert.equal(trend.alert.active, true);
  assert.equal(trend.alert.started, false);
  assert.equal(trend.alert.previous_active, true);
});

test("new cohorts and incomplete runs cannot trigger regressions", () => {
  const baseline = Array.from({ length: 10 }, (_, index) =>
    scorecard(index, 1)
  );
  const newCohort = scorecard(10, 0.1, { judgeModel: "judge-v2" });
  const cohortTrend = buildTrend({
    currentScorecard: newCohort,
    historyEntries: entries(baseline),
    config: CONFIG
  });
  assert.equal(cohortTrend.alert.active, false);
  assert.equal(cohortTrend.cohort.comparable_previous_runs, 0);

  const incomplete = scorecard(10, 0.1, { infrastructureFailure: true });
  assert.equal(isInfrastructureComplete(incomplete), false);
  const incompleteTrend = buildTrend({
    currentScorecard: incomplete,
    historyEntries: entries(baseline),
    config: CONFIG
  });
  assert.equal(incompleteTrend.alert.active, false);
  assert.equal(incompleteTrend.current_run.infrastructure_complete, false);
});

test("detects sustained hard-observable failures", () => {
  const baseline = Array.from({ length: 7 }, (_, index) =>
    scorecard(index, 1)
  );
  const firstTwo = [
    scorecard(7, 1, { compile: false }),
    scorecard(8, 1, { compile: false })
  ];
  const current = scorecard(9, 1, { compile: false });
  const trend = buildTrend({
    currentScorecard: current,
    historyEntries: entries([...baseline, ...firstTwo]),
    config: CONFIG
  });
  assert.equal(trend.alert.active, true);
  assert.ok(
    trend.alert.hard_observables.some(
      (entry) => entry.observable === "compile"
    )
  );
});

test("loads valid scorecards and rejects corrupt history", async (t) => {
  const root = await mkdtemp(path.join(os.tmpdir(), "prompt-eval-history-"));
  t.after(() => rm(root, { recursive: true, force: true }));
  await mkdir(path.join(root, "good"), { recursive: true });
  await mkdir(path.join(root, "bad"), { recursive: true });
  await writeFile(
    path.join(root, "good", "scorecard.json"),
    JSON.stringify(scorecard(1, 1)),
    "utf8"
  );
  await writeFile(
    path.join(root, "bad", "scorecard.json"),
    "{not-json",
    "utf8"
  );
  const history = await loadHistoryScorecards(root, 30);
  assert.equal(history.accepted.length, 1);
  assert.equal(history.rejected.length, 1);
});
