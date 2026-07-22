import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, isAbsolute, resolve } from "node:path";

// The path capture `[^\s}]+` deliberately excludes `}` so the regex
// terminates cleanly at the closing `}}`. The compile-time resolver
// (`resolve_imports_inline` in `src/compile/extensions/ado_script.rs`)
// rejects `}` in paths up front so a marker like `{{#runtime-import foo}bar.md}}`
// fails with a clear compile-time error rather than being silently
// accepted at compile time and silently dropped at runtime.
const MARKER = /\{\{#runtime-import(\?)?\s+([^\s}]+)\s*\}\}/g;

// Detect Windows drive-letter absolute paths (`C:\foo`, `C:/foo`) on
// any host. Node's `path.isAbsolute` is platform-dependent — on Linux
// it doesn't recognise `C:\foo` as absolute — so this string-level
// check guarantees the same rejection regardless of where `import.js`
// runs. POSIX absolute (`/foo`) and UNC (`\\server\share`) are caught
// by `path.isAbsolute` on Linux/Windows respectively, and by the
// explicit prefix checks below otherwise.
function isDriveLetterAbsolute(path: string): boolean {
  return (
    path.length >= 3 &&
    /^[A-Za-z]$/.test(path.charAt(0)) &&
    path.charAt(1) === ":" &&
    (path.charAt(2) === "/" || path.charAt(2) === "\\")
  );
}

function isAbsolutePathStrict(path: string): boolean {
  return (
    isAbsolute(path) ||
    path.startsWith("/") ||
    path.startsWith("\\\\") ||
    isDriveLetterAbsolute(path)
  );
}

// Strip characters that would let an attacker-controlled `rawPath` break
// out of the `##vso[task.logissue type=error]…` framing:
//   * `]`  — closes the VSO command bracket prematurely.
//   * `\r`, `\n` — split the diagnostic line so subsequent text would be
//      parsed as a new ADO logging command.
//
// Note: `[` is intentionally NOT stripped. ADO's `##vso[…]` syntax
// requires a balanced `[…]` pair *and* the leading `##vso` literal to
// be parsed as a logging command. A path containing only `[` (without
// a closing `]` and without a fresh `##vso` prefix) cannot open a new
// command, so leaving `[` intact in the diagnostic message is safe and
// avoids mangling legitimate paths that happen to contain it.
//
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

// Parse the CLI:
//   node import.js <target> [--base <path>] [--var name=value ...]
// The optional --base flag sets the root that relative marker paths
// resolve against. When omitted, falls back to `dirname(target)`.
// In pipeline use the compiler always passes
// `--base "$(Build.SourcesDirectory)"` so that the marker is a
// trigger-repo-relative path (NOT absolute) — see
// `AdoScriptExtension::resolver_step` in
// src/compile/extensions/ado_script.rs.
//
// The repeatable `--var name=value` flag carries a small, fixed set of
// non-secret ADO variables (e.g. `Build.SourcesDirectory`,
// `Build.BuildId`). ADO expands the `$(...)` macros into
// concrete values in the resolver step's bash args before node runs, so
// the values arrive here already resolved. The allowlist is owned by the
// compiler (which emits the flags); import.js is a dumb substitutor and
// never reads these from the environment.
function parseArgs(argv: string[]): {
  target: string;
  base: string | null;
  vars: Map<string, string>;
} {
  if (argv.length === 0) {
    fail(["missing target file argument"]);
  }
  const target = argv[0]!;
  let base: string | null = null;
  const vars = new Map<string, string>();
  let i = 1;
  while (i < argv.length) {
    const arg = argv[i]!;
    if (arg === "--base") {
      const value = argv[i + 1];
      if (value === undefined) {
        fail(["--base requires a value"]);
      }
      base = value;
      i += 2;
    } else if (arg === "--var") {
      const value = argv[i + 1];
      if (value === undefined) {
        fail(["--var requires a value of the form name=value"]);
      }
      const eq = value.indexOf("=");
      // `eq <= 0` rejects both a missing `=` and an empty name.
      if (eq <= 0) {
        fail([`--var expects name=value, got: ${sanitizeForVsoMessage(value)}`]);
      }
      // Last-write-wins on a duplicate name. The compiler emits each var at
      // most once (see PROMPT_ADO_VARS in
      // src/compile/extensions/ado_script.rs), so this only matters for
      // hand-run invocations.
      vars.set(value.slice(0, eq), value.slice(eq + 1));
      i += 2;
    } else {
      fail([`unknown argument: ${sanitizeForVsoMessage(arg)}`]);
    }
  }
  return { target, base, vars };
}

function main(): void {
  const { target, base: baseArg, vars } = parseArgs(process.argv.slice(2));
  if (!existsSync(target)) {
    fail([`target file not found: ${sanitizeForVsoMessage(target)}`]);
  }

  const base = baseArg ?? dirname(target);
  const original = readFileSync(target, "utf8");
  const errors: string[] = [];

  // Single-pass by design: imported snippets are inserted verbatim and any
  // nested runtime-import markers inside them are not expanded. This matches
  // gh-aw's runtime-import behaviour.
  const expanded = original.replace(MARKER, (_whole, optional: string | undefined, rawPath: string) => {
    // Reject `..` segments — a malicious or compromised agent body could
    // otherwise reach files outside `base` (the trigger-repo checkout on
    // the agent VM). Mirrors `resolve_imports_inline` in
    // src/compile/extensions/ado_script.rs.
    const hasDotDotSegment = rawPath.split(/[\/\\]/).some((segment) => segment === "..");
    if (hasDotDotSegment) {
      errors.push(
        `invalid path '${sanitizeForVsoMessage(rawPath)}': '..' path components are not allowed`,
      );
      return "";
    }

    // Reject absolute paths — defence in depth, matching the
    // compile-time resolver. The agent VM has privileged material in
    // well-known locations (`/tmp/awf-tools/staging/mcpg-config.json`
    // contains MCP server config, `$SC_READ_TOKEN` etc.). Author
    // markers in the agent body don't actually reach this code path
    // today (single-pass means nested markers in the inlined body
    // aren't re-expanded), but enforcing the same restriction the
    // compiler enforces keeps the two resolvers in strict parity and
    // protects against future design changes (e.g. multi-pass).
    if (isAbsolutePathStrict(rawPath)) {
      errors.push(
        `invalid path '${sanitizeForVsoMessage(rawPath)}': absolute paths are not allowed (use a relative path rooted at --base)`,
      );
      return "";
    }

    const absPath = resolve(base, rawPath);

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

  // Substitute the compiler-provided ADO variables (e.g.
  // `$(Build.SourcesDirectory)`). Runs on the fully expanded prompt, so it
  // covers both the author body and any inlined snippets, giving the same
  // result whether imports are inlined at compile time (where ADO expands
  // the macro in the heredoc) or resolved here at runtime. Literal
  // split/join avoids regex metacharacter pitfalls in either the name or
  // the value; unknown `$(...)` macros are left untouched.
  let substituted = expanded;
  for (const [name, value] of vars) {
    substituted = substituted.split(`$(${name})`).join(value);
  }

  writeFileSync(target, substituted, "utf8");
}

main();
