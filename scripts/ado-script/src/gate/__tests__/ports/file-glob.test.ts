import { describe, expect, it } from "vitest";
import type { PredicateSpec } from "../../../shared/types.gen.js";
import { evaluatePredicate } from "../../predicates.js";
import { factMap } from "./helpers.js";

describe("TestFileGlobMatch", () => {
  it("test include match", () => {
    const pred = {
      type: "file_glob_match",
      fact: "changed_files",
      include: ["src/*.rs"],
    } as PredicateSpec;
    const facts = factMap({ changed_files: ["src/main.rs", "src/lib.rs"] });
    expect(evaluatePredicate(pred, facts)).toBe(true);
  });

  it("test include no match", () => {
    const pred = {
      type: "file_glob_match",
      fact: "changed_files",
      include: ["src/**/*.rs"],
    } as PredicateSpec;
    const facts = factMap({ changed_files: ["docs/readme.md"] });
    expect(evaluatePredicate(pred, facts)).toBe(false);
  });

  it("test exclude", () => {
    const pred = {
      type: "file_glob_match",
      fact: "changed_files",
      include: ["src/**/*.rs"],
      exclude: ["src/test_*.rs"],
    } as PredicateSpec;
    const facts = factMap({ changed_files: ["src/test_main.rs"] });
    expect(evaluatePredicate(pred, facts)).toBe(false);
  });

  it("test empty file list with include fails", () => {
    const pred = {
      type: "file_glob_match",
      fact: "changed_files",
      include: ["src/*.rs"],
    } as PredicateSpec;
    const facts = factMap({ changed_files: [] });
    expect(evaluatePredicate(pred, facts)).toBe(false);
  });

  it("test empty file list with exclude only passes", () => {
    const pred = {
      type: "file_glob_match",
      fact: "changed_files",
      exclude: ["src/generated/*"],
    } as PredicateSpec;
    const facts = factMap({ changed_files: [] });
    expect(evaluatePredicate(pred, facts)).toBe(true);
  });

  it("test include and exclude allows non excluded match", () => {
    const pred = {
      type: "file_glob_match",
      fact: "changed_files",
      include: ["src/*.rs"],
      exclude: ["src/test_*.rs"],
    } as PredicateSpec;
    const facts = factMap({ changed_files: ["src/test_main.rs", "src/lib.rs"] });
    expect(evaluatePredicate(pred, facts)).toBe(true);
  });
});
