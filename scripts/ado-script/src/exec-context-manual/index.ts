/**
 * exec-context-manual — Stage manual-run signals for the agent on
 * manually-queued Azure DevOps builds.
 *
 * Invoked from the Agent job's prepare phase by `manual.rs::prepare_step`
 * (in the Rust compiler). Reads requestor identity and parameter
 * values from ADO env vars and stages:
 *
 *   - aw-context/manual/requested-for       — display name
 *   - aw-context/manual/requested-for-email — present only when
 *     `manual.include-email: true` (front matter); BUILD_REQUESTEDFOREMAIL
 *     env var is then provided by the prepare step. Absent otherwise.
 *   - aw-context/manual/parameters.json     — pretty-printed JSON
 *     object of user-declared parameter values (NOT the auto-injected
 *     clearMemory parameter; that is auto-added at IR-build time and
 *     therefore not in `front_matter.parameters`, so the Rust
 *     contributor never emits a `PARAM_clearMemory` env var).
 *
 * It also appends a tailored success-or-failure fragment under
 * `## Manual run context` to the agent prompt at
 * `/tmp/awf-tools/agent-prompt.md`.
 *
 * ## Trust boundary
 *
 *   - No bearer; SYSTEM_ACCESSTOKEN is NOT projected into this step's
 *     env (the Rust contributor enforces this — see manual.rs).
 *   - No network calls. All inputs come from ADO env vars.
 *   - Parameter VALUES come from user input at queue time and could
 *     contain arbitrary characters. They are JSON-serialised (which
 *     handles all escaping) before being written to
 *     `parameters.json` and are SANITISED via the shared
 *     `validate.sanitizeForPrompt` helper before any interpolation
 *     into the agent prompt fragment.
 *   - Parameter NAMES are guaranteed identifier-shaped at this point
 *     (validated upstream by `crate::validate::is_valid_parameter_name`
 *     during pipeline build; the Rust contributor re-checks them at
 *     emit time as defence-in-depth). They are safe to interpolate
 *     into the prompt fragment without sanitisation.
 */
import { mkdirSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";

import { appendToAgentPrompt } from "../shared/prompt.js";
import { sanitizeForPrompt } from "../shared/validate.js";

const DEFAULT_AGENT_PROMPT_PATH = "/tmp/awf-tools/agent-prompt.md";

/** Cap each interpolated parameter value at this many characters in the
 * agent prompt fragment (the full untruncated value remains in
 * `parameters.json`). Long values get truncated with an ellipsis
 * marker. Matches the cap used by the PR contributor's
 * `sanitizeForPrompt` default. */
const PROMPT_VALUE_CAP = 200;

/** Pretty-printing indent for the staged JSON snapshot. Two spaces
 * keeps the on-disk artefact human-readable when the agent `cat`s
 * the file. */
const JSON_INDENT = 2;

/**
 * Resolve the agent prompt file path. Production: hard-coded
 * `/tmp/awf-tools/agent-prompt.md` (created by base.yml's
 * "Prepare agent prompt" step). Tests may override via the
 * `AW_AGENT_PROMPT_FILE` env var.
 *
 * SECURITY NOTE: `AW_AGENT_PROMPT_FILE` is a *test-only* seam — see
 * `exec-context-pr/index.ts::agentPromptPath` for the full rationale
 * (same posture mirrored here for consistency).
 */
function agentPromptPath(env: NodeJS.ProcessEnv): string {
  return env.AW_AGENT_PROMPT_FILE && env.AW_AGENT_PROMPT_FILE.length > 0
    ? env.AW_AGENT_PROMPT_FILE
    : DEFAULT_AGENT_PROMPT_PATH;
}

function awContextDir(env: NodeJS.ProcessEnv): string {
  const root =
    env.BUILD_SOURCESDIRECTORY && env.BUILD_SOURCESDIRECTORY.length > 0
      ? env.BUILD_SOURCESDIRECTORY
      : process.cwd();
  return join(root, "aw-context");
}

function awManualDir(env: NodeJS.ProcessEnv): string {
  return join(awContextDir(env), "manual");
}

/** Harvest `PARAM_*` env vars into a `{ name -> value }` object,
 * with keys sorted alphabetically for deterministic JSON output. */
function collectParameters(env: NodeJS.ProcessEnv): Record<string, string> {
  const out: Record<string, string> = {};
  const keys = Object.keys(env)
    .filter((k) => k.startsWith("PARAM_"))
    .sort();
  for (const k of keys) {
    const value = env[k];
    if (value === undefined) continue;
    const name = k.slice("PARAM_".length);
    out[name] = value;
  }
  return out;
}

/**
 * Build the SUCCESS prompt fragment appended to the agent prompt file
 * after the manual context has been staged.
 *
 * Identifier interpolation MUST sanitise any user-provided values
 * (display name, parameter values) — they come from ADO predefined
 * variables or user-supplied parameter inputs at queue time. The
 * parameter NAMES are guaranteed identifier-shaped (validated
 * upstream by the Rust contributor); they are safe to interpolate
 * without sanitisation.
 */
export function successFragment(args: {
  requestedFor: string;
  requestedForEmail: string | undefined;
  parameters: Record<string, string>;
}): string {
  const { requestedFor, requestedForEmail, parameters } = args;
  const lines = ["", "## Manual run context", ""];

  const requestor = sanitizeForPrompt(requestedFor || "<unknown>");
  if (requestedForEmail && requestedForEmail.length > 0) {
    lines.push(
      `This run was queued manually by **${requestor}** (${sanitizeForPrompt(
        requestedForEmail,
      )}).`,
    );
  } else {
    lines.push(`This run was queued manually by **${requestor}**.`);
  }
  lines.push("");

  const paramNames = Object.keys(parameters);
  if (paramNames.length === 0) {
    // No user-declared parameters → no parameter snapshot. The
    // contributor only activates when at least one is declared, so
    // this branch is mainly defensive (e.g. if a future iteration
    // calls into the bundle with no PARAM_* env vars set).
    lines.push("No user-declared parameter values were captured.");
  } else {
    lines.push("Runtime parameter values:");
    lines.push("");
    for (const name of paramNames) {
      const raw = parameters[name] ?? "";
      const trimmedDisplay = sanitizeForPrompt(raw, PROMPT_VALUE_CAP);
      // Use markdown list-row form so the agent can scan
      // name-value pairs at a glance. Names are validated as
      // ADO identifiers so they cannot contain pipe / newline /
      // markdown control characters.
      lines.push(`  - \`${name}\`: \`${trimmedDisplay}\``);
    }
    lines.push("");
    lines.push(
      "The full untruncated parameter object is at `aw-context/manual/parameters.json`.",
    );
  }
  lines.push("");
  return lines.join("\n");
}

/** Build the FAILURE prompt fragment for the rare infra-error case
 * (e.g. workspace not writable). */
export function failureFragment(reason: string): string {
  return [
    "",
    "## Manual run context",
    "",
    `Manual context preparation failed.`,
    `Reason: ${sanitizeForPrompt(reason, PROMPT_VALUE_CAP)}`,
    "",
    "Continue with the task using whatever context you have. Do NOT",
    "invent values for parameters you were supposed to receive.",
    "",
  ].join("\n");
}

export function main(env: NodeJS.ProcessEnv = process.env): number {
  const manualDir = awManualDir(env);
  const promptPath = agentPromptPath(env);

  // Hard-fail on infra-level errors (read-only workspace, missing
  // parent dir, etc.). Matches the PR contributor's posture.
  try {
    mkdirSync(manualDir, { recursive: true });
  } catch (err) {
    process.stderr.write(
      `[aw-context] fatal: could not create ${manualDir} (check BUILD_SOURCESDIRECTORY permissions): ${(err as Error).message}\n`,
    );
    appendToAgentPrompt(promptPath, failureFragment((err as Error).message));
    return 1;
  }

  // Clean any stale artefacts from a prior run. `force: true` makes
  // the call a no-op when the file doesn't exist.
  for (const f of ["requested-for", "requested-for-email", "parameters.json"]) {
    rmSync(join(manualDir, f), { force: true });
  }

  const requestedFor = env.BUILD_REQUESTEDFOR ?? "";
  const requestedForEmail = env.BUILD_REQUESTEDFOREMAIL;
  const parameters = collectParameters(env);

  writeFileSync(join(manualDir, "requested-for"), requestedFor, "utf8");
  if (requestedForEmail !== undefined && requestedForEmail.length > 0) {
    writeFileSync(
      join(manualDir, "requested-for-email"),
      requestedForEmail,
      "utf8",
    );
  }
  // JSON.stringify handles all escaping for us; values can be
  // arbitrary user-supplied strings and the resulting file is
  // guaranteed valid JSON.
  writeFileSync(
    join(manualDir, "parameters.json"),
    JSON.stringify(parameters, null, JSON_INDENT) + "\n",
    "utf8",
  );

  appendToAgentPrompt(
    promptPath,
    successFragment({ requestedFor, requestedForEmail, parameters }),
  );

  process.stdout.write(
    `[aw-context] manual context staged: requestedFor=${sanitizeForPrompt(requestedFor)} parameters=${Object.keys(parameters).length}\n`,
  );
  return 0;
}

// Top-level invocation guarded so tests can import this module and
// call `main(env)` without terminating the test process. The bundle
// is invoked as `node exec-context-manual.js` from the prepare step.
import { fileURLToPath } from "node:url";
if (
  typeof process !== "undefined" &&
  process.argv[1] &&
  process.argv[1] === fileURLToPath(import.meta.url)
) {
  const exitCode = main();
  process.exit(exitCode);
}
