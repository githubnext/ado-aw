import { describe, expect, it } from "vitest";

import {
  buildGateSpec,
  changeCountCheck,
  changedFilesCheck,
  draftCheck,
  encodeGateSpec,
  encodePrSynthSpec,
  labelsCheck,
  targetBranchCheck,
  titleCheck,
} from "../gate-spec.js";

const factKinds = (checks: Parameters<typeof buildGateSpec>[1]) =>
  buildGateSpec("pull-request", checks).facts.map((f) => f.kind);

describe("buildGateSpec fact derivation", () => {
  it("emits no facts and no checks for an empty gate", () => {
    const spec = buildGateSpec("pull-request", []);
    expect(spec.facts).toEqual([]);
    expect(spec.checks).toEqual([]);
    expect(spec.context.tag_prefix).toBe("pr-gate");
    expect(spec.context.build_reason).toBe("PullRequest");
    expect(spec.context.bypass_label).toBe("PR");
  });

  it("derives pr_metadata (skip_dependents) before pr_labels (fail_open) for a labels check", () => {
    const spec = buildGateSpec("pull-request", [labelsCheck({ anyOf: ["run-agent"] })]);
    const kinds = spec.facts.map((f) => f.kind);
    expect(kinds).toEqual(["pr_metadata", "pr_labels"]);
    const meta = Object.fromEntries(spec.facts.map((f) => [f.kind, f]));
    expect(meta.pr_metadata!.failure_policy).toBe("skip_dependents");
    expect(meta.pr_metadata!.dependencies).toEqual([]);
    expect(meta.pr_labels!.failure_policy).toBe("fail_open");
    expect(meta.pr_labels!.dependencies).toEqual(["pr_metadata"]);
  });

  it("derives pr_metadata before pr_is_draft (fail_closed) for a draft check", () => {
    expect(factKinds([draftCheck(true)])).toEqual(["pr_metadata", "pr_is_draft"]);
  });

  it("derives changed_files before changed_file_count for a change-count check", () => {
    expect(factKinds([changeCountCheck({ min: 5 })])).toEqual([
      "changed_files",
      "changed_file_count",
    ]);
  });

  it("derives only changed_files for a changed-files check", () => {
    expect(factKinds([changedFilesCheck({ include: ["src/**"] })])).toEqual(["changed_files"]);
  });

  it("derives a single fail_closed pipeline-var fact for a title check", () => {
    const spec = buildGateSpec("pull-request", [titleCheck("release *")]);
    expect(spec.facts).toEqual([
      { kind: "pr_title", failure_policy: "fail_closed", dependencies: [] },
    ]);
  });

  it("dedupes facts shared across multiple checks", () => {
    const kinds = factKinds([draftCheck(true), labelsCheck({ anyOf: ["x"] })]);
    // pr_metadata appears once, before both dependents, in enum order.
    expect(kinds).toEqual(["pr_metadata", "pr_is_draft", "pr_labels"]);
  });
});

describe("check tag suffixes match the Rust FilterCheck::build_tag_suffix", () => {
  it("emits the expected suffixes", () => {
    const cases: [Parameters<typeof buildGateSpec>[1][number], string][] = [
      [titleCheck("x"), "title-mismatch"],
      [labelsCheck({ anyOf: ["x"] }), "labels-mismatch"],
      [changedFilesCheck({ include: ["src/**"] }), "changed-files-mismatch"],
      [draftCheck(true), "draft-mismatch"],
      [targetBranchCheck("main"), "target-branch-mismatch"],
      [changeCountCheck({ min: 1 }), "changes-mismatch"],
    ];
    for (const [check, suffix] of cases) {
      const spec = buildGateSpec("pull-request", [check]);
      expect(spec.checks[0]!.tag_suffix).toBe(suffix);
    }
  });
});

describe("encoding", () => {
  it("round-trips a gate spec through base64", () => {
    const spec = buildGateSpec("pull-request", [labelsCheck({ anyOf: ["run-agent"] })]);
    const decoded = JSON.parse(Buffer.from(encodeGateSpec(spec), "base64").toString("utf8"));
    expect(decoded).toEqual(spec);
  });

  it("produces the canonical empty PR_SYNTH_SPEC base64", () => {
    expect(encodePrSynthSpec()).toBe(
      "eyJicmFuY2hlcyI6eyJpbmNsdWRlIjpbXSwiZXhjbHVkZSI6W119LCJwYXRocyI6eyJpbmNsdWRlIjpbXSwiZXhjbHVkZSI6W119fQ==",
    );
  });

  it("encodes provided branch/path filters", () => {
    const decoded = JSON.parse(
      Buffer.from(
        encodePrSynthSpec({ branches: { include: ["main"] }, paths: { exclude: ["docs/**"] } }),
        "base64",
      ).toString("utf8"),
    );
    expect(decoded).toEqual({
      branches: { include: ["main"], exclude: [] },
      paths: { include: [], exclude: ["docs/**"] },
    });
  });
});
