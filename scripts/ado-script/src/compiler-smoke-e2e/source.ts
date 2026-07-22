/**
 * Markdown front-matter transform: pins a fixture's `supply-chain:` block to
 * the compiler candidate produced by the current orchestrator run.
 *
 * Parses only the *first* `---` YAML front-matter block (YAML 1.2, via the
 * `yaml` package) and preserves the markdown body byte-for-byte — the
 * transform never touches anything after the closing `---` delimiter line.
 *
 * Test-harness module; not shipped in `ado-script.zip`.
 */
import { Document, parseDocument, isMap } from "yaml";

/** The literal fields injected under `supply-chain.pipeline-artifact:`. */
export interface PipelineArtifactValues {
  readonly project: string;
  readonly definitionId: number;
  readonly runId: number;
  readonly artifact: string;
}

const FRONT_MATTER_RE = /^---\r?\n([\s\S]*?)\r?\n---\r?\n/;

interface SplitMarkdown {
  yamlText: string;
  /** Everything after the closing `---` delimiter line, preserved verbatim. */
  body: string;
}

/** Split a markdown source into its first YAML front-matter block and body. */
export function splitFrontMatter(markdown: string): SplitMarkdown {
  const match = markdown.match(FRONT_MATTER_RE);
  if (!match) {
    throw new Error("expected a leading '---' YAML front-matter block");
  }
  const yamlText = match[1] ?? "";
  const body = markdown.slice(match[0].length);
  return { yamlText, body };
}

function parseFrontMatter(yamlText: string): Document {
  const doc = parseDocument(yamlText, { merge: false, version: "1.2" });
  if (doc.errors.length > 0) {
    throw new Error(
      `failed to parse YAML front matter: ${doc.errors.map((e) => e.message).join("; ")}`,
    );
  }
  return doc;
}

/**
 * Inject `supply-chain.pipeline-artifact` (literal project/definition-id/run-
 * id/artifact) into a fixture's markdown source.
 *
 * Fails closed:
 *   - throws if `supply-chain.feed` or `supply-chain.pipeline-artifact` is
 *     already present (this transform must never silently override an
 *     existing binary source),
 *   - preserves `supply-chain.registry` untouched when present,
 *   - preserves the markdown body byte-for-byte,
 *   - removes only `on.schedule` (and `on` entirely once it has no
 *     remaining keys) so a staged candidate never self-schedules.
 */
export function injectPipelineArtifact(
  markdown: string,
  values: PipelineArtifactValues,
): string {
  const { yamlText, body } = splitFrontMatter(markdown);
  const doc = parseFrontMatter(yamlText);

  if (doc.hasIn(["supply-chain", "feed"])) {
    throw new Error(
      "fixture already defines supply-chain.feed; refusing to override with a pinned pipeline-artifact source",
    );
  }
  if (doc.hasIn(["supply-chain", "pipeline-artifact"])) {
    throw new Error(
      "fixture already defines supply-chain.pipeline-artifact; refusing to override",
    );
  }

  // setIn creates any missing intermediate maps (e.g. a wholly absent
  // `supply-chain:` key), and only touches this one nested key — any sibling
  // `supply-chain.registry` is left exactly as authored.
  doc.setIn(
    ["supply-chain", "pipeline-artifact"],
    doc.createNode({
      project: values.project,
      "definition-id": values.definitionId,
      "run-id": values.runId,
      artifact: values.artifact,
    }),
  );

  if (doc.hasIn(["on", "schedule"])) {
    doc.deleteIn(["on", "schedule"]);
  }
  const on = doc.get("on", true);
  if (isMap(on) && on.items.length === 0) {
    doc.delete("on");
  }

  const rendered = doc.toString({ lineWidth: 0 });
  const frontMatter = rendered.endsWith("\n") ? rendered : `${rendered}\n`;
  return `---\n${frontMatter}---\n${body}`;
}
