import { describe, expect, it } from "vitest";

import { summarise } from "../index.js";
import { allScenarios } from "../scenarios/index.js";
import type { ScenarioResult } from "../scenario.js";

describe("summarise", () => {
  it("renders PASS/FAIL/SKIP lines and a total", () => {
    const results: ScenarioResult[] = [
      { tool: "create-work-item", ok: true, durationMs: 5 },
      { tool: "add-pr-comment", ok: false, phase: "assert", message: "no thread", durationMs: 5 },
      { tool: "queue-build", ok: true, skipped: true, phase: "skipped", message: "no id", durationMs: 1 },
    ];
    const text = summarise(results);
    expect(text).toContain("[PASS] create-work-item");
    expect(text).toContain("[FAIL] add-pr-comment (assert: no thread)");
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
    expect(ids).toContain("create-branch-cross-org");
    expect(ids).toContain("create-git-tag-cross-org");
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
