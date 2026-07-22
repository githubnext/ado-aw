import { describe, expect, it } from "vitest";

import { assertNoForbiddenReleaseUrls, assertPipelineArtifactValues } from "../assertions.js";

const EXPECTED = {
  project: "AgentPlayground",
  pipeline: "2560",
  runId: "630001",
  artifact: "ado-aw-candidate",
};

function specificRunYaml(overrides: Partial<typeof EXPECTED> = {}): string {
  const values = { ...EXPECTED, ...overrides };
  return `
steps:
  - task: DownloadPipelineArtifact@2
    displayName: Download Pipeline Artifact
    inputs:
      targetPath: $(Pipeline.Workspace)/in
      source: specific
      project: ${values.project}
      pipeline: '${values.pipeline}'
      runVersion: specific
      runId: '${values.runId}'
      artifact: ${values.artifact}
`;
}

describe("assertNoForbiddenReleaseUrls", () => {
  it("passes for YAML with no forbidden release URL", () => {
    expect(() => assertNoForbiddenReleaseUrls(specificRunYaml(), "canary")).not.toThrow();
  });

  it("throws when the compiler release URL is present", () => {
    const yaml = `${specificRunYaml()}\n# https://github.com/githubnext/ado-aw/releases/download/v1/ado-aw\n`;
    expect(() => assertNoForbiddenReleaseUrls(yaml, "canary")).toThrow(/release URL/);
  });

  it("throws when the AWF firewall release URL is present", () => {
    const yaml = `${specificRunYaml()}\n# https://github.com/github/gh-aw-firewall/releases/download/v1/awf\n`;
    expect(() => assertNoForbiddenReleaseUrls(yaml, "canary")).toThrow(/release URL/);
  });
});

describe("assertPipelineArtifactValues", () => {
  it("passes when the DownloadPipelineArtifact step matches exactly", () => {
    expect(() => assertPipelineArtifactValues(specificRunYaml(), "canary", EXPECTED)).not.toThrow();
  });

  it("throws when there is no specific-run DownloadPipelineArtifact step at all", () => {
    const yaml = "steps:\n  - task: Bash@3\n    inputs:\n      script: echo hi\n";
    expect(() => assertPipelineArtifactValues(yaml, "canary", EXPECTED)).toThrow(/no 'specific run'/);
  });

  it("throws on a project mismatch", () => {
    expect(() =>
      assertPipelineArtifactValues(specificRunYaml({ project: "WrongProject" }), "canary", EXPECTED),
    ).toThrow(/mismatch/);
  });

  it("throws on a pipeline (definition id) mismatch", () => {
    expect(() =>
      assertPipelineArtifactValues(specificRunYaml({ pipeline: "9999" }), "canary", EXPECTED),
    ).toThrow(/mismatch/);
  });

  it("throws on a runId mismatch", () => {
    expect(() =>
      assertPipelineArtifactValues(specificRunYaml({ runId: "1" }), "canary", EXPECTED),
    ).toThrow(/mismatch/);
  });

  it("throws on an artifact name mismatch", () => {
    expect(() =>
      assertPipelineArtifactValues(specificRunYaml({ artifact: "wrong-name" }), "canary", EXPECTED),
    ).toThrow(/mismatch/);
  });

  it("ignores a DownloadPipelineArtifact step whose source is 'current' (not our pinned source)", () => {
    const yaml = `
steps:
  - task: DownloadPipelineArtifact@2
    inputs:
      targetPath: $(Pipeline.Workspace)/in
      source: current
      artifact: safe_outputs
  - task: DownloadPipelineArtifact@2
    inputs:
      targetPath: $(Pipeline.Workspace)/in2
      source: specific
      project: ${EXPECTED.project}
      pipeline: '${EXPECTED.pipeline}'
      runVersion: specific
      runId: '${EXPECTED.runId}'
      artifact: ${EXPECTED.artifact}
`;
    expect(() => assertPipelineArtifactValues(yaml, "canary", EXPECTED)).not.toThrow();
  });

  it("finds a DownloadPipelineArtifact step nested inside stages/jobs", () => {
    const yaml = `
stages:
  - stage: Agent
    jobs:
      - job: run
        steps:
          - task: DownloadPipelineArtifact@2
            inputs:
              targetPath: in
              source: specific
              project: ${EXPECTED.project}
              pipeline: '${EXPECTED.pipeline}'
              runVersion: specific
              runId: '${EXPECTED.runId}'
              artifact: ${EXPECTED.artifact}
`;
    expect(() => assertPipelineArtifactValues(yaml, "canary", EXPECTED)).not.toThrow();
  });
});
