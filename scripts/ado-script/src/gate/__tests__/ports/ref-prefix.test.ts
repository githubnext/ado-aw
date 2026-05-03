import { describe, expect, it } from "vitest";
import type { PredicateSpec } from "../../../shared/types.gen.js";
import { stripRefPrefix } from "../../../shared/env-facts.js";
import { evaluatePredicate } from "../../predicates.js";
import { factMap } from "./helpers.js";

describe("TestStripRefPrefix", () => {
  it("test refs heads", () => {
    expect(stripRefPrefix("refs/heads/feature/my-branch")).toBe("feature/my-branch");
  });

  it("test refs tags", () => {
    expect(stripRefPrefix("refs/tags/v1.0.0")).toBe("v1.0.0");
  });

  it("test refs pull", () => {
    expect(stripRefPrefix("refs/pull/42/merge")).toBe("42/merge");
  });

  it("test no prefix", () => {
    expect(stripRefPrefix("main")).toBe("main");
  });

  it("test pattern stripping in glob", () => {
    const pred = {
      type: "glob_match",
      fact: "source_branch",
      pattern: "refs/heads/feature/*",
    } satisfies PredicateSpec;
    const facts = factMap({ source_branch: "feature/my-branch" });
    expect(evaluatePredicate(pred, facts)).toBe(true);
  });
});
