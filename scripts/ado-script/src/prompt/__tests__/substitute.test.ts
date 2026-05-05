import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { substitute } from "../substitute.js";

describe("substitute", () => {
  const originalEnv = process.env;

  beforeEach(() => {
    process.env = { ...originalEnv };
  });

  afterEach(() => {
    process.env = originalEnv;
  });

  it("substitutes a declared parameter", () => {
    process.env.ADO_AW_PARAM_TARGET = "main";
    const out = substitute(
      "branch: ${{ parameters.target }}",
      ["target"],
      () => {},
    );
    expect(out).toBe("branch: main");
  });

  it("converts hyphen in param name to underscore in env name", () => {
    process.env.ADO_AW_PARAM_DRY_RUN = "yes";
    const out = substitute(
      "x: ${{ parameters.dry-run }}",
      ["dry-run"],
      () => {},
    );
    expect(out).toBe("x: yes");
  });

  it("leaves an undeclared parameter verbatim with a warning", () => {
    const warnings: string[] = [];
    const out = substitute(
      "x: ${{ parameters.unknown }}",
      [],
      (m) => warnings.push(m),
    );
    expect(out).toBe("x: ${{ parameters.unknown }}");
    expect(warnings.length).toBeGreaterThan(0);
    expect(warnings[0]).toContain("unknown");
  });

  it("leaves a declared but unmapped parameter verbatim with a warning", () => {
    delete process.env.ADO_AW_PARAM_TARGET;
    const warnings: string[] = [];
    const out = substitute(
      "x: ${{ parameters.target }}",
      ["target"],
      (m) => warnings.push(m),
    );
    expect(out).toBe("x: ${{ parameters.target }}");
    expect(warnings.length).toBe(1);
  });

  it("substitutes pipeline variables via dot-to-underscore convention", () => {
    process.env.BUILD_BUILDID = "12345";
    const out = substitute("build: $(Build.BuildId)", [], () => {});
    expect(out).toBe("build: 12345");
  });

  it("leaves unset pipeline variables verbatim with a warning", () => {
    delete process.env.MISSING;
    const warnings: string[] = [];
    const out = substitute("v: $(Missing)", [], (m) => warnings.push(m));
    expect(out).toBe("v: $(Missing)");
    expect(warnings.length).toBe(1);
  });

  it("respects backslash-escaped $(...) literals", () => {
    process.env.FOO = "should not be used";
    const out = substitute("literal: \\$(Foo)", [], () => {});
    expect(out).toBe("literal: $(Foo)");
  });

  it("warns on $[ ... ] expressions and leaves them verbatim", () => {
    const warnings: string[] = [];
    const out = substitute(
      "exp: $[ counter('foo', 0) ]",
      [],
      (m) => warnings.push(m),
    );
    expect(out).toBe("exp: $[ counter('foo', 0) ]");
    expect(warnings.some((w) => w.includes("$[..."))).toBe(true);
  });

  it("substitutes parameters appearing multiple times", () => {
    process.env.ADO_AW_PARAM_X = "Y";
    const src = "${{ parameters.x }} appears twice: ${{ parameters.x }}";
    expect(substitute(src, ["x"], () => {})).toBe("Y appears twice: Y");
  });

  it("does not match $( without an identifier", () => {
    const out = substitute("foo $() bar", [], () => {});
    expect(out).toBe("foo $() bar");
  });

  it("does not match malformed parameter expressions", () => {
    const warnings: string[] = [];
    const out = substitute(
      "x: ${{ parameters.123bad }}",
      ["123bad"],
      (m) => warnings.push(m),
    );
    // Identifier must start with letter or underscore — regex won't match,
    // so the literal is left intact and no warning emitted.
    expect(out).toBe("x: ${{ parameters.123bad }}");
    expect(warnings.length).toBe(0);
  });
});
