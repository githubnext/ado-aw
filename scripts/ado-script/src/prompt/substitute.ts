/**
 * Single-pass substitution engine for `prompt.js`.
 *
 * Recognised tokens (priority order, matched left-to-right):
 *
 * | Token                          | Resolved via                                       | Notes                                          |
 * |--------------------------------|----------------------------------------------------|------------------------------------------------|
 * | `\$(VAR)` / `\$(VAR.SUB)`      | escape                                             | Backslash stripped; `$(...)` left literal.     |
 * | `${{ parameters.NAME }}`       | `ADO_AW_PARAM_<NAME upper, hyphen→underscore>`     | Only declared parameters substitute.           |
 * | `$(VAR)` / `$(VAR.SUB)`        | `<NAME upper, dot→underscore>` (process env)       | Unset vars left verbatim with a warning.       |
 * | `$[ ... ]`                     | not substituted                                    | Left verbatim with one warning per render.     |
 *
 * **Single-pass is load-bearing**: the function walks the input string
 * exactly once with a global regex. Replacement values are returned
 * verbatim and are **never re-scanned**. This blocks the
 * "queue-with-malicious-parameter-value" chaining attack where a
 * caller-supplied parameter value contains `$(...)` and would otherwise
 * be expanded by a subsequent pass.
 *
 * Each "unknown" diagnostic (unset env var, unknown parameter,
 * `$[...]` expression) is reported once per render via the `warn`
 * callback. The caller is expected to forward those to VSO
 * `##vso[task.logissue]`.
 */

// Alternation in priority order:
//   1. Escape:    \$(...)               → strip the backslash, leave $(...) literal
//   2. Parameter: ${{ parameters.NAME }}
//   3. Variable:  $(NAME) or $(NAME.SUB)
//   4. Runtime:   $[ ... ]              → not substituted; warn once
const TOKEN_RE =
  /\\\$\((?<escVar>[^()]*)\)|\$\{\{\s*parameters\.(?<param>[A-Za-z_][A-Za-z0-9_-]*)\s*\}\}|\$\((?<var>[A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z_][A-Za-z0-9_]*)?)\)|\$\[(?<expr>[^\]]*)\]/g;

export type WarnFn = (msg: string) => void;

/**
 * Run the substitution pipeline against `source`.
 *
 * @param source     Rendered prompt content (body + supplements joined).
 * @param parameters List of parameter names declared in `PromptSpec`.
 *                   Names not in this list are not substituted.
 * @param warn       Callback for one-line warnings about unresolved
 *                   tokens. Called at most once per distinct token.
 */
export function substitute(
  source: string,
  parameters: readonly string[],
  warn: WarnFn,
): string {
  const allowedParams = new Set(parameters);
  const warned = new Set<string>();
  const warnOnce = (key: string, msg: string): void => {
    if (warned.has(key)) return;
    warned.add(key);
    warn(msg);
  };

  return source.replace(TOKEN_RE, (match, ...args: unknown[]): string => {
    // String.prototype.replace with a function callback signature is
    // `(match, p1, p2, ..., offset, string, groups)`. We grab the last
    // argument as the named-groups object.
    const groups = args[args.length - 1] as
      | Record<string, string | undefined>
      | undefined;
    if (!groups) return match;

    // ─── Escape `\$(...)` → `$(...)` literal ────────────────────────
    if (groups["escVar"] !== undefined) {
      return `$(${groups["escVar"]})`;
    }

    // ─── `${{ parameters.NAME }}` ───────────────────────────────────
    if (groups["param"] !== undefined) {
      const name = groups["param"];
      if (!allowedParams.has(name)) {
        warnOnce(
          `param:${name}`,
          `Unknown parameter '${name}'; left as-is. Declare it in the agent front matter to enable substitution.`,
        );
        return match;
      }
      const envName = `ADO_AW_PARAM_${name.toUpperCase().replace(/-/g, "_")}`;
      const value = process.env[envName];
      if (value === undefined) {
        warnOnce(
          `paramEnv:${envName}`,
          `Parameter '${name}' is declared but env var '${envName}' is unset; left as-is.`,
        );
        return match;
      }
      // Value is returned verbatim — the outer .replace() does NOT
      // recurse into our return value, so any `$(...)` in `value`
      // stays literal.
      return value;
    }

    // ─── `$(VAR)` / `$(VAR.SUB)` ──────────────────────────────────
    if (groups["var"] !== undefined) {
      const ref = groups["var"];
      const envName = ref.toUpperCase().replace(/\./g, "_");
      const value = process.env[envName];
      if (value === undefined) {
        warnOnce(
          `var:${envName}`,
          `ADO variable '$(${ref})' is unset (env var '${envName}'); left as-is. Secrets are not auto-exposed — set 'env: { ${envName}: $(${ref}) }' on the step to expose.`,
        );
        return match;
      }
      return value;
    }

    // ─── `$[ ... ]` runtime expression — not supported ────────────
    if (groups["expr"] !== undefined) {
      const expr = groups["expr"];
      warnOnce(
        `expr:$[${expr}]`,
        `Runtime expression '$[${expr}]' is not substituted by prompt.js; left as-is.`,
      );
      return match;
    }

    return match;
  });
}

// Re-export the regex so tests can introspect / pin its behaviour.
export const _TOKEN_RE_FOR_TESTS = TOKEN_RE;
