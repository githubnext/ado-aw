/**
 * Markdown front-matter transform applied to a smoke case before it is
 * compiled: pins the `supply-chain:` block to the compiler candidate produced
 * by the current orchestrator run (candidate mode only), and always strips the
 * `on:` trigger block.
 *
 * Parses only the *first* `---` YAML front-matter block (YAML 1.2, via the
 * `yaml` package) and preserves the markdown body byte-for-byte — the
 * transform never touches anything after the closing `---` delimiter line.
 *
 * Test-harness module; not shipped in `ado-script.zip`.
 */
import { Document, parseDocument } from "yaml";

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
 * Prepare a smoke case's markdown source for staging. *
 * Two transforms, both fail-closed:
 *
 *  1. In `candidate` mode, inject `supply-chain.pipeline-artifact` (literal
 *     project/definition-id/run-id/artifact) so the compiled pipeline sources
 *     every binary from this run's own artifact. In `released` mode this is
 *     skipped entirely, leaving the compiled output pointing at public release
 *     assets so release packaging is exercised.
 *  2. In BOTH modes, remove the entire `on:` block.
 *
 * Stripping all of `on:` (not just `on.schedule`) is load-bearing. Every case
 * in a lane is staged to the SAME `.smoke/pipeline.yml` path against the SAME
 * lane definition, so a case declaring `on.pr` or `on.schedule` would compile a
 * real trigger and its ref push would queue the lane in addition to the
 * API-queued run.
 *
 * `on:` is the complete declaration of when a pipeline runs, so removing it
 * makes the compiler emit an explicit `trigger: none` / `pr: none` — a
 * manual / API-queued-only pipeline, which is exactly what a lane needs.
 * `assertNoTriggers` verifies that on the staged bytes rather than trusting it.
 *
 * Also fails closed if `supply-chain.feed` or `supply-chain.pipeline-artifact`
 * is already present (this transform must never silently override an existing
 * binary source), preserves `supply-chain.registry` untouched when present,
 * and preserves the markdown body byte-for-byte.
 */
export function prepareCaseSource(
  markdown: string,
  values: PipelineArtifactValues | undefined,
): string {
  const { yamlText, body } = splitFrontMatter(markdown);
  const doc = parseFrontMatter(yamlText);

  if (values !== undefined) {
    if (doc.hasIn(["supply-chain", "feed"])) {
      throw new Error(
        "case already defines supply-chain.feed; refusing to override with a pinned pipeline-artifact source",
      );
    }
    if (doc.hasIn(["supply-chain", "pipeline-artifact"])) {
      throw new Error(
        "case already defines supply-chain.pipeline-artifact; refusing to override",
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
  }

  // The orchestrator owns scheduling and queueing for every case, so no staged
  // case may carry a trigger of any kind.
  doc.delete("on");

  const rendered = doc.toString({ lineWidth: 0 });
  const frontMatter = rendered.endsWith("\n") ? rendered : `${rendered}\n`;
  return `---\n${frontMatter}---\n${body}`;
}

