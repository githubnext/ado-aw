// AUTO-GENERATED from Rust IR via cargo run -- export-prompt-schema. Do not edit; run npm run codegen.

/**
 * Top-level spec consumed by `prompt.js` at pipeline runtime.
 */
export interface PromptSpec {
  /**
   * Absolute path where the rendered prompt should be written.
   */
  output_path: string;
  /**
   * Declared parameter names available for `${{ parameters.NAME }}`
   * substitution. Names not in this list are left verbatim by
   * `prompt.js` with a runtime warning.
   */
  parameters: string[];
  /**
   * Absolute path to the source `.md` file in the workspace.
   */
  source_path: string;
  /**
   * Extension prompt supplements, in render order
   * (Runtimes phase first, then Tools, stable within each phase).
   */
  supplements: PromptSupplement[];
  /**
   * Schema version; refused on mismatch.
   */
  version: number;
  [k: string]: unknown;
}
/**
 * One block of additional prompt content contributed by an extension.
 */
export interface PromptSupplement {
  /**
   * Markdown to append. May contain `${{ parameters.* }}` or `$(VAR)`
   * references; substituted by `prompt.js` using the same rules as
   * the body.
   */
  content: string;
  /**
   * Extension display name (used for VSO logging only — not rendered).
   */
  name: string;
  [k: string]: unknown;
}
