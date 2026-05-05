/**
 * Prompt renderer entry point.
 *
 * Reads a base64-encoded `PromptSpec` from `ADO_AW_PROMPT_SPEC` env,
 * loads the source markdown from the workspace, strips its YAML front
 * matter, applies variable substitution, appends extension supplements,
 * and writes the result to the configured output path. On any
 * unrecoverable error, logs via VSO and exits non-zero.
 */
import * as fs from "node:fs";
import * as path from "node:path";
import type { PromptSpec } from "../shared/types-prompt.gen.js";
import { stripFrontMatter } from "./frontmatter.js";
import { substitute } from "./substitute.js";
import { complete, logError, logWarning } from "../shared/vso-logger.js";

/**
 * The `version` value this build of `prompt.js` understands. Must be
 * kept in lockstep with `PROMPT_SPEC_VERSION` in
 * `src/compile/prompt_ir.rs`.
 */
const SUPPORTED_VERSION = 1;

function decodeSpec(raw: string): PromptSpec {
  const json = Buffer.from(raw, "base64").toString("utf8");
  return JSON.parse(json) as PromptSpec;
}

export async function main(): Promise<void> {
  const raw = process.env.ADO_AW_PROMPT_SPEC;
  if (!raw) {
    logError("ADO_AW_PROMPT_SPEC env var missing");
    complete("Failed");
    process.exit(1);
  }

  let spec: PromptSpec;
  try {
    spec = decodeSpec(raw);
  } catch (e) {
    logError(
      `Failed to decode ADO_AW_PROMPT_SPEC: ${(e as Error).message}`,
    );
    complete("Failed");
    process.exit(1);
  }

  if (spec.version !== SUPPORTED_VERSION) {
    logError(
      `Unsupported PromptSpec version ${spec.version}; this prompt.js supports version ${SUPPORTED_VERSION}. ` +
        `Recompile your pipeline with a matching ado-aw release.`,
    );
    complete("Failed");
    process.exit(1);
  }

  if (!fs.existsSync(spec.source_path)) {
    logError(`Source markdown not found: ${spec.source_path}`);
    complete("Failed");
    process.exit(1);
  }

  const source = fs.readFileSync(spec.source_path, "utf8");
  let body: string;
  try {
    body = stripFrontMatter(source);
  } catch (e) {
    logError(
      `Failed to strip front matter from ${spec.source_path}: ${(e as Error).message}`,
    );
    complete("Failed");
    process.exit(1);
  }

  const parts: string[] = [body.trim()];
  for (const supp of spec.supplements) {
    parts.push(supp.content.trim());
  }
  let rendered = parts.filter((s) => s.length > 0).join("\n\n");

  rendered = substitute(rendered, spec.parameters, (msg) => logWarning(msg));

  if (rendered.trim().length === 0) {
    logError(
      `Rendered prompt is empty (source: ${spec.source_path}). ` +
        `Front-matter-only files are not valid agents.`,
    );
    complete("Failed");
    process.exit(1);
  }

  fs.mkdirSync(path.dirname(spec.output_path), { recursive: true });
  fs.writeFileSync(spec.output_path, rendered);

  complete("Succeeded", `prompt rendered: ${spec.output_path}`);
}

main().catch((e) => {
  logError(`prompt renderer crashed: ${(e as Error).message}`);
  complete("Failed");
  process.exit(1);
});
