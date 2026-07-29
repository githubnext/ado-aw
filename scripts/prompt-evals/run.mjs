#!/usr/bin/env node

import { createHash } from "node:crypto";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

import {
  ALL_SUITES,
  PROMPT_FILES,
  composeSubjectPrompt,
  digestFiles,
  listFilesRecursive,
  loadCorpus,
  selectSuites
} from "./lib/corpus.mjs";
import { changedFiles, currentFile, currentSha, fileAtRef } from "./lib/git.mjs";
import { runJudgeSuite } from "./lib/judge.mjs";
import { runProcess } from "./lib/process.mjs";
import {
  buildCaseScore,
  summarizeScorecard,
  summarizeSuites
} from "./lib/scorecard.mjs";
import { runSubjectVariant, runToolFreeProbe } from "./lib/subject.mjs";

const SCRIPT_DIR = path.dirname(fileURLToPath(import.meta.url));

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

function sha256(text) {
  return createHash("sha256").update(text).digest("hex");
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

async function promptAt({
  repoRoot,
  mode,
  ref,
  relativePath
}) {
  if (mode === "pr") {
    return fileAtRef(repoRoot, ref, relativePath);
  }
  return currentFile(repoRoot, relativePath);
}

function stripResponses(subjects) {
  return subjects.map((subject) => ({
    ...subject,
    response: undefined
  }));
}

function variantsForMode(mode) {
  return mode === "pr" ? ["base", "head"] : ["current"];
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
  const evaluatorScriptRoot = path.join(repoRoot, "scripts", "prompt-evals");
  const evaluatorFiles = (await listFilesRecursive(evaluatorScriptRoot)).filter(
    (filePath) =>
      !filePath.includes(`${path.sep}test${path.sep}`) &&
      (filePath.endsWith(".mjs") || filePath.endsWith("config.json"))
  );
  evaluatorFiles.push(
    path.join(repoRoot, ".github", "workflows", "prompt-evaluator.md")
  );
  const evaluatorDigest = await digestFiles(evaluatorFiles);
  const engine = await resolveEngineConstants(repoRoot);
  const headSha =
    options.headSha ?? (await currentSha(repoRoot));
  const baseSha = options.mode === "pr" ? options.baseSha : null;
  if (options.mode === "pr" && !baseSha) {
    throw new Error("PR mode requires --base-sha");
  }

  const changed =
    options.mode === "pr"
      ? options.changedFiles ??
        (await changedFiles(repoRoot, baseSha, headSha))
      : [];
  const suites = selectSuites(options.mode, changed);
  const metadata = {
    schema_version: 1,
    mode: options.mode,
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
    evaluator_digest: evaluatorDigest,
    subject_model: engine.subject_model,
    judge_model: config.judge_model,
    copilot_cli_version: engine.copilot_cli_version,
    config_digest: sha256(JSON.stringify(config))
  };
  await writeJson(path.join(outputRoot, "run-metadata.json"), metadata);

  if (suites.length === 0) {
    const emptyScorecard = {
      schema_version: 1,
      ...metadata,
      completed_at: new Date().toISOString(),
      cases: [],
      summary: summarizeScorecard(options.mode, []),
      suites: {}
    };
    await writeJson(path.join(outputRoot, "scorecard.json"), emptyScorecard);
    await writeJson(path.join(outputRoot, "manifest.json"), {
      ...metadata,
      completed_at: emptyScorecard.completed_at,
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
  for (const variant of variantsForMode(options.mode)) {
    const ref =
      variant === "base" ? baseSha : variant === "head" ? headSha : null;
    const shared = await promptAt({
      repoRoot,
      mode: options.mode,
      ref,
      relativePath: "prompts/prompt-contract.md"
    });
    if (shared === null) {
      throw new Error(`shared prompt contract is absent for variant ${variant}`);
    }
    sharedByVariant.set(variant, shared);
    for (const suite of suites) {
      const taskPrompt = await promptAt({
        repoRoot,
        mode: options.mode,
        ref,
        relativePath: PROMPT_FILES[suite]
      });
      promptBySuiteVariant.set(`${suite}:${variant}`, taskPrompt);
    }
  }

  const subjectsByCase = new Map();
  const judgeCasesById = new Map();
  const judgeMetadata = {};

  const subjectTasks = [];
  for (const suite of suites) {
    for (const caseData of corpus.cases.filter(
      (entry) => entry.prompt === suite
    )) {
      subjectsByCase.set(caseData.id, []);
      for (const variant of variantsForMode(options.mode)) {
        const taskPrompt = promptBySuiteVariant.get(`${suite}:${variant}`);
        if (taskPrompt !== null) {
          subjectTasks.push({ suite, caseData, variant, taskPrompt });
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
        variantsForMode(options.mode).indexOf(left.variant) -
        variantsForMode(options.mode).indexOf(right.variant)
    );
  }

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
        judgeCase: judgeCasesById.get(caseData.id),
        mode: options.mode
      })
    );
  const completedAt = new Date().toISOString();
  const scorecard = {
    schema_version: 1,
    ...metadata,
    completed_at: completedAt,
    cases: caseResults,
    summary: summarizeScorecard(options.mode, caseResults),
    suites: summarizeSuites(options.mode, caseResults),
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
  const mode = requireArg(args, "mode");
  if (!["pr", "nightly", "manual"].includes(mode)) {
    throw new Error("--mode must be pr, nightly, or manual");
  }
  const repoRoot = path.resolve(args["repo-root"] ?? process.cwd());
  const outputRoot = path.resolve(
    args.output ?? path.join(repoRoot, ".prompt-eval-output")
  );
  const configPath = path.resolve(
    args.config ?? path.join(SCRIPT_DIR, "config.json")
  );

  try {
    await runEvaluation({
      mode,
      repoRoot,
      outputRoot,
      configPath,
      baseSha: args["base-sha"],
      headSha: args["head-sha"],
      copilotPath: args.copilot ?? "copilot",
      adoAwPath: args["ado-aw"] ?? "ado-aw",
      fakeDir: args["fake-dir"] ? path.resolve(args["fake-dir"]) : null,
      eventName: args["event-name"] ?? process.env.GITHUB_EVENT_NAME ?? mode,
      repository: args.repository ?? process.env.GITHUB_REPOSITORY ?? null,
      runId: args["run-id"] ?? process.env.GITHUB_RUN_ID ?? null,
      runUrl: args["run-url"] ?? null,
      env: process.env
    });
  } catch (error) {
    await mkdir(outputRoot, { recursive: true });
    await writeJson(path.join(outputRoot, "manifest.json"), {
      schema_version: 1,
      mode,
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
