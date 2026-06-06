// Strict allowlist regexes for the PR identifier env vars. These come
// from ADO predefined variables (infra-set, not PR-author-controlled)
// but defence-in-depth is cheap and protects against future regressions
// if ADO ever changes its variable population.
//
// Mirrors the regex set used by the v6.2 bash implementation of this
// step (`src/compile/extensions/exec_context/pr.rs`). Keep these in
// strict parity — the prompt heredoc interpolates these values
// literally.

export const PR_ID_RE = /^[0-9]+$/;

// Project names may contain spaces (e.g. "My Project"); the character
// set matches what ADO accepts at project-creation time.
export const PROJECT_RE = /^[A-Za-z0-9._ -]+$/;

// Repository names have no spaces.
export const REPO_RE = /^[A-Za-z0-9._-]+$/;

// PR target branch is interpolated into a git refspec
// ("+refs/heads/<short>:refs/remotes/origin/<short>"), so it must be a
// valid git branch name. The character set is what git itself accepts
// for `refs/heads/<name>`.
export const TARGET_BRANCH_RE = /^[A-Za-z0-9._/-]+$/;

export type IdentifierError = {
  /** A one-line reason, safe to embed in the agent prompt verbatim. */
  reason: string;
};

export type Identifiers = {
  prId: string;
  project: string;
  repo: string;
  targetBranch: string;
  /** The short branch name (`refs/heads/foo` -> `foo`). */
  targetShort: string;
};

/**
 * Sanitize an arbitrary string for safe embedding in the agent prompt.
 * Replaces CR/LF with spaces and truncates to a short cap so a hostile
 * branch name (which a PR author with push access could choose) cannot
 * inject markdown headers, code fences, or "ignore previous
 * instructions"-style text into the prompt via the failure path.
 *
 * Used by the validation-failure path where the *unvalidated* raw env
 * value is embedded in the failure reason — the value has by definition
 * already failed the strict allowlist regex, so we must treat it as
 * adversarial input.
 */
export function sanitizeForPrompt(value: string, maxLen = 80): string {
  const oneLine = value.replace(/[\r\n]+/g, " ");
  if (oneLine.length <= maxLen) return oneLine;
  return oneLine.slice(0, maxLen) + "…";
}

/**
 * Validate the 4 PR-identifier env vars and return either the parsed
 * identifiers or a structured error. Both `prId === ""` and
 * `targetBranch === ""` are treated as validation failures — every
 * downstream step needs all four values to be present and well-formed.
 *
 * On failure, the raw value of the offending env var is included in the
 * `reason` string for diagnosis, but is passed through
 * [`sanitizeForPrompt`] first so a hostile value (e.g. a branch name
 * with embedded newlines or markdown headers) cannot inject content
 * into the agent prompt via the failure fragment.
 */
export function validateIdentifiers(env: NodeJS.ProcessEnv): Identifiers | IdentifierError {
  const prId = env.SYSTEM_PULLREQUEST_PULLREQUESTID ?? "";
  const targetBranch = env.SYSTEM_PULLREQUEST_TARGETBRANCH ?? "";
  const project = env.SYSTEM_TEAMPROJECT ?? "";
  const repo = env.BUILD_REPOSITORY_NAME ?? "";

  if (!PR_ID_RE.test(prId)) {
    return { reason: `PR identifier validation failed (PR_ID='${sanitizeForPrompt(prId)}' is not a positive integer).` };
  }
  if (!PROJECT_RE.test(project)) {
    return { reason: `PR identifier validation failed (PROJECT='${sanitizeForPrompt(project)}' contains disallowed characters).` };
  }
  if (!REPO_RE.test(repo)) {
    return { reason: `PR identifier validation failed (REPO='${sanitizeForPrompt(repo)}' contains disallowed characters).` };
  }
  if (targetBranch.length === 0) {
    return { reason: "System.PullRequest.TargetBranch is empty; cannot resolve merge-base." };
  }
  if (!TARGET_BRANCH_RE.test(targetBranch)) {
    return {
      reason: `PR identifier validation failed (PR_TARGET_BRANCH='${sanitizeForPrompt(targetBranch)}' contains disallowed characters).`,
    };
  }

  const targetShort = targetBranch.startsWith("refs/heads/")
    ? targetBranch.slice("refs/heads/".length)
    : targetBranch;

  return { prId, project, repo, targetBranch, targetShort };
}

/** Type guard distinguishing the validated identifiers from an error. */
export function isIdentifierError(value: Identifiers | IdentifierError): value is IdentifierError {
  return (value as IdentifierError).reason !== undefined;
}
