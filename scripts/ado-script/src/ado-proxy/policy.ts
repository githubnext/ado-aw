/**
 * The authorization decision for a protected request.
 *
 * Deny-by-default throughout: a request is allowed only when it matches a
 * catalogued operation whose capability is enabled, whose API version is in
 * range, whose every query parameter is explicitly permitted, and whose scope
 * resolves to the organization/project/repository the compiler pinned. Any gap
 * — an unknown route, an unlisted parameter, an unmatched placeholder — is a
 * denial, never a pass-through.
 */
import { ScopeIndex } from "./scope.js";
import { ApiVersionError, resolveApiVersion, type ApiVersion } from "./api-version.js";
import { DENIED_ROUTE_FAMILIES, OPERATIONS, PROTECTED_HOSTS } from "./catalog.js";
import type { ProxyPolicy } from "./config.js";
import {
  matchRoute,
  matchesDeniedFamily,
  type NormalizedTarget,
  type RouteParams,
} from "./route.js";
import type { Capability, Operation } from "../shared/ado-proxy-catalog.types.gen.js";

/** Why a request was refused, in a form safe to log and to return. */
export type DenyReason =
  | "method-not-read"
  | "unknown-host"
  | "denied-route-family"
  | "unknown-route"
  | "capability-disabled"
  | "api-version"
  | "query-not-allowed"
  | "out-of-scope";

export type Decision =
  | {
      readonly allow: true;
      readonly operation: Operation;
      readonly params: RouteParams;
      readonly apiVersion?: ApiVersion;
    }
  | {
      readonly allow: false;
      readonly reason: DenyReason;
      readonly detail: string;
      /** Operation id when the route matched but a later check failed. */
      readonly operationId?: string;
    };

function deny(
  reason: DenyReason,
  detail: string,
  operationId?: string,
): Decision {
  return operationId === undefined
    ? { allow: false, reason, detail }
    : { allow: false, reason, detail, operationId };
}

/** Case-insensitive identifier comparison, as Azure DevOps treats names. */
function sameIdentifier(left: string, right: string | undefined): boolean {
  return right !== undefined && left.toLowerCase() === right.toLowerCase();
}

/**
 * True when a path value names the pinned project.
 *
 * Clients use the name in some calls and the GUID in others — `az` in
 * particular substitutes whichever it cached — so both are accepted, but only
 * for the single project the compiler pinned.
 */
function isCurrentProject(value: string, policy: ProxyPolicy): boolean {
  return (
    sameIdentifier(value, policy.project) || sameIdentifier(value, policy.project_id)
  );
}

/** True when a path value names the pinned repository. */
function isCurrentRepository(value: string, policy: ProxyPolicy): boolean {
  return (
    sameIdentifier(value, policy.repository) ||
    sameIdentifier(value, policy.repository_id)
  );
}

/**
 * Resolve the concrete host an operation's {@link Operation.host} policy names.
 *
 * The catalog stores a *policy* rather than a hostname so the same catalog can
 * describe the organization host without the compiler having to rewrite it per
 * run.
 */
function hostFor(operation: Operation): string | undefined {
  const [organizationHost, spsFallbackHost] = PROTECTED_HOSTS;
  return operation.host === "current-organization" ? organizationHost : spsFallbackHost;
}

function checkQuery(
  operation: Operation,
  target: NormalizedTarget,
): Decision | undefined {
  const allowed = new Set(operation.allowed_query.map((name) => name.toLowerCase()));
  const denied = new Set(operation.denied_query.map((name) => name.toLowerCase()));

  for (const [rawName] of target.query) {
    const name = rawName.toLowerCase();
    // The version is validated separately and is legal on every versioned
    // operation, so it is never listed in `allowed_query`.
    if (name === "api-version") continue;
    if (denied.has(name)) {
      return deny("query-not-allowed", `parameter ${name} is denied`, operation.id);
    }
    if (!allowed.has(name)) {
      return deny(
        "query-not-allowed",
        `parameter ${name} is not permitted on this operation`,
        operation.id,
      );
    }
  }
  return undefined;
}

function checkScope(
  operation: Operation,
  params: RouteParams,
  policy: ProxyPolicy,
  scopes: ScopeIndex,
): Decision | undefined {
  const organization = params.org;
  // Every organization-hosted route carries `{org}`; the SPS fallback route
  // does not, and is scoped by resource-area id instead.
  if (operation.host === "current-organization") {
    if (organization === undefined || !scopes.hasOrganization(organization)) {
      return deny(
        "out-of-scope",
        "request names an organization outside the policy",
        operation.id,
      );
    }
  }

  switch (operation.scope) {
    case "current-organization":
    case "filter-projects-to-current":
    case "filter-resource-areas":
    case "response-current-project":
    case "response-current-repository":
      // Organization scope already checked; the response-scoped variants are
      // additionally validated against the body once it arrives, because their
      // URL carries no project or repository segment to check here.
      return undefined;

    case "allowed-resource-area": {
      const areaId = params.areaId;
      if (
        areaId === undefined ||
        !policy.allowed_resource_areas.some((allowed) => sameIdentifier(areaId, allowed))
      ) {
        return deny(
          "out-of-scope",
          "resource area is not in the allowed set",
          operation.id,
        );
      }
      return undefined;
    }

    case "current-project-path": {
      const project = params.project;
      // Organization-relative: the project is looked up *inside* the
      // organization named by this request, so a project granted in another
      // organization cannot satisfy it.
      if (project === undefined || !scopes.allowsProject(organization, project)) {
        return deny(
          "out-of-scope",
          "request names a project outside the policy for this organization",
          operation.id,
        );
      }
      return undefined;
    }

    case "current-repository-path": {
      const project = params.project;
      const repository = params.repository;
      if (project === undefined) {
        return deny("out-of-scope", "request names no project", operation.id);
      }
      // A repository grant does not imply a project grant — a `repos:`
      // declaration asks for the repository, not the work items and pipelines
      // beside it — so this checks the repository within the project rather
      // than requiring the project itself to be in scope.
      if (
        repository === undefined ||
        !scopes.allowsRepository(organization, project, repository)
      ) {
        return deny(
          "out-of-scope",
          "request names a repository outside the policy for this project",
          operation.id,
        );
      }
      return undefined;
    }

    default: {
      // An unhandled scope policy must never mean "allowed"; a new variant
      // added in Rust fails closed here until it is implemented.
      const exhaustive: never = operation.scope;
      return deny("out-of-scope", `unimplemented scope policy ${String(exhaustive)}`, operation.id);
    }
  }
}

/** Inputs to a single authorization decision. */
export interface RequestFacts {
  readonly method: string;
  /** Canonical host, already confirmed protected and without a port. */
  readonly host: string;
  readonly target: NormalizedTarget;
  readonly accept: string | undefined;
}

/**
 * Authorize one protected request.
 *
 * Ordering matters and is deliberate: method and denied-family checks run
 * before route matching so that a mutation or a credential-bearing family is
 * reported as such rather than as a generic "unknown route", which is what an
 * author needs to see to understand the denial.
 */
export function authorize(
  facts: RequestFacts,
  policy: ProxyPolicy,
  scopes: ScopeIndex = ScopeIndex.from(policy),
): Decision {
  const method = facts.method.toUpperCase();
  if (method !== "GET" && method !== "OPTIONS") {
    return deny("method-not-read", `${method} is not a read method`);
  }

  const [organizationHost, spsFallbackHost] = PROTECTED_HOSTS;
  if (facts.host !== organizationHost && facts.host !== spsFallbackHost) {
    return deny("unknown-host", "host is not a catalogued Azure DevOps host");
  }

  const deniedFamily = matchesDeniedFamily(
    facts.target.segments,
    DENIED_ROUTE_FAMILIES,
    facts.target.query,
  );
  if (deniedFamily !== undefined) {
    return deny("denied-route-family", `route family ${deniedFamily} is always denied`);
  }

  const enabled = new Set<Capability>(policy.capabilities);
  let capabilityBlocked: Operation | undefined;

  for (const operation of OPERATIONS) {
    if (operation.method !== method) continue;
    if (hostFor(operation) !== facts.host) continue;

    const params = matchRoute(operation.route, facts.target.segments);
    if (params === undefined) continue;

    if (!enabled.has(operation.capability)) {
      // Remember it, but keep looking: another capability may catalogue the
      // same shape, and reporting the disabled capability is only right when
      // nothing else matches.
      capabilityBlocked ??= operation;
      continue;
    }

    let apiVersion: ApiVersion | undefined;
    try {
      apiVersion = resolveApiVersion(
        operation.api_version,
        facts.target.query,
        facts.accept,
      );
    } catch (error) {
      if (error instanceof ApiVersionError) {
        return deny("api-version", error.message, operation.id);
      }
      throw error;
    }

    const queryDenial = checkQuery(operation, facts.target);
    if (queryDenial !== undefined) return queryDenial;

    const scopeDenial = checkScope(operation, params, policy, scopes);
    if (scopeDenial !== undefined) return scopeDenial;

    return apiVersion === undefined
      ? { allow: true, operation, params }
      : { allow: true, operation, params, apiVersion };
  }

  if (capabilityBlocked !== undefined) {
    return deny(
      "capability-disabled",
      `operation requires the ${capabilityBlocked.capability} capability`,
      capabilityBlocked.id,
    );
  }
  return deny("unknown-route", "no catalogued operation matches this request");
}
