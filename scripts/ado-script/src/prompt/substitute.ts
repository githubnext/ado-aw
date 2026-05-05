/**
 * Apply runtime variable substitution to rendered prompt content.
 *
 * Patterns recognised:
 *
 * - `${{ parameters.NAME }}` where NAME matches `[A-Za-z_][A-Za-z0-9_-]*`
 *   and NAME is in the `parameters` allow-list:
 *   resolved from env `ADO_AW_PARAM_<NAME upper, hyphen→underscore>`.
 *   Names not in the allow-list, or in the allow-list but with no env
 *   value, are left verbatim with a per-occurrence warning.
 *
 * - `$(VAR)` and `$(VAR.SUB)` where the captured name matches
 *   `[A-Za-z_][A-Za-z0-9_.]*`: resolved from env
 *   `<name upper, dot→underscore>` (ADO native convention). Unset
 *   variables are left verbatim with a warning.
 *
 * - `$[ ... ]`: left verbatim with one warning per render.
 *
 * - `\$(...)`: backslash-escaped form. The backslash is stripped and
 *   the literal `$(...)` is left untouched (no env lookup).
 *
 * Warnings are reported via the `warn` callback so the caller can route
 * them to the VSO logger.
 */
export function substitute(
  source: string,
  parameters: string[],
  warn: (msg: string) => void,
): string {
  const allowed = new Set(parameters);
  const env = process.env;

  // 1. $[ ... ] — leave verbatim, warn once if any present.
  if (/\$\[[^\]]*\]/.test(source)) {
    warn(
      "Found $[...] runtime expressions in prompt; these are not substituted by prompt.js and will reach the agent verbatim.",
    );
  }

  // 2. ${{ parameters.NAME }}
  source = source.replace(
    /\$\{\{\s*parameters\.([A-Za-z_][A-Za-z0-9_-]*)\s*\}\}/g,
    (match, name: string) => {
      if (!allowed.has(name)) {
        warn(
          `Unknown parameter '${name}' referenced in prompt; left verbatim.`,
        );
        return match;
      }
      const envName =
        "ADO_AW_PARAM_" + name.toUpperCase().replace(/-/g, "_");
      const v = env[envName];
      if (v === undefined) {
        warn(
          `Parameter '${name}' has no env mapping (${envName}); left verbatim.`,
        );
        return match;
      }
      return v;
    },
  );

  // 3. $(VAR) — but skip backslash-escaped occurrences.
  // Use a unique placeholder for escaped form; restore at the end.
  const ESCAPE_TOKEN = "\u0000ADOAW_ESC_DOLLAR_PAREN\u0000";
  source = source.replace(/\\\$\(/g, ESCAPE_TOKEN);

  source = source.replace(
    /\$\(([A-Za-z_][A-Za-z0-9_.]*)\)/g,
    (match, name: string) => {
      const envName = name.toUpperCase().replace(/\./g, "_");
      const v = env[envName];
      if (v === undefined) {
        warn(
          `Variable '${name}' has no env value (${envName}); left verbatim.`,
        );
        return match;
      }
      return v;
    },
  );

  // Restore backslash-escaped `$(` to the literal form.
  source = source.split(ESCAPE_TOKEN).join("$(");

  return source;
}
