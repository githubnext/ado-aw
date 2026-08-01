/**
 * Response-side policy.
 *
 * Two of the catalog's operations are unavoidably organization-scoped in their
 * URL — `az repos pr show` and `az boards work-item show` address a pull
 * request or work item by id alone — so the only place their project and
 * repository can be checked is the response body. Two more return a *list* of
 * things the agent is not scoped to see and must be filtered down.
 *
 * Filtering happens before a single byte reaches the agent: an out-of-scope
 * response is replaced by a denial, never truncated or partially forwarded.
 */
import { PROTECTED_HOSTS } from "./catalog.js";
import type { ProxyPolicy } from "./config.js";
import type { Operation, ResponsePolicy } from "../shared/ado-proxy-catalog.types.gen.js";

export type FilterOutcome =
  | { readonly kind: "forward"; readonly body: Buffer }
  | { readonly kind: "deny"; readonly detail: string };

function forward(body: Buffer): FilterOutcome {
  return { kind: "forward", body };
}

function denyBody(detail: string): FilterOutcome {
  return { kind: "deny", detail };
}

function reserialize(value: unknown): FilterOutcome {
  return forward(Buffer.from(JSON.stringify(value), "utf8"));
}

function sameIdentifier(left: unknown, right: string | undefined): boolean {
  return (
    typeof left === "string" &&
    right !== undefined &&
    left.toLowerCase() === right.toLowerCase()
  );
}

function isCurrentProject(value: unknown, policy: ProxyPolicy): boolean {
  return sameIdentifier(value, policy.project) || sameIdentifier(value, policy.project_id);
}

function isCurrentRepository(value: unknown, policy: ProxyPolicy): boolean {
  return (
    sameIdentifier(value, policy.repository) ||
    sameIdentifier(value, policy.repository_id)
  );
}

function asRecord(value: unknown): Record<string, unknown> | undefined {
  return typeof value === "object" && value !== null && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : undefined;
}

/** Extract the `value` array from an Azure DevOps list envelope. */
function listValues(document: Record<string, unknown>): unknown[] | undefined {
  return Array.isArray(document.value) ? (document.value as unknown[]) : undefined;
}

/**
 * Apply the operation's response policy.
 *
 * `json` is the common case and passes the body through unchanged; the body was
 * already size-bounded by the caller. Everything else either narrows the body
 * or refuses it.
 */
export function filterResponse(
  operation: Operation,
  policy: ProxyPolicy,
  body: Buffer,
  /**
   * Origin the client used to reach this proxy, e.g. `https://dev.azure.com`.
   *
   * Only the resource-area rewrite needs it: service locations must point back
   * at whatever origin the client is already talking to, which differs between
   * the intercepted MCP path and the `az` broker path.
   */
  selfOrigin: string,
): FilterOutcome {
  const responsePolicy: ResponsePolicy = operation.response;
  if (responsePolicy === "json") return forward(body);

  let document: unknown;
  try {
    document = JSON.parse(body.toString("utf8"));
  } catch {
    // A scope-validated operation whose body cannot be parsed cannot be
    // validated, so it cannot be forwarded.
    return denyBody("upstream response was not parseable JSON");
  }

  const record = asRecord(document);
  if (record === undefined) {
    return denyBody("upstream response was not a JSON object");
  }

  switch (responsePolicy) {
    case "filter-projects": {
      const values = listValues(record);
      if (values === undefined) return denyBody("project list had no value array");
      const kept = values.filter((entry) => {
        const project = asRecord(entry);
        return (
          project !== undefined &&
          (isCurrentProject(project.name, policy) || isCurrentProject(project.id, policy))
        );
      });
      return reserialize({ count: kept.length, value: kept });
    }

    case "filter-resource-areas": {
      const values = listValues(record);
      if (values === undefined) return denyBody("resource area list had no value array");
      // Rewrite, do not merely filter.
      //
      // `az` resolves service locations from this response, and it is the
      // single point that decides whether it stays on the policy endpoint.
      // Measured (scripts/sps-probe.mjs): omit the `location` area and `az`
      // fails outright; return an incomplete list and it falls back to
      // deployment-level SPS; return the real areas pointing back at the
      // policy endpoint and it completes without ever contacting SPS.
      //
      // Dropping entries whose `locationUrl` is not already a protected host
      // would empty the list and reintroduce exactly that fallback — the
      // opposite of the intent. So each URL is rewritten to the origin the
      // client is already talking to, and only entries that cannot be
      // rewritten are dropped.
      const kept: unknown[] = [];
      for (const value of values) {
        const area = asRecord(value);
        if (area === undefined) continue;
        const rewritten = rewriteLocationUrl(area.locationUrl, selfOrigin);
        if (rewritten === undefined) continue;
        kept.push({ ...area, locationUrl: rewritten });
      }
      return reserialize({ count: kept.length, value: kept });
    }

    case "validate-project": {
      // Work items report their project in `fields["System.TeamProject"]`;
      // other org-level resources carry a nested `project` object. Accept
      // either shape, and deny when neither is present — an unvalidatable
      // response cannot be forwarded.
      const nested = asRecord(record.project);
      const fromFields = asRecord(record.fields)?.["System.TeamProject"];
      const candidates = [nested?.name, nested?.id, fromFields];
      if (!candidates.some((candidate) => isCurrentProject(candidate, policy))) {
        return denyBody("resource belongs to a different project");
      }
      return forward(body);
    }

    case "validate-project-and-repository": {
      const repository = asRecord(record.repository);
      if (repository === undefined) {
        return denyBody("response carried no repository to validate");
      }
      const project = asRecord(repository.project);
      if (
        !isCurrentProject(project?.name, policy) &&
        !isCurrentProject(project?.id, policy)
      ) {
        return denyBody("resource belongs to a different project");
      }
      if (
        !isCurrentRepository(repository.name, policy) &&
        !isCurrentRepository(repository.id, policy)
      ) {
        return denyBody("resource belongs to a different repository");
      }
      return forward(body);
    }

    default: {
      // A response policy added in Rust but not implemented here must not
      // default to forwarding an unvalidated body.
      const exhaustive: never = responsePolicy;
      return denyBody(`unimplemented response policy ${String(exhaustive)}`);
    }
  }
}

/**
 * Point a service `locationUrl` back at the proxy, preserving its path.
 *
 * Azure DevOps returns absolute URLs like
 * `https://dev.azure.com/contoso/` — the host must become the origin the client
 * is already using, or the client's next request leaves the policed path.
 * Returns `undefined` for anything unparseable, which the caller drops.
 */
export function rewriteLocationUrl(
  locationUrl: unknown,
  selfOrigin: string,
): string | undefined {
  if (typeof locationUrl !== "string") return undefined;
  let parsed: URL;
  let origin: URL;
  try {
    parsed = new URL(locationUrl);
    origin = new URL(selfOrigin);
  } catch {
    return undefined;
  }
  parsed.protocol = origin.protocol;
  parsed.host = origin.host;
  return parsed.toString();
}

/** True when a discovery `locationUrl` resolves to a protected host. */
export function isProtectedLocation(locationUrl: unknown): boolean {
  if (typeof locationUrl !== "string") return false;
  let host: string;
  try {
    host = new URL(locationUrl).hostname.toLowerCase();
  } catch {
    return false;
  }
  return PROTECTED_HOSTS.some((protectedHost) => protectedHost.toLowerCase() === host);
}
