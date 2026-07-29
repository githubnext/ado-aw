import assert from "node:assert/strict";
import test from "node:test";

import {
  readInfrastructureStatus,
  renderPrReport
} from "../report.mjs";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";

test("renders an advisory PR report", () => {
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
});

test("surfaces failed evaluator stages as degraded infrastructure", async (t) => {
  const root = await mkdtemp(path.join(os.tmpdir(), "prompt-eval-status-"));
  t.after(() => rm(root, { recursive: true, force: true }));
  await writeFile(path.join(root, "build-exit-code.txt"), "1\n", "utf8");
  await writeFile(path.join(root, "runner-exit-code.txt"), "0\n", "utf8");
  const status = await readInfrastructureStatus(root);
  assert.equal(status.degraded, true);
  assert.equal(status.checks.build.success, false);
  assert.equal(status.checks.runner.success, true);
});
