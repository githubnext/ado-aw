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
  CommentCreate,
  Wiql,
  WorkItem,
  WorkItemRelation,
} from "azure-devops-node-api/interfaces/WorkItemTrackingInterfaces.js";
import type {
  JsonPatchDocument,
  ResourceRef,
} from "azure-devops-node-api/interfaces/common/VSSInterfaces.js";

/** Configuration for filing or appending to a diagnostic work item. */
export interface WorkItemReportConfig {
  enabled: boolean;
  title?: string;
  workItemType: string;
  areaPath?: string;
  iterationPath?: string;
  tags: string[];
  includeStats: boolean;
}

/** Outcome of a `fileOrAppendWorkItem` operation. */
export interface FileOrAppendResult {
  action: "created" | "appended" | "skipped";
  workItemId?: number;
  commentId?: number;
  message: string;
}

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

/**
 * Search for a non-closed work item by exact title using WIQL.
 *
 * Returns the most-recently-changed matching work-item ID, or `null`
 * when no active work item with the same title exists.
 */
export async function findWorkItemByTitle(
  project: string,
  title: string,
): Promise<number | null> {
  return withRetry("findWorkItemByTitle", async () => {
    const wit = await (await getWebApi()).getWorkItemTrackingApi();
    const escapedTitle = title.replaceAll("'", "''");
    const wiql: Wiql = {
      query:
        `SELECT [System.Id] FROM WorkItems ` +
        `WHERE [System.Title] = '${escapedTitle}' ` +
        `AND [System.TeamProject] = @project ` +
        `AND [System.State] NOT IN ('Closed', 'Resolved', 'Done') ` +
        `ORDER BY [System.ChangedDate] DESC`,
    };
    const result = await wit.queryByWiql(wiql, { project });
    const id = result.workItems?.[0]?.id;
    return typeof id === "number" ? id : null;
  });
}

/**
 * Create a new work item from a flat field map.
 *
 * Uses the SDK's JSON Patch document format and always marks
 * `System.Description` as Markdown.
 */
export async function createWorkItem(
  project: string,
  type: string,
  fields: Record<string, string>,
): Promise<{ id: number; url: string }> {
  return withRetry("createWorkItem", async () => {
    const wit = await (await getWebApi()).getWorkItemTrackingApi();
    const patch = [
      ...Object.entries(fields).map(([fieldName, fieldValue]) => ({
        op: "add",
        path: `/fields/${fieldName}`,
        value: fieldValue,
      })),
      {
        op: "add",
        path: "/multilineFieldsFormat/System.Description",
        value: "Markdown",
      },
    ] as unknown as JsonPatchDocument;
    const created = await wit.createWorkItem(
      { "Content-Type": "application/json-patch+json" },
      patch,
      project,
      `$${type}`,
    );
    if (typeof created.id !== "number") {
      throw new Error("createWorkItem returned a work item without a numeric id");
    }
    const url =
      (created._links as { html?: { href?: string } } | undefined)?.html?.href ??
      String(created.url ?? "");
    return { id: created.id, url };
  });
}

/**
 * Add a comment to an existing work item.
 *
 * Uses the work-item comments endpoint via the SDK's `addComment`
 * wrapper and returns the created comment identifier.
 */
export async function addWorkItemComment(
  project: string,
  workItemId: number,
  text: string,
): Promise<{ commentId: number }> {
  return withRetry("addWorkItemComment", async () => {
    const wit = await (await getWebApi()).getWorkItemTrackingApi();
    const request: CommentCreate = { text };
    const comment = await wit.addComment(request, project, workItemId);
    if (typeof comment.id !== "number") {
      throw new Error("addWorkItemComment returned a comment without a numeric id");
    }
    return { commentId: comment.id };
  });
}

/**
 * File a new work item or append a comment to an existing one.
 *
 * Mirrors the Rust `file_or_append_work_item()` helper: exact-title
 * matches append to the newest active work item; otherwise a new work
 * item is created with the supplied description body.
 */
export async function fileOrAppendWorkItem(
  project: string,
  config: WorkItemReportConfig,
  defaultTitle: string,
  body: string,
): Promise<FileOrAppendResult> {
  if (!config.enabled) {
    return {
      action: "skipped",
      message: "Work-item filing disabled via enabled: false",
    };
  }

  const title = config.title ?? defaultTitle;
  const existingId = await findWorkItemByTitle(project, title);

  if (existingId !== null) {
    const { commentId } = await addWorkItemComment(project, existingId, body);
    return {
      action: "appended",
      workItemId: existingId,
      commentId,
      message: `Appended comment #${commentId} to existing work item #${existingId}: ${title}`,
    };
  }

  const fields: Record<string, string> = {
    "System.Title": title,
    "System.Description": body,
  };
  if (config.areaPath) {
    fields["System.AreaPath"] = config.areaPath;
  }
  if (config.iterationPath) {
    fields["System.IterationPath"] = config.iterationPath;
  }
  if (config.tags.length > 0) {
    fields["System.Tags"] = config.tags.join("; ");
  }

  const created = await createWorkItem(project, config.workItemType, fields);
  return {
    action: "created",
    workItemId: created.id,
    message: `Created work item #${created.id}: ${title}`,
  };
}
