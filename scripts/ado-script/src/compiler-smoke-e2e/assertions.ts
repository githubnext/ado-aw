/**
 * Post-compile assertions run against each case's freshly regenerated
 * pipeline YAML inside the detached worktree, before anything is committed or
 * pushed.
 *
 * Test-harness module; not shipped in `ado-script.zip`.
 */
import { parseAllDocuments } from "yaml";

/**
 * Release URLs the candidate lane must never reference — the whole point of
 * pinning `supply-chain.pipeline-artifact` is to source every binary
 * (compiler, AWF, ado-script) from the current build's own artifact instead
 * of a public release. In `released` mode the polarity is inverted: at least
 * one of these must be present, otherwise the run silently stopped exercising
 * release packaging.
 */
const RELEASE_URL_SNIPPETS = [
  "github.com/githubnext/ado-aw/releases",
  "github.com/github/gh-aw-firewall/releases",
] as const;

/** Throws if the compiled YAML still references a public release download URL. */
export function assertNoForbiddenReleaseUrls(yamlText: string, label: string): void {
  for (const snippet of RELEASE_URL_SNIPPETS) {
    if (yamlText.includes(snippet)) {
      throw new Error(
        `${label}: compiled pipeline still references a release URL ('${snippet}') — candidate mode must source binaries exclusively from the pinned pipeline artifact`,
      );
    }
  }
}

/**
 * Throws unless the compiled YAML references a public release download URL.
 *
 * The mirror image of {@link assertNoForbiddenReleaseUrls}, and the reason
 * released mode can replace the retired committed lock files: it proves the
 * staged pipeline will actually fetch released assets at run time.
 */
export function assertReleaseUrlsPresent(yamlText: string, label: string): void {
  if (!RELEASE_URL_SNIPPETS.some((snippet) => yamlText.includes(snippet))) {
    throw new Error(
      `${label}: compiled pipeline references no release URL (expected one of ${RELEASE_URL_SNIPPETS.join(", ")}) — released mode must exercise release asset download`,
    );
  }
}

/**
 * Assert a staged case carries no trigger of any kind.
 *
 * Load-bearing under the lane model: every case is staged to the same
 * `.smoke/pipeline.yml` path against the same lane definition, so a case that
 * compiled a real `trigger:`/`pr:`/`schedules:` block would cause its ref push
 * to CI-trigger the lane *in addition to* the API-queued run — double-queueing
 * the lane and burning parallel jobs.
 *
 * Applies to `raw` cases too, where no front-matter transform runs at all.
 */
export function assertNoTriggers(yamlText: string, label: string): void {
  const docs = parseAllDocuments(yamlText, { merge: false }).map((d) => d.toJS());
  for (const doc of docs) {
    if (!doc || typeof doc !== "object" || Array.isArray(doc)) continue;
    const root = doc as Record<string, unknown>;

    for (const key of ["trigger", "pr"] as const) {
      if (root[key] !== "none") {
        throw new Error(
          `${label}: staged pipeline must declare '${key}: none', got ${JSON.stringify(root[key] ?? null)}`,
        );
      }
    }

    if (root.schedules !== undefined) {
      throw new Error(
        `${label}: staged pipeline must not declare 'schedules:' — the orchestrator owns scheduling`,
      );
    }

    const resources = root.resources as Record<string, unknown> | undefined;
    const pipelines = resources?.pipelines;
    if (Array.isArray(pipelines)) {
      for (const entry of pipelines) {
        if (entry && typeof entry === "object" && (entry as Record<string, unknown>).trigger !== undefined) {
          throw new Error(
            `${label}: staged pipeline must not declare a 'resources.pipelines[].trigger'`,
          );
        }
      }
    }
  }
}

export interface ExpectedPipelineArtifact {
  readonly project: string;
  readonly pipeline: string;
  readonly runId: string;
  readonly artifact: string;
}

function collectDownloadPipelineArtifactSteps(node: unknown, out: Record<string, unknown>[]): void {
  if (Array.isArray(node)) {
    for (const item of node) collectDownloadPipelineArtifactSteps(item, out);
    return;
  }
  if (node && typeof node === "object") {
    const obj = node as Record<string, unknown>;
    if (typeof obj.task === "string" && obj.task.startsWith("DownloadPipelineArtifact")) {
      out.push(obj);
    }
    for (const value of Object.values(obj)) {
      collectDownloadPipelineArtifactSteps(value, out);
    }
  }
}

function collectStepsByDisplayName(
  node: unknown,
  displayName: string,
  out: Record<string, unknown>[],
): void {
  if (Array.isArray(node)) {
    for (const item of node) collectStepsByDisplayName(item, displayName, out);
    return;
  }
  if (node && typeof node === "object") {
    const obj = node as Record<string, unknown>;
    if (obj.displayName === displayName) {
      out.push(obj);
    }
    for (const value of Object.values(obj)) {
      collectStepsByDisplayName(value, displayName, out);
    }
  }
}

function singleStep(
  docs: unknown[],
  label: string,
  displayName: string,
): Record<string, unknown> {
  const steps: Record<string, unknown>[] = [];
  for (const doc of docs) collectStepsByDisplayName(doc, displayName, steps);
  if (steps.length !== 1) {
    throw new Error(
      `${label}: expected exactly one '${displayName}' step, found ${steps.length}`,
    );
  }
  return steps[0]!;
}

/**
 * Assert the Stage 1 credential boundary in freshly compiled YAML.
 *
 * Agent and Detection must not receive any ADO credential, regardless of
 * whether the workflow configures `permissions.read`.
 *
 * `ADO_AW_GITHUB_TOKEN` is included because it is the only credential in the
 * suite that grants write access OUTSIDE the AgentPlayground project (Issues
 * write on an external GitHub repo). The compiler already confines it to the
 * Stage 3 executor env, but nothing else here would catch a regression that
 * projected it into Stage 1 — which is exactly the reach-outside-ADO escape
 * the lane split exists to bound.
 *
 * Note `GITHUB_TOKEN` is deliberately NOT forbidden: that is Copilot CLI
 * authentication and the Agent legitimately receives it.
 */
export function assertAdoTokenIsolation(
  yamlText: string,
  label: string,
): void {
  const docs = parseAllDocuments(yamlText, { merge: false }).map((d) => d.toJS());
  const agent = singleStep(docs, label, "Run copilot (AWF network isolated)");
  const detection = singleStep(docs, label, "Run threat analysis (AWF network isolated)");
  const agentEnv = (agent.env ?? {}) as Record<string, unknown>;
  const detectionEnv = (detection.env ?? {}) as Record<string, unknown>;

  const FORBIDDEN = [
    "AZURE_DEVOPS_EXT_PAT",
    "SC_READ_TOKEN",
    "SC_WRITE_TOKEN",
    "SYSTEM_ACCESSTOKEN",
    "ADO_AW_GITHUB_TOKEN",
  ] as const;

  for (const forbidden of FORBIDDEN) {
    if (agentEnv[forbidden] !== undefined) {
      throw new Error(`${label}: Agent must not receive ${forbidden}`);
    }
  }
  for (const forbidden of FORBIDDEN) {
    if (detectionEnv[forbidden] !== undefined) {
      throw new Error(`${label}: Detection must not receive ${forbidden}`);
    }
  }
}

/** Assert the command/tool policy on the Agent execution step only. */
export function assertAgentCommandPolicy(
  yamlText: string,
  label: string,
  requiredSnippets: readonly string[],
  forbiddenSnippets: readonly string[],
): void {
  const docs = parseAllDocuments(yamlText, { merge: false }).map((d) => d.toJS());
  const agent = singleStep(docs, label, "Run copilot (AWF network isolated)");
  const script = agent.bash;
  if (typeof script !== "string") {
    throw new Error(`${label}: Agent execution step has no bash body`);
  }
  for (const snippet of requiredSnippets) {
    if (!script.includes(snippet)) {
      throw new Error(`${label}: Agent command is missing required snippet '${snippet}'`);
    }
  }
  for (const snippet of forbiddenSnippets) {
    if (script.includes(snippet)) {
      throw new Error(`${label}: Agent command contains forbidden snippet '${snippet}'`);
    }
  }
}

/** Assert required and forbidden snippets against the complete compiled YAML. */
export function assertPipelineTextPolicy(
  yamlText: string,
  label: string,
  requiredSnippets: readonly string[],
  forbiddenSnippets: readonly string[],
): void {
  for (const snippet of requiredSnippets) {
    if (!yamlText.includes(snippet)) {
      throw new Error(`${label}: compiled pipeline is missing required snippet '${snippet}'`);
    }
  }
  for (const snippet of forbiddenSnippets) {
    if (yamlText.includes(snippet)) {
      throw new Error(`${label}: compiled pipeline contains forbidden snippet '${snippet}'`);
    }
  }
}

/**
 * Throws unless every `DownloadPipelineArtifact` "specific run" step in the
 * compiled YAML carries exactly the expected project/pipeline/runId/artifact
 * inputs. Throws if no such step exists at all (the transform is a no-op if
 * the compiler silently dropped the pinned source).
 */
export function assertPipelineArtifactValues(
  yamlText: string,
  label: string,
  expected: ExpectedPipelineArtifact,
): void {
  const docs = parseAllDocuments(yamlText, { merge: false }).map((d) => d.toJS());
  const steps: Record<string, unknown>[] = [];
  for (const doc of docs) collectDownloadPipelineArtifactSteps(doc, steps);

  const specificRunSteps = steps.filter((step) => {
    const inputs = (step.inputs ?? {}) as Record<string, unknown>;
    return inputs.source === "specific";
  });
  if (specificRunSteps.length === 0) {
    throw new Error(`${label}: compiled pipeline has no 'specific run' DownloadPipelineArtifact task`);
  }

  for (const step of specificRunSteps) {
    const inputs = (step.inputs ?? {}) as Record<string, unknown>;
    const actual = {
      project: inputs.project,
      pipeline: inputs.pipeline,
      runId: inputs.runId,
      artifact: inputs.artifact,
    };
    const mismatched = (Object.keys(expected) as (keyof ExpectedPipelineArtifact)[]).filter(
      (key) => actual[key] !== expected[key],
    );
    if (mismatched.length > 0) {
      throw new Error(
        `${label}: DownloadPipelineArtifact inputs mismatch — expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`,
      );
    }
  }
}
