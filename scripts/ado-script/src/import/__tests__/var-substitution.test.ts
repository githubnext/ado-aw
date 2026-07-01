import { describe, expect, it } from "vitest";
import { readText, runImportSource, withScratchDir, writeFixture } from "./helpers.js";

describe("runtime-import ADO variable substitution", () => {
  it("substitutes a single $(name) token with its value", () => {
    withScratchDir("var-single", (dir) => {
      const target = writeFixture(dir, "prompt.md", "cd $(Build.SourcesDirectory) && ls\n");

      const result = runImportSource(target, {
        vars: ["Build.SourcesDirectory=/agent/_work/1/s"],
      });

      expect(result.status).toBe(0);
      expect(readText(target)).toBe("cd /agent/_work/1/s && ls\n");
    });
  });

  it("substitutes multiple distinct variables", () => {
    withScratchDir("var-multi", (dir) => {
      const target = writeFixture(
        dir,
        "prompt.md",
        "root=$(Build.SourcesDirectory) repo=$(Build.Repository.Name)\n",
      );

      const result = runImportSource(target, {
        vars: [
          "Build.SourcesDirectory=/agent/_work/1/s",
          "Build.Repository.Name=my-repo",
        ],
      });

      expect(result.status).toBe(0);
      expect(readText(target)).toBe("root=/agent/_work/1/s repo=my-repo\n");
    });
  });

  it("substitutes variables inside inlined snippets too", () => {
    // The substitution pass runs on the fully expanded prompt, so a
    // `$(...)` token that lives in an imported file is also replaced.
    withScratchDir("var-in-snippet", (dir) => {
      writeFixture(dir, "snippet.md", "workdir is $(Build.SourcesDirectory)\n");
      const target = writeFixture(
        dir,
        "prompt.md",
        "{{#runtime-import ./snippet.md}}\n",
      );

      const result = runImportSource(target, {
        vars: ["Build.SourcesDirectory=/agent/_work/1/s"],
      });

      expect(result.status).toBe(0);
      expect(readText(target)).toBe("workdir is /agent/_work/1/s\n\n");
    });
  });

  it("leaves unknown $(...) macros untouched", () => {
    withScratchDir("var-unknown", (dir) => {
      const target = writeFixture(
        dir,
        "prompt.md",
        "known=$(Build.Repository.Name) unknown=$(System.AccessToken)\n",
      );

      const result = runImportSource(target, {
        vars: ["Build.Repository.Name=my-repo"],
      });

      expect(result.status).toBe(0);
      expect(readText(target)).toBe("known=my-repo unknown=$(System.AccessToken)\n");
    });
  });

  it("treats values literally (no regex metacharacter interpretation)", () => {
    withScratchDir("var-literal-value", (dir) => {
      const target = writeFixture(dir, "prompt.md", "p=$(Build.SourcesDirectory)\n");

      // A value containing characters that would be special in a regex
      // replacement (`$&`, `$1`) must be inserted verbatim.
      const result = runImportSource(target, {
        vars: ["Build.SourcesDirectory=/a$1b$&c"],
      });

      expect(result.status).toBe(0);
      expect(readText(target)).toBe("p=/a$1b$&c\n");
    });
  });

  it("replaces every occurrence of a repeated token", () => {
    withScratchDir("var-repeated", (dir) => {
      const target = writeFixture(
        dir,
        "prompt.md",
        "$(Build.Repository.Name)/$(Build.Repository.Name)\n",
      );

      const result = runImportSource(target, {
        vars: ["Build.Repository.Name=r"],
      });

      expect(result.status).toBe(0);
      expect(readText(target)).toBe("r/r\n");
    });
  });

  it("rejects a --var argument without an '=' separator", () => {
    withScratchDir("var-malformed", (dir) => {
      const target = writeFixture(dir, "prompt.md", "noop\n");

      const result = runImportSource(target, {
        vars: ["noequals"],
      });

      expect(result.status).toBe(1);
      expect(result.stdout).toContain("--var expects name=value");
      // Target must be left untouched when arg parsing fails.
      expect(readText(target)).toBe("noop\n");
    });
  });
});
