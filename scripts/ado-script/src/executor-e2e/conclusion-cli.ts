/**
 * Wrapper around the compiled `conclusion.js` bundle for the deterministic E2E
 * harness.
 *
 * Production shape: the Conclusion job runs `node conclusion.js` after the
 * SafeOutputs job, reading `safe-outputs-executed.ndjson` from the downloaded
 * `safe_outputs` artifact and filing/appending Azure DevOps work items for the
 * diagnostic signals it finds (`noop`, `missing-tool`, `missing-data`) and for
 * upstream job failures. The compiler passes its configuration as flat env vars
 * (`AW_<TOOL>_TITLE_PREFIX`, `AW_<TOOL>_TAGS`, …) — see
 * `src/compile/agentic_pipeline.rs` and `docs/conclusion.md`.
 *
 * This module reproduces exactly that invocation against the manifest a real
 * `ado-aw execute` run just wrote, so the harness covers the whole
 * signal → manifest → work-item path rather than stopping at Stage 3.
 *
 * Test-harness module; not shipped in `ado-script.zip`.
 */
import { existsSync } from "node:fs";

import { partialOutput, spawnCollect } from "./execute-cli.js";
import { SkipError } from "./scenario.js";

/** Env var carrying the path to the compiled `conclusion.js` bundle. */
export const CONCLUSION_BUNDLE_ENV = "EXECUTOR_E2E_CONCLUSION_BUNDLE";

export interface RunConclusionOptions {
  /** Directory holding `safe-outputs-executed.ndjson`. */
  safeOutputDir: string;
  /** Pipeline name the reporter renders into titles and the stats block. */
  pipelineName: string;
  orgUrl: string;
  project: string;
  token: string;
  buildId: string;
  /** Conclusion-specific `AW_*` config vars (title prefix, tags, opt-outs). */
  config: Record<string, string>;
  log: (msg: string) => void;
}

export interface RunConclusionResult {
  exitCode: number;
  stdout: string;
  stderr: string;
}

/**
 * Resolve the compiled bundle path, or skip the scenario when it is absent.
 *
 * The bundle is a build artifact (`npm run build:conclusion`), not a checked-in
 * file, so a harness run that was not given one must skip rather than fail —
 * the same contract the optional-precondition scenarios use.
 */
export function resolveConclusionBundle(): string {
  const configured = process.env[CONCLUSION_BUNDLE_ENV]?.trim();
  if (!configured) {
    throw new SkipError(
      `${CONCLUSION_BUNDLE_ENV} is not set; run 'npm run build:conclusion' and point it at conclusion.js`,
    );
  }
  if (!existsSync(configured)) {
    throw new SkipError(`${CONCLUSION_BUNDLE_ENV}='${configured}' does not exist`);
  }
  return configured;
}

/**
 * Run the conclusion reporter once over `safeOutputDir`.
 *
 * `conclusion.js` is deliberately fail-open (it exits 0 even when work-item
 * filing fails, so post-pipeline housekeeping can never fail an otherwise green
 * build). A non-zero exit therefore means the bundle itself crashed, which we
 * surface as an error; a filing failure is caught by the scenario's assertion
 * against the ADO REST API instead of by the exit code.
 */
export async function runConclusion(
  opts: RunConclusionOptions,
): Promise<RunConclusionResult> {
  const bundle = resolveConclusionBundle();
  const env: NodeJS.ProcessEnv = {
    ...process.env,
    SYSTEM_ACCESSTOKEN: opts.token,
    SYSTEM_COLLECTIONURI: opts.orgUrl,
    SYSTEM_TEAMPROJECT: opts.project,
    BUILD_BUILDID: opts.buildId,
    AW_SAFE_OUTPUT_DIR: opts.safeOutputDir,
    AW_PIPELINE_NAME: opts.pipelineName,
    // The upstream job results the compiler wires in. All succeeded, so the
    // pipeline-failure signal stays silent and only the diagnostic signals in
    // the manifest are reported.
    AW_AGENT_RESULT: "Succeeded",
    AW_DETECTION_RESULT: "Succeeded",
    AW_SAFEOUTPUTS_RESULT: "Succeeded",
    ...opts.config,
  };

  opts.log(`[conclusion] running: node ${bundle} (AW_SAFE_OUTPUT_DIR=${opts.safeOutputDir})`);
  const { exitCode, stdout, stderr } = await spawnCollect(
    process.execPath,
    [bundle],
    env,
    "conclusion.js",
  );
  if (stdout.trim()) opts.log(`[conclusion] stdout:\n${stdout.trim()}`);
  if (stderr.trim()) opts.log(`[conclusion] stderr:\n${stderr.trim()}`);
  if (exitCode !== 0) {
    throw new Error(
      `conclusion.js exited ${exitCode}${partialOutput(stdout, stderr)}`,
    );
  }
  return { exitCode, stdout, stderr };
}
