import assert from "node:assert/strict";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import { loadCorpus } from "../lib/corpus.mjs";
import {
  compareVariants,
  extractJsonObject,
  validateJudgeResponse
} from "../lib/judge.mjs";

const TEST_DIR = path.dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = path.resolve(TEST_DIR, "..", "..", "..");

function rawVariant(caseData, score) {
  return {
    status: "scored",
    criteria: caseData.criteria.map((criterion) => ({
      id: criterion.id,
      score,
      evidence: "Synthetic evidence",
      reason: "Matches the supplied rubric anchor"
    })),
    summary: "Synthetic assessment"
  };
}

test("validates strict judge results and computes normalized scores", async () => {
  const corpus = await loadCorpus(REPO_ROOT);
  const caseData = corpus.cases.find(
    (entry) => entry.id === "create-minimal-manual"
  );
  const subjectsByCase = new Map([
    [
      caseData.id,
      [
        { variant: "base" },
        { variant: "head" }
      ]
    ]
  ]);
  const raw = {
    schema_version: 1,
    suite: "create",
    cases: [
      {
        case_id: caseData.id,
        variants: {
          base: rawVariant(caseData, 1),
          head: rawVariant(caseData, 2)
        }
      }
    ]
  };
  const [validated] = validateJudgeResponse({
    raw,
    suite: "create",
    cases: [caseData],
    subjectsByCase,
    judgeModel: "synthetic-judge"
  });
  assert.equal(validated.variants.base.normalized_score, 0.5);
  assert.equal(validated.variants.head.normalized_score, 1);
  assert.deepEqual(
    compareVariants(validated.variants.base, validated.variants.head),
    { classification: "improved", delta: 0.5 }
  );
});

test("rejects missing or invented criteria", async () => {
  const corpus = await loadCorpus(REPO_ROOT);
  const caseData = corpus.cases.find(
    (entry) => entry.id === "debug-missing-evidence"
  );
  const subjectsByCase = new Map([[caseData.id, [{ variant: "current" }]]]);
  const incomplete = rawVariant(caseData, 2);
  incomplete.criteria.pop();
  assert.throws(
    () =>
      validateJudgeResponse({
        raw: {
          schema_version: 1,
          suite: "debug",
          cases: [
            {
              case_id: caseData.id,
              variants: { current: incomplete }
            }
          ]
        },
        suite: "debug",
        cases: [caseData],
        subjectsByCase,
        judgeModel: "synthetic-judge"
      }),
    /missing criterion/
  );
});

test("extracts raw and fenced judge JSON", () => {
  assert.deepEqual(extractJsonObject('{"schema_version":1}'), {
    schema_version: 1
  });
  assert.deepEqual(
    extractJsonObject("```json\n{\"schema_version\":1}\n```"),
    { schema_version: 1 }
  );
});

