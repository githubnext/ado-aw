// AUTO-GENERATED from Rust IR via cargo run -- export-gate-schema. Do not edit; run npm run codegen.

/**
 * Serialized predicate — the expression tree evaluated at runtime.
 */
export type PredicateSpec =
  | {
      fact: string;
      pattern: string;
      type: "glob_match";
      [k: string]: unknown;
    }
  | {
      fact: string;
      type: "equals";
      value: string;
      [k: string]: unknown;
    }
  | {
      case_insensitive: boolean;
      fact: string;
      type: "value_in_set";
      values: string[];
      [k: string]: unknown;
    }
  | {
      case_insensitive: boolean;
      fact: string;
      type: "value_not_in_set";
      values: string[];
      [k: string]: unknown;
    }
  | {
      fact: string;
      max?: number | null;
      min?: number | null;
      type: "numeric_range";
      [k: string]: unknown;
    }
  | {
      end: string;
      start: string;
      type: "time_window";
      [k: string]: unknown;
    }
  | {
      all_of: string[];
      any_of: string[];
      fact: string;
      none_of: string[];
      type: "label_set_match";
      [k: string]: unknown;
    }
  | {
      exclude: string[];
      fact: string;
      include: string[];
      type: "file_glob_match";
      [k: string]: unknown;
    }
  | {
      operands: PredicateSpec[];
      type: "and";
      [k: string]: unknown;
    }
  | {
      operands: PredicateSpec[];
      type: "or";
      [k: string]: unknown;
    }
  | {
      operand: PredicateSpec;
      type: "not";
      [k: string]: unknown;
    };

/**
 * Serializable gate specification — the JSON document consumed by the
 * Node gate evaluator (`scripts/gate.js`) at pipeline runtime.
 */
export interface GateSpec {
  checks: CheckSpec[];
  context: GateContextSpec;
  facts: FactSpec[];
  [k: string]: unknown;
}
/**
 * Serialized filter check.
 */
export interface CheckSpec {
  name: string;
  predicate: PredicateSpec;
  tag_suffix: string;
  [k: string]: unknown;
}
/**
 * Serialized gate context.
 */
export interface GateContextSpec {
  build_reason: string;
  bypass_label: string;
  step_name: string;
  tag_prefix: string;
  [k: string]: unknown;
}
/**
 * Serialized fact acquisition descriptor.
 */
export interface FactSpec {
  /**
   * Kinds of other facts that must be acquired before this one.
   * Mirrors `Fact::dependencies()`. Carried in the spec so the gate
   * evaluator does not duplicate the dependency graph.
   */
  dependencies: string[];
  failure_policy: string;
  kind: string;
  [k: string]: unknown;
}
