/**
 * Reads pipeline-variable facts from `process.env`.
 *
 * Mirrors the env-var → fact mapping defined by `Fact::ado_exports()` in
 * `src/compile/filter_ir.rs`. Branch-
 * shaped facts (`source_branch`, `target_branch`, `triggering_branch`) have
 * the leading `refs/heads/`, `refs/tags/`, or `refs/pull/` prefix stripped so
 * user patterns like `feature/*` match without the prefix.
 *
 * Env-var contract is set by the compiler in
 * `src/compile/filter_ir.rs::collect_ado_exports` and
 * `Fact::ado_exports`.
 */

export type FactKind =
  | "pr_title"
  | "author_email"
  | "source_branch"
  | "target_branch"
  | "commit_message"
  | "build_reason"
  | "triggered_by_pipeline"
  | "triggering_branch";

const ENV_BY_FACT: Record<FactKind, string> = {
  pr_title: "ADO_PR_TITLE",
  author_email: "ADO_AUTHOR_EMAIL",
  source_branch: "ADO_SOURCE_BRANCH",
  target_branch: "ADO_TARGET_BRANCH",
  commit_message: "ADO_COMMIT_MESSAGE",
  build_reason: "ADO_BUILD_REASON",
  triggered_by_pipeline: "ADO_TRIGGERED_BY_PIPELINE",
  triggering_branch: "ADO_TRIGGERING_BRANCH",
};

const REF_PREFIXES = ["refs/heads/", "refs/tags/", "refs/pull/"] as const;

const BRANCH_FACTS: ReadonlySet<FactKind> = new Set<FactKind>([
  "source_branch",
  "target_branch",
  "triggering_branch",
]);

export function stripRefPrefix(value: string): string {
  for (const p of REF_PREFIXES) {
    if (value.startsWith(p)) return value.slice(p.length);
  }
  return value;
}

export function isPipelineVarFact(kind: string): kind is FactKind {
  return kind in ENV_BY_FACT;
}

export function readEnvFact(fact: FactKind): string | undefined {
  const envVar = ENV_BY_FACT[fact];
  const raw = process.env[envVar];
  if (raw === undefined || raw === "") return undefined;
  return BRANCH_FACTS.has(fact) ? stripRefPrefix(raw) : raw;
}
