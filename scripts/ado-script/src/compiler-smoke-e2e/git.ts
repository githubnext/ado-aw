/**
 * Git operations for staging smoke cases onto the mirror repo: spin up a
 * detached temp worktree, and for each case commit the staged pipeline, push
 * it to a per-case candidate ref, verify, then reset ready for the next case.
 * Cleans up the remote refs and the worktree at the end.
 *
 * Also implements the startup stale-ref scanner (see {@link scanStaleRefs}
 * usage in `index.ts`).
 *
 * Every git invocation goes through an injectable {@link GitRunner} so tests
 * never need a real repository or network access — mirrors the DI pattern
 * used by `trigger-e2e/mirror.ts`.
 *
 * Test-harness module; not shipped in `ado-script.zip`.
 */
import { bearerEnv } from "../shared/git.js";
import { redact, safeSpawn, type SpawnOutcome } from "./process.js";
import { CANDIDATE_BRANCH_PREFIX } from "./config.js";

export interface GitRunOptions {
  cwd: string;
  env?: NodeJS.ProcessEnv;
  timeoutMs: number;
}

export type GitRunner = (args: string[], opts: GitRunOptions) => Promise<SpawnOutcome>;

export const defaultGitRunner: GitRunner = (args, opts) =>
  safeSpawn({
    cmd: "git",
    args,
    cwd: opts.cwd,
    // GIT_TERMINAL_PROMPT=0 ensures a bad/expired token or unreachable
    // mirror fails fast with a normal non-zero exit instead of hanging on an
    // interactive credential prompt in a non-interactive pipeline agent.
    // Applied AFTER `opts.env` (e.g. the bearer-token triple) so it is a
    // true, non-overridable floor — no caller in this harness ever needs to
    // set it itself.
    env: { ...opts.env, GIT_TERMINAL_PROMPT: "0" },
    timeoutMs: opts.timeoutMs,
  });

/** Deterministic commit identity for staged candidate commits. */
export const COMMIT_IDENTITY = {
  name: "ado-aw-smoke-e2e",
  email: "ado-aw-smoke-e2e@users.noreply.github.com",
} as const;

/** Deterministic commit message: `test(smoke): stage <caseId> for candidate <buildId>`. */
export function commitMessage(buildId: number, caseId: string): string {
  return `test(smoke): stage ${caseId} for candidate ${buildId}`;
}

/** Build the ADO Git remote URL for `<orgUrl>/<project>/_git/<repo>`. */
export function mirrorRepoUrl(orgUrl: string, project: string, repo: string): string {
  return `${orgUrl.replace(/\/+$/, "")}/${encodeURIComponent(project)}/_git/${encodeURIComponent(repo)}`;
}

async function run(
  args: string[],
  opts: GitRunOptions,
  runner: GitRunner,
  secrets: readonly (string | undefined)[] = [],
): Promise<string> {
  const outcome = await runner(args, opts);
  if (outcome.timedOut) {
    throw new Error(`git ${args[0] ?? ""} timed out after ${opts.timeoutMs}ms`);
  }
  if (outcome.status !== 0) {
    const detail = redact([outcome.stderr.trim(), outcome.stdout.trim()].filter(Boolean).join("\n"), secrets);
    throw new Error(`git ${args.join(" ")} failed (exit ${outcome.status}): ${detail || "(no output)"}`);
  }
  // Trim only TRAILING whitespace here — a leading `.trim()` would eat the
  // significant leading space of a ` M <path>` line in `git status
  // --porcelain=v1` output (an unmodified-index/modified-worktree entry),
  // silently corrupting the very first parsed path in
  // `worktreeChangedFiles`.
  return redact(outcome.stdout.replace(/\s+$/, ""), secrets);
}

/**
 * Verify `expectedSha` (BUILD_SOURCEVERSION) is present in the self checkout
 * at `cwd` (BUILD_SOURCESDIRECTORY) so the detached worktree can be built
 * directly from it — never fetched from the mirror repo.
 *
 * This is deliberately NOT a mirror fetch: for a GitHub PR build,
 * BUILD_SOURCEBRANCH is a synthetic ref such as `refs/pull/<n>/merge` that
 * only exists on the GitHub-backed remote the pipeline checked out from, not
 * on the ADO `ado-aw-mirror` repo — attempting to fetch it there would fail
 * (or silently resolve to nothing meaningful). The pipeline's own checkout
 * step already fetched every object this build needs into `cwd`, so we only
 * need to confirm `expectedSha` is actually reachable there before basing a
 * worktree on it; the resulting candidate commit is the only thing pushed
 * to the mirror.
 */
export async function verifyLocalCommit(
  opts: { cwd: string; expectedSha: string; timeoutMs: number },
  runner: GitRunner = defaultGitRunner,
): Promise<void> {
  const headSha = await run(["rev-parse", "HEAD"], { cwd: opts.cwd, timeoutMs: opts.timeoutMs }, runner);
  if (headSha.toLowerCase() === opts.expectedSha.toLowerCase()) return;
  // HEAD can legitimately differ from BUILD_SOURCEVERSION (e.g. a synthetic
  // PR merge commit checked out as HEAD while BUILD_SOURCEVERSION names the
  // PR source-branch tip) — fall back to an object-existence check rather
  // than requiring an exact HEAD match.
  const outcome = await runner(
    ["cat-file", "-e", `${opts.expectedSha}^{commit}`],
    { cwd: opts.cwd, timeoutMs: opts.timeoutMs },
  );
  if (outcome.timedOut || outcome.status !== 0) {
    throw new Error(
      `BUILD_SOURCEVERSION ${opts.expectedSha} was not found as a commit object in the local checkout at ` +
        `${opts.cwd} (HEAD is ${headSha}); refusing to fetch it from the mirror repo since a GitHub PR ref ` +
        `(e.g. refs/pull/<n>/merge) would not exist there`,
    );
  }
}

/** Create a detached temp worktree at `worktreeDir`, checked out at `commitish`. */
export async function createDetachedWorktree(
  opts: { cwd: string; worktreeDir: string; commitish: string; timeoutMs: number },
  runner: GitRunner = defaultGitRunner,
): Promise<void> {
  await run(
    ["worktree", "add", "--detach", opts.worktreeDir, opts.commitish],
    { cwd: opts.cwd, timeoutMs: opts.timeoutMs },
    runner,
  );
}

/** Remove a detached worktree (best-effort caller decides how to handle failure). */
export async function removeWorktree(
  opts: { cwd: string; worktreeDir: string; timeoutMs: number },
  runner: GitRunner = defaultGitRunner,
): Promise<void> {
  await run(
    ["worktree", "remove", "--force", opts.worktreeDir],
    { cwd: opts.cwd, timeoutMs: opts.timeoutMs },
    runner,
  );
}

/**
 * List working-tree changes inside `worktreeDir` as a flat list of affected
 * repo-relative paths (renames contribute both their old and new path).
 */
export async function worktreeChangedFiles(
  opts: { worktreeDir: string; timeoutMs: number },
  runner: GitRunner = defaultGitRunner,
): Promise<string[]> {
  const stdout = await run(
    // `--untracked-files=all` is security-significant: the default porcelain
    // output collapses a wholly untracked directory to e.g. `.ado-aw/`, which
    // prevents the exact-path allowlist below from validating each generated
    // import-cache file independently.
    ["status", "--porcelain=v1", "--untracked-files=all"],
    { cwd: opts.worktreeDir, timeoutMs: opts.timeoutMs },
    runner,
  );
  if (!stdout) return [];
  const files: string[] = [];
  for (const line of stdout.split("\n")) {
    if (!line) continue;
    const rest = line.slice(3);
    const arrow = rest.indexOf(" -> ");
    if (arrow >= 0) {
      files.push(rest.slice(0, arrow), rest.slice(arrow + 4));
    } else {
      files.push(rest);
    }
  }
  return files;
}

/** Pure comparison: returns any changed path NOT in `allowed`. Empty = clean. */
export function disallowedChanges(changed: readonly string[], allowed: ReadonlySet<string>): string[] {
  return changed.filter((path) => !allowed.has(path));
}

/** Stage all changes and commit with the deterministic identity/message. Returns the new commit SHA. */
export async function commitAll(
  opts: { worktreeDir: string; buildId: number; caseId: string; timeoutMs: number },
  runner: GitRunner = defaultGitRunner,
): Promise<string> {
  await run(["add", "-A"], { cwd: opts.worktreeDir, timeoutMs: opts.timeoutMs }, runner);
  await run(
    [
      "-c",
      `user.name=${COMMIT_IDENTITY.name}`,
      "-c",
      `user.email=${COMMIT_IDENTITY.email}`,
      "commit",
      "-m",
      commitMessage(opts.buildId, opts.caseId),
    ],
    { cwd: opts.worktreeDir, timeoutMs: opts.timeoutMs },
    runner,
  );
  return run(["rev-parse", "HEAD"], { cwd: opts.worktreeDir, timeoutMs: opts.timeoutMs }, runner);
}

/**
 * Hard-reset the worktree back to `commitish` and remove untracked files.
 *
 * Called between cases so every candidate commit is a *sibling* parented
 * directly on `BUILD_SOURCEVERSION` rather than a chain — each per-case ref
 * then contains exactly its own case's staged pipeline, and all refs share
 * the bulk of their objects so the pushes stay cheap.
 *
 * `clean -fdx` is deliberate: the compiler writes generated artefacts (lock
 * files, `.ado-aw/imports/`) that must not leak from one case into the next.
 */
export async function resetWorktree(
  opts: { worktreeDir: string; commitish: string; timeoutMs: number },
  runner: GitRunner = defaultGitRunner,
): Promise<void> {
  await run(
    ["reset", "--hard", opts.commitish],
    { cwd: opts.worktreeDir, timeoutMs: opts.timeoutMs },
    runner,
  );
  await run(["clean", "-fdx"], { cwd: opts.worktreeDir, timeoutMs: opts.timeoutMs }, runner);
}

/** Push the worktree's HEAD to `ref` on the mirror repo (never force). */
export async function pushCandidate(
  opts: { worktreeDir: string; mirrorUrl: string; ref: string; token: string; timeoutMs: number },
  runner: GitRunner = defaultGitRunner,
): Promise<void> {
  const env = bearerEnv(opts.token);
  await run(
    ["push", "--porcelain", opts.mirrorUrl, `HEAD:${opts.ref}`],
    { cwd: opts.worktreeDir, env, timeoutMs: opts.timeoutMs },
    runner,
    [opts.token],
  );
}

/** Verify the pushed ref resolves to `expectedSha` on the remote. Throws on mismatch. */
export async function verifyRemoteRef(
  opts: { cwd: string; mirrorUrl: string; ref: string; expectedSha: string; token: string; timeoutMs: number },
  runner: GitRunner = defaultGitRunner,
): Promise<void> {
  const env = bearerEnv(opts.token);
  const stdout = await run(
    ["ls-remote", opts.mirrorUrl, opts.ref],
    { cwd: opts.cwd, env, timeoutMs: opts.timeoutMs },
    runner,
    [opts.token],
  );
  const remoteSha = stdout.split(/\s+/)[0]?.trim();
  if (!remoteSha || remoteSha.toLowerCase() !== opts.expectedSha.toLowerCase()) {
    throw new Error(
      `candidate push verification failed: expected ${opts.expectedSha} at ${opts.ref}, got '${remoteSha ?? ""}'`,
    );
  }
}

/** Delete the candidate ref on the mirror repo (best-effort; caller decides how to handle failure). */
export async function deleteRemoteRef(
  opts: { cwd: string; mirrorUrl: string; ref: string; token: string; timeoutMs: number },
  runner: GitRunner = defaultGitRunner,
): Promise<void> {
  await deleteRemoteRefs({ ...opts, refs: [opts.ref] }, runner);
}

/**
 * Delete one or more candidate refs on the mirror repo in a single push.
 *
 * Batched because the lane model creates one ref per case per run, so a
 * five-case run would otherwise pay five round trips. Falls back to
 * individual deletes if the batch fails, so one bad ref cannot strand the
 * rest.
 */
export async function deleteRemoteRefs(
  opts: { cwd: string; mirrorUrl: string; refs: readonly string[]; token: string; timeoutMs: number },
  runner: GitRunner = defaultGitRunner,
): Promise<void> {
  if (opts.refs.length === 0) return;
  const env = bearerEnv(opts.token);
  const run1 = (refs: readonly string[]): Promise<string> =>
    run(
      ["push", "--porcelain", opts.mirrorUrl, "--delete", ...refs],
      { cwd: opts.cwd, env, timeoutMs: opts.timeoutMs },
      runner,
      [opts.token],
    );

  if (opts.refs.length === 1) {
    await run1(opts.refs);
    return;
  }

  try {
    await run1(opts.refs);
  } catch (batchErr) {
    const failures: string[] = [];
    for (const ref of opts.refs) {
      try {
        await run1([ref]);
      } catch (err) {
        failures.push(`${ref}: ${err instanceof Error ? err.message : String(err)}`);
      }
    }
    if (failures.length > 0) {
      throw new Error(
        `batched ref delete failed (${batchErr instanceof Error ? batchErr.message : String(batchErr)}); ` +
          `per-ref fallback also failed for: ${failures.join("; ")}`,
      );
    }
  }
}

export interface RemoteRef {
  ref: string;
  sha: string;
}

/** List every remote ref under the exact `refs/heads/<CANDIDATE_BRANCH_PREFIX>/` prefix. */
export async function listCandidateRefs(
  opts: { cwd: string; mirrorUrl: string; token: string; timeoutMs: number },
  runner: GitRunner = defaultGitRunner,
): Promise<RemoteRef[]> {
  const env = bearerEnv(opts.token);
  const stdout = await run(
    // `**` rather than `*`: candidate refs now carry a per-case segment
    // (`<buildId>/<caseId>`), and some git/server implementations do not match
    // `/` with a single `*`. The exact-prefix guard below remains the real
    // filter either way.
    ["ls-remote", "--heads", opts.mirrorUrl, `refs/heads/${CANDIDATE_BRANCH_PREFIX}/**`],
    { cwd: opts.cwd, env, timeoutMs: opts.timeoutMs },
    runner,
    [opts.token],
  );
  if (!stdout) return [];
  const prefix = `refs/heads/${CANDIDATE_BRANCH_PREFIX}/`;
  const refs: RemoteRef[] = [];
  for (const line of stdout.split("\n")) {
    if (!line.trim()) continue;
    const [sha, ref] = line.split(/\s+/);
    // Exact-prefix guard: ls-remote's glob can match unintended refs on some
    // git/server implementations (e.g. a sibling branch containing the
    // pattern as a substring) — never treat those as our candidate refs.
    if (sha && ref && ref.startsWith(prefix)) {
      refs.push({ ref, sha });
    }
  }
  return refs;
}

/** A parsed candidate ref: the orchestrator build that created it, and the case it stages. */
export interface ParsedCandidateRef {
  readonly buildId: number;
  readonly caseId: string;
}

/**
 * Parse the build id and case id out of a candidate ref, or `undefined` if the
 * ref does not match `refs/heads/<prefix>/<buildId>/<caseId>` exactly.
 *
 * The `caseId` pattern mirrors the manifest's `CASE_ID_RE`. Anything that
 * fails to parse is reported as ambiguous by the stale-ref scanner and never
 * deleted — a fail-closed posture is preferred over guessing at another run's
 * identity.
 */
export function parseCandidateRef(ref: string): ParsedCandidateRef | undefined {
  const prefix = `refs/heads/${CANDIDATE_BRANCH_PREFIX}/`;
  if (!ref.startsWith(prefix)) return undefined;
  const match = /^([0-9]+)\/([a-z0-9][a-z0-9-]{0,48})$/.exec(ref.slice(prefix.length));
  if (!match) return undefined;
  const buildId = Number(match[1]);
  if (!Number.isSafeInteger(buildId) || buildId <= 0) return undefined;
  return { buildId, caseId: match[2]! };
}
