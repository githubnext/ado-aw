import { describe, expect, it } from "vitest";

import {
  assertAgentCommandPolicy,
  assertAdoTokenIsolation,
  assertNoForbiddenReleaseUrls,
  assertNoTriggers,
  assertPipelineArtifactValues,
  assertReleaseUrlsPresent,
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
  agentExtraEnv?: string;
  detectionExtraEnv?: string;
} = {}): string {
  return `
jobs:
  - job: Agent
    steps:
      - bash: echo agent
        displayName: Run copilot (AWF network isolated)
        env:
          GITHUB_TOKEN: $(GITHUB_TOKEN)${opts.agentExtraEnv ?? ""}
  - job: Detection
    steps:
      - bash: echo detection
        displayName: Run threat analysis (AWF network isolated)
        env:
          GITHUB_TOKEN: $(GITHUB_TOKEN)${opts.detectionExtraEnv ?? ""}
`;
}

describe("assertAdoTokenIsolation", () => {
  it("accepts credential-free Agent and Detection environments", () => {
    expect(() => assertAdoTokenIsolation(agentTokenYaml(), "canary")).not.toThrow();
  });

  it.each([
    "AZURE_DEVOPS_EXT_PAT",
    "SC_READ_TOKEN",
    "SC_WRITE_TOKEN",
    "SYSTEM_ACCESSTOKEN",
  ])("rejects %s on the Agent", (credential) => {
    expect(() =>
      assertAdoTokenIsolation(
        agentTokenYaml({
          agentExtraEnv: `\n          ${credential}: $(${credential})`,
        }),
        "canary",
      ),
    ).toThrow(new RegExp(`Agent must not receive ${credential}`));
  });

  it.each([
    "AZURE_DEVOPS_EXT_PAT",
    "SC_READ_TOKEN",
    "SC_WRITE_TOKEN",
    "SYSTEM_ACCESSTOKEN",
  ])("rejects %s on Detection", (credential) => {
    expect(() =>
      assertAdoTokenIsolation(
        agentTokenYaml({
          detectionExtraEnv: `\n          ${credential}: $(${credential})`,
        }),
        "canary",
      ),
    ).toThrow(new RegExp(`Detection must not receive ${credential}`));
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

describe("assertReleaseUrlsPresent", () => {
  it("passes when the compiled pipeline downloads a released asset", () => {
    expect(() =>
      assertReleaseUrlsPresent(
        "steps:\n  - bash: curl -L https://github.com/githubnext/ado-aw/releases/download/v1/ado-aw\n",
        "canary",
      ),
    ).not.toThrow();
  });

  it("accepts the AWF release URL alone", () => {
    expect(() =>
      assertReleaseUrlsPresent(
        "steps:\n  - bash: curl -L https://github.com/github/gh-aw-firewall/releases/download/v1/awf\n",
        "canary",
      ),
    ).toBeTruthy();
  });

  it("fails closed when released mode silently stopped downloading release assets", () => {
    // Without this, a released-mode run that accidentally pinned a pipeline
    // artifact would go green while testing nothing about release packaging.
    expect(() => assertReleaseUrlsPresent("steps:\n  - bash: echo hi\n", "canary")).toThrow(
      /references no release URL/,
    );
  });

  it("is the exact mirror image of assertNoForbiddenReleaseUrls", () => {
    const withRelease =
      "steps:\n  - bash: curl -L https://github.com/githubnext/ado-aw/releases/download/v1/ado-aw\n";
    expect(() => assertNoForbiddenReleaseUrls(withRelease, "x")).toThrow();
    expect(() => assertReleaseUrlsPresent(withRelease, "x")).not.toThrow();

    const withoutRelease = "steps:\n  - bash: echo hi\n";
    expect(() => assertNoForbiddenReleaseUrls(withoutRelease, "x")).not.toThrow();
    expect(() => assertReleaseUrlsPresent(withoutRelease, "x")).toThrow();
  });
});

describe("assertNoTriggers", () => {
  const clean = "trigger: none\npr: none\njobs:\n  - job: Agent\n";

  it("passes for a pipeline that declares trigger: none and pr: none", () => {
    expect(() => assertNoTriggers(clean, "canary")).not.toThrow();
  });

  it("rejects a CI trigger", () => {
    // Load-bearing: every case in a lane shares one definition AND one YAML
    // path, so a surviving trigger would make the ref push CI-trigger the lane
    // on top of the API-queued run.
    expect(() =>
      assertNoTriggers("trigger:\n  branches:\n    include:\n      - main\npr: none\n", "canary"),
    ).toThrow(/must declare 'trigger: none'/);
  });

  it("rejects a PR trigger", () => {
    expect(() =>
      assertNoTriggers("trigger: none\npr:\n  branches:\n    include:\n      - main\n", "canary"),
    ).toThrow(/must declare 'pr: none'/);
  });

  it("rejects a missing trigger key rather than assuming a safe default", () => {
    expect(() => assertNoTriggers("pr: none\njobs: []\n", "canary")).toThrow(
      /must declare 'trigger: none'/,
    );
  });

  it("rejects a schedules block", () => {
    expect(() =>
      assertNoTriggers(
        "trigger: none\npr: none\nschedules:\n  - cron: '0 3 * * *'\n",
        "canary",
      ),
    ).toThrow(/must not declare 'schedules:'/);
  });

  it("rejects a pipeline resource trigger", () => {
    expect(() =>
      assertNoTriggers(
        "trigger: none\npr: none\nresources:\n  pipelines:\n    - pipeline: up\n      source: Other\n      trigger: true\n",
        "canary",
      ),
    ).toThrow(/resources\.pipelines\[\]\.trigger/);
  });

  it("allows a pipeline resource without a trigger", () => {
    expect(() =>
      assertNoTriggers(
        "trigger: none\npr: none\nresources:\n  pipelines:\n    - pipeline: up\n      source: Other\n",
        "canary",
      ),
    ).not.toThrow();
  });
});
