import { spawn } from "node:child_process";

const TOKEN_PATTERNS = [
  /\bgithub_pat_[A-Za-z0-9_]+\b/g,
  /\bgh[pousr]_[A-Za-z0-9_]+\b/g,
  /\bBearer\s+[A-Za-z0-9._~+/=-]+\b/gi,
  /\b(?:token|password|secret)\s*[=:]\s*\S+/gi
];

export function redact(text) {
  let value = String(text ?? "");
  for (const pattern of TOKEN_PATTERNS) {
    value = value.replace(pattern, "[REDACTED]");
  }
  return value;
}

export function restrictedChildEnv(base = process.env) {
  const env = { ...base };
  for (const name of [
    "GH_TOKEN",
    "GITHUB_TOKEN",
    "SYSTEM_ACCESSTOKEN",
    "AZURE_DEVOPS_EXT_PAT",
    "SC_WRITE_TOKEN"
  ]) {
    delete env[name];
  }
  return env;
}

export async function runProcess(
  command,
  args,
  {
    cwd,
    env = process.env,
    timeoutMs = 600_000,
    maxOutputBytes = 1_048_576
  } = {}
) {
  const startedAt = Date.now();
  const child = spawn(command, args, {
    cwd,
    env,
    windowsHide: true,
    stdio: ["ignore", "pipe", "pipe"]
  });

  let stdout = "";
  let stderr = "";
  let truncated = false;

  const append = (current, chunk) => {
    if (Buffer.byteLength(current) >= maxOutputBytes) {
      truncated = true;
      return current;
    }
    const next = current + chunk.toString("utf8");
    if (Buffer.byteLength(next) <= maxOutputBytes) {
      return next;
    }
    truncated = true;
    return Buffer.from(next, "utf8").subarray(0, maxOutputBytes).toString("utf8");
  };

  child.stdout.on("data", (chunk) => {
    stdout = append(stdout, chunk);
  });
  child.stderr.on("data", (chunk) => {
    stderr = append(stderr, chunk);
  });

  let timedOut = false;
  const timeout = setTimeout(() => {
    timedOut = true;
    child.kill("SIGTERM");
    setTimeout(() => child.kill("SIGKILL"), 5_000).unref();
  }, timeoutMs);

  const result = await new Promise((resolve, reject) => {
    child.once("error", reject);
    child.once("close", (code, signal) => {
      resolve({ code, signal });
    });
  }).finally(() => clearTimeout(timeout));

  return {
    command,
    args,
    code: result.code,
    signal: result.signal,
    success: result.code === 0 && !timedOut,
    timed_out: timedOut,
    duration_ms: Date.now() - startedAt,
    stdout: redact(stdout),
    stderr: redact(stderr),
    output_truncated: truncated
  };
}

