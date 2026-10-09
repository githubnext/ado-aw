/**
 * Authorization for the package-feed family (Azure Artifacts).
 *
 * The REST catalog cannot describe package traffic: npm, PyPI, NuGet, and
 * Cargo clients address open-ended URL spaces (package names, versions, file
 * names) under one feed. So the package family authorizes by *shape* instead:
 *
 *   `/{org}[/{project}]/_packaging/{feed}[@{view}]/{protocol-prefix}…`
 *
 * A request is allowed only when every part of that shape matches a feed grant
 * the compiler emitted, the remainder starts with a catalogued prefix for a
 * protocol the grant enables, and the method is a read. Everything else on a
 * package host — notably the `/_apis/packaging/…` management REST surface — is
 * denied.
 *
 * Like `route.ts`, this module decodes exactly once and *refuses* anything
 * ambiguous (traversal, encoded separators, double encoding) rather than
 * normalizing it, so the bytes the policy inspects are the bytes the upstream
 * receives. Pure functions only: no I/O, no credential.
 */
import type {
  PackageProtocolRoute,
} from "../shared/ado-proxy-catalog.types.gen.js";
import {
  PACKAGE_PROTOCOLS,
  PACKAGE_REDIRECT_HOST_SUFFIXES,
  canonicalizeHost,
} from "./catalog.js";
import type { PackageFeedGrant, ProxyPolicy } from "./config.js";

/** Why a package request was refused. Stable strings, safe to log and return. */
export type PackageDenyReason =
  | "unknown-host"
  | "method-not-read"
  | "malformed-target"
  | "path-traversal"
  | "encoded-separator"
  | "double-encoding"
  | "unknown-route"
  | "feed-not-granted"
  | "view-not-granted"
  | "protocol-not-granted";

export type PackageDecision =
  | {
      readonly allow: true;
      readonly feed: PackageFeedGrant;
      readonly protocol: PackageProtocolRoute;
    }
  | {
      readonly allow: false;
      readonly reason: PackageDenyReason;
      /**
       * Human-readable explanation.
       *
       * Never contains request content — no path, query, or segment value —
       * because it is written to the decision log the agent can read and
       * returned to the client. Protocol names come from the catalog.
       */
      readonly detail: string;
      /** The protocol whose prefix matched, when the denial came after that. */
      readonly protocol?: string;
    };

/** The request facts the package authorizer needs. */
export interface PackageRequest {
  readonly method: string;
  /** Canonicalized destination host (SNI / CONNECT target). */
  readonly host: string;
  /** The raw origin-form request target, exactly as received. */
  readonly rawTarget: string;
}

function deny(
  reason: PackageDenyReason,
  detail: string,
  protocol?: string,
): PackageDecision {
  return protocol === undefined
    ? { allow: false, reason, detail }
    : { allow: false, reason, detail, protocol };
}

/** True when `host` is a package host the policy asks this proxy to police. */
export function isPackageHost(host: string, policy: ProxyPolicy): boolean {
  const packages = policy.packages;
  if (packages === undefined) return false;
  const normalized = canonicalizeHost(host);
  if (normalized === "") return false;
  return packages.hosts.some((entry) => canonicalizeHost(entry) === normalized);
}

/** One decoded path segment, remembering whether it carried an encoded `/`. */
interface Segment {
  readonly decoded: string;
  readonly encodedSlash: boolean;
}

/** A parsed package request target. */
interface ParsedTarget {
  readonly segments: readonly Segment[];
}

class TargetError extends Error {
  constructor(
    readonly reason: PackageDenyReason,
    message: string,
  ) {
    super(message);
  }
}

/**
 * Printable ASCII excluding space. A well-formed client percent-encodes
 * everything else, so anything outside this range is either smuggling material
 * (CR/LF, NUL) or a client bug the upstream might interpret differently.
 */
const PRINTABLE_TARGET = /^[\x21-\x7e]*$/;

/** A `%` not followed by two hex digits. */
const INVALID_ESCAPE = /%(?![0-9A-Fa-f]{2})/;

/**
 * Parse an origin-form target into decoded path segments.
 *
 * The query is validated (printable ASCII, no fragment) but otherwise passed
 * through untouched: npm and NuGet put legitimate parameters there, and the
 * feed shape — not the query — is what the policy grants.
 */
function parseTarget(rawTarget: string): ParsedTarget {
  if (!rawTarget.startsWith("/") || rawTarget.startsWith("//")) {
    throw new TargetError("malformed-target", "the request target must be origin-form");
  }
  if (!PRINTABLE_TARGET.test(rawTarget)) {
    throw new TargetError(
      "malformed-target",
      "the request target contains a control, space, or non-ASCII character",
    );
  }
  if (rawTarget.includes("#")) {
    throw new TargetError("malformed-target", "the request target must not carry a fragment");
  }
  if (rawTarget.includes("\\")) {
    throw new TargetError("malformed-target", "the request target must not contain a backslash");
  }

  const split = rawTarget.indexOf("?");
  const rawPath = split === -1 ? rawTarget : rawTarget.slice(0, split);

  if (rawPath.includes("//")) {
    throw new TargetError("malformed-target", "the request path contains an empty segment");
  }

  const rawSegments = rawPath.split("/").slice(1);
  const segments = rawSegments.map((raw, index): Segment => {
    if (raw === "") {
      // Only a single trailing slash survives the `//` check above. It is
      // idiomatic for package indexes (PyPI's `simple/{name}/` is canonical),
      // so it is kept as a final empty segment rather than refused.
      if (index === rawSegments.length - 1) return { decoded: "", encodedSlash: false };
      throw new TargetError("malformed-target", "the request path contains an empty segment");
    }
    return decodeSegment(raw);
  });

  return { segments };
}

/**
 * Decode one raw path segment, refusing every ambiguous form.
 *
 *   - an invalid escape (`%G1`, a trailing `%`) — upstream decoders disagree on
 *     what it means;
 *   - an encoded `%` (`%25`) — a doubly encoded value would decode differently
 *     at each hop, so the policy would inspect one path and the upstream serve
 *     another. No package protocol needs a literal `%` in a path;
 *   - a decoded `.`/`..` — traversal out of the granted feed;
 *   - a decoded `\` or control character — separator and smuggling material.
 *
 * An encoded `/` is *recorded*, not refused: npm encodes the slash of a scoped
 * name (`@scope%2Fname`). Whether it is acceptable depends on the protocol and
 * on which segment carries it, which only the caller knows.
 */
function decodeSegment(raw: string): Segment {
  if (INVALID_ESCAPE.test(raw)) {
    throw new TargetError("malformed-target", "a path segment has an invalid percent escape");
  }
  if (/%25/i.test(raw)) {
    throw new TargetError("double-encoding", "a path segment is doubly percent-encoded");
  }
  let decoded: string;
  try {
    decoded = decodeURIComponent(raw);
  } catch {
    throw new TargetError("malformed-target", "a path segment is not valid percent-encoding");
  }
  if (decoded === "." || decoded === "..") {
    throw new TargetError("path-traversal", "the request path contains a traversal segment");
  }
  if (decoded.includes("\\")) {
    throw new TargetError("malformed-target", "a path segment decodes to a backslash");
  }
  // eslint-disable-next-line no-control-regex
  if (/[\u0000-\u001f\u007f]/.test(decoded)) {
    throw new TargetError("malformed-target", "a path segment decodes to a control character");
  }
  return { decoded, encodedSlash: decoded.includes("/") };
}

/** The feed shape extracted from a package path. */
interface FeedRoute {
  readonly organization: string;
  /** Present only for a project-scoped feed URL. */
  readonly project?: string;
  readonly feed: string;
  /** Present only when the request named `feed@view`. */
  readonly view?: string;
  /** Decoded segments after `{feed}[@{view}]`. */
  readonly rest: readonly Segment[];
}

/**
 * Match the `/{org}[/{project}]/_packaging/{feedSeg}/…` shape.
 *
 * Returns `undefined` for anything else on the package host, including the
 * `/{org}/_apis/packaging/…` management API, which the package family never
 * grants.
 */
function matchFeedRoute(segments: readonly Segment[]): FeedRoute | undefined {
  const isPackaging = (segment: Segment | undefined): boolean =>
    segment !== undefined && segment.decoded.toLowerCase() === "_packaging";

  let packagingIndex: number;
  if (isPackaging(segments[1])) packagingIndex = 1;
  else if (isPackaging(segments[2])) packagingIndex = 2;
  else return undefined;

  const scope = segments.slice(0, packagingIndex);
  const feedSegment = segments[packagingIndex + 1];
  const rest = segments.slice(packagingIndex + 2);
  if (feedSegment === undefined || rest.length === 0) return undefined;
  if (scope.some((segment) => segment.decoded === "") || feedSegment.decoded === "") {
    return undefined;
  }

  // Scope identifiers and the feed segment are matched against the policy as
  // whole values. An encoded slash there could only be an attempt to make one
  // segment look like two to the upstream, whatever the protocol.
  if ([...scope, feedSegment].some((segment) => segment.encodedSlash)) {
    throw new TargetError(
      "encoded-separator",
      "an encoded '/' is not allowed in the organization, project, or feed segment",
    );
  }

  const at = feedSegment.decoded.split("@");
  if (at.length > 2 || at.some((part) => part === "")) {
    throw new TargetError("malformed-target", "the feed segment must be {feed} or {feed}@{view}");
  }

  return {
    organization: (scope[0] as Segment).decoded,
    ...(scope.length === 2 ? { project: (scope[1] as Segment).decoded } : {}),
    feed: at[0] as string,
    ...(at.length === 2 ? { view: at[1] as string } : {}),
    rest,
  };
}

/**
 * Find the protocol whose catalogued prefix the remainder starts with.
 *
 * Matched segment-by-segment rather than on a joined string, so an encoded
 * slash cannot assemble a prefix (`npm%2Fregistry/…` is one segment, not two).
 * A prefix ends in `/`, so at least one further segment — possibly the empty
 * trailing one of an index URL — must follow it.
 */
function matchProtocol(
  rest: readonly Segment[],
): { route: PackageProtocolRoute; prefixLength: number } | undefined {
  for (const route of PACKAGE_PROTOCOLS) {
    for (const prefix of route.path_prefixes) {
      const parts = prefix.split("/").filter((part) => part !== "");
      if (parts.length === 0 || rest.length <= parts.length) continue;
      const matches = parts.every(
        (part, index) => (rest[index] as Segment).decoded.toLowerCase() === part.toLowerCase(),
      );
      if (matches) return { route, prefixLength: parts.length };
    }
  }
  return undefined;
}

/**
 * The only form an encoded slash may take, even for a protocol that allows it.
 *
 * npm's scoped package name `@scope/name` travels as a single `@scope%2Fname`
 * segment. Anything else with an embedded separator has no legitimate reading.
 */
const SCOPED_NAME = /^@[^/]+\/[^/]+$/;

function sameIdentifier(left: string, right: string | undefined): boolean {
  return right !== undefined && left.toLowerCase() === right.toLowerCase();
}

function matchesNameOrId(value: string, name: string | undefined, id: string | undefined): boolean {
  return sameIdentifier(value, name) || sameIdentifier(value, id);
}

/** True when the grant covers the request's organization, project, and feed. */
function grantCoversFeed(grant: PackageFeedGrant, route: FeedRoute): boolean {
  if (!sameIdentifier(route.organization, grant.organization)) return false;
  if (grant.project === undefined) {
    // An organization-scoped grant matches only the organization-scoped URL;
    // otherwise any project's same-named feed would ride on it.
    if (route.project !== undefined) return false;
  } else if (
    route.project === undefined ||
    !matchesNameOrId(route.project, grant.project, grant.project_id)
  ) {
    return false;
  }
  return matchesNameOrId(route.feed, grant.feed, grant.feed_id);
}

/** True when the grant's view pin (if any) admits the request's view. */
function grantCoversView(grant: PackageFeedGrant, route: FeedRoute): boolean {
  if (grant.view === undefined) return true;
  return route.view !== undefined && matchesNameOrId(route.view, grant.view, grant.view_id);
}

/**
 * Decide whether a package request may be forwarded with the package
 * credential.
 *
 * Order: host, method, target shape, protocol prefix, per-protocol method,
 * encoded-separator rules, then the feed grant. Every check is a denial on
 * failure; there is no fallback path.
 */
export function authorizePackageRequest(
  request: PackageRequest,
  policy: ProxyPolicy,
): PackageDecision {
  if (!isPackageHost(request.host, policy) || policy.packages === undefined) {
    return deny("unknown-host", "this host is not a policed package host");
  }

  // Compared exactly: HTTP methods are case-sensitive, and the method checked
  // here is the method forwarded upstream.
  const method = request.method;
  const readMethods = new Set(PACKAGE_PROTOCOLS.flatMap((route) => route.methods as string[]));
  if (!readMethods.has(method)) {
    return deny(
      "method-not-read",
      `package feeds are read-only through this proxy; ${method} is not allowed`,
    );
  }

  let route: FeedRoute | undefined;
  try {
    route = matchFeedRoute(parseTarget(request.rawTarget).segments);
  } catch (error) {
    if (!(error instanceof TargetError)) throw error;
    return deny(error.reason, error.message);
  }
  if (route === undefined) {
    return deny(
      "unknown-route",
      "only /{org}[/{project}]/_packaging/{feed}[@{view}]/… package routes are allowed",
    );
  }

  const matched = matchProtocol(route.rest);
  if (matched === undefined) {
    return deny("unknown-route", "the path under the feed is not a supported package protocol route");
  }
  const protocol = matched.route;

  if (!(protocol.methods as string[]).includes(method)) {
    return deny(
      "method-not-read",
      `${protocol.protocol} feeds are read-only through this proxy; ${method} is not allowed`,
      protocol.protocol,
    );
  }

  for (const segment of route.rest.slice(matched.prefixLength)) {
    if (!segment.encodedSlash) continue;
    if (!protocol.allow_encoded_slash || !SCOPED_NAME.test(segment.decoded)) {
      return deny(
        "encoded-separator",
        `an encoded '/' is not allowed in this ${protocol.protocol} path`,
        protocol.protocol,
      );
    }
  }

  const feedGrants = policy.packages.feeds.filter((grant) => grantCoversFeed(grant, route));
  if (feedGrants.length === 0) {
    return deny(
      "feed-not-granted",
      "the request names a feed this workflow is not granted",
      protocol.protocol,
    );
  }
  const viewGrants = feedGrants.filter((grant) => grantCoversView(grant, route));
  if (viewGrants.length === 0) {
    return deny(
      "view-not-granted",
      "the feed is granted only through a specific view; address it as {feed}@{view}",
      protocol.protocol,
    );
  }
  const grant = viewGrants.find((candidate) => candidate.protocols.includes(protocol.protocol));
  if (grant === undefined) {
    return deny(
      "protocol-not-granted",
      `the ${protocol.protocol} protocol is not granted for this feed`,
      protocol.protocol,
    );
  }

  return { allow: true, feed: grant, protocol };
}

/** Outcome of validating an upstream redirect. */
export type RedirectDecision =
  | { readonly allow: true; readonly location: string }
  | { readonly allow: false; readonly detail: string };

/**
 * True when `host` ends with one of the redirect suffixes, anchored at a label.
 *
 * A suffix is always compared with its leading dot, and the host must have at
 * least one label before it, so neither `evil.blob.core.windows.net.attacker.test`
 * nor the bare `blob.core.windows.net` matches.
 */
export function isAllowedRedirectHost(
  host: string,
  suffixes: readonly string[] = PACKAGE_REDIRECT_HOST_SUFFIXES,
): boolean {
  const normalized = canonicalizeHost(host);
  if (normalized === "" || normalized.startsWith("[")) return false;
  return suffixes.some((suffix) => {
    const anchored = canonicalizeHost(suffix.startsWith(".") ? suffix : `.${suffix}`);
    return normalized.length > anchored.length && normalized.endsWith(anchored);
  });
}

/**
 * Decide whether an upstream `Location` may be relayed to the client.
 *
 * The proxy never follows a redirect itself — following would carry the
 * credential to wherever the upstream pointed. Relaying is allowed only to:
 *
 *   - an `https:` URL on a catalogued blob-storage suffix, which is how Azure
 *     Artifacts hands out package content (a signed URL the client fetches
 *     with no credential of ours); or
 *   - a URL on the same package host whose path would itself be authorized for
 *     a GET — the follow-up request comes back through this proxy and is
 *     authorized again, but refusing an out-of-policy target up front keeps
 *     the client from being steered outside its grants.
 *
 * Embedded userinfo and non-default ports are refused outright. A relative
 * `Location` is resolved against the package host and judged as same-host.
 */
export function authorizePackageRedirect(
  location: string | undefined,
  requestHost: string,
  policy: ProxyPolicy,
): RedirectDecision {
  if (location === undefined || location.trim() === "") {
    return { allow: false, detail: "the upstream redirect carried no Location" };
  }
  if (!PRINTABLE_TARGET.test(location)) {
    return { allow: false, detail: "the upstream redirect Location is malformed" };
  }

  let url: URL;
  try {
    url = new URL(location, `https://${requestHost}`);
  } catch {
    return { allow: false, detail: "the upstream redirect Location is malformed" };
  }
  if (url.protocol !== "https:") {
    return { allow: false, detail: "the upstream redirect is not to an https: URL" };
  }
  if (url.username !== "" || url.password !== "") {
    return { allow: false, detail: "the upstream redirect embeds credentials" };
  }
  if (url.port !== "") {
    return { allow: false, detail: "the upstream redirect names a non-default port" };
  }

  const host = canonicalizeHost(url.hostname);
  if (isAllowedRedirectHost(host)) {
    return { allow: true, location: url.href };
  }

  if (host === canonicalizeHost(requestHost) && isPackageHost(host, policy)) {
    const decision = authorizePackageRequest(
      { method: "GET", host, rawTarget: `${url.pathname}${url.search}` },
      policy,
    );
    if (decision.allow) return { allow: true, location: url.href };
    return {
      allow: false,
      detail: "the upstream redirected to a package route this workflow is not granted",
    };
  }

  return { allow: false, detail: "the upstream redirected to a host outside the package allowlist" };
}

/**
 * Format the package credential for the protocol's `Authorization` header.
 *
 * Mirrors what the protocol's Azure Pipelines authenticate task makes its
 * client send: npm and Cargo send a bearer, NuGet and pip send Basic with an
 * arbitrary user name and the token as the password.
 */
export function packageAuthorizationHeader(
  protocol: PackageProtocolRoute,
  token: string,
): string {
  if (protocol.auth_scheme === "basic") {
    return `Basic ${Buffer.from(`ado-aw:${token}`, "utf8").toString("base64")}`;
  }
  return `Bearer ${token}`;
}
