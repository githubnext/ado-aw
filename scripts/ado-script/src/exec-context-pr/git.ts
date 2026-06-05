import { spawnSync } from "node:child_process";

/**
 * Build the `GIT_CONFIG_*` env-var triple that injects an
 * `http.extraheader: Authorization: bearer <token>` config into a
 * spawned git subprocess WITHOUT writing to `.git/config` and WITHOUT
 * the token appearing on the argv command line. This is the in-process
 * equivalent of the v6.2 bash `git_fetch` wrapper.
 *
 * Returns `{}` when `token` is empty/undefined — the caller should
 * still attempt the fetch (the existing bash path falls through to a
 * plain `git fetch` in that case, which works for public refs and
 * fails for private ones — same posture preserved).
 */
export function bearerEnv(token: string | undefined): Record<string, string> {
  if (!token || token.length === 0) {
    return {};
  }
  return {
    GIT_CONFIG_COUNT: "1",
    GIT_CONFIG_KEY_0: "http.extraheader",
    GIT_CONFIG_VALUE_0: `Authorization: bearer ${token}`,
  };
}

export type GitResult = {
  stdout: string;
  stderr: string;
  status: number | null;
};

/**
 * Run `git` with the given arguments. Output is captured; the caller
 * decides what to do with non-zero exits (this wrapper never throws).
 *
 * The `env` is spread onto `process.env` so callers can layer the
 * bearer triple over the existing environment without leaking it
 * elsewhere. Per `spawnSync` semantics, when `env` is provided it
 * replaces the child's environment entirely; passing `process.env`
 * as the base ensures PATH and other essentials are preserved.
 */
export function runGit(args: string[], env: Record<string, string> = {}): GitResult {
  const result = spawnSync("git", args, {
    env: { ...process.env, ...env },
    encoding: "utf8",
  });
  return {
    stdout: result.stdout ?? "",
    stderr: result.stderr ?? "",
    status: result.status,
  };
}

/**
 * Run `git`, returning `stdout` on success or `null` on non-zero exit.
 * Convenience wrapper for the common "read a SHA" pattern.
 */
export function gitOk(args: string[], env: Record<string, string> = {}): string | null {
  const r = runGit(args, env);
  if (r.status !== 0) return null;
  return r.stdout.trim();
}
