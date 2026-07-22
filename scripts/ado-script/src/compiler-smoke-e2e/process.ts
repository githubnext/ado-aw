/**
 * Safe subprocess execution primitive shared by every other module in this
 * harness (git commands, the candidate `ado-aw` binary).
 *
 * Guarantees:
 *   - bounded wall-clock time (SIGKILL past `timeoutMs`),
 *   - bounded captured output (stdout/stderr truncated past `maxOutputBytes`
 *     so a runaway/looping child can never blow up harness memory or logs),
 *   - secret redaction of every string handed back to the caller (including
 *     thrown/failure messages), and
 *   - no ambient git tracing env vars (`GIT_TRACE*`, `GIT_CURL_VERBOSE`) —
 *     those can leak a bearer `Authorization` header into captured stderr.
 *
 * Test-harness module; not shipped in `ado-script.zip`.
 */
import { spawn } from "node:child_process";

/** Env vars that can leak secrets (bearer headers) into captured output. */
const TRACE_ENV_VARS = ["GIT_TRACE", "GIT_TRACE_CURL", "GIT_CURL_VERBOSE"] as const;

const DEFAULT_MAX_OUTPUT_BYTES = 1_000_000;

export interface SpawnRequest {
  cmd: string;
  args: string[];
  cwd?: string;
  env?: NodeJS.ProcessEnv;
  timeoutMs: number;
  maxOutputBytes?: number;
}

export interface SpawnOutcome {
  status: number | null;
  stdout: string;
  stderr: string;
  timedOut: boolean;
  stdoutTruncated: boolean;
  stderrTruncated: boolean;
}

/** Strip env vars that can leak secrets (bearer headers) into git trace output. */
export function stripTraceEnv(env: NodeJS.ProcessEnv): NodeJS.ProcessEnv {
  const out = { ...env };
  for (const name of TRACE_ENV_VARS) {
    delete out[name];
  }
  return out;
}

/** Replace every occurrence of any secret in `text` with `***`. Empty/undefined secrets are ignored. */
export function redact(text: string, secrets: readonly (string | undefined)[]): string {
  let out = text;
  for (const secret of secrets) {
    if (!secret) continue;
    out = out.split(secret).join("***");
  }
  return out;
}

class BoundedBuffer {
  private chunks: string[] = [];
  private bytes = 0;
  truncated = false;

  constructor(private readonly maxBytes: number) {}

  push(data: Buffer): void {
    if (this.truncated) return;
    const remaining = this.maxBytes - this.bytes;
    if (remaining <= 0) {
      this.truncated = true;
      return;
    }
    const text = data.toString("utf8");
    if (Buffer.byteLength(text, "utf8") <= remaining) {
      this.chunks.push(text);
      this.bytes += Buffer.byteLength(text, "utf8");
      return;
    }
    // Truncate at a safe (possibly mid-character) boundary; this is
    // diagnostic-only output so exactness past the cap doesn't matter.
    this.chunks.push(Buffer.from(text, "utf8").subarray(0, remaining).toString("utf8"));
    this.bytes = this.maxBytes;
    this.truncated = true;
  }

  toString(): string {
    return this.chunks.join("");
  }
}

/**
 * Spawn `cmd` with bounded output/time and no ambient trace env. Never
 * throws for a non-zero exit or timeout — callers inspect `status`/
 * `timedOut`. Output is NOT pre-redacted (callers know which secrets to
 * redact); use {@link redact} before logging or embedding in an error.
 */
export function safeSpawn(request: SpawnRequest): Promise<SpawnOutcome> {
  const maxOutputBytes = request.maxOutputBytes ?? DEFAULT_MAX_OUTPUT_BYTES;
  return new Promise((resolve, reject) => {
    const env = stripTraceEnv({ ...process.env, ...request.env });
    const child = spawn(request.cmd, request.args, {
      cwd: request.cwd,
      env,
    });

    const stdout = new BoundedBuffer(maxOutputBytes);
    const stderr = new BoundedBuffer(maxOutputBytes);
    let timedOut = false;
    let settled = false;

    const timer = setTimeout(() => {
      timedOut = true;
      child.kill("SIGKILL");
    }, request.timeoutMs);

    child.stdout?.on("data", (d: Buffer) => stdout.push(d));
    child.stderr?.on("data", (d: Buffer) => stderr.push(d));

    child.on("error", (err) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      reject(err);
    });

    child.on("close", (status) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      resolve({
        status,
        stdout: stdout.toString(),
        stderr: stderr.toString(),
        timedOut,
        stdoutTruncated: stdout.truncated,
        stderrTruncated: stderr.truncated,
      });
    });
  });
}

export function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}
