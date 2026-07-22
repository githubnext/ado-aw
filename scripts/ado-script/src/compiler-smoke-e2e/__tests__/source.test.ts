import { describe, expect, it } from "vitest";

import { injectPipelineArtifact, splitFrontMatter } from "../source.js";

const VALUES = {
  project: "AgentPlayground",
  definitionId: 2560,
  runId: 630001,
  artifact: "ado-aw-candidate",
};

describe("splitFrontMatter", () => {
  it("splits the first --- front-matter block from the body", () => {
    const md = "---\nname: x\n---\n# Hello\n\nBody text.\n";
    const { yamlText, body } = splitFrontMatter(md);
    expect(yamlText).toBe("name: x");
    expect(body).toBe("# Hello\n\nBody text.\n");
  });

  it("throws when there is no leading front-matter block", () => {
    expect(() => splitFrontMatter("# Just a heading\n")).toThrow(/front-matter/);
  });

  it("only matches the FIRST closing delimiter, never a later one inside the body", () => {
    const md = "---\nname: x\n---\nSome body\n---\nnot-frontmatter: true\n---\nmore\n";
    const { yamlText, body } = splitFrontMatter(md);
    expect(yamlText).toBe("name: x");
    expect(body).toBe("Some body\n---\nnot-frontmatter: true\n---\nmore\n");
  });
});

describe("injectPipelineArtifact", () => {
  it("injects literal project/definition-id/run-id/artifact under supply-chain.pipeline-artifact", () => {
    const md = "---\nname: canary\non:\n  schedule:\n    - cron: '0 3 * * *'\n---\nBody unchanged.\n";
    const out = injectPipelineArtifact(md, VALUES);
    const { yamlText, body } = splitFrontMatter(out);
    expect(yamlText).toContain("supply-chain:");
    expect(yamlText).toContain("pipeline-artifact:");
    expect(yamlText).toContain("project: AgentPlayground");
    expect(yamlText).toContain("definition-id: 2560");
    expect(yamlText).toContain("run-id: 630001");
    expect(yamlText).toContain("artifact: ado-aw-candidate");
    expect(body).toBe("Body unchanged.\n");
  });

  it("preserves the markdown body byte-for-byte, including trailing whitespace quirks", () => {
    const body = "# Title\n\n- a\n- b\n\ntrailing spaces   \n\n\nextra blank lines\n";
    const md = `---\nname: x\n---\n${body}`;
    const out = injectPipelineArtifact(md, VALUES);
    expect(out.endsWith(body)).toBe(true);
  });

  it("removes on.schedule but preserves other 'on' keys", () => {
    const md = "---\nname: x\non:\n  schedule:\n    - cron: '0 3 * * *'\n  workflow_dispatch: {}\n---\nBody.\n";
    const out = injectPipelineArtifact(md, VALUES);
    const { yamlText } = splitFrontMatter(out);
    expect(yamlText).not.toContain("schedule");
    expect(yamlText).toContain("workflow_dispatch");
    expect(yamlText).toContain("on:");
  });

  it("removes the 'on' key entirely once schedule was its only member", () => {
    const md = "---\nname: x\non:\n  schedule:\n    - cron: '0 3 * * *'\n---\nBody.\n";
    const out = injectPipelineArtifact(md, VALUES);
    const { yamlText } = splitFrontMatter(out);
    expect(yamlText).not.toMatch(/^on:/m);
  });

  it("is a no-op with respect to 'on' when there is no 'on' block at all", () => {
    const md = "---\nname: noop-target\n---\nBody.\n";
    const out = injectPipelineArtifact(md, VALUES);
    const { yamlText } = splitFrontMatter(out);
    expect(yamlText).not.toMatch(/^on:/m);
    expect(yamlText).toContain("supply-chain:");
  });

  it("preserves an existing supply-chain.registry untouched", () => {
    const md =
      "---\nname: x\nsupply-chain:\n  registry: my-registry\n---\nBody.\n";
    const out = injectPipelineArtifact(md, VALUES);
    const { yamlText } = splitFrontMatter(out);
    expect(yamlText).toContain("registry: my-registry");
    expect(yamlText).toContain("pipeline-artifact:");
  });

  it("rejects a fixture that already defines supply-chain.feed", () => {
    const md = "---\nname: x\nsupply-chain:\n  feed: my-feed\n---\nBody.\n";
    expect(() => injectPipelineArtifact(md, VALUES)).toThrow(/supply-chain\.feed/);
  });

  it("rejects a fixture that already defines supply-chain.pipeline-artifact", () => {
    const md =
      "---\nname: x\nsupply-chain:\n  pipeline-artifact:\n    project: Other\n    definition-id: 1\n    run-id: 1\n    artifact: a\n---\nBody.\n";
    expect(() => injectPipelineArtifact(md, VALUES)).toThrow(/pipeline-artifact/);
  });

  it("throws on malformed YAML front matter", () => {
    const md = "---\nname: [unterminated\n---\nBody.\n";
    expect(() => injectPipelineArtifact(md, VALUES)).toThrow();
  });
});
