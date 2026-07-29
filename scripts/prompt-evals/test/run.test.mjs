import assert from "node:assert/strict";
import {
  mkdir,
  mkdtemp,
  rm,
  writeFile
} from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { execFileSync } from "node:child_process";

import { loadCorpus } from "../lib/corpus.mjs";
import { runEvaluation } from "../run.mjs";

const TEST_DIR = path.dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = path.resolve(TEST_DIR, "..", "..", "..");
const CONFIG_PATH = path.join(REPO_ROOT, "scripts", "prompt-evals", "config.json");

async function prepareFakeDir(tempRoot, corpus, variants, suiteScores) {
  const fakeDir = path.join(tempRoot, "fake");
  for (const caseData of corpus.cases) {
    const subjectDir = path.join(fakeDir, "subjects", caseData.id);
    await mkdir(subjectDir, { recursive: true });
    for (const variant of variants) {
      await writeFile(
        path.join(subjectDir, `${variant}.md`),
        `Synthetic ${variant} response for ${caseData.id}.`,
        "utf8"
      );
    }
  }
  await mkdir(path.join(fakeDir, "judges"), { recursive: true });
  for (const suite of ["create", "update", "debug"]) {
    const cases = corpus.cases.filter((caseData) => caseData.prompt === suite);
    if (!suiteScores[suite]) {
      continue;
    }
    const response = {
      schema_version: 1,
      suite,
      cases: cases.map((caseData) => ({
        case_id: caseData.id,
        variants: Object.fromEntries(
          variants.map((variant) => [
            variant,
            {
            status: "scored",
            criteria: caseData.criteria.map((criterion) => ({
              id: criterion.id,
                score: suiteScores[suite][variant],
              evidence: "Synthetic response",
              reason: "Synthetic midpoint score"
            })),
            summary: "Synthetic assessment"
            }
          ])
        )
      }))
    };
    await writeFile(
      path.join(fakeDir, "judges", `${suite}.json`),
      JSON.stringify(response),
      "utf8"
    );
  }
  return fakeDir;
}

test("runs a paired PR suite end to end with fake responses", async (t) => {
  const tempRoot = await mkdtemp(path.join(os.tmpdir(), "prompt-eval-pr-"));
  const outputDir = path.join(tempRoot, "output");
  t.after(() => rm(tempRoot, { recursive: true, force: true }));

  const corpus = await loadCorpus(REPO_ROOT);
  const fakeDir = await prepareFakeDir(
    tempRoot,
    corpus,
    ["base", "head"],
    {
      create: { base: 1, head: 2 }
    }
  );
  const headSha = execFileSync("git", ["rev-parse", "HEAD"], {
    cwd: REPO_ROOT,
    encoding: "utf8"
  }).trim();

  const scorecard = await runEvaluation({
    repoRoot: REPO_ROOT,
    outputRoot: outputDir,
    configPath: CONFIG_PATH,
    baseSha: headSha,
    headSha,
    changedFiles: ["prompts/create-ado-agentic-workflow.md"],
    copilotPath: "unused-copilot",
    adoAwPath: "unused-ado-aw",
    fakeDir,
    eventName: "pull_request",
    repository: "githubnext/ado-aw",
    runId: "synthetic-pr",
    runUrl: "https://example.invalid/runs/synthetic-pr",
    env: {}
  });

  assert.equal(scorecard.cases.length, 3);
  assert.equal(scorecard.summary.improved, 3);
  assert.equal(scorecard.summary.regressed, 0);
  assert.deepEqual(Object.keys(scorecard.suites), ["create"]);
});
