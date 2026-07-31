/**
 * Azure DevOps API-version extraction and validation.
 *
 * Azure DevOps accepts the version in two places, and clients use both: `az`
 * and the REST SDKs put it in the `Accept` header
 * (`application/json;api-version=7.1;excludeUrls=true`), while curl and most
 * hand-written callers put it in the query string. The proxy must read both,
 * because honouring only one lets a request declare a benign version in the
 * place the policy looks while the upstream honours a different one.
 */
import { API_VERSION_MAX, API_VERSION_MIN } from "./catalog.js";

/** Marker used by catalog operations that must carry no API version at all. */
export const API_VERSION_ABSENT = "absent";

export class ApiVersionError extends Error {}

/** A parsed `major.minor[-preview[.n]]` version. */
export interface ApiVersion {
  readonly major: number;
  readonly minor: number;
  readonly preview: boolean;
  /** The original text, for logging. */
  readonly raw: string;
}

const VERSION = /^(\d{1,3})\.(\d{1,3})(-preview(?:\.\d{1,3})?)?$/;

/** Parse a version string, or throw with the offending text. */
export function parseApiVersion(raw: string): ApiVersion {
  const match = VERSION.exec(raw.trim());
  if (match === null) {
    throw new ApiVersionError(`unrecognized api-version ${JSON.stringify(raw)}`);
  }
  return {
    major: Number(match[1]),
    minor: Number(match[2]),
    preview: match[3] !== undefined,
    raw: raw.trim(),
  };
}

/** True when the version falls inside the catalog's supported window. */
export function isSupported(version: ApiVersion): boolean {
  const [minMajor, minMinor] = API_VERSION_MIN;
  const [maxMajor, maxMinor] = API_VERSION_MAX;
  const value = version.major * 1000 + version.minor;
  return value >= minMajor * 1000 + minMinor && value <= maxMajor * 1000 + maxMinor;
}

/**
 * Pull the `api-version` parameter out of an `Accept` header.
 *
 * Handles the multi-media-type form clients send, and treats a version that
 * differs between media types as a conflict rather than picking one.
 */
export function apiVersionFromAccept(accept: string | undefined): string | undefined {
  if (accept === undefined) return undefined;
  const found = new Set<string>();
  for (const mediaType of accept.split(",")) {
    for (const parameter of mediaType.split(";").slice(1)) {
      const equals = parameter.indexOf("=");
      if (equals === -1) continue;
      const name = parameter.slice(0, equals).trim().toLowerCase();
      if (name !== "api-version") continue;
      found.add(parameter.slice(equals + 1).trim().replace(/^"|"$/g, ""));
    }
  }
  if (found.size === 0) return undefined;
  if (found.size > 1) {
    throw new ApiVersionError("Accept header declares conflicting api-versions");
  }
  return [...found][0];
}

/** Pull `api-version` out of parsed query pairs. */
export function apiVersionFromQuery(
  query: readonly (readonly [string, string])[],
): string | undefined {
  const values = new Set(
    query
      .filter(([name]) => name.toLowerCase() === "api-version")
      .map(([, value]) => value),
  );
  if (values.size === 0) return undefined;
  if (values.size > 1) {
    throw new ApiVersionError("query string declares conflicting api-versions");
  }
  return [...values][0];
}

/**
 * Resolve and validate the effective API version for a request.
 *
 * `expected` is the catalog operation's `api_version` field: either
 * {@link API_VERSION_ABSENT} for discovery `OPTIONS`, or the range marker for
 * everything else. Throws {@link ApiVersionError} on any disagreement,
 * absence-violation, or out-of-window version — all of which are denials.
 */
export function resolveApiVersion(
  expected: string,
  query: readonly (readonly [string, string])[],
  accept: string | undefined,
): ApiVersion | undefined {
  const fromQuery = apiVersionFromQuery(query);
  const fromAccept = apiVersionFromAccept(accept);

  if (expected === API_VERSION_ABSENT) {
    if (fromQuery !== undefined || fromAccept !== undefined) {
      throw new ApiVersionError(
        "this operation must be sent without an api-version",
      );
    }
    return undefined;
  }

  if (fromQuery !== undefined && fromAccept !== undefined && fromQuery !== fromAccept) {
    // Declaring one version in the query and another in Accept lets a request
    // be policy-checked as one operation and served as another.
    throw new ApiVersionError(
      "api-version in the query string and Accept header disagree",
    );
  }

  const raw = fromQuery ?? fromAccept;
  if (raw === undefined) {
    throw new ApiVersionError("this operation requires an api-version");
  }

  const version = parseApiVersion(raw);
  if (!isSupported(version)) {
    const [minMajor, minMinor] = API_VERSION_MIN;
    const [maxMajor, maxMinor] = API_VERSION_MAX;
    throw new ApiVersionError(
      `api-version ${version.raw} is outside the supported range ` +
        `${minMajor}.${minMinor}-${maxMajor}.${maxMinor}`,
    );
  }
  return version;
}
