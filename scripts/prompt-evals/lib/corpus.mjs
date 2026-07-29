import { createHash } from "node:crypto";
import { readFile, stat } from "node:fs/promises";
import path from "node:path";

export const PROMPT_FILES = {
  create: "prompts/create-ado-agentic-workflow.md",
  update: "prompts/update-ado-agentic-workflow.md",
  debug: "prompts/debug-ado-agentic-workflow.md"
};

export const ALL_SUITES = Object.freeze(Object.keys(PROMPT_FILES));

function assertObject(value, label) {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    throw new Error(`${label} must be an object`);
  }
}

function assertString(value, label) {
  if (typeof value !== "string" || value.trim() === "") {
    throw new Error(`${label} must be a non-empty string`);
  }
}

function assertStringArray(value, label) {
  if (!Array.isArray(value) || value.some((item) => typeof item !== "string")) {
    throw new Error(`${label} must be an array of strings`);
  }
}

export function resolveInside(root, relativePath, label = "path") {
  assertString(relativePath, label);
  if (path.isAbsolute(relativePath)) {
    throw new Error(`${label} must be relative: ${relativePath}`);
  }
  const resolvedRoot = path.resolve(root);
  const resolved = path.resolve(resolvedRoot, relativePath);
  const prefix = `${resolvedRoot}${path.sep}`;
  if (resolved !== resolvedRoot && !resolved.startsWith(prefix)) {
    throw new Error(`${label} escapes its root: ${relativePath}`);
  }
  return resolved;
}

export async function readJson(filePath) {
  const content = await readFile(filePath, "utf8");
  try {
    return JSON.parse(content);
  } catch (error) {
    throw new Error(`invalid JSON in ${filePath}: ${error.message}`);
  }
}

function validateRubric(rubric, filePath) {
  assertObject(rubric, `rubric ${filePath}`);
  if (rubric.schema_version !== 1) {
    throw new Error(`rubric ${filePath} has unsupported schema_version`);
  }
  assertString(rubric.id, `rubric ${filePath}.id`);
  if (!Array.isArray(rubric.criteria) || rubric.criteria.length === 0) {
    throw new Error(`rubric ${filePath}.criteria must not be empty`);
  }
  const ids = new Set();
  for (const [index, criterion] of rubric.criteria.entries()) {
    assertObject(criterion, `rubric ${filePath}.criteria[${index}]`);
    for (const key of [
      "id",
      "question",
      "score_0",
      "score_1",
      "score_2"
    ]) {
      assertString(
        criterion[key],
        `rubric ${filePath}.criteria[${index}].${key}`
      );
    }
    if (ids.has(criterion.id)) {
      throw new Error(`rubric ${filePath} repeats criterion ${criterion.id}`);
    }
    ids.add(criterion.id);
    if (
      typeof criterion.weight !== "number" ||
      !Number.isFinite(criterion.weight) ||
      criterion.weight <= 0
    ) {
      throw new Error(
        `rubric ${filePath}.criteria[${index}].weight must be positive`
      );
    }
  }
}

function validateCase(caseData, casePath) {
  assertObject(caseData, `case ${casePath}`);
  if (caseData.schema_version !== 1) {
    throw new Error(`case ${casePath} has unsupported schema_version`);
  }
  assertString(caseData.id, `case ${casePath}.id`);
  if (!ALL_SUITES.includes(caseData.prompt)) {
    throw new Error(`case ${casePath} has invalid prompt ${caseData.prompt}`);
  }
  assertString(caseData.description, `case ${casePath}.description`);
  assertString(caseData.request_file, `case ${casePath}.request_file`);
  assertStringArray(caseData.context_files, `case ${casePath}.context_files`);
  assertStringArray(caseData.rubric_files, `case ${casePath}.rubric_files`);
  if (caseData.rubric_files.length < 2) {
    throw new Error(`case ${casePath} must reference common and suite rubrics`);
  }
  assertObject(caseData.expected, `case ${casePath}.expected`);
  if (!["workflow", "clarification", "diagnostic"].includes(caseData.expected.outcome)) {
    throw new Error(`case ${casePath} has invalid expected.outcome`);
  }
  if (typeof caseData.expected.artifact_required !== "boolean") {
    throw new Error(`case ${casePath}.expected.artifact_required must be boolean`);
  }
  assertStringArray(
    caseData.expected.required_sections,
    `case ${casePath}.expected.required_sections`
  );
  assertObject(caseData.ground_truth, `case ${casePath}.ground_truth`);
}

export async function digestFiles(filePaths) {
  if (filePaths.length === 0) {
    return createHash("sha256").digest("hex");
  }
  const resolvedPaths = filePaths.map((filePath) => path.resolve(filePath));
  const directoryParts = resolvedPaths.map((filePath) =>
    path.dirname(filePath).split(path.sep)
  );
  const commonParts = [...directoryParts[0]];
  for (const parts of directoryParts.slice(1)) {
    while (
      commonParts.length > 0 &&
      commonParts.some((part, index) => part !== parts[index])
    ) {
      commonParts.pop();
    }
  }
  const commonRoot =
    commonParts.join(path.sep) || path.parse(resolvedPaths[0]).root;
  const hash = createHash("sha256");
  for (const filePath of [...resolvedPaths].sort()) {
    hash.update(path.relative(commonRoot, filePath).replaceAll("\\", "/"));
    hash.update("\0");
    hash.update(await readFile(filePath));
    hash.update("\0");
  }
  return hash.digest("hex");
}

export async function loadCorpus(repoRoot) {
  const fixtureRoot = path.join(repoRoot, "tests", "prompt-evals");
  const manifestPath = path.join(fixtureRoot, "manifest.json");
  const manifest = await readJson(manifestPath);
  assertObject(manifest, "prompt evaluation manifest");
  if (manifest.schema_version !== 1) {
    throw new Error("prompt evaluation manifest has unsupported schema_version");
  }
  if (
    !Number.isInteger(manifest.fixture_set_version) ||
    manifest.fixture_set_version < 1
  ) {
    throw new Error("fixture_set_version must be a positive integer");
  }
  assertStringArray(manifest.cases, "prompt evaluation manifest.cases");
  if (manifest.cases.length === 0) {
    throw new Error("prompt evaluation manifest must include cases");
  }

  const caseIds = new Set();
  const cases = [];
  const corpusFiles = [manifestPath];
  const rubricCache = new Map();

  for (const relativeCasePath of manifest.cases) {
    const casePath = resolveInside(fixtureRoot, relativeCasePath, "case path");
    const caseData = await readJson(casePath);
    validateCase(caseData, casePath);
    if (caseIds.has(caseData.id)) {
      throw new Error(`duplicate prompt evaluation case id ${caseData.id}`);
    }
    caseIds.add(caseData.id);

    const caseDir = path.dirname(casePath);
    const requestPath = resolveInside(
      caseDir,
      caseData.request_file,
      `${caseData.id}.request_file`
    );
    const request = await readFile(requestPath, "utf8");
    const contexts = [];
    for (const relativeContextPath of caseData.context_files) {
      const contextPath = resolveInside(
        caseDir,
        relativeContextPath,
        `${caseData.id}.context_files`
      );
      contexts.push({
        name: relativeContextPath.replaceAll("\\", "/"),
        content: await readFile(contextPath, "utf8"),
        path: contextPath
      });
      corpusFiles.push(contextPath);
    }

    const criteria = [];
    const criterionIds = new Set();
    const rubrics = [];
    for (const relativeRubricPath of caseData.rubric_files) {
      const rubricPath = resolveInside(
        fixtureRoot,
        relativeRubricPath,
        `${caseData.id}.rubric_files`
      );
      let rubric = rubricCache.get(rubricPath);
      if (!rubric) {
        rubric = await readJson(rubricPath);
        validateRubric(rubric, rubricPath);
        rubricCache.set(rubricPath, rubric);
      }
      rubrics.push(rubric);
      for (const criterion of rubric.criteria) {
        if (criterionIds.has(criterion.id)) {
          throw new Error(
            `case ${caseData.id} repeats criterion ${criterion.id}`
          );
        }
        criterionIds.add(criterion.id);
        criteria.push(criterion);
      }
      corpusFiles.push(rubricPath);
    }

    corpusFiles.push(casePath, requestPath);
    cases.push({
      ...caseData,
      case_path: casePath,
      case_dir: caseDir,
      request,
      request_path: requestPath,
      contexts,
      rubrics,
      criteria,
      case_digest: await digestFiles([
        casePath,
        requestPath,
        ...contexts.map((context) => context.path),
        ...caseData.rubric_files.map((relativeRubricPath) =>
          resolveInside(fixtureRoot, relativeRubricPath)
        )
      ])
    });
  }

  return {
    fixture_root: fixtureRoot,
    manifest,
    manifest_path: manifestPath,
    cases,
    fixture_set_digest: await digestFiles([...new Set(corpusFiles)]),
    rubric_digest: await digestFiles([...rubricCache.keys()])
  };
}

export function selectSuites(changedFiles = []) {
  const normalized = changedFiles.map((file) => file.replaceAll("\\", "/"));
  const selected = new Set();
  const selectAll = normalized.some(
    (file) =>
      file === "prompts/prompt-contract.md" ||
      file.startsWith("tests/prompt-evals/") ||
      file.startsWith("scripts/prompt-evals/") ||
      file === "tests/prompt_contract_tests.rs" ||
      file === "tests/prompt_eval_contract_tests.rs" ||
      file === ".github/workflows/prompt-evaluator.md"
  );
  if (selectAll) {
    return [...ALL_SUITES];
  }

  for (const [suite, promptFile] of Object.entries(PROMPT_FILES)) {
    if (normalized.includes(promptFile)) {
      selected.add(suite);
    }
  }
  return ALL_SUITES.filter((suite) => selected.has(suite));
}

export function composeSubjectPrompt({
  sharedContract,
  taskPrompt,
  caseData
}) {
  const context = caseData.contexts
    .map(
      ({ name, content }) =>
        `<context-file path="${name}">\n${content.trimEnd()}\n</context-file>`
    )
    .join("\n\n");

  return [
    "# Synthetic ado-aw prompt evaluation",
    "",
    "Execute the supplied authoring prompt exactly as if the scenario request were the user's message.",
    "This is an isolated synthetic case. You have no tools and must not claim to have read, changed, filed, posted, or executed anything outside the supplied text.",
    "Do not discuss the evaluation harness. Return the normal response required by the authoring prompt.",
    "",
    "<shared-prompt-contract>",
    sharedContract.trim(),
    "</shared-prompt-contract>",
    "",
    "<authoring-prompt>",
    taskPrompt.trim(),
    "</authoring-prompt>",
    "",
    "<user-request>",
    caseData.request.trim(),
    "</user-request>",
    context ? `\n<context-files>\n${context}\n</context-files>` : ""
  ]
    .filter((part) => part !== "")
    .join("\n");
}

export function extractWorkflowArtifact(response) {
  const text = String(response ?? "").trim();
  const candidates = [];
  const fencePattern = /```(?:markdown|md|yaml|yml)?\s*\r?\n([\s\S]*?)```/gi;
  for (const match of text.matchAll(fencePattern)) {
    candidates.push(match[1].trim());
  }
  if (text.startsWith("---")) {
    candidates.push(text);
  }
  return (
    candidates.find((candidate) => {
      if (!candidate.startsWith("---")) {
        return false;
      }
      const remainder = candidate.slice(3);
      return /\r?\n---\r?\n/.test(remainder);
    }) ?? null
  );
}

export function requiredSectionResults(response, requiredSections) {
  const text = String(response ?? "").toLowerCase();
  return requiredSections.map((section) => ({
    section,
    present:
      text.includes(`## ${section.toLowerCase()}`) ||
      text.includes(`### ${section.toLowerCase()}`)
  }));
}

export async function pathExists(filePath) {
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
