import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";
import { parse } from "yaml";

const pipelinePath = resolve(
  dirname(fileURLToPath(import.meta.url)),
  "../../../../../tests/compiler-smoke-e2e/azure-pipelines.yml",
);

describe("candidate compiler trigger policy", () => {
  const text = readFileSync(pipelinePath, "utf8");
  const pipeline = parse(text) as {
    trigger?: string;
    pr?: { branches?: { include?: string[] } };
    schedules?: Array<{
      cron?: string;
      branches?: { include?: string[] };
      always?: boolean;
    }>;
    jobs?: Array<{
      steps?: Array<{
        condition?: string;
        displayName?: string;
        inputs?: {
          artifact?: string;
          targetPath?: string;
        };
        script?: string;
        task?: string;
      }>;
    }>;
  };
  const steps = pipeline.jobs?.flatMap((job) => job.steps ?? []) ?? [];

  it("keeps PRs eligible for the Azure Pipelines comment trigger", () => {
    expect(pipeline.trigger).toBe("none");
    expect(pipeline.pr?.branches?.include).toEqual(["main"]);
  });

  it("runs the latest main candidate every day", () => {
    expect(pipeline.schedules).toEqual([
      expect.objectContaining({
        cron: "0 1 * * *",
        branches: { include: ["main"] },
        always: true,
      }),
    ]);
  });

  it("fails when the live orchestrator loses its all-PR comment gate", () => {
    for (const contract of [
      ".isCommentRequiredForPullRequest == true",
      ".requireCommentsForNonTeamMembersOnly == false",
      ".requireCommentsForNonTeamMemberAndNonContributors == false",
      ".isCommentRequiredForInternalRepoPRs == true",
      '.commentOptionInternalRepos == "all"',
    ]) {
      expect(text).toContain(contract);
    }
  });

  it("preserves bounded ADO response diagnostics when the policy audit fails", () => {
    const initialize = steps.find(
      (step) =>
        step.displayName === "Initialize candidate smoke diagnostics",
    );
    expect(initialize?.script).toContain('mkdir -p "$DIAGNOSTICS"');
    expect(initialize?.script).toContain(
      '"ado-aw/candidate-smoke-diagnostics/1"',
    );

    const audit = steps.find(
      (step) => step.displayName === "Audit AgentPlayground trigger policy",
    );
    expect(audit?.script).toContain("for attempt in 1 2 3");
    expect(audit?.script).toContain("--fail-with-body");
    expect(audit?.script).toContain('--dump-header "$raw_headers"');
    expect(audit?.script).toContain(
      'RAW_DIAGNOSTICS="$(Agent.TempDirectory)/compiler-smoke-policy"',
    );
    expect(audit?.script).toContain(
      'body="$RAW_DIAGNOSTICS/definition-${id}-attempt-${attempt}.body"',
    );
    expect(audit?.script).toContain(
      'raw_headers="$RAW_DIAGNOSTICS/definition-${id}-attempt-${attempt}.headers.raw"',
    );
    expect(audit?.script).not.toContain(
      'body="$DIAGNOSTICS/definition-${id}-attempt-${attempt}.body"',
    );
    expect(audit?.script).toContain(
      '-H "Authorization: Bearer $SYSTEM_ACCESSTOKEN"',
    );
    expect(audit?.script).not.toContain("Authorization: ******");
    expect(audit?.script).not.toContain('SELF_JSON="$(curl');
    expect(audit?.script).toContain("http_code=%{http_code}");
    expect(audit?.script).toContain("jq_error_begin");
    expect(audit?.script).toContain('head -c 16384 "$body"');
    expect(audit?.script).toContain("response_sample_begin");

    const publish = steps.find(
      (step) => step.displayName === "Publish candidate smoke diagnostics",
    );
    expect(publish).toMatchObject({
      condition: "always()",
      inputs: {
        artifact: "compiler-smoke-diagnostics",
        targetPath:
          "$(Build.ArtifactStagingDirectory)/compiler-smoke-diagnostics",
      },
      task: "PublishPipelineArtifact@1",
    });
  });
});
