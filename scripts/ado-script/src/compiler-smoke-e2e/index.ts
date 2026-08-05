/**
 * Entry point for the deterministic smoke E2E orchestrator.
 *
 * Stages every smoke case declared in `tests/smoke/cases.json` for the current
 * compiler-source mode, pushes each one to its own short-lived branch on the
 * mirror repo, queues each against its credential *lane* definition, and
 * asserts they all go green.
 *
 * The lane model inverts the old mapping: a definition is a credential
 * boundary, not a test case. Cases are told apart by their per-case ref
 * (`refs/heads/ado-aw-smoke-candidate/<buildId>/<caseId>`), all staged to the
 * same fixed `.smoke/pipeline.yml` path, so adding a smoke costs a markdown
 * file and a manifest entry — no ADO definition registration.
 *
 * Two modes (`SMOKE_COMPILER_SOURCE`):
 *   - `candidate` — compiles with the binary built from this run's commit and
 *     pins every case to this run's own pipeline artifact.
 *   - `released`  — compiles with the latest released binary and leaves the
 *     output pointing at public release assets, so release packaging and
 *     asset availability are exercised. This replaces the retired committed
 *     `*.lock.yml` files.
 *
 * See `config.ts` for the full required/optional env var contract.
 *
 * Test-harness module; not shipped in `ado-script.zip`.
 */
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { mkdir } from "node:fs/promises";

import { AdoRest } from "./ado-rest.js";
import {
  assertAgentCommandPolicy,
  assertAdoTokenIsolation,
  assertNoForbiddenReleaseUrls,
  assertNoTriggers,
  assertPipelineArtifactValues,
  assertReleaseUrlsPresent,
} from "./assertions.js";
import { loadCases, type ResolvedCase, type ResolvedCases } from "./cases.js";
import { candidateRef, loadConfig, type SmokeConfig } from "./config.js";
import { compileAndCheck } from "./compile-cli.js";
import {
  commitAll,
  createDetachedWorktree,
  deleteRemoteRefs,
  disallowedChanges,
  listCandidateRefs,
  mirrorRepoUrl,
  pushCandidate,
  removeWorktree,
  resetWorktree,
  verifyLocalCommit,
  verifyRemoteRef,
  worktreeChangedFiles,
} from "./git.js";
import { prepareCaseSource } from "./source.js";
import { renderResultsTable } from "./report.js";
import { runFixtures, type FixtureBuildRequest, type FixtureBuildResult } from "./runner.js";
import { verifyCaseSignals } from "./signals.js";
import { scanStaleRefs } from "./stale.js";

function log(msg: string): void {
  // Percent-encode a leading '#' so a message cannot smuggle a ##vso command.
  process.stdout.write(msg.replace(/^#/gm, "%23") + "\n");
}

function errMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

/** Repo-relative lock path the compiler writes for a given markdown source. */
function lockPathFor(relMd: string): string {
  return `${relMd.slice(0, -".md".length)}.lock.yml`;
}

/**
 * The exact set of repo-relative paths ONE case's staging commit may touch.
 *
 * Deliberately exact-match and per-case (rather than a union across every
 * case): the worktree is reset between cases, so a change belonging to a
 * different case indicates the reset failed and must abort before push.
 */
function allowedChangedPaths(entry: ResolvedCase, yamlPath: string): Set<string> {
  const paths = new Set<string>([".gitattributes", yamlPath]);
  if (entry.kind === "compiled") {
    paths.add(entry.source);
    paths.add(lockPathFor(entry.source));
  }
  return paths;
}

/**
 * Produce one case's `.smoke/pipeline.yml` inside the worktree.
 *
 * `compiled` cases are transformed, compiled by the binary under test, and
 * asserted; `raw` cases are copied verbatim. Both end with the same fixed
 * path staged and `assertNoTriggers` enforced.
 */
async function stageCase(
  config: SmokeConfig,
  resolved: ResolvedCases,
  entry: ResolvedCase,
  worktreeDir: string,
  mirrorUrl: string,
): Promise<void> {
  const target = join(worktreeDir, resolved.yamlPath);
  await mkdir(dirname(target), { recursive: true });

  if (entry.kind === "raw") {
    // No compiler runs here, so a raw source must declare `trigger: none` /
    // `pr: none` itself; `assertNoTriggers` is what enforces that.
    const raw = await readFile(join(worktreeDir, entry.source), "utf8");
    await writeFile(target, raw, "utf8");
    assertNoTriggers(raw, entry.id);
    return;
  }

  const relMd = entry.source;
  const relLock = lockPathFor(relMd);

  const original = await readFile(join(worktreeDir, relMd), "utf8");
  // Candidate mode pins every binary to this run's own artifact; released mode
  // deliberately leaves the release URLs in place so asset download is tested.
  const artifact =
    resolved.mode === "candidate"
      ? {
          project: config.project,
          definitionId: config.definitionId,
          runId: config.buildId,
          artifact: config.artifactName,
        }
      : undefined;
  await writeFile(join(worktreeDir, relMd), prepareCaseSource(original, artifact), "utf8");

  const result = await compileAndCheck({
    adoAwBin: config.adoAwBin,
    worktreeDir,
    metadataRemoteUrl: mirrorUrl,
    relMd,
    relLock,
    timeoutMs: config.childTimeoutMs,
    secrets: [config.token],
  });
  if (!result.ok) {
    throw new Error(
      `case '${entry.id}' ${result.phase} failed: ${result.message}\n--- stdout ---\n${result.stdout}\n--- stderr ---\n${result.stderr}`,
    );
  }

  const yamlText = await readFile(join(worktreeDir, relLock), "utf8");
  assertAdoTokenIsolation(yamlText, entry.id);

  if (resolved.mode === "candidate") {
    assertNoForbiddenReleaseUrls(yamlText, entry.id);
    assertPipelineArtifactValues(yamlText, entry.id, {
      project: config.project,
      pipeline: String(config.definitionId),
      runId: String(config.buildId),
      artifact: config.artifactName,
    });
  } else {
    assertReleaseUrlsPresent(yamlText, entry.id);
  }

  const agentCommand = entry.assertions?.agentCommand;
  if (agentCommand) {
    assertAgentCommandPolicy(yamlText, entry.id, agentCommand.required, agentCommand.forbidden);
  }

  // The staged copy is byte-identical to the compiled lock, so the pipeline's
  // own runtime `ado-aw check <lock>` integrity step still passes. Stripping
  // `on:` is what makes the compiler emit `trigger: none` / `pr: none`;
  // assert it on the staged bytes rather than trusting it.
  await writeFile(target, yamlText, "utf8");
  assertNoTriggers(yamlText, entry.id);
}

/** Stage, commit and push every case, returning the per-case ref and commit SHA. */
async function stageAllCases(
  config: SmokeConfig,
  resolved: ResolvedCases,
  worktreeDir: string,
  mirrorUrl: string,
  onPushed: (caseId: string, ref: string) => void,
): Promise<Map<string, { ref: string; sha: string }>> {
  const staged = new Map<string, { ref: string; sha: string }>();

  for (const entry of resolved.cases) {
    await stageCase(config, resolved, entry, worktreeDir, mirrorUrl);

    const changed = await worktreeChangedFiles({ worktreeDir, timeoutMs: config.childTimeoutMs });
    const violations = disallowedChanges(changed, allowedChangedPaths(entry, resolved.yamlPath));
    if (violations.length > 0) {
      throw new Error(
        `refusing to push case '${entry.id}': unexpected path(s) changed: ${violations.join(", ")}`,
      );
    }

    const sha = await commitAll({
      worktreeDir,
      buildId: config.buildId,
      caseId: entry.id,
      timeoutMs: config.childTimeoutMs,
    });
    const ref = candidateRef(config.buildId, entry.id);
    await pushCandidate({
      worktreeDir,
      mirrorUrl,
      ref,
      token: config.token,
      timeoutMs: config.childTimeoutMs,
    });
    onPushed(entry.id, ref);
    await verifyRemoteRef({
      cwd: worktreeDir,
      mirrorUrl,
      ref,
      expectedSha: sha,
      token: config.token,
      timeoutMs: config.childTimeoutMs,
    });
    staged.set(entry.id, { ref, sha });
    log(`[${entry.id}] staged ${sha} at ${ref}`);

    // Reset so the next case's commit is a SIBLING parented on
    // BUILD_SOURCEVERSION, not a chain — each ref then contains exactly its
    // own case, and the per-case allowlist above stays meaningful.
    await resetWorktree({
      worktreeDir,
      commitish: config.sourceVersion,
      timeoutMs: config.childTimeoutMs,
    });
  }

  return staged;
}

async function cleanupStaleRefs(
  config: SmokeConfig,
  resolved: ResolvedCases,
  rest: AdoRest,
  mirrorUrl: string,
  ownRefs: ReadonlySet<string>,
): Promise<void> {
  try {
    const refs = await listCandidateRefs({
      cwd: config.sourcesDirectory,
      mirrorUrl,
      token: config.token,
      timeoutMs: config.childTimeoutMs,
    });
    const decisions = await scanStaleRefs({
      refs: refs.filter((entry) => !ownRefs.has(entry.ref)),
      baseRef: config.sourceBranch,
      ownRef: "",
      definitionId: config.definitionId,
      laneDefinitionIds: resolved.laneDefinitionIds,
      staleRefHours: config.staleRefHours,
      client: rest,
    });
    const eligible = decisions.filter((decision) => decision.outcome === "eligible");
    for (const decision of decisions) {
      if (decision.outcome !== "eligible") {
        log(`[stale-scan] ${decision.ref}: ${decision.outcome} — ${decision.reason}`);
      }
    }
    if (eligible.length === 0) return;
    try {
      await deleteRemoteRefs({
        cwd: config.sourcesDirectory,
        mirrorUrl,
        refs: eligible.map((decision) => decision.ref),
        token: config.token,
        timeoutMs: config.childTimeoutMs,
      });
      for (const decision of eligible) {
        log(`[stale-scan] deleted ${decision.ref}: ${decision.reason}`);
      }
    } catch (err) {
      log(`[stale-scan] WARNING: failed to delete stale ref(s): ${errMessage(err)}`);
    }
  } catch (err) {
    log(`[stale-scan] WARNING: scan failed (best-effort, continuing): ${errMessage(err)}`);
  }
}

export async function main(): Promise<number> {
  const config = loadConfig();
  const rest = new AdoRest({ orgUrl: config.orgUrl, project: config.project, token: config.token, log });
  const mirrorUrl = mirrorRepoUrl(config.orgUrl, config.project, config.mirrorRepo);

  log(
    `smoke-e2e: build #${config.buildId}, mode '${config.compilerSource}', mirror '${config.mirrorRepo}'`,
  );

  // ---- Artifact visibility gate — candidate mode only, before any git work ----
  // Released mode compiles with a downloaded release asset and publishes no
  // candidate artifact, so there is nothing to gate on.
  if (config.compilerSource === "candidate") {
    await rest.getArtifact(config.buildId, config.artifactName);
    log(`[artifact-visibility] '${config.artifactName}' is visible on build #${config.buildId}`);
  }

  const worktreeParent = await mkdtemp(join(tmpdir(), "ado-aw-smoke-"));
  const worktreeDir = join(worktreeParent, "candidate");

  // Refs actually pushed, so cleanup never touches a ref we failed to create.
  const pushedRefs = new Map<string, string>();
  let overallOk = true;
  // Whether we reached the point where builds may have been queued. Only
  // trustworthy because it is set immediately before `runFixtures`; see there.
  let queueAttempted = false;
  // Placeholder only: never trusted directly. It's forced to `false`
  // immediately before `runFixtures` is invoked and only ever set back from
  // that call's own returned outcome.
  let allTerminal = true;
  let failureMessage: string | undefined;
  let results: FixtureBuildResult[] = [];
  let resolved: ResolvedCases | undefined;

  try {
    // The detached worktree is based directly on the LOCALLY checked-out
    // BUILD_SOURCEVERSION — never fetched from the mirror. For a GitHub PR
    // build, BUILD_SOURCEBRANCH is a synthetic ref (e.g.
    // `refs/pull/<n>/merge`) that does not exist on the ADO mirror repo; the
    // self checkout at BUILD_SOURCESDIRECTORY already has every object this
    // build needs. Only the resulting candidate commits are pushed TO the
    // mirror below.
    await verifyLocalCommit({
      cwd: config.sourcesDirectory,
      expectedSha: config.sourceVersion,
      timeoutMs: config.childTimeoutMs,
    });
    await createDetachedWorktree({
      cwd: config.sourcesDirectory,
      worktreeDir,
      commitish: config.sourceVersion,
      timeoutMs: config.childTimeoutMs,
    });

    // Read the manifest from the worktree (an exact checkout of
    // BUILD_SOURCEVERSION), never from the possibly-divergent
    // BUILD_SOURCESDIRECTORY.
    resolved = await loadCases(worktreeDir, process.env, config.compilerSource);
    log(
      `[cases] ${resolved.cases.length} case(s) for mode '${resolved.mode}': ${resolved.cases
        .map((entry) => `${entry.id}(${entry.lane})`)
        .join(", ")}`,
    );

    const ownRefs = new Set(resolved.cases.map((entry) => candidateRef(config.buildId, entry.id)));
    await cleanupStaleRefs(config, resolved, rest, mirrorUrl, ownRefs);

    const staged = await stageAllCases(config, resolved, worktreeDir, mirrorUrl, (caseId, ref) => {
      pushedRefs.set(caseId, ref);
    });

    const requests: FixtureBuildRequest[] = resolved.cases.map((entry) => ({
      caseId: entry.id,
      lane: entry.lane,
      definitionId: entry.definitionId,
      sourceBranch: staged.get(entry.id)!.ref,
      sourceVersion: staged.get(entry.id)!.sha,
      tags: [`smoke-case:${entry.id}`, `smoke-candidate:${config.buildId}`],
    }));

    // Fail-closed: set immediately before the call that might queue builds, so
    // an unexpected throw out of `runFixtures` itself (a runner bug, not a
    // reported build failure) can never be mistaken for "nothing was queued"
    // and delete refs out from under builds that may be running.
    queueAttempted = true;
    allTerminal = false;
    const outcome = await runFixtures(rest, requests, {
      concurrency: config.concurrency,
      timeoutMs: config.childTimeoutMs,
      pollMs: config.pollMs,
      log,
    });
    const signalOutcome = await verifyCaseSignals(rest, resolved.cases, outcome.results);
    results = signalOutcome.results;
    overallOk = outcome.ok && signalOutcome.ok;
    allTerminal = outcome.allTerminal;
    if (!overallOk) failureMessage = "one or more smoke cases did not succeed";
    if (!allTerminal) {
      overallOk = false;
      failureMessage = [
        failureMessage,
        "could not confirm every case build reached a terminal state — retaining its ref for the startup stale-ref scanner to clean up once ADO confirms completion",
      ]
        .filter(Boolean)
        .join("; ");
    }
  } catch (err) {
    overallOk = false;
    failureMessage = errMessage(err);
    log(`FAILED: ${failureMessage}`);
  } finally {
    // Per-case cleanup: delete a ref only when THAT case's terminal state was
    // positively proven. One unproven case no longer strands every other
    // case's ref, as it did when all cases shared one ref.
    const provenById = new Map(results.map((result) => [result.caseId, result.terminalProven]));
    const deletable: string[] = [];
    const retained: string[] = [];
    for (const [caseId, ref] of pushedRefs) {
      // A pushed case with no result is only safe to clean up if we never got
      // as far as queueing. If queueing was attempted, a missing result means
      // `runFixtures` threw and a build may still be running — fail closed and
      // let the stale-ref scanner reclaim it once ADO can prove it stopped.
      const proven = provenById.get(caseId) ?? !queueAttempted;
      (proven ? deletable : retained).push(ref);
    }
    if (deletable.length > 0) {
      try {
        await deleteRemoteRefs({
          cwd: config.sourcesDirectory,
          mirrorUrl,
          refs: deletable,
          token: config.token,
          timeoutMs: config.childTimeoutMs,
        });
        log(`[git] deleted ${deletable.length} candidate ref(s)`);
      } catch (err) {
        overallOk = false;
        failureMessage ??= `failed to delete candidate ref(s): ${errMessage(err)}`;
        log(`WARNING: failed to delete candidate ref(s): ${errMessage(err)}`);
      }
    }
    for (const ref of retained) {
      log(`WARNING: retaining ${ref} because its build's terminal state could not be confirmed`);
    }

    try {
      await removeWorktree({
        cwd: config.sourcesDirectory,
        worktreeDir,
        timeoutMs: config.childTimeoutMs,
      });
    } catch (err) {
      overallOk = false;
      failureMessage ??= `failed to remove worktree: ${errMessage(err)}`;
      log(`WARNING: failed to remove worktree ${worktreeDir}: ${errMessage(err)}`);
    }
    await rm(worktreeParent, { recursive: true, force: true }).catch(() => {});
  }

  if (results.length > 0) {
    log("");
    log("=== Smoke E2E results ===");
    log(renderResultsTable(results));
  }
  if (failureMessage) {
    log(`Overall: FAILED — ${failureMessage}`);
  } else {
    log("Overall: PASSED");
  }

  return overallOk ? 0 : 1;
}

// Run as the bundle entry point. Skipped under Vitest so unit tests can
// import these modules without launching the whole suite.
if (process.env.VITEST !== "true") {
  main().then(
    (code) => process.exit(code),
    (err: unknown) => {
      const e = err as Error;
      log(`smoke-e2e crashed: ${e.stack ?? e.message}`);
      process.exit(1);
    },
  );
}
