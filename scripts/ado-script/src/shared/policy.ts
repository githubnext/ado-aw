import type { FactSpec } from "./types.gen.js";
import { logWarning } from "./vso-logger.js";

export type FailurePolicy = "fail_closed" | "fail_open" | "skip_dependents";

/** Hard-coded fact dependency graph — keep in sync with `Fact::dependencies` in
 *  `src/compile/filter_ir.rs`. Encoded here because `FactSpec` does not carry
 *  the dep graph; it's a property of the kind, not the spec instance. */
const FACT_DEPS = {
  pr_is_draft: ["pr_metadata"],
  pr_labels: ["pr_metadata"],
  changed_file_count: ["changed_files"],
} as const satisfies Record<string, readonly string[]>;

export class PolicyTracker {
  private readonly policyByKind = new Map<string, FailurePolicy>();
  private readonly failedFacts = new Set<string>();
  private readonly skippedFacts = new Set<string>();
  private readonly unavailablePoliciesByKind = new Map<string, Set<FailurePolicy>>();
  private passedChecks = 0;
  private failedChecks = 0;
  private skippedChecks = 0;

  constructor(facts: FactSpec[]) {
    for (const f of facts) {
      this.policyByKind.set(f.kind, this.parsePolicy(f.failure_policy));
    }
  }

  /** Record that a fact failed to acquire. Returns the policy that was applied. */
  recordFactFailure(factKind: string, reason: string): FailurePolicy {
    const policy = this.policyByKind.get(factKind) ?? "fail_closed";
    this.failedFacts.add(factKind);
    this.markUnavailableTransitive(factKind, policy);

    if (policy === "skip_dependents") {
      logWarning(`Fact '${factKind}' failed (${reason}); dependent checks skipped`);
    } else if (policy === "fail_open") {
      logWarning(`Fact '${factKind}' failed (${reason}); fail-open: assuming pass`);
    } else {
      logWarning(`Fact '${factKind}' failed (${reason}); fail-closed: blocking`);
    }
    return policy;
  }

  /** Determine the verdict for a check given a set of facts referenced by its predicate.
   *  - If any referenced fact has skip_dependents (or is transitively skipped) → "skip".
   *  - Otherwise, if any referenced fact failed with fail_closed → "fail" (caller may still
   *    run the predicate; this method is consulted ONLY when a referenced fact is missing).
   *  - If all referenced facts had fail_open → "pass".
   *  - Mixed: if any fail_closed dominates a "fail" outcome.
   *  Note: this method only handles the *missing-fact* case. Predicate evaluation
   *  proper is in `evaluatePredicates`; this returns "evaluate" when there are no
   *  missing facts so the caller can defer to the predicate evaluator.
   */
  verdictForMissingFacts(referencedKinds: string[]): "pass" | "fail" | "skip" | "evaluate" {
    const missing = referencedKinds.filter((k) => this.isUnavailable(k));
    if (missing.length === 0) return "evaluate";

    if (missing.some((k) => this.skippedFacts.has(k))) return "skip";

    let anyClosed = false;
    let allOpen = true;
    for (const k of missing) {
      const policies = this.unavailablePoliciesByKind.get(k) ?? new Set<FailurePolicy>([
        this.policyByKind.get(k) ?? "fail_closed",
      ]);
      const policyValues = [...policies];
      if (policyValues.includes("fail_closed")) anyClosed = true;
      if (policyValues.some((p) => p !== "fail_open")) allOpen = false;
    }
    if (anyClosed) return "fail";
    if (allOpen) return "pass";
    return "fail";
  }

  recordCheckResult(result: "pass" | "fail" | "skip"): void {
    if (result === "pass") this.passedChecks++;
    else if (result === "fail") this.failedChecks++;
    else this.skippedChecks++;
  }

  summary(): { passed: number; failed: number; skipped: number } {
    return { passed: this.passedChecks, failed: this.failedChecks, skipped: this.skippedChecks };
  }

  private parsePolicy(value: string): FailurePolicy {
    if (value === "fail_closed" || value === "fail_open" || value === "skip_dependents") {
      return value;
    }
    return "fail_closed";
  }

  public isUnavailableForAcquisition(factKind: string): boolean {
    return this.isUnavailable(factKind);
  }

  private isUnavailable(factKind: string): boolean {
    return (
      this.failedFacts.has(factKind) ||
      this.skippedFacts.has(factKind) ||
      this.unavailablePoliciesByKind.has(factKind)
    );
  }

  private markUnavailableTransitive(factKind: string, policy: FailurePolicy): void {
    const unavailable = new Set<string>([factKind]);
    let changed = true;
    while (changed) {
      changed = false;
      for (const [kind, deps] of Object.entries(FACT_DEPS)) {
        if (unavailable.has(kind)) continue;
        if (deps.some((dep) => unavailable.has(dep))) {
          unavailable.add(kind);
          changed = true;
        }
      }
    }

    for (const kind of unavailable) {
      this.addUnavailablePolicy(kind, policy);
      if (policy === "skip_dependents") this.skippedFacts.add(kind);
    }
  }

  private addUnavailablePolicy(factKind: string, policy: FailurePolicy): void {
    const existing = this.unavailablePoliciesByKind.get(factKind);
    if (existing) {
      existing.add(policy);
      return;
    }
    this.unavailablePoliciesByKind.set(factKind, new Set([policy]));
  }
}

export const _internalForTest = { FACT_DEPS };
