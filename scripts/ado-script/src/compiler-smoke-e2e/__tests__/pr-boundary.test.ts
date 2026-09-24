import { describe, expect, it } from "vitest";
import { verifyPrBoundary } from "../pr-boundary.js";
import { prepareCaseSource } from "../source.js";
import type { BoundaryTimelineRecord } from "../ado-rest.js";

const succeeded: BoundaryTimelineRecord[] = ["Setup", "Agent", "Detection", "SafeOutputs"]
  .map((identifier) => ({ type: "Job", identifier, result: "succeeded" }));
const rejection = [
  ...succeeded,
  { type: "Job", identifier: "ManualReview", result: "failed" },
  { type: "Job", identifier: "SafeOutputs_Reviewed", result: "skipped" },
];

describe("PR pipeline boundary proof", () => {
  it("requires a real proposal/gate rejection and unchanged PR", () => {
    expect(() => verifyPrBoundary("rejected", 42, "original", "original", rejection)).not.toThrow();
    expect(() => verifyPrBoundary("rejected", 42, "original", "changed", rejection)).toThrow("changed");
    expect(() => verifyPrBoundary("rejected", 42, "original", "original", succeeded)).toThrow("rejection");
    expect(() => verifyPrBoundary("rejected", 42, "original", "original",
      rejection.map((record) => record.identifier === "Detection" ? {...record, result:"failed"} : record)))
      .toThrow("Detection");
  });

  it("does not count a successful noop pipeline as a successful mutation", () => {
    expect(() => verifyPrBoundary("automatic", 42, "original", "original", succeeded)).toThrow("persisted");
    expect(() => verifyPrBoundary("automatic", 42, "original", "ado-aw-pr-boundary-42", succeeded)).not.toThrow();
  });

  it("only passes approved mode after the gate and reviewed executor succeeded", () => {
    expect(() => verifyPrBoundary("approved", 42, "original", "ado-aw-pr-boundary-42", rejection)).toThrow("Approved");
    const approved = rejection.map((record) => ({...record, result:"succeeded"}));
    expect(() => verifyPrBoundary("approved", 42, "original", "ado-aw-pr-boundary-42", approved)).not.toThrow();
  });

  it("retains synthetic setup without enabling push triggers and defaults the gate to reject", () => {
    const source = "---\nname: test\ndescription: test\nsafe-outputs:\n  update-pull-request: {}\n---\nBody unchanged.\n";
    const result = prepareCaseSource(source, undefined, "rejected");
    expect(result).toContain("mode: synthetic");
    expect(result).toContain("push: none");
    expect(result).toContain("on-timeout: reject");
    expect(result).toContain("timeout-minutes: 1");
    expect(result.endsWith("Body unchanged.\n")).toBe(true);
  });
});
