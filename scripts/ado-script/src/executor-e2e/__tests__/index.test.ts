import { afterEach, describe, expect, it, vi } from "vitest";
import { mkdtemp, writeFile, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { booleanOption, main, selectScenarios, summarise } from "../index.js";
import { fileFailureIssue } from "../github-issue.js";
import { allScenarios } from "../scenarios/index.js";
import { SkipError } from "../scenario.js";
import type { ScenarioResult } from "../scenario.js";

vi.mock("../github-issue.js", () => ({
  loadIssueEnv: () => ({ repo: "test/repo" }),
  fileFailureIssue: vi.fn(async () => ({ filed: false })),
}));

describe("diagnostic selection", () => {
  afterEach(() => { vi.unstubAllEnvs(); vi.restoreAllMocks(); vi.clearAllMocks(); });
  it("only selects requested existing scenarios and rejects ambiguous input", () => {
    expect(selectScenarios(allScenarios, "noop,add-pull-request-labels")
      .map((scenario) => scenario.id ?? scenario.tool)).toEqual(["noop", "add-pull-request-labels"]);
    for (const invalid of ["missing-case", ",", "noop,", "noop,noop", " "]) {
      expect(() => selectScenarios(allScenarios, invalid)).toThrow();
    }
    expect(selectScenarios(allScenarios, "")).toBe(allScenarios);
    expect(booleanOption("False", true)).toBe(false);
    expect(() => booleanOption("off", true)).toThrow();
  });

  it("persists failed-run results and disables failure-issue reporting", async () => {
    const dir = await mkdtemp(join(tmpdir(), "ado-diagnostic-report-"));
    try {
      const bin = join(dir, "fixture.js");
      await writeFile(bin, `
const fs=require("node:fs"),path=require("node:path");
const out=process.argv[process.argv.indexOf("--safe-output-dir")+1];
fs.writeFileSync(path.join(out,"safe-outputs-executed.ndjson"), JSON.stringify({
name:"noop",status:"failed",error:"synthetic diagnostic failure"})+"\\n");
`);
      for (const [key, value] of Object.entries({
        SYSTEM_COLLECTIONURI: "https://example.test/", SYSTEM_TEAMPROJECT: "test",
        SYSTEM_ACCESSTOKEN: "not-a-real-token", EXECUTOR_E2E_ADO_AW_BIN: bin,
        EXECUTOR_E2E_SCENARIOS: "noop", EXECUTOR_E2E_REQUIRE_SELECTED: "true",
        EXECUTOR_E2E_FILE_FAILURE_ISSUE: "false",
        EXECUTOR_E2E_RESULTS_PATH: join(dir, "results.json"),
        BUILD_SOURCEVERSION: "candidate-sha", BUILD_BUILDID: "123",
      })) vi.stubEnv(key, value);
      expect(await main()).toBe(1);
      expect(fileFailureIssue).not.toHaveBeenCalled();
      const report = JSON.parse(await readFile(join(dir, "results.json"), "utf8"));
      expect(report).toMatchObject({
        commit: "candidate-sha", buildId: "123", selected: ["noop"],
        results: [{tool: "noop", ok: false, phase: "execute"}],
      });
      expect(JSON.stringify(report)).not.toContain("not-a-real-token");
    } finally { await rm(dir, {recursive: true, force: true}); }
  });

  it.each([
    ["add-pull-request-reviewers", " ", "EXECUTOR_E2E_REVIEWER is unavailable"],
    ["update-pull-request-cross-org", "", "EXECUTOR_E2E_CROSS_ORG_ORGANIZATION"],
    ["create-pull-request-add-reviewers-general", "11111111-1111-1111-1111-111111111111", "not a GUID"],
  ])("fails required %s preflight before creating scenario resources", async (id, reviewer, error) => {
    const dir = await mkdtemp(join(tmpdir(), "ado-required-preflight-"));
    try {
      for (const [key, value] of Object.entries({
        SYSTEM_COLLECTIONURI: "https://example.test/", SYSTEM_TEAMPROJECT: "test",
        SYSTEM_ACCESSTOKEN: "not-a-real-token", EXECUTOR_E2E_ADO_AW_BIN: "must-not-run",
        EXECUTOR_E2E_SCENARIOS: id, EXECUTOR_E2E_REQUIRE_SELECTED: "true",
        EXECUTOR_E2E_REVIEWER: reviewer, EXECUTOR_E2E_CROSS_ORG_ORGANIZATION: "",
        EXECUTOR_E2E_FILE_FAILURE_ISSUE: "false",
        EXECUTOR_E2E_RESULTS_PATH: join(dir, "results.json"),
      })) vi.stubEnv(key, value);
      const scenario = selectScenarios(allScenarios, id)[0]!;
      const setup = vi.spyOn(scenario, "setup");
      expect(await main()).toBe(1);
      expect(setup).not.toHaveBeenCalled();
      expect(fileFailureIssue).not.toHaveBeenCalled();
      const report = JSON.parse(await readFile(join(dir, "results.json"), "utf8"));
      expect(report.results).toEqual([expect.objectContaining({
        tool: "required-preflight", ok: false, phase: "preflight",
        message: expect.stringContaining(error),
      })]);
    } finally { await rm(dir, { recursive: true, force: true }); }
  });

  it("reports a required runtime skip as failed coverage", async () => {
    const dir = await mkdtemp(join(tmpdir(), "ado-required-skip-"));
    try {
      for (const [key, value] of Object.entries({
        SYSTEM_COLLECTIONURI: "https://example.test/", SYSTEM_TEAMPROJECT: "test",
        SYSTEM_ACCESSTOKEN: "not-a-real-token", EXECUTOR_E2E_ADO_AW_BIN: "must-not-run",
        EXECUTOR_E2E_SCENARIOS: "noop", EXECUTOR_E2E_REQUIRE_SELECTED: "true",
        EXECUTOR_E2E_FILE_FAILURE_ISSUE: "false",
        EXECUTOR_E2E_RESULTS_PATH: join(dir, "results.json"),
      })) vi.stubEnv(key, value);
      const scenario = selectScenarios(allScenarios, "noop")[0]!;
      vi.spyOn(scenario, "setup").mockRejectedValue(new SkipError("prerequisite disappeared"));
      expect(await main()).toBe(1);
      const report = JSON.parse(await readFile(join(dir, "results.json"), "utf8"));
      expect(report.results).toEqual([expect.objectContaining({
        tool: "noop", ok: false, skipped: false, phase: "required-coverage",
        message: "Required scenario skipped: prerequisite disappeared",
      })]);
      expect(fileFailureIssue).not.toHaveBeenCalled();
    } finally { await rm(dir, { recursive: true, force: true }); }
  });
});

describe("summarise", () => {
  it("renders PASS/FAIL/SKIP lines and a total", () => {
    const results: ScenarioResult[] = [
      { tool: "create-work-item", ok: true, durationMs: 5 },
      { tool: "add-pull-request-comment", ok: false, phase: "assert", message: "no thread", durationMs: 5 },
      { tool: "queue-build", ok: true, skipped: true, phase: "skipped", message: "no id", durationMs: 1 },
    ];
    const text = summarise(results);
    expect(text).toContain("[PASS] create-work-item");
    expect(text).toContain("[FAIL] add-pull-request-comment (assert: no thread)");
    expect(text).toContain("[SKIP] queue-build");
    expect(text).toContain("Total: 3 | Passed: 1 | Failed: 1 | Skipped: 1");
  });
});

describe("scenario registry", () => {
  it("registers both create-pull-request checkout layouts with unique ids", () => {
    const ids = allScenarios.map((scenario) => scenario.id ?? scenario.tool);
    expect(new Set(ids).size).toBe(ids.length);
    expect(ids).toContain("create-pull-request");
    expect(ids).toContain("create-pull-request-self-multi-checkout");
    expect(ids).toContain("create-pull-request-cross-org");
    expect(ids).toContain("create-pull-request-temporary-id-handoff");
    expect(ids).toContain("create-pull-request-add-reviewers");
    expect(ids).toContain("create-branch-cross-org");
    expect(ids).toContain("create-git-tag-cross-org");
    for (const tool of [
      "update-pull-request", "add-pull-request-labels", "add-pull-request-reviewers",
      "submit-pull-request-review", "set-pull-request-auto-complete", "abandon-pull-request",
    ]) expect(ids).toContain(`${tool}-cross-org`);
  });

  it("registers the GitHub issue scenarios with unique ids", () => {
    const ids = allScenarios.map((scenario) => scenario.id ?? scenario.tool);
    expect(new Set(ids).size).toBe(ids.length);
    for (const id of [
      "create-github-issue",
      "create-github-issue-label-denied",
      "set-github-issue-type",
      "set-github-issue-type-clear",
      "create-github-issue-temporary-id-handoff",
      "comment-on-github-issue",
      "comment-on-github-issue-hide-older",
      "hide-github-issue-comment",
      "add-github-issue-labels",
      "remove-github-issue-labels",
      "close-github-issue",
      "update-github-issue",
      "set-github-issue-field",
      "assign-github-issue-milestone",
      "assign-github-issue-to-user",
      "unassign-github-issue-from-user",
      "link-github-sub-issue",
    ]) {
      expect(ids).toContain(id);
    }
  });
});
