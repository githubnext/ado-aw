import type { GateSpec } from "../../../shared/types.gen.js";

export function factMap(values: Record<string, unknown>): Map<string, unknown> {
  return new Map(Object.entries(values));
}

export function gateSpec(
  checks: GateSpec["checks"],
  facts: GateSpec["facts"] = [],
): GateSpec {
  return {
    checks,
    facts,
    context: {
      build_reason: "PullRequest",
      bypass_label: "run-agent",
      step_name: "Gate",
      tag_prefix: "gate",
    },
  };
}
