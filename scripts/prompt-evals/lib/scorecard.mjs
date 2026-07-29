import { compareVariants } from "./judge.mjs";

export function buildCaseScore({ caseData, subjects, judgeCase }) {
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

  return {
    case_id: caseData.id,
    suite: caseData.prompt,
    case_digest: caseData.case_digest,
    description: caseData.description,
    expected: caseData.expected,
    variants,
    comparison: compareVariants(
      judgeCase.variants.base ?? null,
      judgeCase.variants.head ?? null
    )
  };
}

export function summarizeScorecard(cases) {
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

export function summarizeSuites(cases) {
  const suites = {};
  for (const suite of ["create", "update", "debug"]) {
    const suiteCases = cases.filter((caseResult) => caseResult.suite === suite);
    if (suiteCases.length > 0) {
      suites[suite] = summarizeScorecard(suiteCases);
    }
  }
  return suites;
}
