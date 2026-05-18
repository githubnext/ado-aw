import { describe, expect, it } from "vitest";
import type { PredicateSpec } from "../../../shared/types.gen.js";
import { predicateFacts } from "../../predicates.js";

describe("TestPredicateFacts", () => {
  it("test simple", () => {
    const pred = { type: "glob_match", fact: "pr_title", pattern: "test" } satisfies PredicateSpec;
    expect(predicateFacts(pred)).toEqual(["pr_title"]);
  });

  it("test compound", () => {
    const pred = {
      type: "and",
      operands: [
        { type: "equals", fact: "a", value: "1" },
        { type: "glob_match", fact: "b", pattern: "x" },
      ],
    } satisfies PredicateSpec;
    expect(new Set(predicateFacts(pred))).toEqual(new Set(["a", "b"]));
  });

  it("test not", () => {
    const pred = {
      type: "not",
      operand: { type: "equals", fact: "x", value: "1" },
    } satisfies PredicateSpec;
    expect(predicateFacts(pred)).toEqual(["x"]);
  });
});
