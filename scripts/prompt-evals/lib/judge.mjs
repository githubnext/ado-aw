import { mkdir, readFile, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";

import { buildCopilotArgs } from "./subject.mjs";
import { restrictedChildEnv, runProcess } from "./process.mjs";

async function writeJson(filePath, value) {
  await writeFile(filePath, `${JSON.stringify(value, null, 2)}\n`, "utf8");
}

export function extractJsonObject(text) {
  const trimmed = String(text ?? "").trim();
  if (trimmed.startsWith("{") && trimmed.endsWith("}")) {
    return JSON.parse(trimmed);
  }
  const fenced = trimmed.match(/```(?:json)?\s*\r?\n([\s\S]*?)```/i);
  if (!fenced) {
    throw new Error("judge response did not contain a JSON object");
  }
  return JSON.parse(fenced[1]);
}

function criterionPrompt(criterion) {
  return {
    id: criterion.id,
    weight: criterion.weight,
    question: criterion.question,
    anchors: {
      0: criterion.score_0,
      1: criterion.score_1,
      2: criterion.score_2
    }
  };
}

function subjectForJudge(subject) {
  return {
    variant: subject.variant,
    deterministic_observations: subject.observations,
    response: subject.response
  };
}

export function buildJudgePrompt({ suite, cases, subjectsByCase }) {
  const payload = cases.map((caseData) => ({
    case_id: caseData.id,
    description: caseData.description,
    expected: caseData.expected,
    ground_truth: caseData.ground_truth,
    request: caseData.request,
    criteria: caseData.criteria.map(criterionPrompt),
    variants: subjectsByCase.get(caseData.id).map(subjectForJudge)
  }));

  return [
    "# ado-aw prompt evaluation judge",
    "",
    "You are an independent evaluator. Score each response only against the supplied synthetic case, ground truth, deterministic observations, and rubric anchors.",
    "Do not use tools. Do not infer facts that are absent. A response that merely claims success without evidence must not receive credit for that claim.",
    "Score every criterion with integer 0, 1, or 2. Use status `inconclusive` only when the response or evidence is genuinely unavailable; otherwise use `scored`.",
    "Return JSON only, with no Markdown fence or commentary.",
    "",
    "Required schema:",
    JSON.stringify(
      {
        schema_version: 1,
        suite,
        cases: [
          {
            case_id: "case-id",
            variants: {
              variant_name: {
                status: "scored",
                criteria: [
                  {
                    id: "criterion-id",
                    score: 2,
                    evidence: "short exact excerpt or deterministic observation",
                    reason: "brief rubric-grounded explanation"
                  }
                ],
                summary: "brief assessment"
              }
            }
          }
        ]
      },
      null,
      2
    ),
    "",
    "<evaluation-payload>",
    JSON.stringify(payload, null, 2),
    "</evaluation-payload>"
  ].join("\n");
}

function validateVariantScore({
  raw,
  caseData,
  caseId,
  variant,
  judgeModel
}) {
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) {
    throw new Error(`${caseId}.${variant} must be an object`);
  }
  if (!["scored", "inconclusive"].includes(raw.status)) {
    throw new Error(`${caseId}.${variant}.status is invalid`);
  }
  if (typeof raw.summary !== "string" || raw.summary.trim() === "") {
    throw new Error(`${caseId}.${variant}.summary must be non-empty`);
  }
  if (raw.status === "inconclusive") {
    return {
      status: "inconclusive",
      summary: raw.summary.trim(),
      criteria: [],
      earned_points: null,
      available_points: null,
      normalized_score: null,
      judge_model: judgeModel
    };
  }
  if (!Array.isArray(raw.criteria)) {
    throw new Error(`${caseId}.${variant}.criteria must be an array`);
  }

  const rawCriteria = new Map();
  for (const criterion of raw.criteria) {
    if (!criterion || typeof criterion !== "object" || Array.isArray(criterion)) {
      throw new Error(`${caseId}.${variant} contains an invalid criterion`);
    }
    if (rawCriteria.has(criterion.id)) {
      throw new Error(
        `${caseId}.${variant} repeats criterion ${criterion.id}`
      );
    }
    rawCriteria.set(criterion.id, criterion);
  }

  const criteria = [];
  let earnedPoints = 0;
  let availablePoints = 0;
  for (const expectedCriterion of caseData.criteria) {
    const criterion = rawCriteria.get(expectedCriterion.id);
    if (!criterion) {
      throw new Error(
        `${caseId}.${variant} is missing criterion ${expectedCriterion.id}`
      );
    }
    if (![0, 1, 2].includes(criterion.score)) {
      throw new Error(
        `${caseId}.${variant}.${expectedCriterion.id}.score must be 0, 1, or 2`
      );
    }
    for (const key of ["evidence", "reason"]) {
      if (typeof criterion[key] !== "string" || criterion[key].trim() === "") {
        throw new Error(
          `${caseId}.${variant}.${expectedCriterion.id}.${key} must be non-empty`
        );
      }
    }
    const weight = expectedCriterion.weight;
    earnedPoints += criterion.score * weight;
    availablePoints += 2 * weight;
    criteria.push({
      id: expectedCriterion.id,
      score: criterion.score,
      weight,
      evidence: criterion.evidence.trim().slice(0, 600),
      reason: criterion.reason.trim().slice(0, 600)
    });
  }
  if (rawCriteria.size !== criteria.length) {
    const extras = [...rawCriteria.keys()].filter(
      (id) => !caseData.criteria.some((criterion) => criterion.id === id)
    );
    throw new Error(
      `${caseId}.${variant} contains unexpected criteria: ${extras.join(", ")}`
    );
  }

  return {
    status: "scored",
    summary: raw.summary.trim().slice(0, 800),
    criteria,
    earned_points: earnedPoints,
    available_points: availablePoints,
    normalized_score:
      availablePoints === 0 ? null : earnedPoints / availablePoints,
    judge_model: judgeModel
  };
}

export function validateJudgeResponse({
  raw,
  suite,
  cases,
  subjectsByCase,
  judgeModel
}) {
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) {
    throw new Error("judge response must be an object");
  }
  if (raw.schema_version !== 1 || raw.suite !== suite) {
    throw new Error("judge response schema_version or suite is invalid");
  }
  if (!Array.isArray(raw.cases)) {
    throw new Error("judge response cases must be an array");
  }
  const rawCases = new Map();
  for (const rawCase of raw.cases) {
    if (!rawCase || typeof rawCase !== "object" || Array.isArray(rawCase)) {
      throw new Error("judge response contains an invalid case");
    }
    if (rawCases.has(rawCase.case_id)) {
      throw new Error(`judge response repeats case ${rawCase.case_id}`);
    }
    rawCases.set(rawCase.case_id, rawCase);
  }

  const results = [];
  for (const caseData of cases) {
    const rawCase = rawCases.get(caseData.id);
    if (!rawCase) {
      throw new Error(`judge response is missing case ${caseData.id}`);
    }
    if (
      !rawCase.variants ||
      typeof rawCase.variants !== "object" ||
      Array.isArray(rawCase.variants)
    ) {
      throw new Error(`${caseData.id}.variants must be an object`);
    }

    const expectedSubjects = subjectsByCase.get(caseData.id);
    const expectedVariants = expectedSubjects.map((subject) => subject.variant);
    const rawVariants = Object.keys(rawCase.variants);
    const missing = expectedVariants.filter(
      (variant) => !rawVariants.includes(variant)
    );
    const extras = rawVariants.filter(
      (variant) => !expectedVariants.includes(variant)
    );
    if (missing.length || extras.length) {
      throw new Error(
        `${caseData.id}.variants mismatch; missing=${missing.join(",")} extras=${extras.join(",")}`
      );
    }

    const variants = {};
    for (const variant of expectedVariants) {
      variants[variant] = validateVariantScore({
        raw: rawCase.variants[variant],
        caseData,
        caseId: caseData.id,
        variant,
        judgeModel
      });
    }
    results.push({
      case_id: caseData.id,
      suite,
      variants
    });
  }
  if (rawCases.size !== results.length) {
    const extras = [...rawCases.keys()].filter(
      (id) => !cases.some((caseData) => caseData.id === id)
    );
    throw new Error(`judge response contains unexpected cases: ${extras.join(", ")}`);
  }
  return results;
}

export function compareVariants(base, head) {
  if (!base || !head || base.status !== "scored" || head.status !== "scored") {
    return {
      classification: "inconclusive",
      delta: null
    };
  }
  const delta = head.normalized_score - base.normalized_score;
  return {
    classification:
      delta > 0 ? "improved" : delta < 0 ? "regressed" : "unchanged",
    delta
  };
}

function inconclusiveSuite({ suite, cases, subjectsByCase, judgeModel, error }) {
  return cases.map((caseData) => {
    const variants = {};
    for (const subject of subjectsByCase.get(caseData.id)) {
      variants[subject.variant] = {
        status: "inconclusive",
        summary: `Judge failed: ${error}`,
        criteria: [],
        earned_points: null,
        available_points: null,
        normalized_score: null,
        judge_model: judgeModel
      };
    }
    return {
      case_id: caseData.id,
      suite,
      variants,
      judge_error: error
    };
  });
}

export async function runJudgeSuite({
  suite,
  cases,
  subjectsByCase,
  outputRoot,
  copilotPath,
  judgeModel,
  maxAiCredits,
  timeoutMs,
  maxOutputBytes,
  env = process.env,
  fakeDir = null
}) {
  const judgeDir = path.join(outputRoot, "judges", suite);
  const workDir = path.join(judgeDir, "work");
  const logDir = path.join(
    os.tmpdir(),
    "ado-aw-prompt-eval-logs",
    String(process.pid),
    "judges",
    suite
  );
  await mkdir(workDir, { recursive: true });
  await mkdir(logDir, { recursive: true });
  const prompt = buildJudgePrompt({ suite, cases, subjectsByCase });
  const promptPath = path.join(judgeDir, "prompt.txt");
  const responsePath = path.join(judgeDir, "response.json");
  await writeFile(promptPath, `${prompt.trimEnd()}\n`, "utf8");

  let execution;
  let response;
  if (fakeDir) {
    response = await readFile(
      path.join(fakeDir, "judges", `${suite}.json`),
      "utf8"
    );
    execution = {
      success: true,
      fake: true,
      code: 0,
      signal: null,
      timed_out: false,
      duration_ms: 0,
      stderr: "",
      output_truncated: false
    };
  } else {
    const processResult = await runProcess(
      copilotPath,
      buildCopilotArgs({
        promptPath,
        model: judgeModel,
        maxAiCredits,
        workDir,
        logDir
      }),
      {
        cwd: workDir,
        env: restrictedChildEnv(env),
        timeoutMs,
        maxOutputBytes
      }
    );
    response = processResult.stdout;
    execution = {
      ...processResult,
      stdout: undefined
    };
  }

  await writeFile(responsePath, response, "utf8");
  await writeFile(
    path.join(judgeDir, "stderr.txt"),
    execution.stderr ?? "",
    "utf8"
  );

  let results;
  let error = null;
  try {
    if (!execution.success) {
      throw new Error(execution.stderr || "judge process failed");
    }
    const raw = extractJsonObject(response);
    results = validateJudgeResponse({
      raw,
      suite,
      cases,
      subjectsByCase,
      judgeModel
    });
  } catch (judgeError) {
    error = judgeError.message;
    results = inconclusiveSuite({
      suite,
      cases,
      subjectsByCase,
      judgeModel,
      error
    });
  }

  const metadata = {
    suite,
    model: judgeModel,
    success: error === null,
    error,
    execution: {
      success: execution.success,
      fake: execution.fake ?? false,
      code: execution.code,
      signal: execution.signal,
      timed_out: execution.timed_out,
      duration_ms: execution.duration_ms,
      output_truncated: execution.output_truncated
    }
  };
  await writeJson(path.join(judgeDir, "result.json"), {
    metadata,
    cases: results
  });
  return { metadata, cases: results };
}
