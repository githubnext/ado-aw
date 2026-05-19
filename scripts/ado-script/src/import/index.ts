import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, isAbsolute, resolve } from "node:path";

const MARKER = /\{\{#runtime-import(\?)?\s+([^\s}]+)\s*\}\}/g;

// Strip characters that would let an attacker-controlled `rawPath` break
// out of the `##vso[task.logissue type=error]…` framing:
//   * `]`  — closes the VSO command bracket prematurely.
//   * `\r`, `\n` — split the diagnostic line so subsequent text would be
//      parsed as a new ADO logging command.
// Marker paths normally come from a compile-time-generated location, but
// `import.js` is also invoked against arbitrary author-written markers in
// the agent body, so this is a defence-in-depth guard.
function sanitizeForVsoMessage(value: string): string {
  return value.replace(/[\]\r\n]/g, "");
}

function fail(messages: string[]): never {
  for (const msg of messages) {
    process.stdout.write(`##vso[task.logissue type=error]runtime-import: ${msg}\n`);
  }
  process.exit(1);
}

function main(): void {
  const target = process.argv[2];
  if (!target) {
    fail(["missing target file argument"]);
  }
  if (!existsSync(target)) {
    fail([`target file not found: ${sanitizeForVsoMessage(target)}`]);
  }

  const base = process.env.ADO_AW_IMPORT_BASE ?? dirname(target);
  const original = readFileSync(target, "utf8");
  const errors: string[] = [];

  // Single-pass by design: imported snippets are inserted verbatim and any
  // nested runtime-import markers inside them are not expanded. This matches
  // gh-aw's runtime-import behaviour.
  const expanded = original.replace(MARKER, (_whole, optional: string | undefined, rawPath: string) => {
    const absPath = isAbsolute(rawPath) ? rawPath : resolve(base, rawPath);

    if (!existsSync(absPath)) {
      if (optional === "?") {
        return "";
      }
      errors.push(`file not found: ${sanitizeForVsoMessage(rawPath)}`);
      return "";
    }

    try {
      return readFileSync(absPath, "utf8");
    } catch (error) {
      errors.push(
        `failed to read ${sanitizeForVsoMessage(rawPath)}: ${sanitizeForVsoMessage((error as Error).message)}`,
      );
      return "";
    }
  });

  if (errors.length > 0) {
    fail(errors);
  }
  writeFileSync(target, expanded, "utf8");
}

main();
