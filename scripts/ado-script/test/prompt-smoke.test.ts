/**
 * End-to-end smoke test of the bundled prompt.js.
 *
 * Spawns `node dist/prompt/index.js` as a subprocess with a hand-rolled
 * `PromptSpec` (base64-encoded into `ADO_AW_PROMPT_SPEC`) and a known
 * source `.md` file in a temp directory. Verifies that the rendered
 * output is what the contract promises: front matter stripped,
 * supplements appended, and `${{ parameters.* }}` / `$(VAR)` patterns
 * substituted.
 */
import { spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

const __dirname = dirname(fileURLToPath(import.meta.url));
const bundlePath = resolve(__dirname, "../dist/prompt/index.js");

interface Spec {
  version: number;
  source_path: string;
  output_path: string;
  supplements: { name: string; content: string }[];
  parameters: string[];
}

function encodeSpec(spec: Spec): string {
  return Buffer.from(JSON.stringify(spec)).toString("base64");
}

function runPrompt(
  spec: Spec,
  extraEnv: Record<string, string> = {},
): {
  stdout: string;
  stderr: string;
  status: number | null;
} {
  const result = spawnSync(process.execPath, [bundlePath], {
    env: {
      PATH: process.env["PATH"] ?? "",
      ADO_AW_PROMPT_SPEC: encodeSpec(spec),
      ...extraEnv,
    },
    encoding: "utf8",
  });
  return {
    stdout: result.stdout ?? "",
    stderr: result.stderr ?? "",
    status: result.status,
  };
}

let workdir: string;

beforeEach(() => {
  workdir = mkdtempSync(join(tmpdir(), "prompt-smoke-"));
});

afterEach(() => {
  rmSync(workdir, { recursive: true, force: true });
});

describe("prompt.js smoke", () => {
  it("renders front-matter-stripped body with supplement and parameter", () => {
    const source = join(workdir, "agent.md");
    const output = join(workdir, "out.md");
    writeFileSync(
      source,
      "---\nname: x\ndescription: y\n---\n# Hello ${{ parameters.target }}\n",
    );
    const spec: Spec = {
      version: 1,
      source_path: source,
      output_path: output,
      supplements: [{ name: "Lean 4", content: "Use lake build." }],
      parameters: ["target"],
    };
    const { status, stderr } = runPrompt(spec, { ADO_AW_PARAM_TARGET: "main" });
    expect(status, `prompt failed: ${stderr}`).toBe(0);
    const rendered = readFileSync(output, "utf8");
    expect(rendered).toContain("# Hello main");
    expect(rendered).toContain("Use lake build.");
  });

  it("fails when the source .md does not exist", () => {
    const spec: Spec = {
      version: 1,
      source_path: join(workdir, "missing.md"),
      output_path: join(workdir, "out.md"),
      supplements: [],
      parameters: [],
    };
    const { status, stdout } = runPrompt(spec);
    expect(status).not.toBe(0);
    expect(stdout).toContain("Source markdown not found");
  });

  it("fails when PromptSpec version is unknown", () => {
    const source = join(workdir, "agent.md");
    writeFileSync(source, "---\nname: x\n---\nbody\n");
    const spec: Spec = {
      version: 9999,
      source_path: source,
      output_path: join(workdir, "out.md"),
      supplements: [],
      parameters: [],
    };
    const { status, stdout } = runPrompt(spec);
    expect(status).not.toBe(0);
    expect(stdout).toContain("Unsupported PromptSpec version");
  });

  it("does NOT re-expand $(...) injected via a parameter value (single-pass)", () => {
    // Mirror of the unit-test attack at the smoke layer: even with
    // every env-var supplier in place, the chained value must stay
    // literal in the rendered output.
    const source = join(workdir, "agent.md");
    const output = join(workdir, "out.md");
    writeFileSync(
      source,
      "---\nname: x\n---\nTarget: ${{ parameters.target }}\n",
    );
    const spec: Spec = {
      version: 1,
      source_path: source,
      output_path: output,
      supplements: [],
      parameters: ["target"],
    };
    const { status, stderr } = runPrompt(spec, {
      ADO_AW_PARAM_TARGET: "$(System.AccessToken)",
      SYSTEM_ACCESSTOKEN: "SECRET",
    });
    expect(status, `prompt failed: ${stderr}`).toBe(0);
    const rendered = readFileSync(output, "utf8");
    expect(rendered).toContain("Target: $(System.AccessToken)");
    expect(rendered).not.toContain("SECRET");
  });
});
