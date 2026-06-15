/**
 * Work-item REST helpers.
 *
 * Introduced in Stage 4 of the execution-context contributor build-out
 * (plan.md). Used by the `workitem` contributor (PR-linked mode) to
 * fetch the work items linked to a PR plus per-WI details
 * (description, acceptance criteria, repro steps, comments, links,
 * attachments) needed for an acceptance-criteria-aware PR review.
 *
 * Wraps the existing `withRetry` machinery from `shared/ado-client.ts`
 * for transient-error resilience; raw fetch failures propagate to the
 * caller so the bundle's failure-fragment path can stage error.txt.
 */
import { getWebApi } from "./auth.js";
import { withRetry } from "./ado-client.js";
import type {
  WorkItem,
  WorkItemRelation,
} from "azure-devops-node-api/interfaces/WorkItemTrackingInterfaces.js";
import type { ResourceRef } from "azure-devops-node-api/interfaces/common/VSSInterfaces.js";

/**
 * List the work-item identifiers linked to a PR.
 *
 * Uses the Git API's `getPullRequestWorkItemRefs` endpoint, which
 * returns lightweight `{id, url}` refs (no body). Callers then
 * fan out to `getWorkItem(id)` to retrieve full details for each.
 *
 * Returns an empty array when the PR has no linked work items; the
 * caller stages that case explicitly with an informational
 * fragment (NOT an error — many PRs legitimately have no WI link).
 */
export async function listPullRequestWorkItems(
  project: string,
  repositoryId: string,
  pullRequestId: number,
): Promise<ResourceRef[]> {
  return withRetry("listPullRequestWorkItems", async () => {
    const git = await (await getWebApi()).getGitApi();
    return git.getPullRequestWorkItemRefs(repositoryId, pullRequestId, project);
  });
}

/**
 * Fetch a single work item with full field expansion.
 *
 * Expands all fields the contributor cares about: System.Title,
 * System.WorkItemType, System.State, System.Description,
 * Microsoft.VSTS.Common.AcceptanceCriteria, System.Tags,
 * Microsoft.VSTS.TCM.ReproSteps, System.History (comments are
 * fetched separately via `getComments`), and the relations.
 */
export async function getWorkItem(
  project: string,
  workItemId: number,
): Promise<WorkItem> {
  return withRetry("getWorkItem", async () => {
    const wit = await (await getWebApi()).getWorkItemTrackingApi();
    // SDK signature: getWorkItem(id, fields?, asOf?, expand?, project?)
    // expand=4 == WorkItemExpand.All — pulls all fields + relations.
    // We avoid the typed enum import (saves bundle bytes) and pass
    // the numeric value directly; ADO has used the same enum values
    // since the WIT API was introduced.
    return wit.getWorkItem(
      workItemId,
      undefined, // fields
      undefined, // asOf
      4, // expand: All
      project,
    );
  });
}

/**
 * Fetch the comments for a work item, oldest-first.
 *
 * Returns the raw comment text — callers wrap it via
 * `untrusted.wrapAgentReadableUntrusted` before staging because
 * comments are user-authored prose.
 */
export async function getWorkItemComments(
  project: string,
  workItemId: number,
): Promise<{ comments: { text?: string; createdBy?: { displayName?: string }; createdDate?: Date }[] }> {
  return withRetry("getWorkItemComments", async () => {
    const wit = await (await getWebApi()).getWorkItemTrackingApi();
    // `getComments` is paged on the server; the SDK convenience
    // method already handles a single page (top=200 default), which
    // is plenty for the WI cap the contributor enforces.
    const result = await wit.getComments(project, workItemId);
    return {
      comments: (result.comments ?? []).map((c) => ({
        text: c.text,
        createdBy: c.createdBy
          ? { displayName: c.createdBy.displayName }
          : undefined,
        createdDate: c.createdDate,
      })),
    };
  });
}

/** Convenience extractor: walk a `WorkItem.relations[]` and return
 * the link metadata grouped by category so the contributor can
 * stage `links.json` in a stable shape. Pure function — no REST. */
export function summariseRelations(
  relations: WorkItemRelation[] | undefined,
): { rel: string; url: string; attributes?: Record<string, unknown> }[] {
  if (!relations) return [];
  return relations.map((r) => ({
    rel: r.rel ?? "",
    url: r.url ?? "",
    attributes: r.attributes as Record<string, unknown> | undefined,
  }));
}
