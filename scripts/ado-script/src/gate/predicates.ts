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
import { stripRefPrefix } from "../shared/env-facts.js";

type CheckResult = "pass" | "fail" | "skip";

const BRANCH_FACTS = new Set(["source_branch", "target_branch", "triggering_branch"]);

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
      const pattern = BRANCH_FACTS.has(p.fact) ? stripRefPrefix(p.pattern) : p.pattern;
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
      const value = Number(facts.get(p.fact) ?? 0);
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
          "Update scripts/gate.js (or the bundled scripts.zip) to a " +
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

function globMatch(value: string, pattern: string): boolean {
  // Glob → regex: only `*` (any chars) and `?` (single char) are
  // recognised. Bracket expressions like `[abc]` are escaped to literal
  // characters here. This is a deliberate divergence from Python's
  // `fnmatch.fnmatch`, which supports `[seq]` ranges. The IR currently
  // never emits bracket patterns, but if a future predicate needs them,
  // this builder must be extended (and the parity inventory updated).
  const escaped = pattern.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const regex = `^${escaped.replace(/\\\*/g, ".*").replace(/\\\?/g, ".")}$`;
  return new RegExp(regex, "s").test(value);
}

function parseHm(s: string): number {
  const [h, m] = s.split(":").map((n) => parseInt(n, 10));
  return (h ?? 0) * 60 + (m ?? 0);
}
