import { describe, expect, it } from "vitest";

import {
  assertAgentCommandPolicy,
  assertAdoTokenIsolation,
  assertNoForbiddenReleaseUrls,
  assertPipelineArtifactValues,
} from "../assertions.js";

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

function agentTokenYaml(opts: {
  agentReadToken?: string;
  agentExtraEnv?: string;
  detectionExtraEnv?: string;
} = {}): string {
  const agentReadToken =
    opts.agentReadToken === undefined
      ? ""
      : `\n          AZURE_DEVOPS_EXT_PAT: ${opts.agentReadToken}`;
  return `
jobs:
  - job: Agent
    steps:
      - bash: echo agent
        displayName: Run copilot (AWF network isolated)
        env:
          GITHUB_TOKEN: $(GITHUB_TOKEN)${agentReadToken}${opts.agentExtraEnv ?? ""}
  - job: Detection
    steps:
      - bash: echo detection
        displayName: Run threat analysis (AWF network isolated)
        env:
          GITHUB_TOKEN: $(GITHUB_TOKEN)${opts.detectionExtraEnv ?? ""}
`;
}

describe("assertAdoTokenIsolation", () => {
  it("accepts the read-scoped Agent mapping while keeping Detection isolated", () => {
    expect(() =>
      assertAdoTokenIsolation(agentTokenYaml({ agentReadToken: "$(SC_READ_TOKEN)" }), "canary", true),
    ).not.toThrow();
  });

  it("rejects a missing Agent read-token mapping", () => {
    expect(() => assertAdoTokenIsolation(agentTokenYaml(), "canary", true)).toThrow(
      /read-token contract mismatch/,
    );
  });

  it("accepts a workflow that does not request read permissions", () => {
    expect(() => assertAdoTokenIsolation(agentTokenYaml(), "custom-safe-output", false)).not.toThrow();
  });

  it("rejects a write-scoped token on the Agent", () => {
    expect(() =>
      assertAdoTokenIsolation(
        agentTokenYaml({
          agentReadToken: "$(SC_READ_TOKEN)",
          agentExtraEnv: "\n          SC_WRITE_TOKEN: $(SC_WRITE_TOKEN)",
        }),
        "canary",
        true,
      ),
    ).toThrow(/Agent must not receive SC_WRITE_TOKEN/);
  });

  it("rejects any ADO token on Detection", () => {
    expect(() =>
      assertAdoTokenIsolation(
        agentTokenYaml({
          agentReadToken: "$(SC_READ_TOKEN)",
          detectionExtraEnv: "\n          AZURE_DEVOPS_EXT_PAT: $(SC_READ_TOKEN)",
        }),
        "canary",
        true,
      ),
    ).toThrow(/Detection must not receive AZURE_DEVOPS_EXT_PAT/);
  });
});

describe("assertAgentCommandPolicy", () => {
  it("accepts a restricted Agent command", () => {
    const yaml = agentTokenYaml().replace(
      "echo agent",
      'copilot --allow-tool "shell(az:*)" --allow-tool "shell(head)"',
    );
    expect(() =>
      assertAgentCommandPolicy(
        yaml,
        "azure-cli",
        ["shell(az", "shell(head"],
        ["--allow-all-tools", "--allow-all-paths"],
      ),
    ).not.toThrow();
  });

  it("rejects unrestricted Agent tools", () => {
    const yaml = agentTokenYaml().replace(
      "echo agent",
      "copilot --allow-all-tools --allow-all-paths",
    );
    expect(() =>
      assertAgentCommandPolicy(
        yaml,
        "azure-cli",
        ["shell(az"],
        ["--allow-all-tools", "--allow-all-paths"],
      ),
    ).toThrow(/missing required snippet|forbidden snippet/);
  });
});

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
