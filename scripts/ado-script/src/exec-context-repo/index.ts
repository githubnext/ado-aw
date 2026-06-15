/**
 * exec-context-repo — Stage repository identity for any agent
 * (Stage 7 of the exec-context contributor build-out — see plan.md).
 *
 * Default-OFF (opt-in via `repo.enabled: true`). When active, stages
 * a small set of "what repo am I in" files so agents can frame
 * their work without restating identity in every markdown body:
 *
 *   - aw-context/repo/branch                 # Build.SourceBranchName
 *   - aw-context/repo/sha                    # Build.SourceVersion
 *   - aw-context/repo/last-release-tag       # `git describe --tags --abbrev=0`
 *                                              (empty when no tags exist)
 *   - aw-context/repo/commits-since-tag.txt  # `git log <tag>..HEAD --oneline`
 *                                              (empty when no tag)
 *   - aw-context/repo/conventions.json       # presence flags for common
 *                                              convention files; only
 *                                              when AW_REPO_CONVENTIONS=true
 *
 * Trust boundary: pure git, no REST, no bearer. Operates on the
 * local workspace; reads no env beyond BUILD_SOURCESDIRECTORY /
 * BUILD_SOURCEVERSION / BUILD_SOURCEBRANCH (all ADO predefined,
 * inert).
 */
import {
  mkdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
  existsSync,
} from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { gitOk, runGit } from "../shared/git.js";
import { appendToAgentPrompt } from "../shared/prompt.js";
import { sanitizeForPrompt } from "../shared/validate.js";

const DEFAULT_AGENT_PROMPT_PATH = "/tmp/awf-tools/agent-prompt.md";
/** Files the contributor probes for when AW_REPO_CONVENTIONS=true. */
const CONVENTION_FILES = [
  "CODEOWNERS",
  ".github/CODEOWNERS",
  "CONTRIBUTING.md",
  ".editorconfig",
  "AGENTS.md",
];
const CONVENTION_HEAD_LINES = 50;

function agentPromptPath(env: NodeJS.ProcessEnv): string {
  return env.AW_AGENT_PROMPT_FILE && env.AW_AGENT_PROMPT_FILE.length > 0
    ? env.AW_AGENT_PROMPT_FILE
    : DEFAULT_AGENT_PROMPT_PATH;
}

function repoDir(env: NodeJS.ProcessEnv): { sourcesRoot: string; awDir: string } {
  const sourcesRoot =
    env.BUILD_SOURCESDIRECTORY && env.BUILD_SOURCESDIRECTORY.length > 0
      ? env.BUILD_SOURCESDIRECTORY
      : process.cwd();
  return {
    sourcesRoot,
    awDir: join(sourcesRoot, "aw-context", "repo"),
  };
}

/** Strip the `refs/heads/` prefix from a branch ref if present. */
function shortBranch(ref: string): string {
  return ref.startsWith("refs/heads/") ? ref.slice("refs/heads/".length) : ref;
}

export function successFragment(args: {
  branch: string;
  sha: string;
  lastReleaseTag: string;
  commitsSinceTag: number;
}): string {
  const { branch, sha, lastReleaseTag, commitsSinceTag } = args;
  const lines = ["", "## Repo context", ""];
  lines.push(
    `Running on branch \`${sanitizeForPrompt(branch)}\` at \`${sha}\`.`,
  );
  if (lastReleaseTag.length > 0) {
    lines.push(
      `Last release tag: \`${sanitizeForPrompt(lastReleaseTag)}\` (${commitsSinceTag} commit(s) since).`,
    );
  } else {
    lines.push(
      "No release tags found in this repo (or none reachable from HEAD).",
    );
  }
  lines.push("");
  lines.push("Files in `aw-context/repo/`:");
  lines.push("  - `branch`, `sha` — current branch and SHA");
  lines.push("  - `last-release-tag` — most recent reachable tag (may be empty)");
  lines.push("  - `commits-since-tag.txt` — `git log <tag>..HEAD --oneline`");
  lines.push(
    "  - `conventions.json` — presence flags for CODEOWNERS / CONTRIBUTING.md / etc (only when `repo.conventions: true`)",
  );
  lines.push("");
  return lines.join("\n");
}

function probeConventions(
  sourcesRoot: string,
): Record<string, { present: boolean; head?: string }> {
  const out: Record<string, { present: boolean; head?: string }> = {};
  for (const rel of CONVENTION_FILES) {
    const full = join(sourcesRoot, rel);
    if (!existsSync(full)) {
      out[rel] = { present: false };
      continue;
    }
    try {
      const raw = readFileSync(full, "utf8");
      const head = raw.split("\n").slice(0, CONVENTION_HEAD_LINES).join("\n");
      out[rel] = { present: true, head };
    } catch {
      // Defensive: if we can't read for any reason, surface as present
      // but body unavailable rather than failing the whole step.
      out[rel] = { present: true };
    }
  }
  return out;
}

export function main(env: NodeJS.ProcessEnv = process.env): number {
  const { sourcesRoot, awDir } = repoDir(env);
  const promptPath = agentPromptPath(env);

  try {
    mkdirSync(awDir, { recursive: true });
  } catch (err) {
    process.stderr.write(
      `[aw-context] fatal: could not create ${awDir}: ${(err as Error).message}\n`,
    );
    return 1;
  }
  for (const f of [
    "branch",
    "sha",
    "last-release-tag",
    "commits-since-tag.txt",
    "conventions.json",
  ]) {
    rmSync(join(awDir, f), { force: true });
  }

  const branch = shortBranch(env.BUILD_SOURCEBRANCH ?? "");
  const sha = env.BUILD_SOURCEVERSION ?? "";
  // Best-effort: when these aren't a SHA / branch ref (e.g. detached
  // HEAD or unusual ADO config) we still stage what we have rather
  // than failing — the contributor's value is information, not strict
  // contracts. Empty values are valid.
  writeFileSync(join(awDir, "branch"), branch, "utf8");
  writeFileSync(join(awDir, "sha"), sha, "utf8");

  const tag = gitOk(["describe", "--tags", "--abbrev=0"]) ?? "";
  writeFileSync(join(awDir, "last-release-tag"), tag, "utf8");

  let commitsCount = 0;
  if (tag.length > 0) {
    const log = runGit(["log", "--oneline", `${tag}..HEAD`]);
    const text = log.status === 0 ? log.stdout : "";
    writeFileSync(join(awDir, "commits-since-tag.txt"), text, "utf8");
    commitsCount = text.split("\n").filter((l) => l.length > 0).length;
  } else {
    writeFileSync(join(awDir, "commits-since-tag.txt"), "", "utf8");
  }

  if ((env.AW_REPO_CONVENTIONS ?? "").toLowerCase() === "true") {
    const conventions = probeConventions(sourcesRoot);
    writeFileSync(
      join(awDir, "conventions.json"),
      JSON.stringify(conventions, null, 2) + "\n",
      "utf8",
    );
  }

  appendToAgentPrompt(
    promptPath,
    successFragment({
      branch,
      sha,
      lastReleaseTag: tag,
      commitsSinceTag: commitsCount,
    }),
  );
  process.stdout.write(
    `[aw-context] repo context staged: branch=${branch} sha=${sha} tag=${tag || "<none>"} commits-since-tag=${commitsCount}\n`,
  );
  return 0;
}

if (
  typeof process !== "undefined" &&
  process.argv[1] &&
  process.argv[1] === fileURLToPath(import.meta.url)
) {
  const rc = main();
  process.exit(rc);
}
