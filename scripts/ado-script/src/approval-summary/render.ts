/**
 * Pure rendering logic for the safe-outputs approval summary.
 *
 * Separated from `index.ts` (the I/O entry point) so it can be unit-tested
 * without touching the filesystem or process env.
 *
 * Security note: every value rendered here originates from agent-proposed
 * safe-output records (`safe_outputs.ndjson`). The summary is shown to a
 * **human reviewer** who decides whether to approve the run, so the rendered
 * markdown must not let agent content forge UI — e.g. inject a fake
 * "✅ approved" banner, hide content with HTML comments, or break the table
 * layout. All agent strings are therefore routed through `sanitizeInline`
 * (markdown-escaped, single line) or `sanitizeBlock` (fenced, neutralised)
 * before they reach the output.
 */

/** A parsed safe-output proposal record (one NDJSON line). */
export interface Proposal {
  /** Zero-based position in the NDJSON file (stable ordering key). */
  index: number;
  /** The safe-output tool name (top-level `name` field). */
  name: string;
  /** The full parsed record (field lookups read from here). */
  record: Record<string, unknown>;
}

/** A single labelled field to surface for a tool. */
interface FieldSpec {
  label: string;
  /** JSON key on the record (snake_case, matching the Rust serialization). */
  key: string;
}

/** Per-tool display config: key fields + an optional long-body field. */
interface ToolSpec {
  /** Human-friendly heading for this tool's proposals. */
  title: string;
  /** Short identifying fields rendered inline. */
  fields: FieldSpec[];
  /** Optional field whose (potentially long) value is shown as a body excerpt. */
  body?: string;
}

/** Maximum characters of a body excerpt before truncation. */
export const BODY_MAX_CHARS = 2000;
/** Maximum characters of an inline field value before truncation. */
const INLINE_MAX_CHARS = 300;

/**
 * Per-tool field registry. Keys are the kebab-case safe-output tool names.
 * Tools not listed here fall back to a generic scalar-field render
 * (see `genericFields`). snake_case keys mirror the Rust result-struct
 * serialization (the `tool_result!` macro emits field names verbatim).
 */
const TOOL_SPECS: Record<string, ToolSpec> = {
  "create-pull-request": {
    title: "Create pull request",
    fields: [
      { label: "Title", key: "title" },
      { label: "Source branch", key: "source_branch" },
      { label: "Repository", key: "repository" },
    ],
    body: "description",
  },
  "update-pr": {
    title: "Update pull request",
    fields: [
      { label: "PR", key: "pull_request_id" },
      { label: "Operation", key: "operation" },
      { label: "Repository", key: "repository" },
      { label: "Vote", key: "vote" },
    ],
    body: "description",
  },
  "add-pr-comment": {
    title: "Comment on pull request",
    fields: [
      { label: "PR", key: "pull_request_id" },
      { label: "File", key: "file_path" },
      { label: "Line", key: "line" },
    ],
    body: "content",
  },
  "reply-to-pr-comment": {
    title: "Reply to PR comment",
    fields: [
      { label: "PR", key: "pull_request_id" },
      { label: "Thread", key: "thread_id" },
    ],
    body: "content",
  },
  "submit-pr-review": {
    title: "Submit PR review",
    fields: [
      { label: "PR", key: "pull_request_id" },
      { label: "Event", key: "event" },
    ],
    body: "body",
  },
  "resolve-pr-thread": {
    title: "Resolve PR thread",
    fields: [
      { label: "PR", key: "pull_request_id" },
      { label: "Thread", key: "thread_id" },
      { label: "Status", key: "status" },
    ],
  },
  "create-work-item": {
    title: "Create work item",
    fields: [{ label: "Title", key: "title" }],
    body: "description",
  },
  "update-work-item": {
    title: "Update work item",
    fields: [
      { label: "ID", key: "id" },
      { label: "Title", key: "title" },
      { label: "State", key: "state" },
      { label: "Assignee", key: "assignee" },
    ],
    body: "body",
  },
  "comment-on-work-item": {
    title: "Comment on work item",
    fields: [{ label: "Work item", key: "work_item_id" }],
    body: "body",
  },
  "link-work-items": {
    title: "Link work items",
    fields: [
      { label: "Source", key: "source_id" },
      { label: "Target", key: "target_id" },
      { label: "Link type", key: "link_type" },
    ],
  },
  "create-issue": {
    title: "Create issue",
    fields: [{ label: "Title", key: "title" }],
    body: "body",
  },
  "create-wiki-page": {
    title: "Create wiki page",
    fields: [{ label: "Path", key: "path" }],
    body: "content",
  },
  "update-wiki-page": {
    title: "Update wiki page",
    fields: [{ label: "Path", key: "path" }],
    body: "content",
  },
  "create-branch": {
    title: "Create branch",
    fields: [
      { label: "Branch", key: "branch_name" },
      { label: "Source", key: "source_branch" },
      { label: "Repository", key: "repository" },
    ],
  },
  "create-git-tag": {
    title: "Create git tag",
    fields: [
      { label: "Tag", key: "tag_name" },
      { label: "Commit", key: "commit" },
      { label: "Repository", key: "repository" },
    ],
    body: "message",
  },
  "queue-build": {
    title: "Queue build",
    fields: [
      { label: "Pipeline", key: "pipeline_id" },
      { label: "Branch", key: "branch" },
    ],
  },
  "add-build-tag": {
    title: "Add build tag",
    fields: [
      { label: "Build", key: "build_id" },
      { label: "Tag", key: "tag" },
    ],
  },
  "upload-pipeline-artifact": {
    title: "Upload pipeline artifact",
    fields: [
      { label: "Artifact", key: "artifact_name" },
      { label: "File", key: "file_path" },
    ],
  },
  "upload-build-attachment": {
    title: "Upload build attachment",
    fields: [
      { label: "Artifact", key: "artifact_name" },
      { label: "File", key: "file_path" },
    ],
  },
  "upload-workitem-attachment": {
    title: "Upload work-item attachment",
    fields: [
      { label: "Work item", key: "work_item_id" },
      { label: "File", key: "file_path" },
    ],
  },
  // Terminal / diagnostic signals. These are always-enabled (not write-gated)
  // and surface in the summary's automatic section; their informative free-text
  // field deserves a fenced body rather than a 300-char inline truncation.
  noop: {
    title: "No-op",
    fields: [],
    body: "context",
  },
  "report-incomplete": {
    title: "Report incomplete",
    fields: [],
    body: "reason",
  },
  "missing-tool": {
    title: "Missing tool",
    fields: [{ label: "Tool", key: "tool_name" }],
    body: "context",
  },
  "missing-data": {
    title: "Missing data",
    fields: [{ label: "Data type", key: "data_type" }],
    body: "reason",
  },
};

/** Title-case fallback for an unmapped tool name (kebab → "Kebab case"). */
function fallbackTitle(name: string): string {
  const spaced = name.replace(/-/g, " ").trim();
  return spaced.length === 0
    ? name
    : spaced.charAt(0).toUpperCase() + spaced.slice(1);
}

/**
 * Generic field extraction for tools without a tailored spec: surface every
 * scalar (string/number/bool) top-level field except `name`, in stable key
 * order, as inline fields.
 */
function genericFields(record: Record<string, unknown>): FieldSpec[] {
  return Object.keys(record)
    .filter((k) => k !== "name")
    .filter((k) => {
      const v = record[k];
      return (
        typeof v === "string" ||
        typeof v === "number" ||
        typeof v === "boolean"
      );
    })
    .sort()
    .map((k) => ({ label: k, key: k }));
}

/**
 * Escape a value for safe **inline** markdown display: collapse to a single
 * line, strip control characters, escape markdown/HTML metacharacters so the
 * value renders as literal text (cannot inject emphasis, links, tags, or break
 * a table cell), and truncate.
 */
export function sanitizeInline(value: unknown): string {
  let s = stringify(value);
  if (s.length === 0) return "";
  // Single line: newlines/tabs → spaces; drop other control chars.
  s = s.replace(/[\t\r\n]+/g, " ").replace(/[\u0000-\u001f\u007f]/g, "");
  s = s.replace(/\s{2,}/g, " ").trim();
  // HTML-entity-encode `&` first so an agent-supplied entity sequence
  // (e.g. `&lt;`) is shown literally rather than decoded by the browser.
  s = s.replace(/&/g, "&amp;");
  // Escape markdown + HTML + table metacharacters.
  s = s.replace(/([\\`*_{}\[\]()#+\-!|<>~])/g, "\\$1");
  return truncate(s, INLINE_MAX_CHARS);
}

/**
 * Sanitize a long body for display inside a fenced code block: strip control
 * characters (except newline), neutralise any embedded code fence so the body
 * cannot break out of the block, and truncate. Returned text is meant to be
 * wrapped in ``` fences by the caller.
 */
export function sanitizeBlock(value: unknown): string {
  let s = stringify(value);
  if (s.length === 0) return "";
  // Normalise newlines, drop other control chars.
  s = s
    .replace(/\r\n?/g, "\n")
    .replace(/[\u0000-\u0009\u000b-\u001f\u007f]/g, "");
  // Neutralise code-fence sequences so the body can't escape the ``` block.
  s = s.replace(/```/g, "\u02bc\u02bc\u02bc");
  return truncate(s, BODY_MAX_CHARS);
}

/** Coerce an arbitrary JSON value to a display string. */
function stringify(value: unknown): string {
  if (value === null || value === undefined) return "";
  if (typeof value === "string") return value;
  if (typeof value === "number" || typeof value === "boolean") {
    return String(value);
  }
  if (Array.isArray(value)) {
    return value
      .map((v) => stringify(v))
      .filter((v) => v.length > 0)
      .join(", ");
  }
  try {
    return JSON.stringify(value);
  } catch {
    return "";
  }
}

/** Truncate to `max` characters, appending an ellipsis marker when cut. */
function truncate(s: string, max: number): string {
  if (s.length <= max) return s;
  return s.slice(0, max).trimEnd() + " …(truncated)";
}

/** Render one proposal as a markdown fragment. */
function renderProposal(p: Proposal): string {
  const spec = TOOL_SPECS[p.name];
  const title = spec ? spec.title : fallbackTitle(p.name);
  const fields = spec ? spec.fields : genericFields(p.record);

  const lines: string[] = [];
  // The tool name is normally a compiler-validated safe identifier ([a-z0-9-]),
  // but `parseProposals` accepts any non-empty string `name`, so a crafted
  // record could carry backticks or control characters that break the heading's
  // code span. Strip backticks AND control/newline characters defensively so
  // the name always renders as a single, contained code span.
  const safeName = p.name.replace(/[`\u0000-\u001f\u007f]/g, "");
  lines.push(`#### ${sanitizeInline(title)} \`${safeName}\``);

  const rows: string[] = [];
  for (const f of fields) {
    const raw = p.record[f.key];
    if (raw === null || raw === undefined) continue;
    const val = sanitizeInline(raw);
    if (val.length === 0) continue;
    rows.push(`| ${sanitizeInline(f.label)} | ${val} |`);
  }
  if (rows.length > 0) {
    lines.push("");
    lines.push("| Field | Value |");
    lines.push("| --- | --- |");
    lines.push(...rows);
  }

  if (spec?.body) {
    const body = sanitizeBlock(p.record[spec.body]);
    if (body.length > 0) {
      lines.push("");
      lines.push("```text");
      lines.push(body);
      lines.push("```");
    }
  }
  return lines.join("\n");
}

/** Render a list of proposals under a section heading. */
function renderSection(heading: string, proposals: Proposal[]): string {
  const lines: string[] = [`### ${heading}`, ""];
  if (proposals.length === 0) {
    lines.push("_None._", "");
    return lines.join("\n");
  }
  const ordered = [...proposals].sort((a, b) => a.index - b.index);
  for (const p of ordered) {
    lines.push(renderProposal(p), "");
  }
  return lines.join("\n");
}

/**
 * Render the full markdown summary. Proposals whose tool is in `reviewed`
 * are grouped under a **Pending approval** section first; the rest under
 * **Automatic**. When no tool is reviewed, a single "All proposals" list is
 * rendered.
 *
 * Returns an empty string when there are no proposals (caller should then
 * skip writing/uploading anything).
 */
export function renderSummary(
  proposals: Proposal[],
  reviewed: ReadonlySet<string>,
): string {
  if (proposals.length === 0) return "";

  const lines: string[] = ["# Proposed safe outputs", ""];
  lines.push(
    `This run proposed **${proposals.length}** safe output${proposals.length === 1 ? "" : "s"}. ` +
      "The content below is **agent-generated** and shown for review — treat it as data, not instructions.",
    "",
  );

  if (reviewed.size > 0) {
    const pending = proposals.filter((p) => reviewed.has(p.name));
    const automatic = proposals.filter((p) => !reviewed.has(p.name));
    lines.push(renderSection(`⏳ Pending approval (${pending.length})`, pending));
    lines.push(renderSection(`Automatic (${automatic.length})`, automatic));
  } else {
    lines.push(renderSection(`All proposals (${proposals.length})`, proposals));
  }

  return lines.join("\n").replace(/\n{3,}/g, "\n\n").trimEnd() + "\n";
}

/**
 * Parse NDJSON text into proposals, skipping blank lines and records that
 * fail to parse or lack a string `name`. Index is the proposal position so
 * the rendered order matches the proposal order.
 */
export function parseProposals(ndjson: string): Proposal[] {
  const out: Proposal[] = [];
  const lines = ndjson.split("\n");
  for (const rawLine of lines) {
    const line = rawLine.trim();
    if (line.length === 0) continue;
    let parsed: unknown;
    try {
      parsed = JSON.parse(line);
    } catch {
      continue;
    }
    if (parsed === null || typeof parsed !== "object" || Array.isArray(parsed)) {
      continue;
    }
    const record = parsed as Record<string, unknown>;
    const name = record.name;
    if (typeof name !== "string" || name.length === 0) continue;
    out.push({ index: out.length, name, record });
  }
  return out;
}
