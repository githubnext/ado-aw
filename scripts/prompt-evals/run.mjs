#!/usr/bin/env node

import { mkdir, readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

import {
  PROMPT_FILES,
  composeSubjectPrompt,
  loadCorpus,
  selectSuites
} from "./lib/corpus.mjs";
import { changedFiles, fileAtRef } from "./lib/git.mjs";
import { runJudgeSuite } from "./lib/judge.mjs";
import { runProcess } from "./lib/process.mjs";
import {
  buildCaseScore,
  summarizeScorecard,
  summarizeSuites
} from "./lib/scorecard.mjs";
import { runSubjectVariant, runToolFreeProbe } from "./lib/subject.mjs";

const SCRIPT_DIR = path.dirname(fileURLToPath(import.meta.url));
const VARIANTS = ["base", "head"];

function parseArgs(argv) {
  const result = {};
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (!arg.startsWith("--")) {
      throw new Error(`unexpected argument: ${arg}`);
    }
    const equals = arg.indexOf("=");
    if (equals !== -1) {
      result[arg.slice(2, equals)] = arg.slice(equals + 1);
      continue;
    }
    const key = arg.slice(2);
    const value = argv[index + 1];
    if (!value || value.startsWith("--")) {
      result[key] = true;
      continue;
    }
    result[key] = value;
    index += 1;
  }
  return result;
}

function requireArg(args, name) {
  const value = args[name];
  if (typeof value !== "string" || value.trim() === "") {
    throw new Error(`--${name} is required`);
  }
  return value;
}

async function writeJson(filePath, value) {
  await writeFile(filePath, `${JSON.stringify(value, null, 2)}\n`, "utf8");
}

async function readConfig(configPath) {
  const parsed = JSON.parse(await readFile(configPath, "utf8"));
  if (parsed.schema_version !== 1) {
    throw new Error("prompt evaluator config has unsupported schema_version");
  }
  return parsed;
}

export async function resolveEngineConstants(repoRoot) {
  const source = await readFile(path.join(repoRoot, "src", "engine.rs"), "utf8");
  const subjectModel = source.match(
    /pub const DEFAULT_COPILOT_MODEL: &str = "([^"]+)";/
  )?.[1];
  const cliVersion = source.match(
    /pub const COPILOT_CLI_VERSION: &str = "([^"]+)";/
  )?.[1];
  if (!subjectModel || !cliVersion) {
    throw new Error("failed to resolve Copilot model/version from src/engine.rs");
  }
  return {
    subject_model: subjectModel,
    copilot_cli_version: cliVersion
  };
}

async function validateCatalog({
  adoAwPath,
  subjectModel,
  judgeModel,
  cliVersion,
  repoRoot,
  timeoutMs,
  maxOutputBytes
}) {
  const modelsResult = await runProcess(
    adoAwPath,
    ["catalog", "--kind", "models", "--json"],
    { cwd: repoRoot, timeoutMs, maxOutputBytes }
  );
  if (!modelsResult.success) {
    throw new Error(`ado-aw model catalog failed: ${modelsResult.stderr}`);
  }
  const models = JSON.parse(modelsResult.stdout).models;
  for (const model of [subjectModel, judgeModel]) {
    if (!Array.isArray(models) || !models.includes(model)) {
      throw new Error(`model ${model} is absent from the ado-aw catalog`);
    }
  }

  const versionsResult = await runProcess(
    adoAwPath,
    ["catalog", "--kind", "versions", "--json"],
    { cwd: repoRoot, timeoutMs, maxOutputBytes }
  );
  if (!versionsResult.success) {
    throw new Error(`ado-aw version catalog failed: ${versionsResult.stderr}`);
  }
  const catalogVersion = JSON.parse(versionsResult.stdout).versions?.copilot_cli;
  if (catalogVersion !== cliVersion) {
    throw new Error(
      `Copilot CLI version mismatch: source=${cliVersion} catalog=${catalogVersion}`
    );
  }
}

function stripResponses(subjects) {
  return subjects.map((subject) => ({
    ...subject,
    response: undefined
  }));
}

export async function mapLimit(items, limit, worker) {
  if (!Number.isInteger(limit) || limit < 1) {
    throw new Error("concurrency limit must be a positive integer");
  }
  const results = new Array(items.length);
  let nextIndex = 0;
  async function consume() {
    while (true) {
      const index = nextIndex;
      nextIndex += 1;
      if (index >= items.length) {
        return;
      }
      results[index] = await worker(items[index], index);
    }
  }
  await Promise.all(
    Array.from({ length: Math.min(limit, items.length) }, () => consume())
  );
  return results;
}

export async function runEvaluation(options) {
  const startedAt = new Date().toISOString();
  const repoRoot = path.resolve(options.repoRoot);
  const outputRoot = path.resolve(options.outputRoot);
  const config = await readConfig(options.configPath);
  const timeoutMs = config.session_timeout_seconds * 1000;
  const maxOutputBytes = config.max_output_bytes;
  await mkdir(outputRoot, { recursive: true });

  const corpus = await loadCorpus(repoRoot);
  const engine = await resolveEngineConstants(repoRoot);
  const headSha = options.headSha;
  if (!headSha) {
    throw new Error("paired prompt evaluation requires --head-sha");
  }
  const baseSha = options.baseSha;
  if (!baseSha) {
    throw new Error("paired prompt evaluation requires --base-sha");
  }

  const changed =
    options.changedFiles ?? (await changedFiles(repoRoot, baseSha, headSha));
  const suites = selectSuites(changed);
  const metadata = {
    schema_version: 1,
    mode: "pr",
    event_name: options.eventName,
    repository: options.repository,
    run_id: options.runId,
    run_url: options.runUrl,
    started_at: startedAt,
    base_sha: baseSha,
    head_sha: headSha,
    changed_files: changed,
    suites,
    fixture_set_version: corpus.manifest.fixture_set_version,
    fixture_set_digest: corpus.fixture_set_digest,
    rubric_digest: corpus.rubric_digest,
    subject_model: engine.subject_model,
    judge_model: config.judge_model,
    copilot_cli_version: engine.copilot_cli_version
  };
  await writeJson(path.join(outputRoot, "run-metadata.json"), metadata);

  if (suites.length === 0) {
    const completedAt = new Date().toISOString();
    const emptyScorecard = {
      schema_version: 1,
      ...metadata,
      completed_at: completedAt,
      cases: [],
      summary: summarizeScorecard([]),
      suites: {}
    };
    await writeJson(path.join(outputRoot, "scorecard.json"), emptyScorecard);
    await writeJson(path.join(outputRoot, "manifest.json"), {
      ...metadata,
      completed_at: completedAt,
      status: "no-suites-selected",
      scorecard_path: path.join(outputRoot, "scorecard.json")
    });
    return emptyScorecard;
  }

  if (!options.fakeDir) {
    await validateCatalog({
      adoAwPath: options.adoAwPath,
      subjectModel: engine.subject_model,
      judgeModel: config.judge_model,
      cliVersion: engine.copilot_cli_version,
      repoRoot,
      timeoutMs,
      maxOutputBytes
    });
  }

  await runToolFreeProbe({
    copilotPath: options.copilotPath,
    model: engine.subject_model,
    maxAiCredits: config.subject_max_ai_credits,
    timeoutMs,
    maxOutputBytes,
    outputDir: path.join(outputRoot, "probe"),
    env: options.env,
    fakeDir: options.fakeDir
  });

  const sharedByVariant = new Map();
  const promptBySuiteVariant = new Map();
  for (const variant of VARIANTS) {
    const ref = variant === "base" ? baseSha : headSha;
    const shared = await fileAtRef(
      repoRoot,
      ref,
      "prompts/prompt-contract.md"
    );
    if (shared === null) {
      throw new Error(`shared prompt contract is absent for variant ${variant}`);
    }
    sharedByVariant.set(variant, shared);
    for (const suite of suites) {
      promptBySuiteVariant.set(
        `${suite}:${variant}`,
        await fileAtRef(repoRoot, ref, PROMPT_FILES[suite])
      );
    }
  }

  const subjectsByCase = new Map();
  const subjectTasks = [];
  for (const suite of suites) {
    for (const caseData of corpus.cases.filter(
      (entry) => entry.prompt === suite
    )) {
      subjectsByCase.set(caseData.id, []);
      for (const variant of VARIANTS) {
        const taskPrompt = promptBySuiteVariant.get(`${suite}:${variant}`);
        if (taskPrompt !== null) {
          subjectTasks.push({ caseData, variant, taskPrompt });
        }
      }
    }
  }

  const subjectResults = await mapLimit(
    subjectTasks,
    config.subject_concurrency,
    async ({ caseData, variant, taskPrompt }) => {
      const prompt = composeSubjectPrompt({
        sharedContract: sharedByVariant.get(variant),
        taskPrompt,
        caseData
      });
      return runSubjectVariant({
        caseData,
        variant,
        prompt,
        outputRoot,
        copilotPath: options.copilotPath,
        adoAwPath: options.adoAwPath,
        model: engine.subject_model,
        maxAiCredits: config.subject_max_ai_credits,
        timeoutMs,
        maxOutputBytes,
        env: options.env,
        fakeDir: options.fakeDir
      });
    }
  );
  for (const subject of subjectResults) {
    subjectsByCase.get(subject.case_id).push(subject);
  }
  for (const subjects of subjectsByCase.values()) {
    subjects.sort(
      (left, right) =>
        VARIANTS.indexOf(left.variant) - VARIANTS.indexOf(right.variant)
    );
  }

  const judgeCasesById = new Map();
  const judgeMetadata = {};
  const judgeResults = await mapLimit(
    suites,
    config.judge_concurrency,
    async (suite) => {
      const suiteCases = corpus.cases.filter(
        (caseData) => caseData.prompt === suite
      );
      return runJudgeSuite({
        suite,
        cases: suiteCases,
        subjectsByCase,
        outputRoot,
        copilotPath: options.copilotPath,
        judgeModel: config.judge_model,
        maxAiCredits: config.judge_max_ai_credits,
        timeoutMs,
        maxOutputBytes,
        env: options.env,
        fakeDir: options.fakeDir
      });
    }
  );
  for (const [index, judge] of judgeResults.entries()) {
    const suite = suites[index];
    judgeMetadata[suite] = judge.metadata;
    for (const judgeCase of judge.cases) {
      judgeCasesById.set(judgeCase.case_id, judgeCase);
    }
  }

  const caseResults = corpus.cases
    .filter((caseData) => suites.includes(caseData.prompt))
    .map((caseData) =>
      buildCaseScore({
        caseData,
        subjects: subjectsByCase.get(caseData.id),
        judgeCase: judgeCasesById.get(caseData.id)
      })
    );
  const completedAt = new Date().toISOString();
  const scorecard = {
    schema_version: 1,
    ...metadata,
    completed_at: completedAt,
    cases: caseResults,
    summary: summarizeScorecard(caseResults),
    suites: summarizeSuites(caseResults),
    judges: judgeMetadata
  };
  await writeJson(path.join(outputRoot, "scorecard.json"), scorecard);
  await writeJson(path.join(outputRoot, "manifest.json"), {
    ...metadata,
    completed_at: completedAt,
    status: "completed",
    scorecard_path: path.join(outputRoot, "scorecard.json"),
    case_results: [...subjectsByCase.entries()].map(([caseId, subjects]) => ({
      case_id: caseId,
      variants: stripResponses(subjects)
    }))
  });
  return scorecard;
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  const repoRoot = path.resolve(args["repo-root"] ?? process.cwd());
  const outputRoot = path.resolve(
    args.output ?? path.join(repoRoot, ".prompt-eval-output")
  );
  const configPath = path.resolve(
    args.config ?? path.join(SCRIPT_DIR, "config.json")
  );

  try {
    await runEvaluation({
      repoRoot,
      outputRoot,
      configPath,
      baseSha: requireArg(args, "base-sha"),
      headSha: requireArg(args, "head-sha"),
      copilotPath: args.copilot ?? "copilot",
      adoAwPath: args["ado-aw"] ?? "ado-aw",
      fakeDir: args["fake-dir"] ? path.resolve(args["fake-dir"]) : null,
      eventName: args["event-name"] ?? process.env.GITHUB_EVENT_NAME ?? "pull_request",
      repository: args.repository ?? process.env.GITHUB_REPOSITORY ?? null,
      runId: args["run-id"] ?? process.env.GITHUB_RUN_ID ?? null,
      runUrl: args["run-url"] ?? null,
      env: process.env
    });
  } catch (error) {
    await mkdir(outputRoot, { recursive: true });
    await writeJson(path.join(outputRoot, "manifest.json"), {
      schema_version: 1,
      mode: "pr",
      status: "infrastructure-failure",
      completed_at: new Date().toISOString(),
      error: error.stack ?? error.message
    });
    throw error;
  }
}

if (path.resolve(process.argv[1] ?? "") === fileURLToPath(import.meta.url)) {
  main().catch((error) => {
    console.error(error.stack ?? error.message);
    process.exitCode = 1;
  });
}
