/**
 * Wrapper around the `ado-aw execute` (Stage 3) binary for the deterministic
 * E2E harness.
 *
 * Responsibilities:
 *  - render a minimal source markdown file carrying the per-tool
 *    `safe-outputs:` config (and, for repo-targeting tools, a `repos:` block),
 *  - stage the crafted `safe_outputs.ndjson` (plus any extra files),
 *  - spawn the real binary,
 *  - parse the resulting `safe-outputs-executed.ndjson`.
 *
 * Test-harness module; not shipped in `ado-script.zip`.
 */
import { spawn } from "node:child_process";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { existsSync } from "node:fs";
import { join } from "node:path";

import type { ExecutedRecord } from "./scenario.js";

const SAFE_OUTPUT_FILENAME = "safe_outputs.ndjson";
const EXECUTED_FILENAME = "safe-outputs-executed.ndjson";

export interface RenderSourceOptions {
  tool: string;
  /** Per-tool `safe-outputs: <tool>:` config object. */
  config: Record<string, unknown>;
  /** ADO repo name for repo-targeting tools (emits a `repos:` block). */
  adoRepo?: string;
}

/**
 * Render a minimal but valid agent source markdown. The `safe-outputs` config
 * value is emitted as inline JSON, which is valid YAML, so we avoid pulling in
 * a YAML serialiser and keep the mapping exact.
 */
export function renderSourceMarkdown(opts: RenderSourceOptions): string {
  const lines: string[] = [
    "---",
    // JSON-stringify the string values (valid YAML) so a tool name containing
    // a quote or backslash can't emit malformed front-matter.
    `name: ${JSON.stringify(`executor-e2e: ${opts.tool}`)}`,
    `description: ${JSON.stringify(`Deterministic Stage 3 executor check for ${opts.tool}`)}`,
    "target: standalone",
    "engine:",
    "  id: copilot",
  ];
  if (opts.adoRepo) {
    lines.push("repos:");
    // JSON-stringify both the alias and the repo name (valid YAML) to stay
    // consistent with the rest of the rendered front-matter and guard against
    // repo names containing YAML-significant characters.
    lines.push(`  - ${JSON.stringify(`${opts.adoRepo}=${opts.adoRepo}`)}`);
  }
  lines.push("safe-outputs:");
  // Quote the tool key (JSON string is valid YAML) so an unusual tool name
  // containing ": " or a leading "{" can't emit broken YAML.
  lines.push(`  ${JSON.stringify(opts.tool)}: ${JSON.stringify(opts.config)}`);
  lines.push("---");
  lines.push("");
  lines.push(`Deterministic executor E2E fixture for \`${opts.tool}\`.`);
  lines.push("");
  return lines.join("\n");
}

/** Serialise one executor NDJSON entry line (name + params). */
export function renderNdjsonLine(tool: string, entry: Record<string, unknown>): string {
  return JSON.stringify({ name: tool, ...entry }) + "\n";
}

export interface RunExecuteOptions {
  adoAwBin: string;
  /** Directory into which source.md + ndjson + extra files are written. */
  scenarioDir: string;
  tool: string;
  config: Record<string, unknown>;
  entry: Record<string, unknown>;
  adoRepo?: string;
  orgUrl: string;
  project: string;
  token: string;
  /** Relative-path -> contents files to stage into the safe-output dir. */
  files?: Record<string, string>;
  /** Extra env for the child process (merged over the harness env). */
  extraEnv?: Record<string, string>;
  log: (msg: string) => void;
}

export interface RunExecuteResult {
  exitCode: number;
  stdout: string;
  stderr: string;
  records: ExecutedRecord[];
  /** The record matching `tool` (dashes -> underscores), if any. */
  record?: ExecutedRecord;
}

/** Parse `safe-outputs-executed.ndjson` content into typed records. */
export function parseExecutedRecords(content: string): ExecutedRecord[] {
  const out: ExecutedRecord[] = [];
  for (const line of content.split(/\r?\n/)) {
    const trimmed = line.trim();
    if (!trimmed) continue;
    try {
      const parsed = JSON.parse(trimmed) as Record<string, unknown>;
      if (typeof parsed.name === "string" && typeof parsed.status === "string") {
        out.push(parsed as unknown as ExecutedRecord);
      }
    } catch {
      // ignore malformed lines
    }
  }
  return out;
}

export async function runExecute(opts: RunExecuteOptions): Promise<RunExecuteResult> {
  const safeOutputDir = join(opts.scenarioDir, "out");
  await mkdir(safeOutputDir, { recursive: true });

  const sourcePath = join(opts.scenarioDir, "source.md");
  await writeFile(
    sourcePath,
    renderSourceMarkdown({ tool: opts.tool, config: opts.config, adoRepo: opts.adoRepo }),
    "utf8",
  );

  // Stage any extra files (patch, attachment payloads) relative to the safe-output dir.
  for (const [rel, contents] of Object.entries(opts.files ?? {})) {
    const target = join(safeOutputDir, rel);
    await mkdir(join(target, ".."), { recursive: true });
    await writeFile(target, contents, "utf8");
  }

  await writeFile(
    join(safeOutputDir, SAFE_OUTPUT_FILENAME),
    renderNdjsonLine(opts.tool, opts.entry),
    "utf8",
  );

  const args = [
    "execute",
    "--source",
    sourcePath,
    "--safe-output-dir",
    safeOutputDir,
    "--ado-org-url",
    opts.orgUrl,
    "--ado-project",
    opts.project,
  ];

  const env: NodeJS.ProcessEnv = {
    ...process.env,
    SYSTEM_ACCESSTOKEN: opts.token,
    AZURE_DEVOPS_ORG_URL: opts.orgUrl,
    SYSTEM_TEAMPROJECT: opts.project,
    ...opts.extraEnv,
  };

  opts.log(`[${opts.tool}] running: ${opts.adoAwBin} ${args.join(" ")}`);
  const { exitCode, stdout, stderr } = await spawnCollect(opts.adoAwBin, args, env);
  if (stdout.trim()) opts.log(`[${opts.tool}] stdout:\n${stdout.trim()}`);
  if (stderr.trim()) opts.log(`[${opts.tool}] stderr:\n${stderr.trim()}`);

  const executedPath = join(safeOutputDir, EXECUTED_FILENAME);
  let records: ExecutedRecord[] = [];
  if (existsSync(executedPath)) {
    records = parseExecutedRecords(await readFile(executedPath, "utf8"));
  }
  const snake = opts.tool.replaceAll("-", "_");
  const record = records.find((r) => r.name === snake);

  return { exitCode, stdout, stderr, records, record };
}

function spawnCollect(
  cmd: string,
  args: string[],
  env: NodeJS.ProcessEnv,
): Promise<{ exitCode: number; stdout: string; stderr: string }> {
  // Guard against a hung `ado-aw execute` blocking the whole suite: kill the
  // child after a bounded timeout and surface a meaningful error instead of
  // waiting for the ADO job-level timeout.
  const timeoutMs = Number(process.env.EXECUTOR_E2E_EXECUTE_TIMEOUT_MS) || 600_000;
  return new Promise((resolve, reject) => {
    const child = spawn(cmd, args, { env });
    let stdout = "";
    let stderr = "";
    let timedOut = false;
    const timer = setTimeout(() => {
      timedOut = true;
      child.kill("SIGKILL");
    }, timeoutMs);
    child.stdout.on("data", (d: Buffer) => (stdout += d.toString()));
    child.stderr.on("data", (d: Buffer) => (stderr += d.toString()));
    child.on("error", (err) => {
      clearTimeout(timer);
      reject(err);
    });
    child.on("close", (code) => {
      clearTimeout(timer);
      if (timedOut) {
        reject(new Error(`ado-aw execute timed out after ${timeoutMs}ms`));
        return;
      }
      resolve({ exitCode: code ?? -1, stdout, stderr });
    });
  });
}
