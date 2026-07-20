import { spawn } from "node:child_process";

import { bearerEnv } from "../shared/git.js";
import type { ScenarioResult } from "./scenario.js";

const DEFAULT_TIMEOUT_MS = 300_000;
const MAIN_REF = "refs/heads/main";
const MAX_DIAGNOSTIC_CHARS = 2_000;

export interface MirrorSyncConfig {
  orgUrl: string;
  project: string;
  repo: string;
  token: string;
  cwd: string;
  sourceBranch: string;
  sourceVersion?: string;
  timeoutMs: number;
}

export interface MirrorGitRequest {
  args: string[];
  cwd: string;
  env: Record<string, string>;
  timeoutMs: number;
}

export interface MirrorGitResult {
  status: number | null;
  stdout: string;
  stderr: string;
}

export type MirrorGitRunner = (request: MirrorGitRequest) => Promise<MirrorGitResult>;

export function mirrorGitEnv(
  requestEnv: Record<string, string>,
  baseEnv: NodeJS.ProcessEnv = process.env,
): NodeJS.ProcessEnv {
  const env: NodeJS.ProcessEnv = {
    ...baseEnv,
    ...requestEnv,
    GIT_TERMINAL_PROMPT: "0",
  };
  // Git treats any non-empty trace value — including "0" — as enabled.
  // Remove ambient tracing completely so bearer-bearing HTTP headers cannot
  // enter captured stderr or displace the actionable error tail.
  delete env.GIT_TRACE;
  delete env.GIT_TRACE_CURL;
  delete env.GIT_CURL_VERBOSE;
  return env;
}

function cleanVar(raw: string | undefined): string | undefined {
  const value = raw?.trim();
  if (!value || /^\$\(.*\)$/.test(value)) return undefined;
  return value;
}

function required(env: NodeJS.ProcessEnv, name: string): string {
  const value = cleanVar(env[name]);
  if (!value) throw new Error(`${name} is required when TRIGGER_E2E_SYNC_MIRROR=true`);
  return value;
}

function enabled(raw: string | undefined): boolean {
  return cleanVar(raw)?.toLowerCase() === "true";
}

function timeoutMs(raw: string | undefined): number {
  const parsed = Number(cleanVar(raw));
  return Number.isFinite(parsed) && parsed > 0 ? parsed : DEFAULT_TIMEOUT_MS;
}

export function mirrorRepoUrl(orgUrl: string, project: string, repo: string): string {
  return `${orgUrl.replace(/\/+$/, "")}/${encodeURIComponent(project)}/_git/${encodeURIComponent(repo)}`;
}

export function loadMirrorSyncConfig(
  env: NodeJS.ProcessEnv = process.env,
): MirrorSyncConfig | undefined {
  if (!enabled(env.TRIGGER_E2E_SYNC_MIRROR)) return undefined;
  // Preserve the harness's existing "bypass-only" baseline when no ADO PR
  // repository is configured. Once a repo is supplied, synchronization is
  // mandatory and fail-closed.
  const repo = cleanVar(env.TRIGGER_E2E_VICTIM_REPO);
  if (!repo) return undefined;

  return {
    orgUrl: required(env, "SYSTEM_COLLECTIONURI"),
    project: required(env, "SYSTEM_TEAMPROJECT"),
    repo,
    token: required(env, "SYSTEM_ACCESSTOKEN"),
    cwd: cleanVar(env.BUILD_SOURCESDIRECTORY) ?? process.cwd(),
    sourceBranch: required(env, "BUILD_SOURCEBRANCH"),
    sourceVersion: cleanVar(env.BUILD_SOURCEVERSION),
    timeoutMs: timeoutMs(env.TRIGGER_E2E_MIRROR_SYNC_TIMEOUT_MS),
  };
}

function runGit(request: MirrorGitRequest): Promise<MirrorGitResult> {
  return new Promise((resolve, reject) => {
    const child = spawn("git", request.args, {
      cwd: request.cwd,
      env: mirrorGitEnv(request.env),
    });
    let stdout = "";
    let stderr = "";
    let timedOut = false;
    let settled = false;

    const finish = (result: MirrorGitResult): void => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      resolve(result);
    };

    const timer = setTimeout(() => {
      timedOut = true;
      child.kill("SIGKILL");
      finish({
        status: null,
        stdout,
        stderr: `${stderr}\ngit timed out after ${request.timeoutMs}ms`,
      });
    }, request.timeoutMs);

    child.stdout.on("data", (data: Buffer) => {
      stdout += data.toString();
    });
    child.stderr.on("data", (data: Buffer) => {
      stderr += data.toString();
    });
    child.on("error", (err) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      reject(err);
    });
    child.on("close", (status) => {
      if (timedOut) return;
      finish({ status, stdout, stderr });
    });
  });
}

function diagnostic(result: MirrorGitResult, secret: string): string {
  const text = [result.stderr.trim(), result.stdout.trim()]
    .filter(Boolean)
    .join("\n")
    .replaceAll(secret, "***");
  if (!text) return "(no output)";
  return text.length <= MAX_DIAGNOSTIC_CHARS
    ? text
    : `…${text.slice(-MAX_DIAGNOSTIC_CHARS)}`;
}

async function git(
  config: MirrorSyncConfig,
  args: string[],
  runner: MirrorGitRunner,
): Promise<string> {
  const result = await runner({
    args,
    cwd: config.cwd,
    env: bearerEnv(config.token),
    timeoutMs: config.timeoutMs,
  });
  if (result.status !== 0) {
    throw new Error(`git ${args[0] ?? "command"} failed: ${diagnostic(result, config.token)}`);
  }
  return result.stdout.trim();
}

export async function syncMirror(
  config: MirrorSyncConfig,
  runner: MirrorGitRunner = runGit,
): Promise<string> {
  if (config.sourceBranch !== MAIN_REF) {
    throw new Error(
      `mirror sync requires BUILD_SOURCEBRANCH=${MAIN_REF} (got '${config.sourceBranch}')`,
    );
  }

  const shallow = await git(config, ["rev-parse", "--is-shallow-repository"], runner);
  if (shallow !== "false") {
    throw new Error(
      `mirror sync requires a full checkout (git reported is-shallow-repository='${shallow}'); set fetchDepth: 0`,
    );
  }

  const head = await git(config, ["rev-parse", "HEAD"], runner);
  if (config.sourceVersion && head.toLowerCase() !== config.sourceVersion.toLowerCase()) {
    throw new Error(
      `checked-out HEAD ${head} does not match BUILD_SOURCEVERSION ${config.sourceVersion}`,
    );
  }

  const url = mirrorRepoUrl(config.orgUrl, config.project, config.repo);
  await git(config, ["push", "--porcelain", url, `HEAD:${MAIN_REF}`], runner);

  const remote = await git(config, ["ls-remote", url, MAIN_REF], runner);
  const remoteHead = remote.split(/\s+/)[0]?.trim();
  if (!remoteHead || remoteHead.toLowerCase() !== head.toLowerCase()) {
    throw new Error(
      `mirror verification failed: expected ${head} at ${MAIN_REF}, got '${remoteHead ?? ""}'`,
    );
  }
  return head;
}

export async function runMirrorSyncPreflight(
  env: NodeJS.ProcessEnv,
  log: (message: string) => void,
  runner: MirrorGitRunner = runGit,
): Promise<ScenarioResult | undefined> {
  const start = Date.now();
  try {
    if (
      enabled(env.TRIGGER_E2E_SYNC_MIRROR) &&
      !cleanVar(env.TRIGGER_E2E_VICTIM_REPO)
    ) {
      log("[mirror-sync] skipped: TRIGGER_E2E_VICTIM_REPO is not configured");
      return undefined;
    }
    const config = loadMirrorSyncConfig(env);
    if (!config) return undefined;

    log(`[mirror-sync] syncing checked-out main to ADO repo '${config.repo}'`);
    const head = await syncMirror(config, runner);
    log(`[mirror-sync] OK: '${config.repo}' ${MAIN_REF} is ${head}`);
    return {
      id: "mirror-sync",
      ok: true,
      durationMs: Date.now() - start,
    };
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    log(`[mirror-sync] FAILED: ${message}`);
    return {
      id: "mirror-sync",
      ok: false,
      phase: "setup",
      message,
      durationMs: Date.now() - start,
    };
  }
}
