import assert from "node:assert/strict";
import { cp, mkdtemp, rm } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import {
  composeSubjectPrompt,
  extractWorkflowArtifact,
  loadCorpus,
  requiredSectionResults,
  selectSuites
} from "../lib/corpus.mjs";
import { restrictedChildEnv } from "../lib/process.mjs";
import { buildCopilotArgs } from "../lib/subject.mjs";

const TEST_DIR = path.dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = path.resolve(TEST_DIR, "..", "..", "..");

test("loads the versioned nine-case corpus", async () => {
  const corpus = await loadCorpus(REPO_ROOT);
  assert.equal(corpus.cases.length, 9);
  assert.equal(corpus.fixture_set_digest.length, 64);
  assert.equal(corpus.rubric_digest.length, 64);
  assert.deepEqual(
    Object.fromEntries(
      ["create", "update", "debug"].map((suite) => [
        suite,
        corpus.cases.filter((entry) => entry.prompt === suite).length
      ])
    ),
    { create: 3, update: 3, debug: 3 }
  );
});

test("fixture digests do not depend on checkout location", async (t) => {
  const firstRoot = await mkdtemp(path.join(os.tmpdir(), "prompt-eval-a-"));
  const secondRoot = await mkdtemp(path.join(os.tmpdir(), "prompt-eval-b-"));
  t.after(async () => {
    await rm(firstRoot, { recursive: true, force: true });
    await rm(secondRoot, { recursive: true, force: true });
  });
  for (const root of [firstRoot, secondRoot]) {
    await cp(
      path.join(REPO_ROOT, "tests", "prompt-evals"),
      path.join(root, "tests", "prompt-evals"),
      { recursive: true }
    );
  }
  const first = await loadCorpus(firstRoot);
  const second = await loadCorpus(secondRoot);
  assert.equal(first.fixture_set_digest, second.fixture_set_digest);
  assert.equal(first.rubric_digest, second.rubric_digest);
  assert.deepEqual(
    first.cases.map((entry) => entry.case_digest),
    second.cases.map((entry) => entry.case_digest)
  );
});

test("selects affected prompt suites", () => {
  assert.deepEqual(
    selectSuites("pr", ["prompts/create-ado-agentic-workflow.md"]),
    ["create"]
  );
  assert.deepEqual(
    selectSuites("pr", ["prompts/prompt-contract.md"]),
    ["create", "update", "debug"]
  );
  assert.deepEqual(
    selectSuites("pr", ["docs/front-matter.md"]),
    []
  );
  assert.deepEqual(selectSuites("nightly"), ["create", "update", "debug"]);
});

test("composes isolated prompts and extracts workflow artifacts", async () => {
  const corpus = await loadCorpus(REPO_ROOT);
  const caseData = corpus.cases.find(
    (entry) => entry.id === "create-minimal-manual"
  );
  const prompt = composeSubjectPrompt({
    sharedContract: "shared rules",
    taskPrompt: "task rules",
    caseData
  });
  assert.match(prompt, /<shared-prompt-contract>\nshared rules/);
  assert.match(prompt, /<authoring-prompt>\ntask rules/);
  assert.match(prompt, /<user-request>/);
  assert.match(prompt, /You have no tools/);

  const response = [
    "Here is the workflow:",
    "```markdown",
    "---",
    'name: "Synthetic"',
    'description: "Synthetic workflow"',
    "---",
    "",
    "Body",
    "```"
  ].join("\n");
  const artifact = extractWorkflowArtifact(response);
  assert.match(artifact, /^---\nname:/);
  assert.equal(extractWorkflowArtifact("No workflow yet."), null);
  assert.deepEqual(requiredSectionResults("## Evidence\nx", ["Evidence"]), [
    { section: "Evidence", present: true }
  ]);
});

test("Copilot arguments expose no tools and child env strips write tokens", () => {
  const args = buildCopilotArgs({
    promptPath: "prompt.txt",
    model: "synthetic-model",
    maxAiCredits: 1,
    workDir: "work",
    logDir: "logs"
  });
  assert.ok(args.includes("--available-tools="));
  assert.ok(args.includes("--disable-builtin-mcps"));
  assert.ok(args.includes("--no-custom-instructions"));
  assert.ok(args.includes("--no-remote-export"));

  const env = restrictedChildEnv({
    COPILOT_GITHUB_TOKEN: "model-auth",
    GITHUB_TOKEN: "write-token",
    GH_TOKEN: "write-token",
    SYSTEM_ACCESSTOKEN: "ado-token"
  });
  assert.equal(env.COPILOT_GITHUB_TOKEN, "model-auth");
  assert.equal(env.GITHUB_TOKEN, undefined);
  assert.equal(env.GH_TOKEN, undefined);
  assert.equal(env.SYSTEM_ACCESSTOKEN, undefined);
});

