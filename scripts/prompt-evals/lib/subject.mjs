import { mkdir, readFile, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";

import {
  extractWorkflowArtifact,
  pathExists,
  requiredSectionResults
} from "./corpus.mjs";
import { restrictedChildEnv, runProcess } from "./process.mjs";

async function writeJson(filePath, value) {
  await writeFile(filePath, `${JSON.stringify(value, null, 2)}\n`, "utf8");
}

export function buildCopilotArgs({
  promptPath,
  model,
  maxAiCredits,
  workDir,
  logDir
}) {
  return [
    "--prompt-file",
    promptPath,
    "--model",
    model,
    "--max-ai-credits",
    String(maxAiCredits),
    "--silent",
    "--no-color",
    "--stream",
    "off",
    "--output-format",
    "text",
    "--no-custom-instructions",
    "--disable-builtin-mcps",
    "--no-ask-user",
    "--no-remote",
    "--no-remote-export",
    "--no-auto-update",
    "--disallow-temp-dir",
    "--available-tools=",
    "--allow-all-tools",
    "--secret-env-vars=COPILOT_GITHUB_TOKEN,GH_TOKEN,GITHUB_TOKEN,SYSTEM_ACCESSTOKEN,AZURE_DEVOPS_EXT_PAT,SC_WRITE_TOKEN",
    "--log-level",
    "error",
    "--log-dir",
    logDir,
    "-C",
    workDir
  ];
}

export async function runToolFreeProbe({
  copilotPath,
  model,
  maxAiCredits,
  timeoutMs,
  maxOutputBytes,
  outputDir,
  env = process.env,
  fakeDir = null
}) {
  await mkdir(outputDir, { recursive: true });
  const resultPath = path.join(outputDir, "tool-free-probe.json");

  if (fakeDir) {
    const result = {
      success: true,
      fake: true,
      response: "TOOL_FREE_OK"
    };
    await writeJson(resultPath, result);
    return result;
  }

  const workDir = path.join(outputDir, "work");
  const logDir = path.join(
    os.tmpdir(),
    "ado-aw-prompt-eval-logs",
    String(process.pid),
    "probe"
  );
  await mkdir(workDir, { recursive: true });
  await mkdir(logDir, { recursive: true });
  const promptPath = path.join(workDir, "prompt.txt");
  await writeFile(
    promptPath,
    [
      "Reply with exactly TOOL_FREE_OK.",
      "Do not call any tool.",
      "Do not add punctuation or explanation."
    ].join("\n"),
    "utf8"
  );

  const execution = await runProcess(
    copilotPath,
    buildCopilotArgs({
      promptPath,
      model,
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
  const response = execution.stdout.trim();
  const result = {
    success: execution.success && response === "TOOL_FREE_OK",
    response,
    execution: {
      ...execution,
      stdout: undefined
    }
  };
  await writeJson(resultPath, result);
  if (!result.success) {
    throw new Error(
      `tool-free Copilot CLI probe failed: ${execution.stderr || response || "no output"}`
    );
  }
  return result;
}

async function runCompilerChecks({
  adoAwPath,
  artifactPath,
  runDir,
  timeoutMs,
  maxOutputBytes,
  env
}) {
  const compiledPath = path.join(runDir, "artifact.yml");
  const compile = await runProcess(
    adoAwPath,
    ["compile", artifactPath, "-o", compiledPath],
    {
      cwd: runDir,
      env: restrictedChildEnv(env),
      timeoutMs,
      maxOutputBytes
    }
  );
  await writeFile(path.join(runDir, "compile.stdout.txt"), compile.stdout, "utf8");
  await writeFile(path.join(runDir, "compile.stderr.txt"), compile.stderr, "utf8");

  const lint = await runProcess(adoAwPath, ["lint", artifactPath, "--json"], {
    cwd: runDir,
    env: restrictedChildEnv(env),
    timeoutMs,
    maxOutputBytes
  });
  await writeFile(path.join(runDir, "lint.stdout.json"), lint.stdout, "utf8");
  await writeFile(path.join(runDir, "lint.stderr.txt"), lint.stderr, "utf8");

  let lintReport = null;
  let lintParseError = null;
  if (lint.stdout.trim() !== "") {
    try {
      lintReport = JSON.parse(lint.stdout);
    } catch (error) {
      lintParseError = error.message;
    }
  }

  return {
    compile: {
      success: compile.success,
      code: compile.code,
      timed_out: compile.timed_out,
      duration_ms: compile.duration_ms,
      output_truncated: compile.output_truncated,
      compiled_path: compile.success && (await pathExists(compiledPath))
        ? compiledPath
        : null
    },
    lint: {
      success: lint.success,
      code: lint.code,
      timed_out: lint.timed_out,
      duration_ms: lint.duration_ms,
      output_truncated: lint.output_truncated,
      parsed: lintReport !== null,
      parse_error: lintParseError,
      summary: lintReport?.summary ?? null
    }
  };
}

export async function runSubjectVariant({
  caseData,
  variant,
  prompt,
  outputRoot,
  copilotPath,
  adoAwPath,
  model,
  maxAiCredits,
  timeoutMs,
  maxOutputBytes,
  env = process.env,
  fakeDir = null
}) {
  const runDir = path.join(outputRoot, "cases", caseData.id, variant);
  const workDir = path.join(runDir, "work");
  const logDir = path.join(
    os.tmpdir(),
    "ado-aw-prompt-eval-logs",
    String(process.pid),
    caseData.id,
    variant
  );
  await mkdir(workDir, { recursive: true });
  await mkdir(logDir, { recursive: true });

  const promptPath = path.join(runDir, "prompt.txt");
  const responsePath = path.join(runDir, "response.md");
  await writeFile(promptPath, `${prompt.trimEnd()}\n`, "utf8");

  let execution;
  let response;
  if (fakeDir) {
    const fakeResponsePath = path.join(
      fakeDir,
      "subjects",
      caseData.id,
      `${variant}.md`
    );
    response = await readFile(fakeResponsePath, "utf8");
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
        model,
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
    path.join(runDir, "subject.stderr.txt"),
    execution.stderr ?? "",
    "utf8"
  );

  const artifact = extractWorkflowArtifact(response);
  const artifactPath = artifact ? path.join(runDir, "artifact.md") : null;
  if (artifactPath) {
    await writeFile(artifactPath, `${artifact.trimEnd()}\n`, "utf8");
  }

  let compiler = null;
  if (artifactPath && (caseData.expected.compile || caseData.expected.lint)) {
    compiler = await runCompilerChecks({
      adoAwPath,
      artifactPath,
      runDir,
      timeoutMs,
      maxOutputBytes,
      env
    });
  }

  const sections = requiredSectionResults(
    response,
    caseData.expected.required_sections
  );
  const result = {
    case_id: caseData.id,
    suite: caseData.prompt,
    variant,
    model,
    execution: {
      success: execution.success,
      fake: execution.fake ?? false,
      code: execution.code,
      signal: execution.signal,
      timed_out: execution.timed_out,
      duration_ms: execution.duration_ms,
      output_truncated: execution.output_truncated,
      error: execution.success ? null : execution.stderr || "subject run failed"
    },
    response_path: responsePath,
    response_length: response.length,
    artifact: {
      expected: caseData.expected.artifact_required,
      found: artifact !== null,
      path: artifactPath
    },
    required_sections: sections,
    compiler,
    observations: {
      subject_succeeded: execution.success,
      response_nonempty: response.trim() !== "",
      artifact_expectation_met:
        caseData.expected.artifact_required === (artifact !== null),
      required_sections_present: sections.every((section) => section.present),
      compile_succeeded: compiler?.compile.success ?? null,
      lint_succeeded: compiler?.lint.success ?? null,
      lint_errors: compiler?.lint.summary?.errors ?? null
    },
    response
  };
  await writeJson(path.join(runDir, "result.json"), {
    ...result,
    response: undefined
  });
  return result;
}
