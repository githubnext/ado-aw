import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { PredicateSpec } from "../../../shared/types.gen.js";
import { PolicyTracker } from "../../../shared/policy.js";
import { evaluatePredicates } from "../../predicates.js";
import { factMap, gateSpec } from "./helpers.js";

describe("evaluatePredicates integration ports", () => {
  let writes: string[];

  beforeEach(() => {
    writes = [];
    vi.spyOn(process.stdout, "write").mockImplementation((chunk: string | Uint8Array) => {
      writes.push(typeof chunk === "string" ? chunk : Buffer.from(chunk).toString());
      return true;
    });
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("test evaluate predicates with policy tracker", () => {
    const predicate = { type: "equals", fact: "pr_title", value: "ok" } satisfies PredicateSpec;
    const spec = gateSpec(
      [{ name: "title", tag_suffix: "title", predicate }],
      [{ kind: "pr_title", failure_policy: "fail_closed", dependencies: [] }],
    );
    const tracker = new PolicyTracker(spec.facts);

    expect(evaluatePredicates(spec, factMap({ pr_title: "bad" }), tracker)).toEqual(["fail"]);
    expect(tracker.summary()).toEqual({ passed: 0, failed: 1, skipped: 0 });
    expect(writes).toContain("##vso[build.addbuildtag]gate:title\n");
  });
});
