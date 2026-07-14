/**
 * Base64 `GateSpec` / `PrSynthSpec` builders for the trigger-condition E2E
 * harness.
 *
 * These are a faithful TypeScript port of the Rust spec construction in
 * `src/compile/filter_ir.rs` (`lower_pr_filters` + `build_gate_spec` +
 * `build_pr_synth_spec`). They deliberately import the codegen'd `GateSpec`
 * shape from `../shared/types.gen.js` so that any drift in the serialized
 * schema surfaces as a TypeScript compile error rather than a silent
 * wrong-answer at runtime.
 *
 * We do NOT run the Rust compiler here: the harness crafts the exact filter
 * under test directly, so a single hand-authored victim pipeline can exercise
 * every predicate by receiving a different base64 `GATE_SPEC` per queued build.
 *
 * Fidelity contract (must stay in lock-step with `filter_ir.rs`):
 *   - fact `kind` strings              → `Fact::kind()`
 *   - fact `failure_policy`            → `Fact::failure_policy()`
 *   - fact `dependencies`             → `Fact::dependencies()`
 *   - canonical fact emission order    → `Fact` enum declaration order
 *                                        (already topological; see
 *                                        `collect_ordered_facts`)
 *   - check `tag_suffix` values        → `FilterCheck::build_tag_suffix`
 *
 * Test-harness module; not shipped in `ado-script.zip`.
 */
import type {
  CheckSpec,
  FactSpec,
  GateContextSpec,
  GateSpec,
  PredicateSpec,
} from "../shared/types.gen.js";

// ─── Fact metadata (mirror of filter_ir.rs Fact::{kind,failure_policy,dependencies}) ──

type FailurePolicy = "fail_closed" | "fail_open" | "skip_dependents";

interface FactMeta {
  readonly policy: FailurePolicy;
  readonly deps: readonly string[];
}

/**
 * Fact kind → metadata. Keys are declared in `Fact` enum order, which the
 * evaluator relies on being topological (a dependency always precedes its
 * dependents). `buildGateSpec` emits facts in this iteration order.
 *
 * DRIFT GUARD: this table is deep-compared against the Rust-generated
 * `fact-catalog.gen.json` by `gate-spec.test.ts` (see `factMetaCatalog`), so a
 * divergence from `filter_ir.rs::Fact` fails a unit test rather than silently
 * producing wrong specs at runtime.
 */
const FACT_META: ReadonlyMap<string, FactMeta> = new Map<string, FactMeta>([
  ["pr_title", { policy: "fail_closed", deps: [] }],
  ["author_email", { policy: "fail_closed", deps: [] }],
  ["source_branch", { policy: "fail_closed", deps: [] }],
  ["target_branch", { policy: "fail_closed", deps: [] }],
  ["commit_message", { policy: "fail_closed", deps: [] }],
  ["build_reason", { policy: "fail_closed", deps: [] }],
  ["triggered_by_pipeline", { policy: "fail_closed", deps: [] }],
  ["triggering_branch", { policy: "fail_closed", deps: [] }],
  ["pr_metadata", { policy: "skip_dependents", deps: [] }],
  ["pr_is_draft", { policy: "fail_closed", deps: ["pr_metadata"] }],
  ["pr_labels", { policy: "fail_open", deps: ["pr_metadata"] }],
  ["changed_files", { policy: "fail_open", deps: [] }],
  ["changed_file_count", { policy: "fail_open", deps: ["changed_files"] }],
  ["current_utc_minutes", { policy: "fail_closed", deps: [] }],
]);

/** One catalog row: the serialized view of a fact's kind/policy/dependencies. */
export interface FactCatalogEntry {
  readonly kind: string;
  readonly failure_policy: FailurePolicy;
  readonly dependencies: string[];
}

/**
 * `FACT_META` projected into the exact shape of the Rust-generated
 * `fact-catalog.gen.json` (array, in insertion/declaration order). The
 * `gate-spec.test.ts` drift test deep-compares this against that committed
 * catalog so any Rust-side change to a fact's policy/dependencies — or a
 * new/removed `Fact` — fails a cheap unit test instead of silently producing
 * wrong specs at runtime.
 */
export function factMetaCatalog(): FactCatalogEntry[] {
  return [...FACT_META].map(([kind, meta]) => ({
    kind,
    failure_policy: meta.policy,
    dependencies: [...meta.deps],
  }));
}

/**
 * Facts referenced directly by a predicate tree (mirror of predicates.ts
 * collectFacts). Switches on the `type` discriminant so each variant's `fact`
 * field is accessed type-safely: if a future `PredicateSpec` variant renames or
 * drops `fact`, this fails to compile rather than silently returning
 * `undefined` and under-specifying the fact list. The `never` default makes a
 * newly-added variant a compile error too.
 */
function predicateFacts(p: PredicateSpec, out: Set<string> = new Set()): Set<string> {
  switch (p.type) {
    case "glob_match":
    case "equals":
    case "value_in_set":
    case "value_not_in_set":
    case "numeric_range":
    case "label_set_match":
    case "file_glob_match":
      out.add(p.fact);
      break;
    case "time_window":
      out.add("current_utc_minutes");
      break;
    case "and":
    case "or":
      for (const sub of p.operands) predicateFacts(sub, out);
      break;
    case "not":
      predicateFacts(p.operand, out);
      break;
    default: {
      const _exhaustive: never = p;
      throw new Error(`gate-spec: unhandled predicate type '${(_exhaustive as { type?: string }).type}'`);
    }
  }
  return out;
}

/** Add a fact and all its transitive dependencies to `out`. */
function collectFactWithDeps(kind: string, out: Set<string>): void {
  if (out.has(kind)) return;
  out.add(kind);
  const meta = FACT_META.get(kind);
  if (!meta) throw new Error(`gate-spec: unknown fact kind '${kind}'`);
  for (const dep of meta.deps) collectFactWithDeps(dep, out);
}

// ─── Check + context ─────────────────────────────────────────────────────────

/** A filter check paired with the build tag emitted on failure. */
export interface Check {
  readonly name: string;
  readonly tagSuffix: string;
  readonly predicate: PredicateSpec;
}

export type GateContextKind = "pull-request" | "pipeline-completion";

function contextSpec(kind: GateContextKind): GateContextSpec {
  return kind === "pull-request"
    ? {
        build_reason: "PullRequest",
        tag_prefix: "pr-gate",
        step_name: "prGate",
        bypass_label: "PR",
      }
    : {
        build_reason: "ResourceTrigger",
        tag_prefix: "pipeline-gate",
        step_name: "pipelineGate",
        bypass_label: "pipeline",
      };
}

/**
 * Assemble a `GateSpec` from a context and a set of checks, deriving the fact
 * list (with transitive deps, in canonical order) exactly as `build_gate_spec`
 * does in Rust.
 */
export function buildGateSpec(context: GateContextKind, checks: Check[]): GateSpec {
  const required = new Set<string>();
  for (const c of checks) {
    for (const f of predicateFacts(c.predicate)) collectFactWithDeps(f, required);
  }

  const facts: FactSpec[] = [];
  for (const [kind, meta] of FACT_META) {
    if (!required.has(kind)) continue;
    facts.push({ kind, failure_policy: meta.policy, dependencies: [...meta.deps] });
  }

  const checkSpecs: CheckSpec[] = checks.map((c) => ({
    name: c.name,
    predicate: c.predicate,
    tag_suffix: c.tagSuffix,
  }));

  return { context: contextSpec(context), facts, checks: checkSpecs };
}

/** Base64-encode a spec for the `GATE_SPEC` env / template parameter. */
export function encodeGateSpec(spec: GateSpec): string {
  return Buffer.from(JSON.stringify(spec), "utf8").toString("base64");
}

// ─── Check builders (mirror of lower_pr_filters) ─────────────────────────────

export function titleCheck(pattern: string): Check {
  return {
    name: "title",
    tagSuffix: "title-mismatch",
    predicate: { type: "glob_match", fact: "pr_title", pattern },
  };
}

export function sourceBranchCheck(pattern: string): Check {
  return {
    name: "source-branch",
    tagSuffix: "source-branch-mismatch",
    predicate: { type: "glob_match", fact: "source_branch", pattern },
  };
}

export function targetBranchCheck(pattern: string): Check {
  return {
    name: "target-branch",
    tagSuffix: "target-branch-mismatch",
    predicate: { type: "glob_match", fact: "target_branch", pattern },
  };
}

export function commitMessageCheck(pattern: string): Check {
  return {
    name: "commit-message",
    tagSuffix: "commit-message-mismatch",
    predicate: { type: "glob_match", fact: "commit_message", pattern },
  };
}

export function authorIncludeCheck(values: string[]): Check {
  return {
    name: "author include",
    tagSuffix: "author-mismatch",
    predicate: { type: "value_in_set", fact: "author_email", values, case_insensitive: true },
  };
}

export function authorExcludeCheck(values: string[]): Check {
  return {
    name: "author exclude",
    tagSuffix: "author-excluded",
    predicate: { type: "value_not_in_set", fact: "author_email", values, case_insensitive: true },
  };
}

export function labelsCheck(opts: {
  anyOf?: string[];
  allOf?: string[];
  noneOf?: string[];
}): Check {
  return {
    name: "labels",
    tagSuffix: "labels-mismatch",
    predicate: {
      type: "label_set_match",
      fact: "pr_labels",
      any_of: opts.anyOf ?? [],
      all_of: opts.allOf ?? [],
      none_of: opts.noneOf ?? [],
    },
  };
}

export function draftCheck(expected: boolean): Check {
  return {
    name: "draft",
    tagSuffix: "draft-mismatch",
    predicate: { type: "equals", fact: "pr_is_draft", value: expected ? "true" : "false" },
  };
}

export function changedFilesCheck(opts: { include?: string[]; exclude?: string[] }): Check {
  return {
    name: "changed-files",
    tagSuffix: "changed-files-mismatch",
    predicate: {
      type: "file_glob_match",
      fact: "changed_files",
      include: opts.include ?? [],
      exclude: opts.exclude ?? [],
    },
  };
}

export function timeWindowCheck(start: string, end: string): Check {
  return {
    name: "time-window",
    tagSuffix: "time-window-mismatch",
    predicate: { type: "time_window", start, end },
  };
}

export function changeCountCheck(opts: { min?: number; max?: number }): Check {
  return {
    name: "change-count",
    tagSuffix: "changes-mismatch",
    predicate: {
      type: "numeric_range",
      fact: "changed_file_count",
      min: opts.min ?? null,
      max: opts.max ?? null,
    },
  };
}

export function buildReasonIncludeCheck(values: string[]): Check {
  return {
    name: "build-reason include",
    tagSuffix: "build-reason-mismatch",
    predicate: { type: "value_in_set", fact: "build_reason", values, case_insensitive: true },
  };
}

export function buildReasonExcludeCheck(values: string[]): Check {
  return {
    name: "build-reason exclude",
    tagSuffix: "build-reason-excluded",
    predicate: { type: "value_not_in_set", fact: "build_reason", values, case_insensitive: true },
  };
}

// ─── PR_SYNTH_SPEC builder (mirror of build_pr_synth_spec) ────────────────────

export interface PrSynthSpecInput {
  branches?: { include?: string[]; exclude?: string[] };
  paths?: { include?: string[]; exclude?: string[] };
}

/** Base64-encode a `PR_SYNTH_SPEC` for the synthetic-PR promotion path. */
export function encodePrSynthSpec(input: PrSynthSpecInput = {}): string {
  const spec = {
    branches: {
      include: input.branches?.include ?? [],
      exclude: input.branches?.exclude ?? [],
    },
    paths: {
      include: input.paths?.include ?? [],
      exclude: input.paths?.exclude ?? [],
    },
  };
  return Buffer.from(JSON.stringify(spec), "utf8").toString("base64");
}
