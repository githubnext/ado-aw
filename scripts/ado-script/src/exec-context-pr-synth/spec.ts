/**
 * Decodes the compiler-emitted `PR_SYNTH_SPEC` env var into a typed
 * filter spec. The compiler builds this via
 * `crate::compile::filter_ir::build_pr_synth_spec` (Rust side); both
 * shapes must stay in lock-step.
 *
 * Decode failures are HARD errors — a malformed spec indicates either
 * compiler corruption or a manual env tamper. Treating it as a soft
 * skip would silently widen the permitted attack surface (any branch
 * could fool the synth path into matching).
 */

export interface PrSynthSpec {
  branches: { include: string[]; exclude: string[] };
  paths: { include: string[]; exclude: string[] };
}

export function decodeSpec(b64: string | undefined): PrSynthSpec {
  if (!b64 || b64.length === 0) {
    throw new Error("PR_SYNTH_SPEC env var is missing or empty");
  }
  let json: string;
  try {
    json = Buffer.from(b64, "base64").toString("utf8");
  } catch (e) {
    throw new Error(`PR_SYNTH_SPEC: base64 decode failed: ${(e as Error).message}`);
  }
  let parsed: unknown;
  try {
    parsed = JSON.parse(json);
  } catch (e) {
    throw new Error(`PR_SYNTH_SPEC: JSON parse failed: ${(e as Error).message}`);
  }
  return validate(parsed);
}

function validate(value: unknown): PrSynthSpec {
  if (!value || typeof value !== "object") {
    throw new Error("PR_SYNTH_SPEC: expected object at root");
  }
  const obj = value as Record<string, unknown>;
  return {
    branches: validateGlobs(obj.branches, "branches"),
    paths: validateGlobs(obj.paths, "paths"),
  };
}

function validateGlobs(
  value: unknown,
  fieldName: string,
): { include: string[]; exclude: string[] } {
  if (!value || typeof value !== "object") {
    throw new Error(`PR_SYNTH_SPEC: expected ${fieldName} to be an object`);
  }
  const obj = value as Record<string, unknown>;
  return {
    include: validateStringArray(obj.include, `${fieldName}.include`),
    exclude: validateStringArray(obj.exclude, `${fieldName}.exclude`),
  };
}

function validateStringArray(value: unknown, fieldName: string): string[] {
  if (!Array.isArray(value)) {
    throw new Error(`PR_SYNTH_SPEC: expected ${fieldName} to be an array of strings`);
  }
  for (const item of value) {
    if (typeof item !== "string") {
      throw new Error(`PR_SYNTH_SPEC: ${fieldName} contains non-string entry`);
    }
  }
  return value as string[];
}
