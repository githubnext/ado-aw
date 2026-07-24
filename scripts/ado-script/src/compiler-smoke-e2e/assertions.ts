/**
 * Post-compile assertions run against each fixture's freshly regenerated
 * `*.lock.yml` inside the detached worktree, before anything is committed or
 * pushed.
 *
 * Test-harness module; not shipped in `ado-script.zip`.
 */
import { parseAllDocuments } from "yaml";

/**
 * Release URLs the candidate lane must never reference — the whole point of
 * pinning `supply-chain.pipeline-artifact` is to source every binary
 * (compiler, AWF, ado-script) from the current build's own artifact instead
 * of a public release.
 */
const FORBIDDEN_URL_SNIPPETS = [
  "github.com/githubnext/ado-aw/releases",
  "github.com/github/gh-aw-firewall/releases",
] as const;

/** Throws if the compiled YAML still references a public release download URL. */
export function assertNoForbiddenReleaseUrls(yamlText: string, label: string): void {
  for (const snippet of FORBIDDEN_URL_SNIPPETS) {
    if (yamlText.includes(snippet)) {
      throw new Error(
        `${label}: compiled pipeline still references a release URL ('${snippet}') — the candidate lane must source binaries exclusively from the pinned pipeline artifact`,
      );
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
 * Assert the Stage 1 ADO credential boundary in freshly compiled YAML.
 *
 * Workflows with `permissions.read` must project the read-scoped token to the
 * Agent as `AZURE_DEVOPS_EXT_PAT`. Workflows without it must not gain the
 * mapping. Detection must never receive any ADO token, and neither execution
 * step may receive the write-scoped token.
 */
export function assertAdoTokenIsolation(
  yamlText: string,
  label: string,
  expectsAgentReadToken: boolean,
): void {
  const docs = parseAllDocuments(yamlText, { merge: false }).map((d) => d.toJS());
  const agent = singleStep(docs, label, "Run copilot (AWF network isolated)");
  const detection = singleStep(docs, label, "Run threat analysis (AWF network isolated)");
  const agentEnv = (agent.env ?? {}) as Record<string, unknown>;
  const detectionEnv = (detection.env ?? {}) as Record<string, unknown>;
  const actualReadToken = agentEnv.AZURE_DEVOPS_EXT_PAT;

  if (expectsAgentReadToken && actualReadToken !== "$(SC_READ_TOKEN)") {
    throw new Error(
      `${label}: Agent read-token contract mismatch — expected AZURE_DEVOPS_EXT_PAT='$(SC_READ_TOKEN)', got ${JSON.stringify(actualReadToken)}`,
    );
  }
  if (!expectsAgentReadToken && actualReadToken !== undefined) {
    throw new Error(
      `${label}: Agent unexpectedly received AZURE_DEVOPS_EXT_PAT=${JSON.stringify(actualReadToken)}`,
    );
  }

  for (const forbidden of ["SC_READ_TOKEN", "SC_WRITE_TOKEN", "SYSTEM_ACCESSTOKEN"]) {
    if (agentEnv[forbidden] !== undefined) {
      throw new Error(`${label}: Agent must not receive ${forbidden}`);
    }
  }
  for (const forbidden of [
    "AZURE_DEVOPS_EXT_PAT",
    "SC_READ_TOKEN",
    "SC_WRITE_TOKEN",
    "SYSTEM_ACCESSTOKEN",
  ]) {
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
