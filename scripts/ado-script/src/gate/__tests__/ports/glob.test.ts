import { describe, expect, it } from "vitest";
import type { PredicateSpec } from "../../../shared/types.gen.js";
import { evaluatePredicate } from "../../predicates.js";
import { factMap } from "./helpers.js";

describe("TestGlobMatch", () => {
  it("test match", () => {
    const pred = { type: "glob_match", fact: "pr_title", pattern: "*[review]*" } satisfies PredicateSpec;
    const facts = factMap({ pr_title: "feat: add feature [review]" });
    expect(evaluatePredicate(pred, facts)).toBe(true);
  });

  it("test no match", () => {
    const pred = { type: "glob_match", fact: "pr_title", pattern: "*[review]*" } satisfies PredicateSpec;
    const facts = factMap({ pr_title: "feat: add feature" });
    expect(evaluatePredicate(pred, facts)).toBe(false);
  });

  it("test wildcard", () => {
    const pred = { type: "glob_match", fact: "source_branch", pattern: "feature/*" } satisfies PredicateSpec;
    const facts = factMap({ source_branch: "feature/my-branch" });
    expect(evaluatePredicate(pred, facts)).toBe(true);
  });

  it("test exact", () => {
    const pred = { type: "glob_match", fact: "target_branch", pattern: "main" } satisfies PredicateSpec;
    const facts = factMap({ target_branch: "main" });
    expect(evaluatePredicate(pred, facts)).toBe(true);
  });

  it("test exact no match", () => {
    const pred = { type: "glob_match", fact: "target_branch", pattern: "main" } satisfies PredicateSpec;
    const facts = factMap({ target_branch: "develop" });
    expect(evaluatePredicate(pred, facts)).toBe(false);
  });

  it("test empty value", () => {
    const pred = { type: "glob_match", fact: "pr_title", pattern: "*" } satisfies PredicateSpec;
    const facts = factMap({ pr_title: "" });
    expect(evaluatePredicate(pred, facts)).toBe(true);
  });

  it("test dotall across newlines", () => {
    const pred = { type: "glob_match", fact: "commit_message", pattern: "feat:*details" } satisfies PredicateSpec;
    const facts = factMap({ commit_message: "feat: add thing\n\nbody details" });
    expect(evaluatePredicate(pred, facts)).toBe(true);
  });
});

describe("globMatch hardening", () => {
  it("rejects patterns over the length cap (fail-closed)", () => {
    const longPattern = "a".repeat(2048);
    const pred = { type: "glob_match", fact: "pr_title", pattern: longPattern } satisfies PredicateSpec;
    const facts = factMap({ pr_title: longPattern });
    expect(evaluatePredicate(pred, facts)).toBe(false);
  });

  it("rejects patterns with too many '*' wildcards (fail-closed)", () => {
    const wildcardBomb = "*".repeat(128);
    const pred = { type: "glob_match", fact: "pr_title", pattern: wildcardBomb } satisfies PredicateSpec;
    const facts = factMap({ pr_title: "anything" });
    expect(evaluatePredicate(pred, facts)).toBe(false);
  });

  it("caches compiled regex across repeated invocations", async () => {
    const { _resetGlobCacheForTesting } = await import("../../predicates.js");
    _resetGlobCacheForTesting();
    const pred = { type: "glob_match", fact: "pr_title", pattern: "feat/*" } satisfies PredicateSpec;
    const facts = factMap({ pr_title: "feat/x" });

    // First call compiles, subsequent calls hit the cache. We can't directly
    // observe the cache from outside, so we just assert idempotency under
    // a large number of repeated calls and that we don't blow the stack /
    // hit any allocation cliff. (A negative perf assertion is unreliable in
    // CI — this is a smoke test for the cache integration, not a perf gate.)
    for (let i = 0; i < 5000; i++) {
      expect(evaluatePredicate(pred, facts)).toBe(true);
    }
  });
});
