/**
 * Startup stale-ref scanner for leftover `refs/heads/ado-aw-smoke-candidate/*`
 * branches on the mirror repo (e.g. left behind by a run that crashed before
 * its own cleanup).
 *
 * Deliberately conservative: a candidate ref is only ever deleted when this
 * scanner can PROVE, via the ADO Build REST API, that (a) the ref name
 * encodes a build id of THIS orchestrator's own definition
 * (`SYSTEM_DEFINITIONID`), (b) that build is old enough
 * (`COMPILER_SMOKE_STALE_REF_HOURS`), and (c) that parent build is
 * terminal. Note that (c) is NOT by itself proof the orchestration it
 * started is done — an abruptly canceled/killed parent process can reach a
 * terminal ADO build status while its fixture builds are still running. The
 * scanner therefore also queries each fixed
 * child definitions on the ref's exact branch (see
 * `listBuildsForDefinitionBranch`) and inspects their statuses directly;
 * only when every child build found there is ALSO terminal (or none exist)
 * is a ref considered `"eligible"` for deletion. Any active child, or any
 * error looking one up, marks the ref `"active"`/`"ambiguous"` instead.
 *
 * Every other case (unparseable ref, build not found, build belongs to a
 * different definition, or any lookup error) is reported but never deleted
 * — a fail-closed posture is preferred over a plausible-but-unverifiable
 * guess at another run's identity.
 *
 * Test-harness module; not shipped in `ado-script.zip`.
 */
import { parseCandidateBuildId, type RemoteRef } from "./git.js";

export type StaleRefOutcome = "eligible" | "too-recent" | "active" | "ambiguous";

export interface StaleRefDecision {
  ref: string;
  sha: string;
  outcome: StaleRefOutcome;
  reason: string;
}

export interface StaleScanBuild {
  status?: string;
  result?: string;
  definition?: { id?: number };
  finishTime?: string;
  queueTime?: string;
}

export interface StaleScanClient {
  getBuild(buildId: number): Promise<StaleScanBuild>;
  /** List builds of `definitionId` on the exact candidate `branch` (see {@link AdoRest.listBuildsForDefinitionBranch}). */
  listBuildsForDefinitionBranch(definitionId: number, branch: string): Promise<StaleScanBuild[]>;
}

export interface ScanStaleRefsOptions {
  refs: readonly RemoteRef[];
  /** The checked-out base ref — defensive exclusion (should already be outside the candidate prefix). */
  baseRef: string;
  /** This run's own about-to-be-created candidate ref — never treated as stale. */
  ownRef: string;
  /** This orchestrator pipeline's own definition id (SYSTEM_DEFINITIONID). */
  definitionId: number;
  /**
   * Every fixed fixture ("child") pipeline definition id the orchestrator
   * queues builds against. An orchestrator run completing (even abruptly,
   * e.g. cancelled) does NOT prove these have also finished — they are
   * independently queued builds. A candidate ref is only ever eligible for
   * deletion once none of these definitions has a still-active build on
   * that ref's exact branch.
   */
  childDefinitionIds: readonly number[];
  staleRefHours: number;
  client: StaleScanClient;
  /** Injectable clock for deterministic tests. */
  now?: () => number;
}

function parseTimestampMs(value: string | undefined): number | undefined {
  if (!value) return undefined;
  const ms = Date.parse(value);
  return Number.isNaN(ms) ? undefined : ms;
}

/** Evaluate every candidate ref and classify it. Never mutates anything. */
export async function scanStaleRefs(opts: ScanStaleRefsOptions): Promise<StaleRefDecision[]> {
  const now = opts.now ?? (() => Date.now());
  const thresholdMs = opts.staleRefHours * 60 * 60 * 1000;
  const decisions: StaleRefDecision[] = [];

  for (const { ref, sha } of opts.refs) {
    if (ref === opts.baseRef || ref === opts.ownRef) continue;

    const buildId = parseCandidateBuildId(ref);
    if (buildId === undefined) {
      decisions.push({
        ref,
        sha,
        outcome: "ambiguous",
        reason: "ref name does not match the expected <prefix>/<buildId> pattern",
      });
      continue;
    }

    let build: StaleScanBuild;
    try {
      build = await opts.client.getBuild(buildId);
    } catch (err) {
      decisions.push({
        ref,
        sha,
        outcome: "ambiguous",
        reason: `orchestrator build #${buildId} lookup failed: ${
          err instanceof Error ? err.message : String(err)
        }`,
      });
      continue;
    }

    if (build.definition?.id !== opts.definitionId) {
      decisions.push({
        ref,
        sha,
        outcome: "ambiguous",
        reason: `build #${buildId} belongs to definition ${build.definition?.id ?? "?"}, not this orchestrator's own definition ${opts.definitionId}`,
      });
      continue;
    }

    if (build.status !== "completed") {
      decisions.push({
        ref,
        sha,
        outcome: "active",
        reason: `orchestrator build #${buildId} is still ${build.status ?? "in an unknown state"}`,
      });
      continue;
    }

    const finishedAtMs = parseTimestampMs(build.finishTime) ?? parseTimestampMs(build.queueTime);
    if (finishedAtMs === undefined) {
      decisions.push({
        ref,
        sha,
        outcome: "ambiguous",
        reason: `orchestrator build #${buildId} is completed but has no usable finishTime/queueTime`,
      });
      continue;
    }

    const ageMs = now() - finishedAtMs;
    if (ageMs < thresholdMs) {
      decisions.push({
        ref,
        sha,
        outcome: "too-recent",
        reason: `orchestrator build #${buildId} finished ${Math.round(ageMs / 3_600_000)}h ago, below the ${opts.staleRefHours}h threshold`,
      });
      continue;
    }

    // The orchestrator's own run is old enough and terminal, but that alone
    // does not prove the fixture ("child") builds it queued on this
    // exact branch have also finished — an abruptly cancelled orchestrator
    // run can "complete" while its queued children keep running. Check
    // every fixed child definition on this exact branch before declaring
    // the ref deletable; any lookup failure or non-completed child build
    // fails closed.
    let childLookupError: string | undefined;
    let activeChildDefinitionId: number | undefined;
    for (const childDefinitionId of opts.childDefinitionIds) {
      let childBuilds: StaleScanBuild[];
      try {
        childBuilds = await opts.client.listBuildsForDefinitionBranch(childDefinitionId, ref);
      } catch (err) {
        childLookupError = `child definition ${childDefinitionId} build lookup on ${ref} failed: ${
          err instanceof Error ? err.message : String(err)
        }`;
        break;
      }
      if (childBuilds.some((b) => b.status !== "completed")) {
        activeChildDefinitionId = childDefinitionId;
        break;
      }
    }

    if (childLookupError) {
      decisions.push({ ref, sha, outcome: "ambiguous", reason: childLookupError });
      continue;
    }

    if (activeChildDefinitionId !== undefined) {
      decisions.push({
        ref,
        sha,
        outcome: "active",
        reason: `fixture definition ${activeChildDefinitionId} still has a non-completed build on ${ref}`,
      });
      continue;
    }

    decisions.push({
      ref,
      sha,
      outcome: "eligible",
      reason: `orchestrator build #${buildId} completed ${Math.round(ageMs / 3_600_000)}h ago and no fixture build is active on ${ref}`,
    });
  }

  return decisions;
}
