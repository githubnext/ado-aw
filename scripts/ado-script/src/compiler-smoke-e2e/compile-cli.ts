/**
 * Invokes the candidate `ado-aw` binary against one fixture inside the
 * detached worktree: `compile --force <md>` followed by `check <lock>`.
 *
 * Test-harness module; not shipped in `ado-script.zip`.
 */
import { redact, safeSpawn } from "./process.js";

export interface CompileCheckResult {
  ok: boolean;
  /** Which sub-step failed ("compile" | "check"), if any. */
  phase?: "compile" | "check";
  stdout: string;
  stderr: string;
  message?: string;
}

export interface CompileCheckOptions {
  /** Path to the candidate ado-aw binary. */
  adoAwBin: string;
  /** Detached worktree root (subprocess cwd). */
  worktreeDir: string;
  /**
   * Remote URL whose ADO identity must be embedded in generated metadata.
   *
   * The candidate is compiled in a GitHub worktree but executed from an Azure
   * Repos mirror. Pass the compiler-owned context override only to these child
   * processes so staging and runtime integrity compilation infer the same
   * org/repo without mutating the worktree's shared Git configuration.
   */
  metadataRemoteUrl: string;
  /** Repo-relative path to the fixture markdown source. */
  relMd: string;
  /** Repo-relative path to the compiled lock file. */
  relLock: string;
  timeoutMs: number;
  /** Secrets to redact from any captured output/failure message. */
  secrets?: readonly (string | undefined)[];
}

/** Run `compile --force <md>` then `check <lock>` for one fixture. Never throws. */
export async function compileAndCheck(opts: CompileCheckOptions): Promise<CompileCheckResult> {
  const secrets = opts.secrets ?? [];
  const metadataEnv = {
    ADO_AW_COMPILE_REMOTE_URL: opts.metadataRemoteUrl,
  };

  const compileOutcome = await safeSpawn({
    cmd: opts.adoAwBin,
    args: ["compile", "--force", opts.relMd],
    cwd: opts.worktreeDir,
    env: metadataEnv,
    timeoutMs: opts.timeoutMs,
  });
  if (compileOutcome.timedOut || compileOutcome.status !== 0) {
    return {
      ok: false,
      phase: "compile",
      stdout: redact(compileOutcome.stdout, secrets),
      stderr: redact(compileOutcome.stderr, secrets),
      message: compileOutcome.timedOut
        ? `compile --force ${opts.relMd} timed out after ${opts.timeoutMs}ms`
        : `compile --force ${opts.relMd} exited ${compileOutcome.status}`,
    };
  }

  const checkOutcome = await safeSpawn({
    cmd: opts.adoAwBin,
    args: ["check", opts.relLock],
    cwd: opts.worktreeDir,
    env: metadataEnv,
    timeoutMs: opts.timeoutMs,
  });
  if (checkOutcome.timedOut || checkOutcome.status !== 0) {
    return {
      ok: false,
      phase: "check",
      stdout: redact(checkOutcome.stdout, secrets),
      stderr: redact(checkOutcome.stderr, secrets),
      message: checkOutcome.timedOut
        ? `check ${opts.relLock} timed out after ${opts.timeoutMs}ms`
        : `check ${opts.relLock} exited ${checkOutcome.status}`,
    };
  }

  return {
    ok: true,
    stdout: redact(compileOutcome.stdout + checkOutcome.stdout, secrets),
    stderr: redact(compileOutcome.stderr + checkOutcome.stderr, secrets),
  };
}
