import { describe, expect, it } from "vitest";
import type { PredicateSpec } from "../../../shared/types.gen.js";
import { evaluatePredicate } from "../../predicates.js";
import { factMap } from "./helpers.js";

describe("TestNumericRange", () => {
  it("test in range", () => {
    const pred = { type: "numeric_range", fact: "changed_file_count", min: 5, max: 100 } satisfies PredicateSpec;
    const facts = factMap({ changed_file_count: 50 });
    expect(evaluatePredicate(pred, facts)).toBe(true);
  });

  it("test below min", () => {
    const pred = { type: "numeric_range", fact: "changed_file_count", min: 5, max: 100 } satisfies PredicateSpec;
    const facts = factMap({ changed_file_count: 2 });
    expect(evaluatePredicate(pred, facts)).toBe(false);
  });

  it("test above max", () => {
    const pred = { type: "numeric_range", fact: "changed_file_count", min: 5, max: 100 } satisfies PredicateSpec;
    const facts = factMap({ changed_file_count: 200 });
    expect(evaluatePredicate(pred, facts)).toBe(false);
  });

  it("test min only", () => {
    const pred = { type: "numeric_range", fact: "changed_file_count", min: 3 } satisfies PredicateSpec;
    const facts = factMap({ changed_file_count: 10 });
    expect(evaluatePredicate(pred, facts)).toBe(true);
  });

  it("test max only", () => {
    const pred = { type: "numeric_range", fact: "changed_file_count", max: 50 } satisfies PredicateSpec;
    const facts = factMap({ changed_file_count: 100 });
    expect(evaluatePredicate(pred, facts)).toBe(false);
  });
});
