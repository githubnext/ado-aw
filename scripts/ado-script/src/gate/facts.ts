/**
 * Acquire runtime facts referenced by the gate spec.
 *
 * Pipeline-variable facts
 * come from `process.env`; PR-derived facts come from the ADO REST API
 * via `shared/ado-client.ts`.
 */
import type { GateSpec, FactSpec } from "../shared/types.gen.js";
import type { PolicyTracker } from "../shared/policy.js";
import { readEnvFact, isPipelineVarFact } from "../shared/env-facts.js";
import * as adoClient from "../shared/ado-client.js";

export async function acquireFacts(
  spec: GateSpec,
  tracker: PolicyTracker,
): Promise<Map<string, unknown>> {
  const facts = new Map<string, unknown>();
  const project = process.env.ADO_PROJECT ?? "";
  const repoId = process.env.ADO_REPO_ID ?? "";
  const prIdRaw = process.env.ADO_PR_ID ?? "";
  const prId = prIdRaw ? Number(prIdRaw) : NaN;

  for (const fs of spec.facts) {
    if (tracker.isUnavailableForAcquisition(fs.kind)) {
      continue;
    }

    try {
      const value = await acquireOne(fs, facts, { project, repoId, prId });
      if (value === undefined) {
        tracker.recordFactFailure(fs.kind, "value undefined / missing env");
      } else {
        facts.set(fs.kind, value);
      }
    } catch (e) {
      tracker.recordFactFailure(fs.kind, (e as Error).message);
    }
  }

  return facts;
}

interface AdoCtx {
  project: string;
  repoId: string;
  prId: number;
}

async function acquireOne(
  fs: FactSpec,
  facts: Map<string, unknown>,
  ctx: AdoCtx,
): Promise<unknown> {
  const kind = fs.kind;
  if (isPipelineVarFact(kind)) {
    return readEnvFact(kind);
  }

  switch (kind) {
    case "pr_metadata": {
      requireAdoCtx(ctx, "pr_metadata");
      return adoClient.getPullRequestById(ctx.project, ctx.repoId, ctx.prId);
    }
    case "pr_is_draft": {
      const md = facts.get("pr_metadata") as { isDraft?: boolean } | undefined;
      if (!md) return undefined;
      return md.isDraft ? "true" : "false";
    }
    case "pr_labels": {
      const md = facts.get("pr_metadata") as
        | { labels?: { name?: string }[] }
        | undefined;
      const labels = md?.labels ?? [];
      return labels.map((l) => l.name ?? "");
    }
    case "changed_files": {
      requireAdoCtx(ctx, "changed_files");
      const iters = await adoClient.getPullRequestIterations(
        ctx.project,
        ctx.repoId,
        ctx.prId,
      );
      if (!iters || iters.length === 0) return [];
      const last = iters[iters.length - 1]!;
      const lastId = last.id;
      if (typeof lastId !== "number") return [];
      const changes = await adoClient.getIterationChanges(
        ctx.project,
        ctx.repoId,
        ctx.prId,
        lastId,
      );
      const entries =
        (changes as { changeEntries?: Array<{ item?: { path?: string } }> })
          .changeEntries ?? [];
      return entries
        .map((e) => e.item?.path ?? "")
        .filter((p) => !!p)
        .map((p) => p.replace(/^\/+/, ""));
    }
    case "changed_file_count": {
      const cf = facts.get("changed_files");
      return Array.isArray(cf) ? cf.length : 0;
    }
    case "current_utc_minutes": {
      const now = new Date();
      return now.getUTCHours() * 60 + now.getUTCMinutes();
    }
    default:
      throw new Error(`Unknown fact kind: ${kind}`);
  }
}

function requireAdoCtx(ctx: AdoCtx, kind: string): void {
  if (!ctx.project || !ctx.repoId || !Number.isFinite(ctx.prId)) {
    throw new Error(
      `Missing ADO env vars (ADO_PROJECT/ADO_REPO_ID/ADO_PR_ID) required for fact '${kind}'`,
    );
  }
}
