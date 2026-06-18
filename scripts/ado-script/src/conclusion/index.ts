import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";

import { fileOrAppendWorkItem, type WorkItemReportConfig } from "../shared/wit.js";
import { logError, logInfo, logWarning } from "../shared/vso-logger.js";

const EXECUTED_MANIFEST_FILENAME = "safe-outputs-executed.ndjson";

type JobResult =
  | "Succeeded"
  | "Failed"
  | "SucceededWithIssues"
  | "Skipped"
  | "Canceled"
  | "Unknown";

type SignalKind = "pipeline_failure" | "noop" | "missing_tool" | "missing_data";

interface ManifestEntry {
  name: string;
  status?: string;
  context?: string | null;
  result?: Record<string, unknown> | null;
  error?: string | null;
  timestamp?: string;
  tool_name?: string;
  data_type?: string;
  reason?: string;
}

interface JobStatus {
  name: "Agent" | "Detection" | "SafeOutputs";
  result: JobResult;
}

interface RuntimeConfig {
  reportFailureAsWorkItem: boolean;
  pipelineName: string;
  safeOutputDir?: string;
  project?: string;
  buildUri?: string;
  buildId?: string;
  jobs: JobStatus[];
  toolConfigs: Record<string, PerToolConfig>;
}

interface PerToolConfig {
  reportAsWorkItem: boolean;
  titlePrefix?: string;
  workItemType?: string;
  areaPath?: string;
  iterationPath?: string;
  tags?: string[];
}

interface SignalReport {
  kind: SignalKind;
  defaultTitle: string;
  body: string;
}

function readOptionalEnv(env: NodeJS.ProcessEnv, name: string): string | undefined {
  const value = env[name]?.trim();
  return value ? value : undefined;
}

function readBooleanEnv(
  env: NodeJS.ProcessEnv,
  name: string,
  defaultValue: boolean,
): boolean {
  const raw = readOptionalEnv(env, name);
  if (!raw) return defaultValue;
  if (raw === "true") return true;
  if (raw === "false") return false;
  logWarning(`${name}='${raw}' is invalid; defaulting to ${defaultValue}`);
  return defaultValue;
}

function readJobResult(env: NodeJS.ProcessEnv, name: string): JobResult {
  const raw = readOptionalEnv(env, name);
  switch (raw) {
    case "Succeeded":
    case "Failed":
    case "SucceededWithIssues":
    case "Skipped":
    case "Canceled":
      return raw;
    case undefined:
      return "Unknown";
    default:
      logWarning(`${name}='${raw}' is not a recognised job result`);
      return "Unknown";
  }
}

function parsePerToolConfigFlat(env: NodeJS.ProcessEnv, prefix: string): PerToolConfig {
  return {
    reportAsWorkItem: readBooleanEnv(env, `${prefix}_REPORT_AS_WORK_ITEM`, true),
    titlePrefix: readOptionalEnv(env, `${prefix}_TITLE_PREFIX`),
    workItemType: readOptionalEnv(env, `${prefix}_WORK_ITEM_TYPE`),
    areaPath: readOptionalEnv(env, `${prefix}_AREA_PATH`),
    iterationPath: readOptionalEnv(env, `${prefix}_ITERATION_PATH`),
    tags: readTagsFromEnv(env, `${prefix}_TAGS`),
  };
}

function readTagsFromEnv(env: NodeJS.ProcessEnv, key: string): string[] | undefined {
  const raw = readOptionalEnv(env, key);
  if (!raw) return undefined;
  try {
    const parsed: unknown = JSON.parse(raw);
    if (!Array.isArray(parsed)) return undefined;
    return parsed.filter((v): v is string => typeof v === "string");
  } catch {
    logWarning(`Failed to parse ${key} as JSON array`);
    return undefined;
  }
}

function loadConfig(env: NodeJS.ProcessEnv): RuntimeConfig {
  return {
    reportFailureAsWorkItem: readBooleanEnv(env, "AW_REPORT_FAILURE_AS_WORK_ITEM", true),
    pipelineName: readOptionalEnv(env, "AW_PIPELINE_NAME") ?? "unknown pipeline",
    safeOutputDir: readOptionalEnv(env, "AW_SAFE_OUTPUT_DIR"),
    project: readOptionalEnv(env, "SYSTEM_TEAMPROJECT"),
    buildUri: readOptionalEnv(env, "BUILD_BUILDURI"),
    buildId: readOptionalEnv(env, "BUILD_BUILDID"),
    jobs: [
      { name: "Agent", result: readJobResult(env, "AW_AGENT_RESULT") },
      { name: "Detection", result: readJobResult(env, "AW_DETECTION_RESULT") },
      { name: "SafeOutputs", result: readJobResult(env, "AW_SAFEOUTPUTS_RESULT") },
    ],
    toolConfigs: {
      noop: parsePerToolConfigFlat(env, "AW_NOOP"),
      missing_tool: parsePerToolConfigFlat(env, "AW_MISSING_TOOL"),
      missing_data: parsePerToolConfigFlat(env, "AW_MISSING_DATA"),
    },
  };
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function toOptionalString(value: unknown): string | undefined {
  return typeof value === "string" ? value : undefined;
}

function toOptionalResult(value: unknown): Record<string, unknown> | null | undefined {
  if (value === null) return null;
  return isRecord(value) ? value : undefined;
}

function readManifestEntries(safeOutputDir: string | undefined): ManifestEntry[] {
  if (!safeOutputDir) {
    logInfo("AW_SAFE_OUTPUT_DIR is not set; skipping safe-outputs execution manifest scan");
    return [];
  }

  const manifestPath = join(safeOutputDir, EXECUTED_MANIFEST_FILENAME);
  if (!existsSync(manifestPath)) {
    logWarning(`Conclusion manifest not found: ${manifestPath}`);
    return [];
  }

  let raw: string;
  try {
    raw = readFileSync(manifestPath, "utf8");
  } catch (error) {
    logWarning(
      `Failed to read ${EXECUTED_MANIFEST_FILENAME}: ${(error as Error).message}`,
    );
    return [];
  }

  const entries: ManifestEntry[] = [];
  const lines = raw.split(/\r?\n/);
  for (let index = 0; index < lines.length; index += 1) {
    const line = lines[index]?.trim() ?? "";
    if (!line) continue;

    try {
      const parsed: unknown = JSON.parse(line);
      if (!isRecord(parsed) || typeof parsed.name !== "string") {
        logWarning(
          `Skipping ${EXECUTED_MANIFEST_FILENAME} line ${index + 1}: missing string 'name' field`,
        );
        continue;
      }
      entries.push({
        name: parsed.name,
        status: toOptionalString(parsed.status),
        context: parsed.context === null ? null : toOptionalString(parsed.context),
        result: toOptionalResult(parsed.result),
        error: parsed.error === null ? null : toOptionalString(parsed.error),
        timestamp: toOptionalString(parsed.timestamp),
        tool_name: toOptionalString(parsed.tool_name),
        data_type: toOptionalString(parsed.data_type),
        reason: toOptionalString(parsed.reason),
      });
    } catch (error) {
      logWarning(
        `Skipping malformed ${EXECUTED_MANIFEST_FILENAME} line ${index + 1}: ${(error as Error).message}`,
      );
    }
  }

  logInfo(`Loaded ${entries.length} conclusion manifest entr${entries.length === 1 ? "y" : "ies"}`);
  return entries;
}

function normalizeToolName(name: string): string {
  return name.replaceAll("-", "_");
}

function unique(values: readonly (string | null | undefined)[]): string[] {
  const seen = new Set<string>();
  const result: string[] = [];
  for (const value of values) {
    const trimmed = value?.trim();
    if (!trimmed || seen.has(trimmed)) continue;
    seen.add(trimmed);
    result.push(trimmed);
  }
  return result;
}

function renderList(items: readonly string[], emptyMessage: string): string {
  if (items.length === 0) return `- ${emptyMessage}`;
  return items.map((item) => `- ${item}`).join("\n");
}

function appendStats(body: string, config: RuntimeConfig, extra: readonly string[] = []): string {
  const lines = [
    "## Conclusion stats",
    `- Pipeline: ${config.pipelineName}`,
    `- Project: ${config.project ?? "unknown"}`,
    `- Build ID: ${config.buildId ?? "unknown"}`,
    `- Build URL: ${config.buildUri ?? "unknown"}`,
    ...config.jobs.map((job) => `- ${job.name}: ${job.result}`),
    ...extra,
  ];
  return `${body}\n\n${lines.join("\n")}`;
}

function extractNamedValue(
  entry: ManifestEntry,
  fieldName: "tool_name" | "data_type",
  pattern: RegExp,
): string | undefined {
  const direct = entry[fieldName];
  if (typeof direct === "string" && direct.trim().length > 0) {
    return direct.trim();
  }
  const context = entry.context?.trim();
  if (!context) return undefined;
  const match = pattern.exec(context);
  return match?.[1]?.trim();
}

function buildPipelineFailureReport(config: RuntimeConfig): SignalReport | null {
  const failedJobs = config.jobs.filter((job) => job.result === "Failed");
  if (failedJobs.length === 0) return null;

  const lines = [
    "The conclusion job detected an upstream agentic-pipeline failure.",
    "",
    "## Failed jobs",
    renderList(
      failedJobs.map((job) => `${job.name} (${job.result})`),
      "No failed jobs were identified",
    ),
    "",
    "## Build",
    `- Build URL: ${config.buildUri ?? "unknown"}`,
    `- Build ID: ${config.buildId ?? "unknown"}`,
  ];

  return {
    kind: "pipeline_failure",
    defaultTitle: `[ado-aw] Pipeline failure: ${config.pipelineName}`,
    body: appendStats(lines.join("\n"), config),
  };
}

function buildNoopReport(config: RuntimeConfig, entries: readonly ManifestEntry[]): SignalReport | null {
  if (entries.length === 0) return null;

  const contexts = unique(entries.map((entry) => entry.context ?? undefined));
  const lines = [
    "The conclusion job detected one or more `noop` diagnostic signals.",
    "",
    `Occurrences: ${entries.length}`,
    "",
    "## Reported context",
    renderList(contexts, "No noop context was recorded"),
  ];

  return {
    kind: "noop",
    defaultTitle: `[ado-aw] Agent reported no operation: ${config.pipelineName}`,
    body: appendStats(lines.join("\n"), config),
  };
}

function buildMissingToolReport(
  config: RuntimeConfig,
  entries: readonly ManifestEntry[],
): SignalReport | null {
  if (entries.length === 0) return null;

  const toolNames = unique(
    entries.map((entry) =>
      extractNamedValue(entry, "tool_name", /tool[_ -]?name\s*:\s*([^\r\n,;]+)/i)
    ),
  );
  const contexts = unique(entries.map((entry) => entry.context ?? undefined));
  const lines = [
    "The conclusion job detected one or more `missing_tool` diagnostic signals.",
    "",
    `Occurrences: ${entries.length}`,
    "",
    "## Missing tools",
    renderList(toolNames, "Tool names were not recorded in the execution manifest"),
    "",
    "## Reported context",
    renderList(contexts, "No additional context was recorded"),
  ];

  return {
    kind: "missing_tool",
    defaultTitle: `[ado-aw] Agent encountered missing tool: ${config.pipelineName}`,
    body: appendStats(lines.join("\n"), config),
  };
}

function buildMissingDataReport(
  config: RuntimeConfig,
  entries: readonly ManifestEntry[],
): SignalReport | null {
  if (entries.length === 0) return null;

  const dataTypes = unique(
    entries.map((entry) =>
      extractNamedValue(entry, "data_type", /data[_ -]?type\s*:\s*([^\r\n,;]+)/i)
    ),
  );
  const contexts = unique(entries.map((entry) => entry.context ?? undefined));
  const reasons = unique(entries.map((entry) => entry.reason ?? undefined));
  const lines = [
    "The conclusion job detected one or more `missing_data` diagnostic signals.",
    "",
    `Occurrences: ${entries.length}`,
    "",
    "## Missing data types",
    renderList(dataTypes, "Data types were not recorded in the execution manifest"),
    "",
    "## Reasons",
    renderList(reasons, "No explicit reason was recorded"),
    "",
    "## Reported context",
    renderList(contexts, "No additional context was recorded"),
  ];

  return {
    kind: "missing_data",
    defaultTitle: `[ado-aw] Agent reported missing data: ${config.pipelineName}`,
    body: appendStats(lines.join("\n"), config),
  };
}

/** Azure DevOps work-item titles (System.Title) are capped at 255 chars. */
const MAX_WORK_ITEM_TITLE_LEN = 255;

/**
 * Build the work-item title from the per-tool title-prefix.
 * Mirrors gh-aw's convention: `${titlePrefix} ${pipelineName}`.
 * When no prefix is configured, returns undefined so the caller
 * can fall back to the signal's built-in default title.
 *
 * The result is truncated to ADO's 255-character System.Title limit so an
 * over-long prefix + pipeline name can't make createWorkItem throw (which
 * fileSignal would otherwise swallow as a warning, silently dropping the
 * work item). Truncation preserves dedup stability because the same inputs
 * always produce the same truncated title.
 */
function buildTitle(
  titlePrefix: string | undefined,
  pipelineName: string,
): string | undefined {
  if (!titlePrefix) return undefined;
  const title = `${titlePrefix} ${pipelineName}`.trim();
  if (title.length <= MAX_WORK_ITEM_TITLE_LEN) return title;
  return title.slice(0, MAX_WORK_ITEM_TITLE_LEN);
}

function getToolConfigKey(kind: SignalKind): string {
  switch (kind) {
    case "pipeline_failure": return "pipeline_failure";
    case "noop": return "noop";
    case "missing_tool": return "missing_tool";
    case "missing_data": return "missing_data";
  }
}

function buildWorkItemConfig(
  config: RuntimeConfig,
  signal: SignalReport,
): WorkItemReportConfig {
  const toolConfig = config.toolConfigs[getToolConfigKey(signal.kind)];
  return {
    // Note: `enabled` is always true here — main() returns early when
    // reportFailureAsWorkItem is false, and per-tool opt-out is handled
    // in fileSignal(). The field exists in WorkItemReportConfig for
    // callers outside conclusion.js (e.g. direct wit.ts consumers).
    enabled: true,
    title: buildTitle(
      toolConfig?.titlePrefix,
      config.pipelineName,
    ),
    workItemType: toolConfig?.workItemType ?? "Task",
    areaPath: toolConfig?.areaPath,
    iterationPath: toolConfig?.iterationPath,
    tags: toolConfig?.tags ?? [],
    includeStats: true,
  };
}

async function fileSignal(
  config: RuntimeConfig,
  signal: SignalReport,
): Promise<void> {
  if (!config.project) {
    logWarning(`SYSTEM_TEAMPROJECT is not set; skipping ${signal.kind} work-item filing`);
    return;
  }

  // Per-tool opt-out: report-as-work-item: false.
  // Note: pipeline_failure has no entry in toolConfigs (intentional — matches
  // gh-aw, which has no per-tool config for pipeline failures). When toolConfig
  // is undefined the guard is skipped and filing proceeds normally.
  const toolConfig = config.toolConfigs[getToolConfigKey(signal.kind)];
  if (toolConfig && !toolConfig.reportAsWorkItem) {
    logInfo(`${signal.kind}: per-tool report-as-work-item is false, skipping`);
    return;
  }

  try {
    const result = await fileOrAppendWorkItem(
      config.project,
      buildWorkItemConfig(config, signal),
      signal.defaultTitle,
      signal.body,
    );
    logInfo(`${signal.kind}: ${result.message}`);
  } catch (error) {
    logWarning(
      `Failed to file ${signal.kind} work item: ${(error as Error).message}`,
    );
  }
}

export async function main(env: NodeJS.ProcessEnv = process.env): Promise<number> {
  const config = loadConfig(env);
  logInfo(`Conclusion reporting started for ${config.pipelineName}`);

  const manifestEntries = readManifestEntries(config.safeOutputDir);
  const noopEntries = manifestEntries.filter((entry) => normalizeToolName(entry.name) === "noop");
  const missingToolEntries = manifestEntries.filter(
    (entry) => normalizeToolName(entry.name) === "missing_tool",
  );
  const missingDataEntries = manifestEntries.filter(
    (entry) => normalizeToolName(entry.name) === "missing_data",
  );

  const signals = [
    buildPipelineFailureReport(config),
    buildNoopReport(config, noopEntries),
    buildMissingToolReport(config, missingToolEntries),
    buildMissingDataReport(config, missingDataEntries),
  ].filter((signal): signal is SignalReport => signal !== null);

  if (signals.length === 0) {
    logInfo("Conclusion reporting found no failure or diagnostic signals");
    return 0;
  }

  logInfo(
    `Conclusion reporting detected signals: ${signals.map((signal) => signal.kind).join(", ")}`,
  );

  if (!config.reportFailureAsWorkItem) {
    logInfo("Conclusion work-item filing disabled via AW_REPORT_FAILURE_AS_WORK_ITEM=false");
    return 0;
  }

  for (const signal of signals) {
    await fileSignal(config, signal);
  }

  return 0;
}

void main().then(
  (exitCode) => {
    process.exit(exitCode);
  },
  (error: unknown) => {
    logError(`conclusion bundle crashed: ${(error as Error).message}`);
    process.exit(0);
  },
);
