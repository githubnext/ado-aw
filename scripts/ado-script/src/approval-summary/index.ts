/**
 * approval-summary — render the run's proposed safe outputs to a sanitized
 * markdown file and attach it to the build summary tab.
 *
 * Runs at the **end of the Agent job** (after safe outputs are collected,
 * before the artifact publish) — NOT in the Detection/threat-analysis stage,
 * whose sole job is inspecting proposals for threats.
 *
 * Always-on: emitted whenever a workflow has any safe-output tool enabled, so
 * non-elevated runs get the same transparency. When manual approval is
 * configured, the reviewed (pending-approval) proposals are listed first.
 *
 * I/O contract (all via env so no agent-controlled value is ever spliced into
 * a shell command — see the compiler wiring):
 *   - AW_SAFE_OUTPUTS_NDJSON   path to safe_outputs.ndjson (required)
 *   - AW_APPROVAL_SUMMARY_OUT  path to write the markdown file (required;
 *                              MUST use a namespaced base name, e.g.
 *                              ado-aw-safe-outputs.md, so the auto-derived ADO
 *                              summary-tab title never collides with a
 *                              consumer/template-target tab)
 *   - AW_REVIEWED_TOOLS        newline-separated reviewed tool names (optional;
 *                              newline, not comma, because a comma can legally
 *                              appear in a YAML map key — see the Rust
 *                              `safe_outputs_summary_step` doc comment)
 *   - AW_GITHUB_REPOSITORY_POLICIES compiler-resolved GitHub repository policy
 *                              JSON keyed by tool name
 *   - AW_CURRENT_REPOSITORY / AW_CURRENT_REPOSITORY_PROVIDER trusted ADO build
 *                              metadata used only for GitHub-source fallback
 *   - AW_GITHUB_API_URL        operator-resolved GitHub API URL
 *
 * Failure policy: best-effort. Any error is logged as a warning and the
 * program exits 0 — rendering the summary must never fail the build or block
 * the manual-review gate.
 */
import { readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

import { logWarning, uploadSummary } from "../shared/vso-logger.js";
import {
  parseProposals,
  renderSummary,
  type GithubRepositoryPolicy,
  type TrustedRepositoryContext,
} from "./render.js";

/**
 * Parse the reviewed-tool list (newline-delimited — see the compiler's
 * `safe_outputs_summary_step`) into a Set. Newline is used rather than a comma
 * because a comma can legally appear in a YAML map key but a newline cannot.
 */
export function parseReviewed(value: string | undefined): Set<string> {
  const out = new Set<string>();
  if (!value) return out;
  for (const part of value.split("\n")) {
    const t = part.trim();
    if (t.length > 0) out.add(t);
  }
  return out;
}

export function parseRepositoryPolicies(
  value: string | undefined,
): Map<string, GithubRepositoryPolicy> {
  const policies = new Map<string, GithubRepositoryPolicy>();
  if (!value) return policies;
  let parsed: unknown;
  try {
    parsed = JSON.parse(value);
  } catch {
    return policies;
  }
  if (parsed === null || typeof parsed !== "object" || Array.isArray(parsed)) {
    return policies;
  }
  for (const [tool, rawPolicy] of Object.entries(parsed)) {
    if (
      rawPolicy === null ||
      typeof rawPolicy !== "object" ||
      Array.isArray(rawPolicy)
    ) {
      continue;
    }
    const candidate = rawPolicy as Record<string, unknown>;
    const targetRepo =
      typeof candidate.targetRepo === "string"
        ? candidate.targetRepo
        : undefined;
    const allowedRepos = Array.isArray(candidate.allowedRepos)
      ? candidate.allowedRepos.filter(
          (repository): repository is string =>
            typeof repository === "string",
        )
      : [];
    policies.set(tool, { targetRepo, allowedRepos });
  }
  return policies;
}

export function main(env: NodeJS.ProcessEnv = process.env): number {
  const ndjsonPath = env.AW_SAFE_OUTPUTS_NDJSON ?? "";
  const outPath = env.AW_APPROVAL_SUMMARY_OUT ?? "";
  if (ndjsonPath.length === 0 || outPath.length === 0) {
    logWarning(
      "approval-summary: AW_SAFE_OUTPUTS_NDJSON and AW_APPROVAL_SUMMARY_OUT must be set; skipping summary.",
    );
    return 0;
  }

  let raw: string;
  try {
    raw = readFileSync(ndjsonPath, "utf8");
  } catch {
    // No proposals file (agent proposed nothing, or it was never created) is
    // a normal no-op, not an error.
    process.stdout.write(
      `approval-summary: no proposals file at ${ndjsonPath}; nothing to summarise.\n`,
    );
    return 0;
  }

  const proposals = parseProposals(raw);
  if (proposals.length === 0) {
    process.stdout.write("approval-summary: no proposals to summarise.\n");
    return 0;
  }

  const reviewed = parseReviewed(env.AW_REVIEWED_TOOLS);
  const repositoryContext: TrustedRepositoryContext = {
    policies: parseRepositoryPolicies(env.AW_GITHUB_REPOSITORY_POLICIES),
    currentRepository: env.AW_CURRENT_REPOSITORY,
    currentProvider: env.AW_CURRENT_REPOSITORY_PROVIDER,
    githubApiUrl: env.AW_GITHUB_API_URL,
  };
  const markdown = renderSummary(proposals, reviewed, repositoryContext);
  if (markdown.length === 0) {
    return 0;
  }

  try {
    writeFileSync(outPath, markdown, "utf8");
  } catch (err) {
    logWarning(
      `approval-summary: failed to write summary to ${outPath}: ${String(err)}`,
    );
    return 0;
  }

  uploadSummary(outPath);
  process.stdout.write(
    `approval-summary: wrote summary for ${proposals.length} proposal(s) to ${outPath}.\n`,
  );
  return 0;
}

if (
  typeof process !== "undefined" &&
  process.argv[1] &&
  process.argv[1] === fileURLToPath(import.meta.url)
) {
  process.exit(main());
}
