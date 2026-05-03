import { describe, expect, it } from "vitest";
import type { PredicateSpec } from "../../../shared/types.gen.js";
import { evaluatePredicate } from "../../predicates.js";
import { factMap } from "./helpers.js";

describe("TestValueInSet", () => {
  it("test case insensitive match", () => {
    const pred = {
      type: "value_in_set",
      fact: "author_email",
      values: ["Alice@Corp.com"],
      case_insensitive: true,
    } satisfies PredicateSpec;
    const facts = factMap({ author_email: "alice@corp.com" });
    expect(evaluatePredicate(pred, facts)).toBe(true);
  });

  it("test case sensitive no match", () => {
    const pred = {
      type: "value_in_set",
      fact: "author_email",
      values: ["Alice@Corp.com"],
      case_insensitive: false,
    } satisfies PredicateSpec;
    const facts = factMap({ author_email: "alice@corp.com" });
    expect(evaluatePredicate(pred, facts)).toBe(false);
  });

  it("test not in set", () => {
    const pred = {
      type: "value_in_set",
      fact: "build_reason",
      values: ["PullRequest", "Manual"],
      case_insensitive: true,
    } satisfies PredicateSpec;
    const facts = factMap({ build_reason: "Schedule" });
    expect(evaluatePredicate(pred, facts)).toBe(false);
  });
});

describe("TestValueNotInSet", () => {
  it("test not in set", () => {
    const pred = {
      type: "value_not_in_set",
      fact: "author_email",
      values: ["bot@noreply.com"],
      case_insensitive: true,
    } satisfies PredicateSpec;
    const facts = factMap({ author_email: "dev@corp.com" });
    expect(evaluatePredicate(pred, facts)).toBe(true);
  });

  it("test in set", () => {
    const pred = {
      type: "value_not_in_set",
      fact: "author_email",
      values: ["bot@noreply.com"],
      case_insensitive: true,
    } satisfies PredicateSpec;
    const facts = factMap({ author_email: "bot@noreply.com" });
    expect(evaluatePredicate(pred, facts)).toBe(false);
  });
});
