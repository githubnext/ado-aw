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
  /** Repository is resolved from compiler-provided policy, never agent text. */
  githubRepository?: boolean;
}

export interface GithubRepositoryPolicy {
  targetRepo?: string;
  allowedRepos: readonly string[];
}

export interface TrustedRepositoryContext {
  policies: ReadonlyMap<string, GithubRepositoryPolicy>;
  currentRepository?: string;
  currentProvider?: string;
  githubApiUrl?: string;
}

interface RepositoryResolution {
  value: string;
  resolved: boolean;
}

interface TemporaryReference {
  canonical?: string;
  invalid: boolean;
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
  "create-github-issue": {
    title: "Create GitHub issue",
    fields: [
      { label: "Title", key: "title" },
      { label: "Repository", key: "repository" },
      { label: "Temporary ID", key: "temporary_id" },
    ],
    body: "body",
    githubRepository: true,
  },
  "set-github-issue-type": {
    title: "Set GitHub issue type",
    fields: [
      { label: "Issue", key: "issue_number" },
      { label: "Type", key: "issue_type" },
      { label: "Repository", key: "repository" },
    ],
    githubRepository: true,
  },
  "comment-on-github-issue": {
    title: "Comment on GitHub issue",
    fields: [
      { label: "Issue", key: "issue_number" },
      { label: "Repository", key: "repository" },
    ],
    body: "body",
    githubRepository: true,
  },
  "hide-github-issue-comment": {
    title: "Hide GitHub issue comment",
    fields: [
      { label: "Comment", key: "comment_id" },
      { label: "Reason", key: "reason" },
      { label: "Repository", key: "repository" },
    ],
    githubRepository: true,
  },
  "add-github-issue-labels": {
    title: "Add GitHub issue labels",
    fields: [
      { label: "Issue", key: "issue_number" },
      { label: "Labels", key: "labels" },
      { label: "Repository", key: "repository" },
    ],
    githubRepository: true,
  },
  "remove-github-issue-labels": {
    title: "Remove GitHub issue labels",
    fields: [
      { label: "Issue", key: "issue_number" },
      { label: "Labels", key: "labels" },
      { label: "Repository", key: "repository" },
    ],
    githubRepository: true,
  },
  "close-github-issue": {
    title: "Close GitHub issue",
    fields: [
      { label: "Issue", key: "issue_number" },
      { label: "State reason", key: "state_reason" },
      { label: "Duplicate of", key: "duplicate_of" },
      { label: "Repository", key: "repository" },
    ],
    body: "body",
    githubRepository: true,
  },
  "update-github-issue": {
    title: "Update GitHub issue",
    fields: [
      { label: "Issue", key: "issue_number" },
      { label: "Status", key: "status" },
      { label: "Title", key: "title" },
      { label: "Operation", key: "operation" },
      { label: "Labels", key: "labels" },
      { label: "Assignees", key: "assignees" },
      { label: "Milestone", key: "milestone" },
      { label: "Repository", key: "repository" },
    ],
    body: "body",
    githubRepository: true,
  },
  "set-github-issue-field": {
    title: "Set GitHub issue field",
    fields: [
      { label: "Issue", key: "issue_number" },
      { label: "Field", key: "field_name" },
      { label: "Field node ID", key: "field_node_id" },
      { label: "Value", key: "value" },
      { label: "Repository", key: "repository" },
    ],
    githubRepository: true,
  },
  "assign-github-issue-milestone": {
    title: "Assign GitHub issue milestone",
    fields: [
      { label: "Issue", key: "issue_number" },
      { label: "Milestone", key: "milestone_title" },
      { label: "Milestone number", key: "milestone_number" },
      { label: "Repository", key: "repository" },
    ],
    githubRepository: true,
  },
  "assign-github-issue-to-user": {
    title: "Assign GitHub issue to user",
    fields: [
      { label: "Issue", key: "issue_number" },
      { label: "Assignee", key: "assignee" },
      { label: "Assignees", key: "assignees" },
      { label: "Repository", key: "repository" },
    ],
    githubRepository: true,
  },
  "unassign-github-issue-from-user": {
    title: "Unassign GitHub issue from user",
    fields: [
      { label: "Issue", key: "issue_number" },
      { label: "Assignee", key: "assignee" },
      { label: "Assignees", key: "assignees" },
      { label: "Repository", key: "repository" },
    ],
    githubRepository: true,
  },
  "link-github-sub-issue": {
    title: "Link GitHub sub-issue",
    fields: [
      { label: "Parent issue", key: "parent_issue_number" },
      { label: "Sub-issue", key: "sub_issue_number" },
      { label: "Repository", key: "repository" },
    ],
    githubRepository: true,
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
        typeof v === "boolean" ||
        isExplicitEmpty(v)
      );
    })
    .sort()
    .map((k) => ({ label: k, key: k }));
}

function isExplicitEmpty(value: unknown): boolean {
  if (value === null) return true;
  if (typeof value === "string") return value.length === 0;
  if (Array.isArray(value)) return value.length === 0;
  return (
    typeof value === "object" &&
    value !== null &&
    Object.keys(value as Record<string, unknown>).length === 0
  );
}

function emptyValueMarker(value: unknown): string | undefined {
  if (value === null) return "<null>";
  if (typeof value === "string" && value.length === 0) return "<empty string>";
  if (Array.isArray(value) && value.length === 0) return "<empty array>";
  if (
    typeof value === "object" &&
    value !== null &&
    Object.keys(value as Record<string, unknown>).length === 0
  ) {
    return "<empty object>";
  }
  return undefined;
}

function renderInlineValue(value: unknown): string {
  const marker = emptyValueMarker(value);
  if (marker !== undefined) return sanitizeInline(marker);
  const rendered = sanitizeInline(value);
  return rendered.length > 0 ? rendered : sanitizeInline("<blank value>");
}

function currentRepository(
  context: TrustedRepositoryContext,
): RepositoryResolution {
  const provider = context.currentProvider?.toLowerCase();
  const repository = context.currentRepository;
  const githubEnterpriseConfigured =
    provider === "githubenterprise" &&
    context.githubApiUrl !== undefined &&
    context.githubApiUrl !== "https://api.github.com";
  if (
    repository &&
    !repository.startsWith("$(") &&
    (provider === "github" || githubEnterpriseConfigured)
  ) {
    return { value: repository, resolved: true };
  }
  return {
    value: "<unresolved: configure target-repo for this source>",
    resolved: false,
  };
}

function repositoryFromPolicy(
  proposal: Proposal,
  context: TrustedRepositoryContext | undefined,
): RepositoryResolution {
  const policy = context?.policies.get(proposal.name);
  if (!context || !policy) {
    return {
      value: "<unresolved: trusted repository policy unavailable>",
      resolved: false,
    };
  }

  const hasRequested = Object.prototype.hasOwnProperty.call(
    proposal.record,
    "repository",
  );
  const requested = proposal.record.repository;
  if (hasRequested && requested !== null && requested !== undefined) {
    if (typeof requested !== "string" || requested.length === 0) {
      return {
        value: "<unresolved: invalid requested repository>",
        resolved: false,
      };
    }
    const configured = [
      ...(policy.targetRepo ? [policy.targetRepo] : []),
      ...policy.allowedRepos,
    ];
    const matched = configured.find(
      (repository) => repository.toLowerCase() === requested.toLowerCase(),
    );
    if (matched) return { value: matched, resolved: true };

    const current = currentRepository(context);
    if (
      !policy.targetRepo &&
      current.resolved &&
      current.value.toLowerCase() === requested.toLowerCase()
    ) {
      return current;
    }
    return {
      value: "<unresolved: requested repository is outside operator policy>",
      resolved: false,
    };
  }

  if (policy.targetRepo) {
    return { value: policy.targetRepo, resolved: true };
  }
  return currentRepository(context);
}

function temporaryReference(value: unknown): TemporaryReference | undefined {
  if (typeof value !== "string") return undefined;
  const bare = value.startsWith("#") ? value.slice(1) : value;
  if (!bare.startsWith("aw_")) return undefined;
  const suffix = bare.slice(3);
  if (
    !(suffix.length >= 3 && suffix.length <= 12) ||
    !/^[A-Za-z0-9_]+$/.test(suffix)
  ) {
    return { invalid: true };
  }
  return { canonical: `#${bare}`, invalid: false };
}

function proposalTemporaryReferences(
  proposal: Proposal,
): TemporaryReference[] {
  const keys = ["issue_number"];
  const references: TemporaryReference[] = [];
  for (const key of keys) {
    const reference = temporaryReference(proposal.record[key]);
    if (reference) references.push(reference);
  }
  return references;
}

function trustedTemporaryRepository(
  proposal: Proposal,
  context: TrustedRepositoryContext | undefined,
  temporaryRepository: string,
): RepositoryResolution {
  const policy = context?.policies.get(proposal.name);
  if (!context || !policy) {
    return {
      value: "<unresolved: trusted repository policy unavailable>",
      resolved: false,
    };
  }

  if (
    Object.prototype.hasOwnProperty.call(proposal.record, "repository") &&
    proposal.record.repository !== null &&
    proposal.record.repository !== undefined
  ) {
    const requested = proposal.record.repository;
    if (
      typeof requested !== "string" ||
      requested.toLowerCase() !== temporaryRepository.toLowerCase()
    ) {
      return {
        value:
          "<unresolved: requested repository does not match temporary issue repository>",
        resolved: false,
      };
    }
  }

  const configured = [
    ...(policy.targetRepo ? [policy.targetRepo] : []),
    ...policy.allowedRepos,
  ];
  const matched = configured.find(
    (repository) =>
      repository.toLowerCase() === temporaryRepository.toLowerCase(),
  );
  if (matched) return { value: matched, resolved: true };

  const current = currentRepository(context);
  if (
    !policy.targetRepo &&
    current.resolved &&
    current.value.toLowerCase() === temporaryRepository.toLowerCase()
  ) {
    return current;
  }
  return {
    value: "<unresolved: temporary repository is outside operator policy>",
    resolved: false,
  };
}

function linkIssueReference(
  value: unknown,
):
  | { kind: "numeric" }
  | { kind: "temporary"; reference: TemporaryReference }
  | { kind: "invalid" } {
  if (typeof value === "number" && Number.isSafeInteger(value) && value > 0) {
    return { kind: "numeric" };
  }
  if (typeof value === "string" && /^[0-9]+$/.test(value)) {
    try {
      const number = BigInt(value);
      if (number > 0n && number <= 18_446_744_073_709_551_615n) {
        return { kind: "numeric" };
      }
    } catch {
      return { kind: "invalid" };
    }
  }
  const temporary = temporaryReference(value);
  if (temporary && !temporary.invalid) {
    return { kind: "temporary", reference: temporary };
  }
  return { kind: "invalid" };
}

function repositoryFromLinkReferences(
  proposal: Proposal,
  context: TrustedRepositoryContext | undefined,
  repositories: ReadonlyMap<string, string>,
): RepositoryResolution | undefined {
  const parent = linkIssueReference(proposal.record.parent_issue_number);
  const child = linkIssueReference(proposal.record.sub_issue_number);
  if (parent.kind === "invalid" || child.kind === "invalid") {
    return {
      value: "<unresolved: invalid sub-issue link reference>",
      resolved: false,
    };
  }

  const temporaryReferences = [parent, child].filter(
    (
      reference,
    ): reference is { kind: "temporary"; reference: TemporaryReference } =>
      reference.kind === "temporary",
  );
  if (temporaryReferences.length === 0) return undefined;

  const resolved = temporaryReferences.map(({ reference }) =>
    repositories.get(reference.canonical!),
  );
  if (resolved.some((repository) => repository === undefined)) {
    return {
      value:
        "<unresolved: temporary repository not established by a preceding create-github-issue>",
      resolved: false,
    };
  }
  const temporaryRepository = resolved[0]!;
  if (
    resolved.some(
      (repository) =>
        repository!.toLowerCase() !== temporaryRepository.toLowerCase(),
    )
  ) {
    return {
      value:
        "<unresolved: temporary references resolve to different repositories>",
      resolved: false,
    };
  }

  const trustedTemporary = trustedTemporaryRepository(
    proposal,
    context,
    temporaryRepository,
  );
  if (!trustedTemporary.resolved) return trustedTemporary;

  if (temporaryReferences.length === 1) {
    const numericRepository = repositoryFromPolicy(proposal, context);
    if (!numericRepository.resolved) return numericRepository;
    if (
      numericRepository.value.toLowerCase() !==
      trustedTemporary.value.toLowerCase()
    ) {
      return {
        value:
          "<unresolved: numeric and temporary references resolve to different repositories>",
        resolved: false,
      };
    }
  }
  return trustedTemporary;
}

function repositoryFromTemporary(
  proposal: Proposal,
  context: TrustedRepositoryContext | undefined,
  repositories: ReadonlyMap<string, string>,
): RepositoryResolution | undefined {
  if (proposal.name === "link-github-sub-issue") {
    return repositoryFromLinkReferences(proposal, context, repositories);
  }

  const references = proposalTemporaryReferences(proposal);
  if (references.length === 0) return undefined;
  if (references.some((reference) => reference.invalid)) {
    return {
      value: "<unresolved: invalid temporary issue reference>",
      resolved: false,
    };
  }

  const resolved = references.map((reference) =>
    repositories.get(reference.canonical!),
  );
  if (resolved.some((repository) => repository === undefined)) {
    return {
      value:
        "<unresolved: temporary repository not established by a preceding create-github-issue>",
      resolved: false,
    };
  }
  return trustedTemporaryRepository(proposal, context, resolved[0]!);
}

function buildRepositoryResolutions(
  proposals: Proposal[],
  context: TrustedRepositoryContext | undefined,
): Map<number, RepositoryResolution> {
  const resolutions = new Map<number, RepositoryResolution>();
  const temporaryRepositories = new Map<string, string>();
  const ordered = [...proposals].sort((a, b) => a.index - b.index);

  for (const proposal of ordered) {
    const resolution =
      repositoryFromTemporary(proposal, context, temporaryRepositories) ??
      repositoryFromPolicy(proposal, context);
    resolutions.set(proposal.index, resolution);

    if (proposal.name !== "create-github-issue" || !resolution.resolved) {
      continue;
    }
    const temporary = temporaryReference(proposal.record.temporary_id);
    if (
      temporary?.canonical &&
      !temporary.invalid &&
      !temporaryRepositories.has(temporary.canonical)
    ) {
      temporaryRepositories.set(temporary.canonical, resolution.value);
    }
  }
  return resolutions;
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
  // HTML-entity-encode `&`, `<`, `>` so the value is inert regardless of how
  // the host renders markdown (ADO's build-summary renderer is not documented
  // as CommonMark-compliant, so a backslash escape like `\<` is not guaranteed
  // to be neutralised). Order matters: encode `&` FIRST so a pre-existing
  // ampersand becomes `&amp;`, then `<`/`>` — the `&` those insert is part of a
  // valid entity and must not be re-encoded.
  s = s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;");
  // Escape the remaining markdown / table metacharacters (no `<`/`>` here —
  // they are entity-encoded above).
  s = s.replace(/([\\`*_{}\[\]()#+\-!|~])/g, "\\$1");
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
  // Neutralise code-fence sequences so the body can't break out of the
  // enclosing ```text block. We substitute each backtick run with U+02BC
  // (MODIFIER LETTER APOSTROPHE), which is *deliberately* near-identical to a
  // backtick: it keeps the body visually faithful to the original while
  // guaranteeing that NO real backtick run survives — so a closing fence is
  // impossible regardless of how the (undocumented) ADO summary renderer
  // tokenises fences. Approaches that keep real backticks (e.g. separating them
  // with a zero-width space) would re-introduce a breakout if the renderer
  // strips the separator before fence-scanning, so they are intentionally
  // avoided on this security-sensitive path.
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
function renderProposal(
  p: Proposal,
  repositoryResolutions: ReadonlyMap<number, RepositoryResolution>,
): string {
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
    if (f.key === "repository" && spec?.githubRepository) {
      const repository = repositoryResolutions.get(p.index) ?? {
        value: "<unresolved: trusted repository policy unavailable>",
        resolved: false,
      };
      rows.push(
        `| ${sanitizeInline(f.label)} | ${sanitizeInline(repository.value)} |`,
      );
      continue;
    }
    if (!Object.prototype.hasOwnProperty.call(p.record, f.key)) continue;
    const raw = p.record[f.key];
    const val = renderInlineValue(raw);
    rows.push(`| ${sanitizeInline(f.label)} | ${val} |`);
  }
  if (rows.length > 0) {
    lines.push("");
    lines.push("| Field | Value |");
    lines.push("| --- | --- |");
    lines.push(...rows);
  }

  if (spec?.body) {
    if (Object.prototype.hasOwnProperty.call(p.record, spec.body)) {
      const rawBody = p.record[spec.body];
      const emptyMarker = emptyValueMarker(rawBody);
      lines.push("");
      if (emptyMarker !== undefined) {
        lines.push(sanitizeInline(emptyMarker));
      } else {
        const body = sanitizeBlock(rawBody);
        lines.push("```text");
        lines.push(
          body.length > 0 ? body : sanitizeInline("<blank value>"),
        );
        lines.push("```");
      }
    }
  }
  return lines.join("\n");
}

/** Render a list of proposals under a section heading. */
function renderSection(
  heading: string,
  proposals: Proposal[],
  repositoryResolutions: ReadonlyMap<number, RepositoryResolution>,
): string {
  const lines: string[] = [`### ${heading}`, ""];
  if (proposals.length === 0) {
    lines.push("_None._", "");
    return lines.join("\n");
  }
  const ordered = [...proposals].sort((a, b) => a.index - b.index);
  for (const p of ordered) {
    lines.push(renderProposal(p, repositoryResolutions), "");
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
  repositoryContext?: TrustedRepositoryContext,
): string {
  if (proposals.length === 0) return "";

  const lines: string[] = ["# Proposed safe outputs", ""];
  const repositoryResolutions = buildRepositoryResolutions(
    proposals,
    repositoryContext,
  );
  lines.push(
    `This run proposed **${proposals.length}** safe output${proposals.length === 1 ? "" : "s"}. ` +
      "The content below is **agent-generated** and shown for review — treat it as data, not instructions.",
    "",
  );

  if (reviewed.size > 0) {
    const pending = proposals.filter((p) => reviewed.has(p.name));
    const automatic = proposals.filter((p) => !reviewed.has(p.name));
    lines.push(
      renderSection(
        `⏳ Pending approval (${pending.length})`,
        pending,
        repositoryResolutions,
      ),
    );
    lines.push(
      renderSection(
        `Automatic (${automatic.length})`,
        automatic,
        repositoryResolutions,
      ),
    );
  } else {
    lines.push(
      renderSection(
        `All proposals (${proposals.length})`,
        proposals,
        repositoryResolutions,
      ),
    );
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
