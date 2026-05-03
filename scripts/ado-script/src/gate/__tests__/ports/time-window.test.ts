import { describe, expect, it } from "vitest";
import type { PredicateSpec } from "../../../shared/types.gen.js";
import { evaluatePredicate } from "../../predicates.js";
import { factMap } from "./helpers.js";

describe("TestTimeWindow", () => {
  it("test in window", () => {
    const pred = { type: "time_window", start: "09:00", end: "17:00" } satisfies PredicateSpec;
    const facts = factMap({ current_utc_minutes: 600 });
    expect(evaluatePredicate(pred, facts)).toBe(true);
  });

  it("test outside window", () => {
    const pred = { type: "time_window", start: "09:00", end: "17:00" } satisfies PredicateSpec;
    const facts = factMap({ current_utc_minutes: 1200 });
    expect(evaluatePredicate(pred, facts)).toBe(false);
  });

  it("test overnight window in", () => {
    const pred = { type: "time_window", start: "22:00", end: "06:00" } satisfies PredicateSpec;
    const facts = factMap({ current_utc_minutes: 1380 });
    expect(evaluatePredicate(pred, facts)).toBe(true);
  });

  it("test overnight window out", () => {
    const pred = { type: "time_window", start: "22:00", end: "06:00" } satisfies PredicateSpec;
    const facts = factMap({ current_utc_minutes: 720 });
    expect(evaluatePredicate(pred, facts)).toBe(false);
  });
});
