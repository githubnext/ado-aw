/**
 * Predicate evaluation.
 *
 * Each predicate variant
 * is ported exactly; behavioral parity is verified by ports of the
 * existing Python parametric tests.
 */
import type { GateSpec, PredicateSpec } from "../shared/types.gen.js";
import type { PolicyTracker } from "../shared/policy.js";
import { addBuildTag, logWarning } from "../shared/vso-logger.js";
import { stripRefPrefix, BRANCH_FACTS } from "../shared/env-facts.js";

type CheckResult = "pass" | "fail" | "skip";

// Set<FactKind>.has(p.fact) is rejected because p.fact is `string`. The
// type-system narrowing isn't useful here — we just want runtime membership.
const isBranchFact = (fact: string): boolean =>
  (BRANCH_FACTS as ReadonlySet<string>).has(fact);

// BRANCH_FACTS is sourced from env-facts.ts so the read-time strip (in
// readEnvFact) and the match-time strip (here in glob_match below) cannot
// drift. Adding a new branch-shaped fact requires updating exactly one set.

export function evaluatePredicates(
  spec: GateSpec,
  facts: Map<string, unknown>,
  tracker: PolicyTracker,
): CheckResult[] {
  const results: CheckResult[] = [];

  for (const check of spec.checks) {
    const refs = predicateFacts(check.predicate);
    const policyVerdict = tracker.verdictForMissingFacts(refs);
    let result: CheckResult;

    if (policyVerdict === "evaluate") {
      result = evaluatePredicate(check.predicate, facts) ? "pass" : "fail";
    } else {
      result = policyVerdict;
    }

    if (result === "fail") {
      addBuildTag(`${spec.context.tag_prefix}:${check.tag_suffix}`);
    }
    tracker.recordCheckResult(result);
    results.push(result);
  }

  return results;
}

export function predicateFacts(p: PredicateSpec): string[] {
  const out = new Set<string>();
  collectFacts(p, out);
  return [...out];
}

function collectFacts(p: PredicateSpec, out: Set<string>): void {
  const fact = (p as { fact?: unknown }).fact;
  if (typeof fact === "string") {
    out.add(fact);
  }

  if (p.type === "time_window") {
    out.add("current_utc_minutes");
  }

  if (p.type === "and" || p.type === "or") {
    for (const sub of p.operands) collectFacts(sub, out);
  }

  if (p.type === "not") {
    collectFacts(p.operand, out);
  }
}

export function evaluatePredicate(p: PredicateSpec, facts: Map<string, unknown>): boolean {
  switch (p.type) {
    case "glob_match": {
      const value = String(facts.get(p.fact) ?? "");
      const pattern = isBranchFact(p.fact) ? stripRefPrefix(p.pattern) : p.pattern;
      return globMatch(value, pattern);
    }
    case "equals":
      return String(facts.get(p.fact) ?? "") === p.value;
    case "value_in_set": {
      const value = String(facts.get(p.fact) ?? "");
      if (p.case_insensitive) {
        const lower = p.values.map((v) => v.toLowerCase());
        return lower.includes(value.toLowerCase());
      }
      return p.values.includes(value);
    }
    case "value_not_in_set": {
      const value = String(facts.get(p.fact) ?? "");
      if (p.case_insensitive) {
        const lower = p.values.map((v) => v.toLowerCase());
        return !lower.includes(value.toLowerCase());
      }
      return !p.values.includes(value);
    }
    case "numeric_range": {
      const raw = facts.get(p.fact);
      // Fail-closed if the fact is missing or non-numeric. The PolicyTracker
      // normally short-circuits before evaluatePredicate is reached for a
      // missing fact, but defending here independently means a future change
      // to the policy gate can't silently cause a missing fact to satisfy a
      // range that includes 0 (the previous `?? 0` default did exactly that).
      if (raw === undefined || raw === null) return false;
      const value = Number(raw);
      if (!Number.isFinite(value)) return false;
      if (p.min !== undefined && p.min !== null && value < p.min) return false;
      if (p.max !== undefined && p.max !== null && value > p.max) return false;
      return true;
    }
    case "time_window": {
      const current = Number(facts.get("current_utc_minutes") ?? 0);
      const start = parseHm(p.start);
      const end = parseHm(p.end);
      if (start <= end) return current >= start && current < end;
      return current >= start || current < end;
    }
    case "label_set_match": {
      const labels = stringsFromFact(facts.get(p.fact) ?? []);
      const labelsLower = labels.map((l) => l.toLowerCase());
      const anyOf = p.any_of ?? [];
      const allOf = p.all_of ?? [];
      const noneOf = p.none_of ?? [];
      if (anyOf.length > 0 && !anyOf.some((a) => labelsLower.includes(a.toLowerCase()))) {
        return false;
      }
      if (allOf.length > 0 && !allOf.every((a) => labelsLower.includes(a.toLowerCase()))) {
        return false;
      }
      if (noneOf.length > 0 && noneOf.some((n) => labelsLower.includes(n.toLowerCase()))) {
        return false;
      }
      return true;
    }
    case "file_glob_match": {
      const files = stringsFromFact(facts.get(p.fact) ?? []);
      const includes = p.include ?? [];
      const excludes = p.exclude ?? [];
      if (files.length === 0) {
        return includes.length === 0;
      }
      for (const f of files) {
        const inc = includes.length === 0 || includes.some((pat) => globMatch(f, pat));
        const exc = excludes.some((pat) => globMatch(f, pat));
        if (inc && !exc) return true;
      }
      return false;
    }
    case "and":
      return p.operands.every((sub) => evaluatePredicate(sub, facts));
    case "or":
      return p.operands.some((sub) => evaluatePredicate(sub, facts));
    case "not":
      return !evaluatePredicate(p.operand, facts);
    default: {
      // Unknown predicate type — likely a newer compiler emitted a spec
      // a bundled gate.js doesn't recognise. Surface in pipeline logs;
      // fail-closed so the missing logic doesn't silently auto-pass.
      const unknownType = (p as { type?: unknown }).type;
      logWarning(
        `Unknown predicate type '${String(unknownType)}'; failing closed. ` +
          "Update scripts/ado-script/gate.js (or the bundled ado-script.zip) to a " +
          "release that supports this predicate.",
      );
      return false;
    }
  }
}

function stringsFromFact(raw: unknown): string[] {
  if (Array.isArray(raw)) return raw.map(String);
  return String(raw)
    .split(/\r?\n/)
    .map((s) => s.trim())
    .filter(Boolean);
}

// Glob-hardening caps. Patterns come from the Rust IR (the compiler is
// the trust boundary), so these are belt-and-braces caps that bound
// pathological worst-case behaviour rather than a defence against a
// realistic attacker. A 1024-char glob pattern is already nonsensical;
// 64 `*` wildcards in one pattern produces a regex that backtracks
// catastrophically against non-matching inputs.
const MAX_GLOB_PATTERN_LEN = 1024;
const MAX_GLOB_WILDCARDS = 64;
const MAX_GLOB_CACHE_ENTRIES = 1024;

// Pre-compiled regex cache. The gate process is one-shot per pipeline run,
// so an unbounded cache would be fine for memory — we still cap defensively
// so a future caller in a longer-lived process doesn't bloat indefinitely.
//
// IMPORTANT: the cache key is `pattern` alone. The compiled RegExp uses the
// fixed `"s"` flag (dotall). If a future caller wants to vary flags (e.g.
// case-insensitive globs), it must change the cache key to include flags —
// e.g. `${pattern}|${flags}` — otherwise the cache will silently return a
// regex compiled with the wrong flags for the same pattern string.
const globRegexCache = new Map<string, RegExp | null>();

function compileGlobRegex(pattern: string): RegExp | null {
  const cached = globRegexCache.get(pattern);
  if (cached !== undefined) return cached;

  if (pattern.length > MAX_GLOB_PATTERN_LEN) {
    logWarning(
      `globMatch: pattern length ${pattern.length} exceeds cap ${MAX_GLOB_PATTERN_LEN}; rejecting (fail-closed)`,
    );
    cacheGlobResult(pattern, null);
    return null;
  }

  let wildcardCount = 0;
  for (let i = 0; i < pattern.length; i++) {
    if (pattern.charCodeAt(i) === 42 /* '*' */) {
      wildcardCount++;
      if (wildcardCount > MAX_GLOB_WILDCARDS) {
        logWarning(
          `globMatch: pattern contains more than ${MAX_GLOB_WILDCARDS} '*' wildcards; rejecting (fail-closed)`,
        );
        cacheGlobResult(pattern, null);
        return null;
      }
    }
  }

  if (/\[/.test(pattern)) {
    logWarning(
      `globMatch: pattern "${pattern}" contains "[" which is treated as a literal, not a character class`,
    );
  }

  const escaped = pattern.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const body = escaped.replace(/\\\*/g, ".*").replace(/\\\?/g, ".");
  const compiled = new RegExp(`^${body}$`, "s");
  cacheGlobResult(pattern, compiled);
  return compiled;
}

function cacheGlobResult(pattern: string, compiled: RegExp | null): void {
  if (globRegexCache.size >= MAX_GLOB_CACHE_ENTRIES) {
    // Drop the oldest entry. Map iteration is insertion-ordered in JS,
    // so .keys().next().value gives us the oldest.
    const oldest = globRegexCache.keys().next().value;
    if (oldest !== undefined) globRegexCache.delete(oldest);
  }
  globRegexCache.set(pattern, compiled);
}

/** For tests only: reset the glob regex cache. */
export function _resetGlobCacheForTesting(): void {
  globRegexCache.clear();
}

function globMatch(value: string, pattern: string): boolean {
  // Glob → regex: only `*` (any chars) and `?` (single char) are
  // recognised. Bracket expressions like `[abc]` are treated as literals.
  // The IR currently never emits bracket patterns; warn if one appears so
  // a compiler/evaluator parity drift is caught early.
  const regex = compileGlobRegex(pattern);
  if (regex === null) return false;
  return regex.test(value);
}

function parseHm(s: string): number {
  const [h, m] = s.split(":").map((n) => parseInt(n, 10));
  return (h ?? 0) * 60 + (m ?? 0);
}

// Known predicate `type` discriminants. Kept in sync manually with the
// switch in evaluatePredicate; the colocation makes drift obvious in
// review. The codegen'd types.gen.ts is the source of truth for the
// type names — if it adds a variant, this set must too.
const KNOWN_PREDICATE_TYPES: ReadonlySet<string> = new Set([
  "glob_match",
  "equals",
  "value_in_set",
  "value_not_in_set",
  "numeric_range",
  "time_window",
  "label_set_match",
  "file_glob_match",
  "and",
  "or",
  "not",
]);

/**
 * Recursively walk a predicate tree and throw on any unknown `type`
 * discriminant. Run *before* fact acquisition so version drift between
 * compiler and bundled evaluator surfaces as a fast, loud failure rather
 * than a silent skip when the required fact happens to be unavailable
 * (`PolicyTracker.verdictForMissingFacts` would otherwise short-circuit
 * `evaluatePredicate` and the fail-closed default would never run).
 *
 * Throws `Error` with a clear message naming the offending type and
 * pointing at the version-mismatch likely cause. Caller is expected to
 * translate this into a `##vso[task.logissue type=error]` + Failed
 * complete via the index.ts entry point.
 */
export function validatePredicateTree(p: PredicateSpec): void {
  const node = p as { type?: unknown; operands?: unknown; operand?: unknown };
  const type = node.type;
  if (typeof type !== "string" || !KNOWN_PREDICATE_TYPES.has(type)) {
    throw new Error(
      `Unknown predicate type '${String(type)}' encountered during pre-flight validation. ` +
        "This usually indicates the bundled ado-script.zip is older than the ado-aw " +
        "compiler that emitted this spec. Update the bundle to a release that supports " +
        "this predicate, or pin the compiler to a matching version.",
    );
  }

  if (type === "and" || type === "or") {
    if (!Array.isArray(node.operands)) {
      throw new Error(`Predicate '${type}' is missing required 'operands' array`);
    }
    for (const sub of node.operands) validatePredicateTree(sub as PredicateSpec);
  } else if (type === "not") {
    if (node.operand === undefined || node.operand === null) {
      throw new Error("Predicate 'not' is missing required 'operand'");
    }
    validatePredicateTree(node.operand as PredicateSpec);
  }
}
