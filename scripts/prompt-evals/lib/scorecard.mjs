import { compareVariants } from "./judge.mjs";

function finiteAverage(values) {
  const filtered = values.filter(
    (value) => typeof value === "number" && Number.isFinite(value)
  );
  if (filtered.length === 0) {
    return null;
  }
  return filtered.reduce((sum, value) => sum + value, 0) / filtered.length;
}

function rate(values) {
  if (values.length === 0) {
    return null;
  }
  return values.filter(Boolean).length / values.length;
}

function criterionScore(score, criterionId) {
  if (!score || score.status !== "scored") {
    return null;
  }
  return score.criteria.find((criterion) => criterion.id === criterionId)?.score ?? null;
}

export function buildCaseScore({
  caseData,
  subjects,
  judgeCase,
  mode
}) {
  const variants = {};
  for (const subject of subjects) {
    variants[subject.variant] = {
      subject: {
        model: subject.model,
        execution: subject.execution,
        response_path: subject.response_path,
        response_length: subject.response_length,
        artifact: subject.artifact,
        required_sections: subject.required_sections,
        compiler: subject.compiler,
        observations: subject.observations
      },
      score: judgeCase.variants[subject.variant]
    };
  }

  const comparison =
    mode === "pr"
      ? compareVariants(
          judgeCase.variants.base ?? null,
          judgeCase.variants.head ?? null
        )
      : null;

  return {
    case_id: caseData.id,
    suite: caseData.prompt,
    case_digest: caseData.case_digest,
    description: caseData.description,
    expected: caseData.expected,
    variants,
    comparison
  };
}

export function summarizeScorecard(mode, cases) {
  if (mode === "pr") {
    const classifications = cases.map(
      (caseResult) => caseResult.comparison?.classification ?? "inconclusive"
    );
    return {
      case_count: cases.length,
      improved: classifications.filter((value) => value === "improved").length,
      unchanged: classifications.filter((value) => value === "unchanged").length,
      regressed: classifications.filter((value) => value === "regressed").length,
      inconclusive: classifications.filter((value) => value === "inconclusive")
        .length
    };
  }

  const current = cases
    .map((caseResult) => caseResult.variants.current)
    .filter(Boolean);
  const scores = current.map((variant) => variant.score);
  const compileExpected = cases.filter(
    (caseResult) => caseResult.expected.compile
  );
  const lintExpected = cases.filter((caseResult) => caseResult.expected.lint);
  const artifactExpected = cases.filter(
    (caseResult) => caseResult.expected.artifact_required
  );

  return {
    case_count: cases.length,
    scored_cases: scores.filter((score) => score.status === "scored").length,
    inconclusive_cases: scores.filter((score) => score.status !== "scored").length,
    normalized_score: finiteAverage(
      scores.map((score) => score.normalized_score)
    ),
    artifact_extraction_rate: rate(
      artifactExpected.map(
        (caseResult) => caseResult.variants.current.subject.artifact.found
      )
    ),
    compile_success_rate: rate(
      compileExpected.map(
        (caseResult) =>
          caseResult.variants.current.subject.compiler?.compile.success === true
      )
    ),
    lint_success_rate: rate(
      lintExpected.map(
        (caseResult) =>
          caseResult.variants.current.subject.compiler?.lint.success === true
      )
    ),
    safety_consent_pass_rate: rate(
      scores
        .map((score) => criterionScore(score, "safety_and_consent"))
        .filter((score) => score !== null)
        .map((score) => score === 2)
    ),
    infrastructure_failure_rate: rate(
      current.map(
        (variant) =>
          !variant.subject.execution.success ||
          variant.score.status === "inconclusive"
      )
    ),
    average_duration_ms: finiteAverage(
      current.map((variant) => variant.subject.execution.duration_ms)
    )
  };
}

export function summarizeSuites(mode, cases) {
  const suites = {};
  for (const suite of ["create", "update", "debug"]) {
    const suiteCases = cases.filter((caseResult) => caseResult.suite === suite);
    if (suiteCases.length > 0) {
      suites[suite] = summarizeScorecard(mode, suiteCases);
    }
  }
  return suites;
}

