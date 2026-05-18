import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { substitute, type WarnFn } from "../substitute.js";

const ORIGINAL_ENV = { ...process.env };

beforeEach(() => {
  // Clear any ADO_*/ADO_AW_PARAM_*/PIPELINE_* env vars between tests so
  // accidental host env doesn't leak into assertions.
  for (const k of Object.keys(process.env)) {
    if (
      k.startsWith("ADO_AW_PARAM_") ||
      k.startsWith("ADO_") ||
      k.startsWith("BUILD_") ||
      k.startsWith("SYSTEM_") ||
      k.startsWith("MYVAR")
    ) {
      delete process.env[k];
    }
  }
});

afterEach(() => {
  process.env = { ...ORIGINAL_ENV };
});

function noopWarn(): WarnFn {
  return vi.fn();
}

function collectWarn(): { warn: WarnFn; warnings: string[] } {
  const warnings: string[] = [];
  return { warn: (m: string) => warnings.push(m), warnings };
}

describe("substitute", () => {
  it("leaves a string with no tokens unchanged", () => {
    expect(substitute("just plain text", [], noopWarn())).toBe(
      "just plain text",
    );
  });

  it("substitutes a declared parameter from env", () => {
    process.env["ADO_AW_PARAM_TARGET"] = "main";
    const out = substitute(
      "Build ${{ parameters.target }} now.",
      ["target"],
      noopWarn(),
    );
    expect(out).toBe("Build main now.");
  });

  it("handles hyphenated parameter names (hyphen → underscore in env)", () => {
    process.env["ADO_AW_PARAM_TARGET_BRANCH"] = "release/1.0";
    const out = substitute(
      "Branch ${{ parameters.target-branch }}",
      ["target-branch"],
      noopWarn(),
    );
    expect(out).toBe("Branch release/1.0");
  });

  it("leaves an undeclared parameter verbatim and warns once", () => {
    const { warn, warnings } = collectWarn();
    const out = substitute(
      "Hi ${{ parameters.nope }} and ${{ parameters.nope }} again.",
      ["target"],
      warn,
    );
    expect(out).toBe(
      "Hi ${{ parameters.nope }} and ${{ parameters.nope }} again.",
    );
    expect(warnings).toHaveLength(1);
    expect(warnings[0]).toMatch(/Unknown parameter 'nope'/);
  });

  it("leaves a declared-but-unset parameter verbatim and warns once", () => {
    const { warn, warnings } = collectWarn();
    const out = substitute("Run ${{ parameters.target }}.", ["target"], warn);
    expect(out).toBe("Run ${{ parameters.target }}.");
    expect(warnings).toHaveLength(1);
    expect(warnings[0]).toMatch(/env var 'ADO_AW_PARAM_TARGET' is unset/);
  });

  it("substitutes $(VAR) from env", () => {
    process.env["BUILD_ID"] = "42";
    expect(substitute("Run $(Build.Id)", [], noopWarn())).toBe("Run 42");
  });

  it("substitutes $(VAR) with simple uppercase mapping", () => {
    process.env["MYVAR"] = "hello";
    expect(substitute("$(myvar)", [], noopWarn())).toBe("hello");
  });

  it("leaves $(VAR) verbatim and warns when env is unset", () => {
    const { warn, warnings } = collectWarn();
    const out = substitute("Secret $(MissingSecret)", [], warn);
    expect(out).toBe("Secret $(MissingSecret)");
    expect(warnings).toHaveLength(1);
    expect(warnings[0]).toMatch(/MISSINGSECRET/);
  });

  it("does NOT re-expand $(...) injected via a parameter value", () => {
    // SECURITY: the chaining attack flagged by PR #395's bot review.
    // A user queues the pipeline with `target = "$(System.AccessToken)"`.
    // We must NOT then expand that `$(System.AccessToken)` against the
    // env in a second pass.
    process.env["ADO_AW_PARAM_TARGET"] = "$(System.AccessToken)";
    process.env["SYSTEM_ACCESSTOKEN"] = "SECRET";
    const out = substitute(
      "target=${{ parameters.target }}",
      ["target"],
      noopWarn(),
    );
    expect(out).toBe("target=$(System.AccessToken)");
    expect(out).not.toContain("SECRET");
  });

  it("does NOT re-expand ${{ parameters.* }} injected via $(VAR)", () => {
    // The mirror attack: an env var value containing a parameter token.
    // Substitution must stay single-pass in both directions.
    process.env["INJECTED"] = "${{ parameters.secret }}";
    process.env["ADO_AW_PARAM_SECRET"] = "leaked";
    const out = substitute("v=$(INJECTED)", ["secret"], noopWarn());
    expect(out).toBe("v=${{ parameters.secret }}");
    expect(out).not.toContain("leaked");
  });

  it("strips the backslash from \\$(VAR) and leaves $(VAR) literal", () => {
    process.env["X"] = "should not appear";
    const out = substitute("literal: \\$(X)", [], noopWarn());
    expect(out).toBe("literal: $(X)");
  });

  it("leaves $[...] expressions verbatim and warns once", () => {
    const { warn, warnings } = collectWarn();
    const out = substitute(
      "ver = $[counter('x',0)] and again $[counter('x',0)]",
      [],
      warn,
    );
    expect(out).toBe("ver = $[counter('x',0)] and again $[counter('x',0)]");
    expect(warnings).toHaveLength(1);
    expect(warnings[0]).toMatch(/Runtime expression/);
  });

  it("substitutes multiple distinct tokens in a single pass", () => {
    process.env["ADO_AW_PARAM_TARGET"] = "main";
    process.env["BUILD_ID"] = "99";
    const out = substitute(
      "build $(Build.Id) of ${{ parameters.target }}",
      ["target"],
      noopWarn(),
    );
    expect(out).toBe("build 99 of main");
  });

  it("warns at most once per distinct unresolved token", () => {
    const { warn, warnings } = collectWarn();
    substitute("$(X) $(Y) $(X) $(Y) $(X)", [], warn);
    // Two distinct env vars → two warnings, regardless of how many
    // times each appears.
    expect(warnings).toHaveLength(2);
  });

  it("preserves surrounding whitespace and punctuation", () => {
    process.env["ADO_AW_PARAM_NAME"] = "Sue";
    const out = substitute(
      "Hi, ${{ parameters.name }}!",
      ["name"],
      noopWarn(),
    );
    expect(out).toBe("Hi, Sue!");
  });
});
