import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import type { GateSpec, PredicateSpec } from "../shared/types.gen.js";
import { PolicyTracker } from "../shared/policy.js";
import { evaluatePredicate, evaluatePredicates, predicateFacts } from "./predicates.js";

function factMap(values: Record<string, unknown>): Map<string, unknown> {
  return new Map(Object.entries(values));
}

function evalWith(p: PredicateSpec, values: Record<string, unknown>): boolean {
  return evaluatePredicate(p, factMap(values));
}

function gateSpec(checks: GateSpec["checks"], facts: GateSpec["facts"] = []): GateSpec {
  return {
    checks,
    facts,
    context: {
      build_reason: "PullRequest",
      bypass_label: "run-anyway",
      step_name: "Gate",
      tag_prefix: "gate",
    },
  };
}

describe("evaluatePredicate", () => {
  describe("glob_match", () => {
    it("matches a positive glob", () => {
      const pred = { type: "glob_match", fact: "pr_title", pattern: "*[review]*" } satisfies PredicateSpec;
      expect(evalWith(pred, { pr_title: "feat: add feature [review]" })).toBe(true);
    });

    it("rejects a negative glob", () => {
      const pred = { type: "glob_match", fact: "pr_title", pattern: "*[review]*" } satisfies PredicateSpec;
      expect(evalWith(pred, { pr_title: "feat: add feature" })).toBe(false);
    });

    it("strips ref prefixes from branch patterns", () => {
      const pred = {
        type: "glob_match",
        fact: "source_branch",
        pattern: "refs/heads/feature/*",
      } satisfies PredicateSpec;
      expect(evalWith(pred, { source_branch: "feature/my-branch" })).toBe(true);
    });
  });

  describe("equals", () => {
    it("matches equal strings", () => {
      const pred = { type: "equals", fact: "pr_is_draft", value: "false" } satisfies PredicateSpec;
      expect(evalWith(pred, { pr_is_draft: "false" })).toBe(true);
    });

    it("rejects different strings", () => {
      const pred = { type: "equals", fact: "pr_is_draft", value: "false" } satisfies PredicateSpec;
      expect(evalWith(pred, { pr_is_draft: "true" })).toBe(false);
    });
  });

  describe("value_in_set", () => {
    it("matches case-sensitive values", () => {
      const pred = {
        type: "value_in_set",
        fact: "author_email",
        values: ["Alice@Corp.com"],
        case_insensitive: false,
      } satisfies PredicateSpec;
      expect(evalWith(pred, { author_email: "Alice@Corp.com" })).toBe(true);
    });

    it("rejects case-sensitive values with different case", () => {
      const pred = {
        type: "value_in_set",
        fact: "author_email",
        values: ["Alice@Corp.com"],
        case_insensitive: false,
      } satisfies PredicateSpec;
      expect(evalWith(pred, { author_email: "alice@corp.com" })).toBe(false);
    });

    it("matches case-insensitive values", () => {
      const pred = {
        type: "value_in_set",
        fact: "author_email",
        values: ["Alice@Corp.com"],
        case_insensitive: true,
      } satisfies PredicateSpec;
      expect(evalWith(pred, { author_email: "alice@corp.com" })).toBe(true);
    });

    it("rejects absent case-insensitive values", () => {
      const pred = {
        type: "value_in_set",
        fact: "build_reason",
        values: ["PullRequest", "Manual"],
        case_insensitive: true,
      } satisfies PredicateSpec;
      expect(evalWith(pred, { build_reason: "Schedule" })).toBe(false);
    });
  });

  describe("value_not_in_set", () => {
    it("passes when the value is not present", () => {
      const pred = {
        type: "value_not_in_set",
        fact: "author_email",
        values: ["bot@noreply.com"],
        case_insensitive: true,
      } satisfies PredicateSpec;
      expect(evalWith(pred, { author_email: "dev@corp.com" })).toBe(true);
    });

    it("fails when the value is present", () => {
      const pred = {
        type: "value_not_in_set",
        fact: "author_email",
        values: ["bot@noreply.com"],
        case_insensitive: true,
      } satisfies PredicateSpec;
      expect(evalWith(pred, { author_email: "BOT@noreply.com" })).toBe(false);
    });
  });

  describe("numeric_range", () => {
    it("passes values in range", () => {
      const pred = { type: "numeric_range", fact: "changed_file_count", min: 5, max: 100 } satisfies PredicateSpec;
      expect(evalWith(pred, { changed_file_count: 50 })).toBe(true);
    });

    it("fails values below min", () => {
      const pred = { type: "numeric_range", fact: "changed_file_count", min: 5, max: 100 } satisfies PredicateSpec;
      expect(evalWith(pred, { changed_file_count: 2 })).toBe(false);
    });

    it("fails values above max", () => {
      const pred = { type: "numeric_range", fact: "changed_file_count", min: 5, max: 100 } satisfies PredicateSpec;
      expect(evalWith(pred, { changed_file_count: 200 })).toBe(false);
    });

    it("passes with no min", () => {
      const pred = { type: "numeric_range", fact: "changed_file_count", max: 50 } satisfies PredicateSpec;
      expect(evalWith(pred, { changed_file_count: 10 })).toBe(true);
    });

    it("passes with no max", () => {
      const pred = { type: "numeric_range", fact: "changed_file_count", min: 3 } satisfies PredicateSpec;
      expect(evalWith(pred, { changed_file_count: 10 })).toBe(true);
    });
  });

  describe("time_window", () => {
    it.each([
      ["passes inside a same-day window", 600, true],
      ["fails outside a same-day window", 1200, false],
      ["passes late in an overnight window", 1380, true],
      ["passes early in an overnight window", 300, true],
      ["fails midday in an overnight window", 720, false],
      ["fails at the exclusive overnight end", 360, false],
    ])("%s", (_name, current, expected) => {
      const pred =
        current === 600 || current === 1200
          ? ({ type: "time_window", start: "09:00", end: "17:00" } satisfies PredicateSpec)
          : ({ type: "time_window", start: "22:00", end: "06:00" } satisfies PredicateSpec);
      expect(evalWith(pred, { current_utc_minutes: current })).toBe(expected);
    });
  });

  describe("label_set_match", () => {
    it("matches any_of case-insensitively", () => {
      const pred = {
        type: "label_set_match",
        fact: "pr_labels",
        any_of: ["run-agent"],
        all_of: [],
        none_of: [],
      } satisfies PredicateSpec;
      expect(evalWith(pred, { pr_labels: ["Run-Agent", "other"] })).toBe(true);
    });

    it("requires all_of labels", () => {
      const pred = {
        type: "label_set_match",
        fact: "pr_labels",
        any_of: [],
        all_of: ["approved", "tested"],
        none_of: [],
      } satisfies PredicateSpec;
      expect(evalWith(pred, { pr_labels: ["approved", "tested", "other"] })).toBe(true);
    });

    it("rejects none_of labels", () => {
      const pred = {
        type: "label_set_match",
        fact: "pr_labels",
        any_of: [],
        all_of: [],
        none_of: ["do-not-run"],
      } satisfies PredicateSpec;
      expect(evalWith(pred, { pr_labels: ["Do-Not-Run", "other"] })).toBe(false);
    });

    it("supports mixed any_of, all_of, and none_of from newline strings", () => {
      const pred = {
        type: "label_set_match",
        fact: "pr_labels",
        any_of: ["run-agent"],
        all_of: ["approved"],
        none_of: ["blocked"],
      } satisfies PredicateSpec;
      expect(evalWith(pred, { pr_labels: "Run-Agent\napproved\nother" })).toBe(true);
    });
  });

  describe("file_glob_match", () => {
    it("matches include-only filters", () => {
      const pred = {
        type: "file_glob_match",
        fact: "changed_files",
        include: ["src/*.ts"],
        exclude: [],
      } satisfies PredicateSpec;
      expect(evalWith(pred, { changed_files: ["src/main.ts", "docs/readme.md"] })).toBe(true);
    });

    it("passes exclude-only filters with an empty file list", () => {
      const pred = {
        type: "file_glob_match",
        fact: "changed_files",
        include: [],
        exclude: ["src/generated/*"],
      } satisfies PredicateSpec;
      expect(evalWith(pred, { changed_files: [] })).toBe(true);
    });

    it("matches mixed include and exclude filters", () => {
      const pred = {
        type: "file_glob_match",
        fact: "changed_files",
        include: ["src/*.ts"],
        exclude: ["src/*.test.ts"],
      } satisfies PredicateSpec;
      expect(evalWith(pred, { changed_files: ["src/main.test.ts", "src/main.ts"] })).toBe(true);
    });

    it("fails include filters with an empty file list", () => {
      const pred = {
        type: "file_glob_match",
        fact: "changed_files",
        include: ["src/*.ts"],
        exclude: [],
      } satisfies PredicateSpec;
      expect(evalWith(pred, { changed_files: [] })).toBe(false);
    });
  });

  describe("and", () => {
    it("passes when all operands pass", () => {
      const pred = {
        type: "and",
        operands: [
          { type: "equals", fact: "a", value: "1" },
          { type: "equals", fact: "b", value: "2" },
        ],
      } satisfies PredicateSpec;
      expect(evalWith(pred, { a: "1", b: "2" })).toBe(true);
    });

    it("fails when one operand fails", () => {
      const pred = {
        type: "and",
        operands: [
          { type: "equals", fact: "a", value: "1" },
          { type: "equals", fact: "b", value: "3" },
        ],
      } satisfies PredicateSpec;
      expect(evalWith(pred, { a: "1", b: "2" })).toBe(false);
    });
  });

  describe("or", () => {
    it("passes when any operand passes", () => {
      const pred = {
        type: "or",
        operands: [
          { type: "equals", fact: "a", value: "wrong" },
          { type: "equals", fact: "b", value: "2" },
        ],
      } satisfies PredicateSpec;
      expect(evalWith(pred, { a: "1", b: "2" })).toBe(true);
    });

    it("fails when all operands fail", () => {
      const pred = {
        type: "or",
        operands: [
          { type: "equals", fact: "a", value: "wrong" },
          { type: "equals", fact: "b", value: "wrong" },
        ],
      } satisfies PredicateSpec;
      expect(evalWith(pred, { a: "1", b: "2" })).toBe(false);
    });
  });

  describe("not", () => {
    it("inverts a passing operand", () => {
      const pred = {
        type: "not",
        operand: { type: "equals", fact: "a", value: "1" },
      } satisfies PredicateSpec;
      expect(evalWith(pred, { a: "1" })).toBe(false);
    });

    it("inverts a failing operand", () => {
      const pred = {
        type: "not",
        operand: { type: "equals", fact: "a", value: "1" },
      } satisfies PredicateSpec;
      expect(evalWith(pred, { a: "2" })).toBe(true);
    });
  });
});

describe("predicateFacts", () => {
  it("collects a simple fact", () => {
    const pred = { type: "glob_match", fact: "pr_title", pattern: "test" } satisfies PredicateSpec;
    expect(predicateFacts(pred)).toEqual(["pr_title"]);
  });

  it("collects compound facts recursively", () => {
    const pred = {
      type: "and",
      operands: [
        { type: "equals", fact: "a", value: "1" },
        {
          type: "or",
          operands: [
            { type: "glob_match", fact: "b", pattern: "x" },
            { type: "not", operand: { type: "equals", fact: "c", value: "3" } },
          ],
        },
      ],
    } satisfies PredicateSpec;
    expect(new Set(predicateFacts(pred))).toEqual(new Set(["a", "b", "c"]));
  });

  it("adds current_utc_minutes for time windows", () => {
    const pred = { type: "time_window", start: "09:00", end: "17:00" } satisfies PredicateSpec;
    expect(predicateFacts(pred)).toEqual(["current_utc_minutes"]);
  });
});

describe("evaluatePredicates", () => {
  let writes: string[];

  beforeEach(() => {
    writes = [];
    vi.spyOn(process.stdout, "write").mockImplementation((chunk: any) => {
      writes.push(typeof chunk === "string" ? chunk : chunk.toString());
      return true;
    });
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("returns one result per check, records summary, and emits tags for failures", () => {
    const spec = gateSpec(
      [
        {
          name: "title",
          tag_suffix: "title",
          predicate: { type: "equals", fact: "pr_title", value: "ok" },
        },
        {
          name: "reason",
          tag_suffix: "reason",
          predicate: { type: "equals", fact: "build_reason", value: "PullRequest" },
        },
      ],
      [
        { kind: "pr_title", failure_policy: "fail_closed" },
        { kind: "build_reason", failure_policy: "fail_closed" },
      ],
    );
    const tracker = new PolicyTracker(spec.facts);

    const results = evaluatePredicates(
      spec,
      factMap({ pr_title: "ok", build_reason: "Manual" }),
      tracker,
    );

    expect(results).toEqual(["pass", "fail"]);
    expect(tracker.summary()).toEqual({ passed: 1, failed: 1, skipped: 0 });
    expect(writes).toContain("##vso[build.addbuildtag]gate:reason\n");
  });

  it("uses tracker verdicts for unavailable facts before evaluating predicates", () => {
    const spec = gateSpec(
      [
        {
          name: "missing-title",
          tag_suffix: "missing-title",
          predicate: { type: "equals", fact: "pr_title", value: "ok" },
        },
      ],
      [{ kind: "pr_title", failure_policy: "fail_closed" }],
    );
    const tracker = new PolicyTracker(spec.facts);
    tracker.recordFactFailure("pr_title", "test failure");

    const results = evaluatePredicates(spec, factMap({ pr_title: "ok" }), tracker);

    expect(results).toEqual(["fail"]);
    expect(tracker.summary()).toEqual({ passed: 0, failed: 1, skipped: 0 });
    expect(writes).toContain("##vso[build.addbuildtag]gate:missing-title\n");
  });
});
