/**
 * Strip the leading YAML front-matter block from a markdown source.
 *
 * Front matter is defined as the region between an opening `---` line at
 * the very start of the file (optionally preceded by a UTF-8 BOM) and the
 * next standalone `---` line. Both LF and CRLF line endings are accepted.
 *
 * Returns the body unchanged when no front matter is present. Throws if a
 * front matter opens but never closes.
 */
export function stripFrontMatter(source: string): string {
  // Strip leading BOM if present so our line-prefix checks line up.
  const text = source.replace(/^\uFEFF/, "");

  // Front matter must start at byte 0 with `---` followed by EOL.
  const opener = /^---(?:\r\n|\n)/;
  const m = opener.exec(text);
  if (!m) {
    return source;
  }

  const afterOpen = m[0].length;

  // Find the closing fence: `---` on its own line.
  const closer = /(?:\r\n|\n)---(?=\r\n|\n|$)/;
  const c = closer.exec(text.slice(afterOpen));
  if (!c) {
    throw new Error("Unterminated YAML front matter (missing closing '---')");
  }
  const closeIdx = afterOpen + c.index + c[0].length;

  // Skip the EOL immediately after the closing fence, if any.
  let body = text.slice(closeIdx);
  body = body.replace(/^(?:\r\n|\n)/, "");

  return body;
}
