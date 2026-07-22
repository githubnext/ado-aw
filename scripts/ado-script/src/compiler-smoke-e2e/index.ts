/**
 * Entry point for the deterministic compiler-smoke E2E orchestrator.
 *
 * Stages the compiler candidate produced by the current build (PR or
 * nightly `main`) as a pinned `supply-chain.pipeline-artifact` source across
 * the five real fixtures documented in `tests/safe-outputs/README.md`
 * (canary, azure-cli, noop-target, janitor, smoke-failure-reporter), pushes
 * the staged candidate to a short-lived branch on the mirror repo, queues
 * the five FIXED "candidate lane" pipeline definitions (tracked in
 * `tests/compiler-smoke-e2e/REGISTERED.md`), and asserts they all go green.
 *
 * See `config.ts` for the full required/optional env var contract.
 *
 * Test-harness module; not shipped in `ado-script.zip`.
 */
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { AdoRest } from "./ado-rest.js";
import { assertNoForbiddenReleaseUrls, assertPipelineArtifactValues } from "./assertions.js";
import { candidateRef, FIXTURE_NAMES, loadConfig, type CompilerSmokeConfig } from "./config.js";
import { compileAndCheck } from "./compile-cli.js";
import { ALL_FIXTURES, allowedChangedPaths } from "./fixtures.js";
import {
  commitAll,
  createDetachedWorktree,
  deleteRemoteRef,
  disallowedChanges,
  listCandidateRefs,
  mirrorRepoUrl,
  pushCandidate,
  removeWorktree,
  verifyLocalCommit,
  verifyRemoteRef,
  worktreeChangedFiles,
} from "./git.js";
import { injectPipelineArtifact } from "./source.js";
import { renderResultsTable } from "./report.js";
import { runFixtures, type FixtureBuildRequest, type FixtureBuildResult } from "./runner.js";
import { scanStaleRefs } from "./stale.js";

function log(msg: string): void {
  // Percent-encode a leading '#' so a message cannot smuggle a ##vso command.
  process.stdout.write(msg.replace(/^#/gm, "%23") + "\n");
}

function errMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

/** Read each fixture's markdown directly from the detached candidate worktree (an exact checkout of BUILD_SOURCEVERSION — never the possibly-divergent BUILD_SOURCESDIRECTORY), then apply the pipeline-artifact transform in place. */
async function stageFixtures(config: CompilerSmokeConfig, worktreeDir: string): Promise<void> {
  for (const fixture of ALL_FIXTURES) {
    const selfContent = await readFile(join(worktreeDir, fixture.relMd), "utf8");
    const transformed = injectPipelineArtifact(selfContent, {
      project: config.project,
      definitionId: config.definitionId,
      runId: config.buildId,
      artifact: config.artifactName,
    });
    await writeFile(join(worktreeDir, fixture.relMd), transformed, "utf8");
  }
}

async function compileFixtures(config: CompilerSmokeConfig, worktreeDir: string): Promise<void> {
  for (const fixture of ALL_FIXTURES) {
    const result = await compileAndCheck({
      adoAwBin: config.adoAwBin,
      worktreeDir,
      relMd: fixture.relMd,
      relLock: fixture.relLock,
      timeoutMs: config.childTimeoutMs,
      secrets: [config.token],
    });
    if (!result.ok) {
      throw new Error(
        `fixture '${fixture.name}' ${result.phase} failed: ${result.message}\n--- stdout ---\n${result.stdout}\n--- stderr ---\n${result.stderr}`,
      );
    }

    const yamlText = await readFile(join(worktreeDir, fixture.relLock), "utf8");
    assertNoForbiddenReleaseUrls(yamlText, fixture.name);
    assertPipelineArtifactValues(yamlText, fixture.name, {
      project: config.project,
      pipeline: String(config.definitionId),
      runId: String(config.buildId),
      artifact: config.artifactName,
    });
  }
}

async function cleanupStaleRefs(config: CompilerSmokeConfig, rest: AdoRest, mirrorUrl: string, ownRef: string): Promise<void> {
  try {
    const refs = await listCandidateRefs({
      cwd: config.sourcesDirectory,
      mirrorUrl,
      token: config.token,
      timeoutMs: config.childTimeoutMs,
    });
    const decisions = await scanStaleRefs({
      refs,
      baseRef: config.sourceBranch,
      ownRef,
      definitionId: config.definitionId,
      childDefinitionIds: FIXTURE_NAMES.map((name) => config.definitionIds[name]),
      staleRefHours: config.staleRefHours,
      client: rest,
    });
    for (const decision of decisions) {
      if (decision.outcome !== "eligible") {
        log(`[stale-scan] ${decision.ref}: ${decision.outcome} — ${decision.reason}`);
        continue;
      }
      try {
        await deleteRemoteRef({
          cwd: config.sourcesDirectory,
          mirrorUrl,
          ref: decision.ref,
          token: config.token,
          timeoutMs: config.childTimeoutMs,
        });
        log(`[stale-scan] deleted ${decision.ref}: ${decision.reason}`);
      } catch (err) {
        log(`[stale-scan] WARNING: failed to delete ${decision.ref}: ${errMessage(err)}`);
      }
    }
  } catch (err) {
    log(`[stale-scan] WARNING: scan failed (best-effort, continuing): ${errMessage(err)}`);
  }
}

export async function main(): Promise<number> {
  const config = loadConfig();
  const rest = new AdoRest({ orgUrl: config.orgUrl, project: config.project, token: config.token, log });
  const mirrorUrl = mirrorRepoUrl(config.orgUrl, config.project, config.mirrorRepo);
  const ownRef = candidateRef(config.buildId);

  log(
    `compiler-smoke-e2e: build #${config.buildId}, candidate ref ${ownRef}, mirror '${config.mirrorRepo}'`,
  );

  // ---- Artifact visibility gate — before any source/git work or queueing ----
  await rest.getArtifact(config.buildId, config.artifactName);
  log(`[artifact-visibility] '${config.artifactName}' is visible on build #${config.buildId}`);

  // ---- Best-effort startup stale-ref cleanup ----
  await cleanupStaleRefs(config, rest, mirrorUrl, ownRef);

  const worktreeParent = await mkdtemp(join(tmpdir(), "ado-aw-compiler-smoke-"));
  const worktreeDir = join(worktreeParent, "candidate");

  let pushed = false;
  let overallOk = true;
  // Placeholder only: never trusted directly. It's forced to `false`
  // immediately before `runFixtures` is invoked (see below) and only ever
  // set back to `true` from that call's own returned outcome.
  let allChildrenTerminal = true;
  let failureMessage: string | undefined;
  let results: FixtureBuildResult[] = [];

  try {
    // The detached worktree is based directly on the LOCALLY checked-out
    // BUILD_SOURCEVERSION — never fetched from the mirror. For a GitHub PR
    // build, BUILD_SOURCEBRANCH is a synthetic ref (e.g.
    // `refs/pull/<n>/merge`) that does not exist on the ADO mirror repo; the
    // self checkout at BUILD_SOURCESDIRECTORY already has every object this
    // build needs. Only the resulting candidate commit is pushed TO the
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

    await stageFixtures(config, worktreeDir);
    await compileFixtures(config, worktreeDir);

    const changed = await worktreeChangedFiles({ worktreeDir, timeoutMs: config.childTimeoutMs });
    const violations = disallowedChanges(changed, allowedChangedPaths());
    if (violations.length > 0) {
      throw new Error(`refusing to push: unexpected path(s) changed: ${violations.join(", ")}`);
    }

    const candidateSha = await commitAll({
      worktreeDir,
      buildId: config.buildId,
      timeoutMs: config.childTimeoutMs,
    });
    await pushCandidate({
      worktreeDir,
      mirrorUrl,
      ref: ownRef,
      token: config.token,
      timeoutMs: config.childTimeoutMs,
    });
    pushed = true;
    await verifyRemoteRef({
      cwd: worktreeDir,
      mirrorUrl,
      ref: ownRef,
      expectedSha: candidateSha,
      token: config.token,
      timeoutMs: config.childTimeoutMs,
    });
    log(`[git] candidate ${candidateSha} pushed to ${ownRef}`);

    const requests: FixtureBuildRequest[] = FIXTURE_NAMES.map((name) => ({
      name,
      definitionId: config.definitionIds[name],
      sourceBranch: ownRef,
      sourceVersion: candidateSha,
    }));
    // Fail-closed: flip to `false` right before the call that might queue
    // builds, so an unexpected throw out of `runFixtures` itself (a runner
    // bug, not a reported build failure) can never leave this at its
    // initial `true` and delete the ref out from under a build that may
    // have been queued. Only a normally-returned outcome is trusted to set
    // this back to `true`.
    allChildrenTerminal = false;
    const outcome = await runFixtures(rest, requests, {
      concurrency: config.concurrency,
      timeoutMs: config.childTimeoutMs,
      pollMs: config.pollMs,
      log,
    });
    results = outcome.results;
    overallOk = outcome.ok;
    allChildrenTerminal = outcome.allTerminal;
    if (!overallOk) failureMessage = "one or more fixture builds did not succeed";
    if (!allChildrenTerminal) {
      overallOk = false;
      failureMessage = [
        failureMessage,
        `could not confirm every fixture build reached a terminal state — retaining ${ownRef} for the startup stale-ref scanner to clean up once ADO confirms completion`,
      ]
        .filter(Boolean)
        .join("; ");
    }
  } catch (err) {
    overallOk = false;
    failureMessage = errMessage(err);
    log(`FAILED: ${failureMessage}`);
  } finally {
    if (pushed) {
      if (allChildrenTerminal) {
        try {
          await deleteRemoteRef({
            cwd: config.sourcesDirectory,
            mirrorUrl,
            ref: ownRef,
            token: config.token,
            timeoutMs: config.childTimeoutMs,
          });
        } catch (err) {
          overallOk = false;
          failureMessage ??= `failed to delete candidate ref ${ownRef}: ${errMessage(err)}`;
          log(`WARNING: failed to delete candidate ref ${ownRef}: ${errMessage(err)}`);
        }
      } else {
        // Never delete a ref while any queued build might still be
        // running against it — retain it and let the fail-closed
        // stale-ref scanner reclaim it on a later run once it can prove
        // every child build actually terminated.
        log(
          `WARNING: retaining candidate ref ${ownRef} because not every fixture build's terminal state could be confirmed`,
        );
      }
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
    log("=== Compiler smoke E2E results ===");
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
      log(`compiler-smoke-e2e crashed: ${e.stack ?? e.message}`);
      process.exit(1);
    },
  );
}
