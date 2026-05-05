import { describe, expect, it } from "vitest";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";

/**
 * End-to-end test that exercises the compiled `prompt.js` bundle by
 * spawning a real Node process. Only runs when the bundle has been
 * built (via `npm run build`); otherwise the test is a no-op so it
 * doesn't fail in the unit-test profile.
 */
describe("prompt.js end-to-end (compiled bundle)", () => {
  it("reads source, strips front matter, appends supplements, writes output", () => {
    const __dirname = path.dirname(fileURLToPath(import.meta.url));
    const distPath = path.resolve(__dirname, "../dist/prompt/index.js");
    if (!fs.existsSync(distPath)) {
      // Bundle not built; skip silently in non-smoke runs.
      return;
    }
    const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "prompt-it-"));
    try {
      const src = path.join(tmp, "agent.md");
      const out = path.join(tmp, "out.md");
      fs.writeFileSync(
        src,
        "---\nname: x\n---\nHello ${{ parameters.who }}!\n",
      );
      const spec = {
        version: 1,
        source_path: src,
        output_path: out,
        supplements: [{ name: "Demo", content: "## Demo\nbody" }],
        parameters: ["who"],
      };
      const env: NodeJS.ProcessEnv = {
        ...process.env,
        ADO_AW_PROMPT_SPEC: Buffer.from(JSON.stringify(spec)).toString(
          "base64",
        ),
        ADO_AW_PARAM_WHO: "world",
      };
      execFileSync("node", [distPath], { env });
      const rendered = fs.readFileSync(out, "utf8");
      expect(rendered).toContain("Hello world!");
      expect(rendered).toContain("## Demo");
      expect(rendered).not.toContain("name: x");
    } finally {
      fs.rmSync(tmp, { recursive: true, force: true });
    }
  });

  it("fails with non-zero exit when source is missing", () => {
    const __dirname = path.dirname(fileURLToPath(import.meta.url));
    const distPath = path.resolve(__dirname, "../dist/prompt/index.js");
    if (!fs.existsSync(distPath)) {
      return;
    }
    const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "prompt-it-"));
    try {
      const out = path.join(tmp, "out.md");
      const spec = {
        version: 1,
        source_path: path.join(tmp, "missing.md"),
        output_path: out,
        supplements: [],
        parameters: [],
      };
      const env: NodeJS.ProcessEnv = {
        ...process.env,
        ADO_AW_PROMPT_SPEC: Buffer.from(JSON.stringify(spec)).toString(
          "base64",
        ),
      };
      let threw = false;
      try {
        execFileSync("node", [distPath], { env, stdio: "pipe" });
      } catch {
        threw = true;
      }
      expect(threw).toBe(true);
    } finally {
      fs.rmSync(tmp, { recursive: true, force: true });
    }
  });

  it("fails on unknown spec version", () => {
    const __dirname = path.dirname(fileURLToPath(import.meta.url));
    const distPath = path.resolve(__dirname, "../dist/prompt/index.js");
    if (!fs.existsSync(distPath)) {
      return;
    }
    const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "prompt-it-"));
    try {
      const src = path.join(tmp, "a.md");
      const out = path.join(tmp, "out.md");
      fs.writeFileSync(src, "---\nname: x\n---\nbody\n");
      const spec = {
        version: 999,
        source_path: src,
        output_path: out,
        supplements: [],
        parameters: [],
      };
      const env: NodeJS.ProcessEnv = {
        ...process.env,
        ADO_AW_PROMPT_SPEC: Buffer.from(JSON.stringify(spec)).toString(
          "base64",
        ),
      };
      let threw = false;
      try {
        execFileSync("node", [distPath], { env, stdio: "pipe" });
      } catch {
        threw = true;
      }
      expect(threw).toBe(true);
    } finally {
      fs.rmSync(tmp, { recursive: true, force: true });
    }
  });
});
