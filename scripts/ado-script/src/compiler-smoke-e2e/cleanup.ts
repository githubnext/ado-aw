import type { OwnedBoundaryPr } from "./ado-rest.js";
import { boundaryTargetRef } from "./config.js";
import { parseCandidateRef, type RemoteRef } from "./git.js";

interface CleanupClient {
  findBoundaryPr(repo: string, source: string): Promise<OwnedBoundaryPr | undefined>;
  abandonBoundaryPr(repo: string, pr: OwnedBoundaryPr): Promise<void>;
}

/** Caller must prove the source's child builds terminal before entering cleanup. */
export async function cleanupCaseResources(opts: {
  client: CleanupClient;
  repository: string;
  sourceRef: string;
  refs: readonly RemoteRef[];
  observedRefs?: readonly RemoteRef[];
  expectedPrId?: number;
  deleteRefs: (refs: readonly RemoteRef[]) => Promise<void>;
}): Promise<void> {
  const identity = parseCandidateRef(opts.sourceRef);
  if (!identity) throw new Error("Case cleanup requires an owned source ref");
  const pr = await opts.client.findBoundaryPr(opts.repository, opts.sourceRef);
  if (pr && (opts.observedRefs ?? opts.refs).some((entry) => entry.ref === pr.targetRefName)
    && !opts.refs.some((entry) => entry.ref === pr.targetRefName)) {
    throw new Error("Boundary target belongs to an unproven or ambiguous resource group; retaining both refs");
  }
  if (identity.caseId.endsWith("-target")) {
    const possibleParent = opts.sourceRef.slice(0, -"-target".length);
    const parentPr = await opts.client.findBoundaryPr(opts.repository, possibleParent);
    if (parentPr?.targetRefName === opts.sourceRef) {
      throw new Error("Source ref is also a legacy boundary target; retaining ambiguous resources");
    }
  }
  if (opts.expectedPrId !== undefined && pr?.pullRequestId !== opts.expectedPrId) {
    throw new Error("Known boundary PR could not be recovered; retaining both refs");
  }
  const target = boundaryTargetRef(identity.buildId, identity.caseId);
  for (const { ref } of opts.refs) {
    if (ref !== opts.sourceRef && ref !== target && ref !== pr?.targetRefName) {
      throw new Error(`Cannot establish ownership of paired ref ${ref}`);
    }
  }
  if (pr) await opts.client.abandonBoundaryPr(opts.repository, pr);
  await opts.deleteRefs(opts.refs);
}
