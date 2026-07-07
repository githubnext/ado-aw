import { describe, expect, it } from "vitest";

import { summarise } from "../index.js";
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
