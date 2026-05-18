import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, isAbsolute, resolve } from "node:path";

const MARKER = /\{\{#runtime-import(\?)?\s+([^\s}]+)\s*\}\}/g;

function fail(msg: string): never {
  process.stdout.write(`##vso[task.logissue type=error]runtime-import: ${msg}\n`);
  process.exit(1);
}

function main(): void {
  const target = process.argv[2];
  if (!target) {
    fail("missing target file argument");
  }
  if (!existsSync(target)) {
    fail(`target file not found: ${target}`);
  }

  const base = process.env.ADO_AW_IMPORT_BASE ?? dirname(target);
  const original = readFileSync(target, "utf8");
  let hadError: string | null = null;

  // Single-pass by design: imported snippets are inserted verbatim and any
  // nested runtime-import markers inside them are not expanded. This matches
  // gh-aw's runtime-import behaviour.
  const expanded = original.replace(MARKER, (_whole, optional: string | undefined, rawPath: string) => {
    const absPath = isAbsolute(rawPath) ? rawPath : resolve(base, rawPath);

    if (!existsSync(absPath)) {
      if (optional === "?") {
        return "";
      }
      hadError ??= `file not found: ${rawPath}`;
      return "";
    }

    try {
      return readFileSync(absPath, "utf8");
    } catch (error) {
      hadError ??= `failed to read ${rawPath}: ${(error as Error).message}`;
      return "";
    }
  });

  if (hadError) {
    fail(hadError);
  }
  writeFileSync(target, expanded, "utf8");
}

main();
