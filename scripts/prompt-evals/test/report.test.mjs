import assert from "node:assert/strict";
import test from "node:test";

import {
  decideReportAction,
  readInfrastructureStatus,
  renderDiscussionReport,
  renderPrReport
} from "../report.mjs";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";

test("selects PR, weekly, transition, and noop report actions", () => {
  assert.equal(
    decideReportAction({ eventName: "pull_request" }).action,
    "pr-comment"
  );
  assert.equal(
    decideReportAction({
      eventName: "schedule",
      trend: { weekly_due: true, alert: { active: false, started: false } }
    }).action,
    "discussion"
  );
  assert.equal(
    decideReportAction({
      eventName: "schedule",
      trend: { weekly_due: false, alert: { active: true, started: true } }
    }).action,
    "discussion"
  );
  assert.equal(
    decideReportAction({
      eventName: "schedule",
      trend: { weekly_due: false, alert: { active: true, started: false } }
    }).action,
    "noop"
  );
});

test("renders advisory PR and continuous discussion reports", () => {
  const score = {
    status: "scored",
    normalized_score: 0.75,
    criteria: [
      {
        id: "task_completion",
        score: 1,
        evidence: "evidence",
        reason: "reason"
      }
    ]
  };
  const scorecard = {
    base_sha: "base-sha-1234567890",
    head_sha: "head-sha-1234567890",
    subject_model: "subject",
    judge_model: "judge",
    copilot_cli_version: "1.0.0",
    completed_at: "2026-07-27T02:00:00Z",
    run_id: "42",
    run_url: "https://example.invalid/runs/42",
    suites: {
      create: {
        case_count: 1,
        improved: 0,
        unchanged: 0,
        regressed: 1,
        inconclusive: 0,
        normalized_score: 0.75,
        compile_success_rate: 1,
        lint_success_rate: 1,
        safety_consent_pass_rate: 1,
        inconclusive_cases: 0
      }
    },
    cases: [
      {
        case_id: "create-case",
        suite: "create",
        comparison: { classification: "regressed", delta: -0.25 },
        variants: {
          base: {
            score: {
              ...score,
              normalized_score: 1,
              criteria: [{ ...score.criteria[0], score: 2 }]
            }
          },
          head: { score }
        }
      }
    ]
  };
  const pr = renderPrReport(scorecard);
  assert.match(pr, /Semantic results are advisory/);
  assert.match(pr, /create-case/);
  assert.match(pr, /-25.0 pp/);

  const discussion = renderDiscussionReport(
    scorecard,
    {
      windows: {
        latest_seven: { normalized_score: 0.75 },
        previous_seven: { normalized_score: 1 }
      },
      alert: {
        active: true,
        eligible: true,
        semantic: {
          affected_cases: [
            {
              case_id: "create-case",
              baseline_median: 1,
              recent_scores: [0.75, 0.75, 0.75]
            }
          ]
        },
        hard_observables: []
      },
      cohort: { key: "cohort", comparable_previous_runs: 10 },
      history: { excluded_incomplete_runs: 0 },
      case_trends: []
    },
    "new sustained regression"
  );
  assert.match(discussion, /Sustained regression/);
  assert.match(discussion, /create-case/);
});

test("surfaces failed evaluator stages as degraded infrastructure", async (t) => {
  const root = await mkdtemp(path.join(os.tmpdir(), "prompt-eval-status-"));
  t.after(() => rm(root, { recursive: true, force: true }));
  await writeFile(path.join(root, "history-exit-code.txt"), "1\n", "utf8");
  await writeFile(path.join(root, "runner-exit-code.txt"), "0\n", "utf8");
  const status = await readInfrastructureStatus(root);
  assert.equal(status.degraded, true);
  assert.equal(status.checks.history.success, false);
  assert.equal(status.checks.runner.success, true);
});
