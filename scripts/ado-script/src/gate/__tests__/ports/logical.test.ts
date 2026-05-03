import { describe, expect, it } from "vitest";
import type { PredicateSpec } from "../../../shared/types.gen.js";
import { evaluatePredicate } from "../../predicates.js";
import { factMap } from "./helpers.js";

describe("TestLogicalCombinators", () => {
  it("test and all pass", () => {
    const pred = {
      type: "and",
      operands: [
        { type: "equals", fact: "a", value: "1" },
        { type: "equals", fact: "b", value: "2" },
      ],
    } satisfies PredicateSpec;
    const facts = factMap({ a: "1", b: "2" });
    expect(evaluatePredicate(pred, facts)).toBe(true);
  });

  it("test and one fails", () => {
    const pred = {
      type: "and",
      operands: [
        { type: "equals", fact: "a", value: "1" },
        { type: "equals", fact: "b", value: "3" },
      ],
    } satisfies PredicateSpec;
    const facts = factMap({ a: "1", b: "2" });
    expect(evaluatePredicate(pred, facts)).toBe(false);
  });

  it("test or one passes", () => {
    const pred = {
      type: "or",
      operands: [
        { type: "equals", fact: "a", value: "wrong" },
        { type: "equals", fact: "b", value: "2" },
      ],
    } satisfies PredicateSpec;
    const facts = factMap({ a: "1", b: "2" });
    expect(evaluatePredicate(pred, facts)).toBe(true);
  });

  it("test not", () => {
    const pred = {
      type: "not",
      operand: { type: "equals", fact: "a", value: "1" },
    } satisfies PredicateSpec;
    const facts = factMap({ a: "2" });
    expect(evaluatePredicate(pred, facts)).toBe(true);
  });
});
