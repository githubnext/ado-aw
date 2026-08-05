import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";
import { parse } from "yaml";

const smokeDir = resolve(dirname(fileURLToPath(import.meta.url)), "../../../../../tests/smoke");
const pipelinePath = resolve(smokeDir, "azure-pipelines-candidate.yml");
const releasePath = resolve(smokeDir, "azure-pipelines-release.yml");
const stepsPath = resolve(smokeDir, "orchestrator-steps.yml");

interface StepLike {
  condition?: string;
  displayName?: string;
  inputs?: { artifact?: string; targetPath?: string };
  script?: string;
  task?: string;
  env?: Record<string, string>;
}

/**
 * Flatten the shared steps template, descending into `${{ if … }}:`
 * conditional blocks (which parse as a single-key map whose value is a list of
 * steps) so mode-gated steps are still reachable.
 */
function collectSteps(node: unknown, out: StepLike[]): void {
  if (Array.isArray(node)) {
    for (const item of node) collectSteps(item, out);
    return;
  }
  if (!node || typeof node !== "object") return;
  const obj = node as Record<string, unknown>;
  if (typeof obj.displayName === "string" || typeof obj.task === "string") {
    out.push(obj as StepLike);
    return;
  }
  for (const value of Object.values(obj)) collectSteps(value, out);
}

describe("candidate orchestrator trigger policy", () => {
  const text = readFileSync(stepsPath, "utf8");
  const pipeline = parse(readFileSync(pipelinePath, "utf8")) as {
    trigger?: string;
    pr?: { branches?: { include?: string[] }; paths?: { include?: string[] } };
    schedules?: Array<{
      cron?: string;
      branches?: { include?: string[] };
      always?: boolean;
    }>;
    jobs?: Array<{
      steps?: Array<{ template?: string; parameters?: { compilerSource?: string } }>;
    }>;
  };
  const steps: StepLike[] = [];
  collectSteps((parse(text) as { steps?: unknown }).steps, steps);

  it("path-filters on the relocated smoke directory", () => {
    expect(pipeline.pr?.paths?.include).toContain("tests/smoke/**");
    expect(pipeline.pr?.paths?.include).toContain("tests/safe-outputs/**");
    expect(pipeline.pr?.paths?.include).not.toContain("tests/compiler-smoke-e2e/**");
  });

  it("passes candidate mode to the shared steps template", () => {
    const template = pipeline.jobs?.[0]?.steps?.[0];
    expect(template?.template).toBe("orchestrator-steps.yml");
    expect(template?.parameters?.compilerSource).toBe("candidate");
  });

  it("builds Rust only in candidate mode and downloads a release only in released mode", () => {
    expect(text).toContain("${{ if eq(parameters.compilerSource, 'candidate') }}");
    expect(text).toContain("${{ if eq(parameters.compilerSource, 'released') }}");
    const download = steps.find((step) => step.displayName === "Download latest released ado-aw");
    expect(download?.script).toContain("releases/latest");
    expect(download?.script).toContain("ado-aw-linux-x64");
  });

  it("passes the compiler source and exactly one definition id per lane to the harness", () => {
    const run = steps.find((step) => step.displayName?.startsWith("Run all smoke cases"));
    const env = run?.env ?? {};
    expect(env.SMOKE_COMPILER_SOURCE).toBe("${{ parameters.compilerSource }}");
    expect(Object.keys(env).filter((key) => key.endsWith("_DEFINITION_ID")).sort()).toEqual([
      "SMOKE_LANE_AGENTIC_DEFINITION_ID",
      "SMOKE_LANE_INFRA_DEFINITION_ID",
    ]);
  });

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
        step.displayName === "Initialize smoke diagnostics",
    );
    expect(initialize?.script).toContain('mkdir -p "$DIAGNOSTICS"');
    expect(initialize?.script).toContain(
      '"ado-aw/smoke-diagnostics/1"',
    );

    const audit = steps.find(
      (step) => step.displayName === "Audit AgentPlayground trigger policy",
    );
    expect(audit?.script).toContain("for attempt in 1 2 3");
    expect(audit?.script).toContain("--fail-with-body");
    expect(audit?.script).toContain('--dump-header "$raw_headers"');
    expect(audit?.script).toContain(
      'RAW_DIAGNOSTICS="$(Agent.TempDirectory)/smoke-policy"',
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
      (step) => step.displayName === "Publish smoke diagnostics",
    );
    expect(publish).toMatchObject({
      condition: "always()",
      inputs: {
        artifact: "smoke-diagnostics",
        targetPath:
          "$(Build.ArtifactStagingDirectory)/smoke-diagnostics",
      },
      task: "PublishPipelineArtifact@1",
    });
  });

  it("audits SELF under the rule matching how the orchestrator is triggered", () => {
    // Regression: SELF_ID was unconditionally appended to the PR-definition
    // list, so the released orchestrator - scheduled-only by design - was
    // required to carry a pullRequest trigger it must never have, and failed
    // its own audit. Caught by the first live released run (build 629514).
    const audit = steps.find(
      (step) => step.displayName === "Audit AgentPlayground trigger policy",
    );
    expect(audit?.script).toContain(
      "SELF_IS_PR_DEFINITION=${{ eq(parameters.compilerSource, 'candidate') }}",
    );
    // SELF must land in exactly one list, never unconditionally in PR_IDS.
    expect(audit?.script).not.toMatch(/\.pr_definition_ids\[\]\s*'\s*"\$POLICY"\s*\)\s*\$SELF_ID"/);
    expect(audit?.script).toContain('PR_IDS="$PR_IDS $SELF_ID"');
    expect(audit?.script).toContain(
      'SCHEDULED_ONLY_IDS="$SCHEDULED_ONLY_IDS $SELF_ID"',
    );
    // The comment-gate assertion is PR-only; in released mode there is no PR
    // trigger to carry a comment gate.
    expect(audit?.script).toMatch(
      /if \[ "\$SELF_IS_PR_DEFINITION" = "True" \]; then[\s\S]*was not included in the PR policy audit/,
    );
  });
});

describe("released orchestrator trigger policy", () => {
  const release = parse(readFileSync(releasePath, "utf8")) as {
    trigger?: string;
    pr?: string;
    schedules?: Array<{ cron?: string; branches?: { include?: string[] }; always?: boolean }>;
    jobs?: Array<{ steps?: Array<{ template?: string; parameters?: { compilerSource?: string } }> }>;
  };

  it("is scheduled/manual only - never CI or PR triggered", () => {
    expect(release.trigger).toBe("none");
    expect(release.pr).toBe("none");
  });

  it("runs daily on main", () => {
    expect(release.schedules).toEqual([
      expect.objectContaining({
        cron: "0 3 * * *",
        branches: { include: ["main"] },
        always: true,
      }),
    ]);
  });

  it("passes released mode to the shared steps template", () => {
    const template = release.jobs?.[0]?.steps?.[0];
    expect(template?.template).toBe("orchestrator-steps.yml");
    expect(template?.parameters?.compilerSource).toBe("released");
  });
});
