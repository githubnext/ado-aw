import { appendFileSync } from "node:fs";

import { sanitizeForPrompt, type Identifiers } from "./validate.js";

/**
 * Build the SUCCESS prompt fragment appended to the agent prompt file
 * after `base.sha` and `head.sha` have been staged.
 *
 * Identifier interpolation MUST be done with already-validated
 * `Identifiers` (see `validate.ts`) so the values cannot contain
 * arbitrary control characters / quotes / newlines.
 */
export function successFragment(ids: Identifiers): string {
  return [
    "",
    "## PR context",
    "",
    `This is PR #${ids.prId} in project '${ids.project}' / repository '${ids.repo}'.`,
    "",
    "For git inspection (offline; objects are already in the workspace):",
    "",
    "  BASE=$(cat aw-context/pr/base.sha)",
    "  HEAD=$(cat aw-context/pr/head.sha)",
    "  git diff --stat $BASE..$HEAD          # size budget first",
    "  git diff --name-status $BASE..$HEAD   # changed files",
    "  git diff $BASE..$HEAD                 # full patch",
    "  git diff $BASE..$HEAD -- <path>       # per-file",
    "  git show $HEAD:<path>                  # file at PR head",
    "  git log  $BASE..$HEAD                 # PR commits",
    "",
    "For Azure DevOps MCP (if the `azure-devops` tool is configured),",
    "the PR identifiers are pre-filled in these example calls:",
    "",
    `  repo_get_pull_request_by_id(project='${ids.project}', repositoryId='${ids.repo}', pullRequestId=${ids.prId})`,
    `  repo_list_pull_request_threads(project='${ids.project}', repositoryId='${ids.repo}', pullRequestId=${ids.prId})`,
    `  repo_create_pull_request_thread(project='${ids.project}', repositoryId='${ids.repo}', pullRequestId=${ids.prId}, comments=[...], status='active')`,
    "",
  ].join("\n");
}

/**
 * Build the FAILURE prompt fragment appended to the agent prompt file
 * when validation or merge-base resolution failed.
 *
 * Uses placeholders (`<unknown>`) when identifiers are themselves the
 * source of failure (mirrors the v6.2 bash `${PR_ID:-<unknown>}` form).
 *
 * The `partial` values are passed in **raw and unvalidated** from
 * `index.ts` (they come straight from the failure-path env-var reads),
 * so each one is run through [`sanitizeForPrompt`] before
 * interpolation. Defence-in-depth against a hostile env value (e.g. a
 * branch name with embedded newlines + markdown headers) injecting
 * content into the agent prompt via this failure fragment. ADO's
 * predefined variables are infra-set today, so exploitability is low —
 * but the consistent-sanitisation posture matches `reason` (which
 * `validateIdentifiers` already sanitises).
 */
export function failureFragment(reason: string, partial: {
  prId?: string;
  project?: string;
  repo?: string;
}): string {
  const prId = partial.prId && partial.prId.length > 0 ? sanitizeForPrompt(partial.prId) : "<unknown>";
  const project = partial.project && partial.project.length > 0 ? sanitizeForPrompt(partial.project) : "<unknown>";
  const repo = partial.repo && partial.repo.length > 0 ? sanitizeForPrompt(partial.repo) : "<unknown>";
  return [
    "",
    "## PR context",
    "",
    `PR #${prId} in project ${project} / repository ${repo} -- context preparation failed.`,
    `Reason: ${reason}`,
    "",
    "Local `git diff` is unavailable (the PR merge-base could not be resolved",
    "within the depth budget, or PR identifier validation failed). You may",
    "still call Azure DevOps MCP using the identifiers above",
    "(e.g. `repo_get_pull_request_by_id`), OR surface the failure and stop.",
    "Do NOT produce an empty review or pretend the PR has no changes.",
    "",
  ].join("\n");
}

/** Append `text` to the agent prompt file. The file is guaranteed to
 * already exist (created by base.yml's "Prepare agent prompt" step
 * before any prepare_steps run).
 */
export function appendToAgentPrompt(promptPath: string, text: string): void {
  appendFileSync(promptPath, text, "utf8");
}
