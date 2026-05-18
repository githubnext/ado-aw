/**
 * YAML front-matter stripper for prompt source files.
 *
 * The agent `.md` is structured as:
 *
 * ```
 * ---
 * <yaml front matter>
 * ---
 *
 * <markdown body>
 * ```
 *
 * `stripFrontMatter` returns the markdown body, leaving line breaks
 * inside the body untouched so any author-supplied indentation /
 * fenced-code blocks survive intact.
 *
 * Mirrors `extract_front_matter` in `src/compile/common.rs` (Rust) —
 * keep semantics in lockstep.
 */
export class FrontMatterError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "FrontMatterError";
  }
}

const FRONT_MATTER_OPEN = /^---\r?\n/;

export function stripFrontMatter(source: string): string {
  // No front-matter delimiter at the very top → treat the entire source
  // as body. (Matches Rust's behaviour of accepting plain markdown.)
  if (!FRONT_MATTER_OPEN.test(source)) {
    return source;
  }

  // Strip the opening `---` line.
  const afterOpen = source.replace(FRONT_MATTER_OPEN, "");

  // Find the next `---` line. It must start at column 0 of a line
  // (anchored with `\n` before) — a `---` mid-content is not a close
  // delimiter.
  const closeMatch = afterOpen.match(/(^|\r?\n)---(\r?\n|$)/);
  if (!closeMatch) {
    throw new FrontMatterError(
      "Front matter is open (`---`) but no closing `---` was found.",
    );
  }
  // Consume the full close-delimiter match including its trailing
  // newline (group 2). Anything after that is the body.
  const fullEnd = closeMatch.index! + closeMatch[0].length;
  return afterOpen.slice(fullEnd);
}
