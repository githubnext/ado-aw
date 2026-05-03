import { describe, expect, it } from "vitest";
import type { PredicateSpec } from "../../../shared/types.gen.js";
import { evaluatePredicate } from "../../predicates.js";
import { factMap } from "./helpers.js";

describe("TestLabelSetMatch", () => {
  it("test any of match", () => {
    const pred = {
      type: "label_set_match",
      fact: "pr_labels",
      any_of: ["run-agent", "needs-review"],
    } as PredicateSpec;
    const facts = factMap({ pr_labels: ["run-agent", "other"] });
    expect(evaluatePredicate(pred, facts)).toBe(true);
  });

  it("test any of no match", () => {
    const pred = {
      type: "label_set_match",
      fact: "pr_labels",
      any_of: ["run-agent"],
    } as PredicateSpec;
    const facts = factMap({ pr_labels: ["other"] });
    expect(evaluatePredicate(pred, facts)).toBe(false);
  });

  it("test all of match", () => {
    const pred = {
      type: "label_set_match",
      fact: "pr_labels",
      all_of: ["approved", "tested"],
    } as PredicateSpec;
    const facts = factMap({ pr_labels: ["approved", "tested", "other"] });
    expect(evaluatePredicate(pred, facts)).toBe(true);
  });

  it("test all of missing", () => {
    const pred = {
      type: "label_set_match",
      fact: "pr_labels",
      all_of: ["approved", "tested"],
    } as PredicateSpec;
    const facts = factMap({ pr_labels: ["approved"] });
    expect(evaluatePredicate(pred, facts)).toBe(false);
  });

  it("test none of pass", () => {
    const pred = {
      type: "label_set_match",
      fact: "pr_labels",
      none_of: ["do-not-run"],
    } as PredicateSpec;
    const facts = factMap({ pr_labels: ["run-agent"] });
    expect(evaluatePredicate(pred, facts)).toBe(true);
  });

  it("test none of fail", () => {
    const pred = {
      type: "label_set_match",
      fact: "pr_labels",
      none_of: ["do-not-run"],
    } as PredicateSpec;
    const facts = factMap({ pr_labels: ["do-not-run", "other"] });
    expect(evaluatePredicate(pred, facts)).toBe(false);
  });

  it("test empty labels", () => {
    const pred = { type: "label_set_match", fact: "pr_labels" } as PredicateSpec;
    const facts = factMap({ pr_labels: [] });
    expect(evaluatePredicate(pred, facts)).toBe(true);
  });

  it("test case insensitive labels", () => {
    const pred = {
      type: "label_set_match",
      fact: "pr_labels",
      any_of: ["run-agent"],
    } as PredicateSpec;
    const facts = factMap({ pr_labels: ["Run-Agent", "other"] });
    expect(evaluatePredicate(pred, facts)).toBe(true);
  });
});
