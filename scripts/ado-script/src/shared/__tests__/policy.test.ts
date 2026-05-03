import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { PolicyTracker } from "../policy.js";
import type { FactSpec } from "../types.gen.js";

function spec(kind: string, fp: string): FactSpec {
  return { kind, failure_policy: fp };
}

describe("PolicyTracker", () => {
  beforeEach(() => {
    vi.spyOn(process.stdout, "write").mockImplementation(() => true);
  });
  afterEach(() => vi.restoreAllMocks());

  it("returns 'evaluate' when no referenced facts are missing", () => {
    const t = new PolicyTracker([spec("pr_title", "fail_closed")]);
    expect(t.verdictForMissingFacts(["pr_title"])).toBe("evaluate");
  });

  it("fail_closed missing fact → fail", () => {
    const t = new PolicyTracker([spec("pr_title", "fail_closed")]);
    t.recordFactFailure("pr_title", "missing env var");
    expect(t.verdictForMissingFacts(["pr_title"])).toBe("fail");
  });

  it("fail_open missing fact → pass", () => {
    const t = new PolicyTracker([spec("pr_labels", "fail_open")]);
    t.recordFactFailure("pr_labels", "no metadata");
    expect(t.verdictForMissingFacts(["pr_labels"])).toBe("pass");
  });

  it("skip_dependents missing fact directly referenced → skip", () => {
    const t = new PolicyTracker([spec("pr_metadata", "skip_dependents")]);
    t.recordFactFailure("pr_metadata", "API error");
    expect(t.verdictForMissingFacts(["pr_metadata"])).toBe("skip");
  });

  it("transitive skip: pr_metadata fails skip_dependents → pr_is_draft skipped", () => {
    const t = new PolicyTracker([
      spec("pr_metadata", "skip_dependents"),
      spec("pr_is_draft", "fail_closed"),
    ]);
    t.recordFactFailure("pr_metadata", "API error");
    expect(t.verdictForMissingFacts(["pr_is_draft"])).toBe("skip");
  });

  it("transitive skip: pr_metadata fails skip_dependents → pr_labels skipped", () => {
    const t = new PolicyTracker([
      spec("pr_metadata", "skip_dependents"),
      spec("pr_labels", "fail_open"),
    ]);
    t.recordFactFailure("pr_metadata", "API error");
    // Even though pr_labels is fail_open, the *skip* propagates because
    // its dep failed with skip_dependents. The skip dominates.
    expect(t.verdictForMissingFacts(["pr_labels"])).toBe("skip");
  });

  it("transitive skip: changed_files fails skip_dependents → changed_file_count skipped", () => {
    const t = new PolicyTracker([
      spec("changed_files", "skip_dependents"),
      spec("changed_file_count", "fail_open"),
    ]);
    t.recordFactFailure("changed_files", "iter API error");
    expect(t.verdictForMissingFacts(["changed_file_count"])).toBe("skip");
  });

  it("multiple missing facts: skip dominates fail_closed", () => {
    const t = new PolicyTracker([
      spec("pr_metadata", "skip_dependents"),
      spec("pr_title", "fail_closed"),
    ]);
    t.recordFactFailure("pr_metadata", "API error");
    t.recordFactFailure("pr_title", "missing");
    expect(t.verdictForMissingFacts(["pr_metadata", "pr_title"])).toBe("skip");
  });

  it("multiple missing facts: fail_closed dominates fail_open", () => {
    const t = new PolicyTracker([
      spec("pr_title", "fail_closed"),
      spec("pr_labels", "fail_open"),
    ]);
    t.recordFactFailure("pr_title", "missing");
    t.recordFactFailure("pr_labels", "no md");
    expect(t.verdictForMissingFacts(["pr_title", "pr_labels"])).toBe("fail");
  });

  it("recordFactFailure for skip_dependents only emits a warning", () => {
    const writes: string[] = [];
    const spyWrite = vi.spyOn(process.stdout, "write").mockImplementation((c: any) => {
      writes.push(typeof c === "string" ? c : c.toString());
      return true;
    });
    const t = new PolicyTracker([spec("pr_metadata", "skip_dependents")]);
    t.recordFactFailure("pr_metadata", "test reason");
    expect(writes.some((w) => w.includes("logissue type=warning"))).toBe(true);
    expect(writes.some((w) => w.includes("pr_metadata"))).toBe(true);
    spyWrite.mockRestore();
  });

  it("summary tallies recordCheckResult outcomes", () => {
    const t = new PolicyTracker([]);
    t.recordCheckResult("pass");
    t.recordCheckResult("pass");
    t.recordCheckResult("fail");
    t.recordCheckResult("skip");
    expect(t.summary()).toEqual({ passed: 2, failed: 1, skipped: 1 });
  });

  it("unknown fact kind defaults to fail_closed", () => {
    const t = new PolicyTracker([]);
    t.recordFactFailure("nonexistent_fact", "test");
    expect(t.verdictForMissingFacts(["nonexistent_fact"])).toBe("fail");
  });
});
