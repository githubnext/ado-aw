import { describe, expect, it } from "vitest";
import type { PredicateSpec } from "../../../shared/types.gen.js";
import { evaluatePredicate } from "../../predicates.js";
import { factMap } from "./helpers.js";

describe("TestEquals", () => {
  it("test match", () => {
    const pred = { type: "equals", fact: "pr_is_draft", value: "false" } satisfies PredicateSpec;
    const facts = factMap({ pr_is_draft: "false" });
    expect(evaluatePredicate(pred, facts)).toBe(true);
  });

  it("test no match", () => {
    const pred = { type: "equals", fact: "pr_is_draft", value: "false" } satisfies PredicateSpec;
    const facts = factMap({ pr_is_draft: "true" });
    expect(evaluatePredicate(pred, facts)).toBe(false);
  });

  it("test missing fact", () => {
    const pred = { type: "equals", fact: "missing", value: "x" } satisfies PredicateSpec;
    expect(evaluatePredicate(pred, factMap({}))).toBe(false);
  });
});
