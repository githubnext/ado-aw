/**
 * Request-target normalization and catalog route matching.
 *
 * Everything downstream — capability checks, scope checks, response filtering —
 * keys off the normalized form produced here, so this module is the single
 * place where an attacker could smuggle a different effective path past the
 * policy. It therefore decodes exactly once and rejects anything ambiguous
 * rather than trying to be lenient.
 */

/** A request path split into decoded segments, plus its raw query string. */
export interface NormalizedTarget {
  /** Decoded, non-empty path segments. `/a/b` becomes `["a", "b"]`. */
  readonly segments: readonly string[];
  /** Parsed query parameters, preserving order and duplicates. */
  readonly query: readonly (readonly [string, string])[];
}

export class NormalizeError extends Error {}

/**
 * Characters that must never survive decoding inside a single path segment.
 *
 * A decoded `/` or `\` would mean the client encoded a separator to make one
 * segment look like several; a decoded `%` means the value was encoded twice
 * and would decode differently upstream than it does here; control characters
 * are request-smuggling material.
 */
function rejectDangerousDecoded(segment: string, raw: string): void {
  if (segment.includes("/") || segment.includes("\\")) {
    throw new NormalizeError(`path segment decodes to a separator: ${raw}`);
  }
  if (segment.includes("%")) {
    throw new NormalizeError(`path segment is doubly encoded: ${raw}`);
  }
  // eslint-disable-next-line no-control-regex
  if (/[\u0000-\u001f\u007f]/.test(segment)) {
    throw new NormalizeError(`path segment contains a control character: ${raw}`);
  }
}

function decodeSegment(raw: string): string {
  let decoded: string;
  try {
    decoded = decodeURIComponent(raw);
  } catch {
    throw new NormalizeError(`path segment is not valid percent-encoding: ${raw}`);
  }
  rejectDangerousDecoded(decoded, raw);
  return decoded;
}

/**
 * Normalize an origin-form request target (`/path?query`).
 *
 * Rejects: absolute-form targets, traversal segments, empty segments, and
 * anything that decodes ambiguously. There is no path *rewriting* here — a
 * target that would need normalizing to become safe is refused instead, so the
 * bytes the policy inspects are the bytes the upstream receives.
 */
export function normalizeTarget(target: string): NormalizedTarget {
  if (!target.startsWith("/")) {
    throw new NormalizeError(`request target must be origin-form, got ${target}`);
  }
  if (target.includes("#")) {
    throw new NormalizeError("request target must not contain a fragment");
  }

  const split = target.indexOf("?");
  const rawPath = split === -1 ? target : target.slice(0, split);
  const rawQuery = split === -1 ? "" : target.slice(split + 1);

  if (rawPath.includes("//")) {
    throw new NormalizeError("request path contains an empty segment");
  }

  const rawSegments = rawPath.split("/").slice(1);
  // A single trailing slash is idiomatic and harmless; drop it before the
  // empty-segment check so `/org/_apis/` matches `/{org}/_apis`.
  if (rawSegments.length > 1 && rawSegments[rawSegments.length - 1] === "") {
    rawSegments.pop();
  }

  const segments = rawSegments.map((raw) => {
    if (raw === "") {
      throw new NormalizeError("request path contains an empty segment");
    }
    const decoded = decodeSegment(raw);
    if (decoded === "." || decoded === "..") {
      throw new NormalizeError("request path contains a traversal segment");
    }
    return decoded;
  });

  return { segments, query: parseQuery(rawQuery) };
}

/**
 * Parse a query string into ordered pairs.
 *
 * Duplicates are preserved rather than collapsed: `api-version=7.1&api-version=1.0`
 * must be visible to the policy as a conflict, not silently reduced to one
 * value that may differ from the one the upstream honours.
 */
export function parseQuery(raw: string): (readonly [string, string])[] {
  if (raw === "") return [];
  return raw.split("&").map((pair) => {
    if (pair === "") {
      throw new NormalizeError("query string contains an empty parameter");
    }
    const equals = pair.indexOf("=");
    const rawName = equals === -1 ? pair : pair.slice(0, equals);
    const rawValue = equals === -1 ? "" : pair.slice(equals + 1);
    return [decodeQueryPart(rawName), decodeQueryPart(rawValue)] as const;
  });
}

function decodeQueryPart(raw: string): string {
  try {
    return decodeURIComponent(raw.replace(/\+/g, " "));
  } catch {
    throw new NormalizeError(`query part is not valid percent-encoding: ${raw}`);
  }
}

/** Placeholder values captured while matching a route template. */
export type RouteParams = Readonly<Record<string, string>>;

const PLACEHOLDER = /^\{([A-Za-z]+)\}$/;

/**
 * Per-placeholder value shape.
 *
 * Constraining these keeps a numeric id from carrying a path-like or
 * filter-like payload into a route the catalog believes is fully bounded.
 * Placeholders that name a *scope* (`org`, `project`, `repository`) are
 * deliberately absent: they are checked against the policy's own values, which
 * is strictly stronger than a shape check.
 */
const PLACEHOLDER_SHAPE: Readonly<Record<string, RegExp>> = {
  area: /^[A-Za-z][A-Za-z0-9._-]{0,63}$/,
  areaId: /^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$/,
  commitId: /^[0-9a-fA-F]{7,40}$/,
  id: /^[1-9][0-9]{0,17}$/,
  buildId: /^[1-9][0-9]{0,17}$/,
  definitionId: /^[1-9][0-9]{0,17}$/,
  pipelineId: /^[1-9][0-9]{0,17}$/,
  pullRequestId: /^[1-9][0-9]{0,17}$/,
  runId: /^[1-9][0-9]{0,17}$/,
};

/**
 * Match normalized path segments against a catalog route template.
 *
 * Returns the captured placeholders, or `undefined` when the route does not
 * apply. Literal segments compare case-insensitively because Azure DevOps
 * routes are case-insensitive and clients vary (`_apis/wit` vs `_apis/WIT`).
 */
export function matchRoute(
  route: string,
  segments: readonly string[],
): RouteParams | undefined {
  const template = route.split("/").slice(1);
  if (template.length !== segments.length) return undefined;

  const params: Record<string, string> = {};
  for (let index = 0; index < template.length; index += 1) {
    const expected = template[index] as string;
    const actual = segments[index] as string;
    const placeholder = PLACEHOLDER.exec(expected);

    if (placeholder === null) {
      if (expected.toLowerCase() !== actual.toLowerCase()) return undefined;
      continue;
    }

    const name = placeholder[1] as string;
    if (actual === "") return undefined;
    const shape = PLACEHOLDER_SHAPE[name];
    if (shape !== undefined && !shape.test(actual)) return undefined;
    params[name] = actual;
  }
  return params;
}

/**
 * True when the request falls in an always-denied route family.
 *
 * Families are matched **structurally**, not by substring: the family is split
 * into segments, `{placeholder}` matches any one segment, and the sequence must
 * appear contiguously in the request path. A substring test cannot work,
 * because the catalog authors families such as
 * `/_apis/build/builds/{buildId}/oauthtoken` — the literal text never appears
 * in a real path, so the denial would be silently inert.
 *
 * A family may also pin a query parameter with a `?name=` suffix
 * (`/_apis/wit/workitems?ids=`), which is why the query is an input here.
 *
 * Checked before capability and route matching so a denied family can never be
 * reached by a route that happens to look allowable, and so the denial reason
 * reported to the author names the family rather than "unknown route".
 */
export function matchesDeniedFamily(
  segments: readonly string[],
  families: readonly string[],
  query: readonly (readonly [string, string])[] = [],
): string | undefined {
  const lowerSegments = segments.map((segment) => segment.toLowerCase());
  const queryNames = new Set(query.map(([name]) => name.toLowerCase()));

  return families.find((family) => {
    const [pathPart, queryPart] = splitFamily(family);
    if (queryPart !== undefined && !queryNames.has(queryPart)) return false;
    return containsSegmentRun(lowerSegments, pathPart);
  });
}

/** Split `/a/b?name=` into its segment list and the pinned query name. */
function splitFamily(family: string): [readonly string[], string | undefined] {
  const question = family.indexOf("?");
  const path = question === -1 ? family : family.slice(0, question);
  const rawQuery = question === -1 ? undefined : family.slice(question + 1);
  const queryName =
    rawQuery === undefined ? undefined : rawQuery.split("=")[0]?.toLowerCase();
  const parts = path
    .split("/")
    .filter((part) => part !== "")
    .map((part) => part.toLowerCase());
  return [parts, queryName === "" ? undefined : queryName];
}

/** True when `family` appears as a contiguous run of `segments`. */
function containsSegmentRun(
  segments: readonly string[],
  family: readonly string[],
): boolean {
  if (family.length === 0) return false;
  for (let start = 0; start + family.length <= segments.length; start += 1) {
    let matched = true;
    for (let offset = 0; offset < family.length; offset += 1) {
      const expected = family[offset] as string;
      // `{placeholder}` stands for exactly one segment of any value.
      if (expected.startsWith("{") && expected.endsWith("}")) continue;
      if (expected !== segments[start + offset]) {
        matched = false;
        break;
      }
    }
    if (matched) return true;
  }
  return false;
}
