/**
 * exec-context-pr-synth — Setup-job script that synthesises
 * `Build.Reason == PullRequest` semantics on CI-triggered builds when
 * an open PR matches the agent's `on.pr.branches` / `on.pr.paths`
 * filters.
 *
 * Why this exists: Azure DevOps Services ignores the YAML `pr:` block
 * unless a per-branch Build Validation policy is registered server-
 * side. Without that policy, a push to a feature branch fires the
 * pipeline as `Build.Reason = IndividualCI` even when an open PR
 * exists, so the gate evaluator's "not a PR build" bypass triggers
 * and `exec-context-pr.js` is skipped entirely.
 *
 * This script runs in the Setup job before `prGate`, calls the ADO
 * REST API to find the active PR for `Build.SourceBranch`, applies
 * the front-matter filters, and emits `AW_SYNTHETIC_PR*` outputs
 * that downstream gate + exec-context-pr steps coalesce with the
 * real `System.PullRequest.*` variables.
 *
 * Skeleton-only in this commit — full runtime contract is implemented
 * in the synth-bundle-logic todo.
 */

export function main(_env: NodeJS.ProcessEnv = process.env): number {
  return 0;
}

// Top-level invocation. `process.exit` is called here (not in `main`)
// so tests can call `main(env)` and inspect the return value without
// terminating the test process.
const exitCode = main();
process.exit(exitCode);
