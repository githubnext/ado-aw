export interface AdoRepoIdentity {
  collectionUri: string;
  organization: string;
  project: string;
  repository: string;
}

function decodeSegment(value: string): string | null {
  try {
    const decoded = decodeURIComponent(value);
    return decoded.length > 0 && !/[\/\\\u0000-\u001f\u007f]/.test(decoded) ? decoded : null;
  } catch {
    return null;
  }
}

/**
 * Parse Azure DevOps Services Git HTTPS remotes. Unknown/on-premises shapes
 * deliberately return null so callers can use their git-only fallback rather
 * than guessing repository identity.
 */
export function parseAdoRepoUrl(raw: string): AdoRepoIdentity | null {
  let url: URL;
  try {
    url = new URL(raw);
  } catch {
    return null;
  }
  if (url.protocol !== "https:" || url.port || url.password || url.search || url.hash) return null;

  const host = url.hostname.toLowerCase();
  const parts = url.pathname.replace(/\/$/, "").split("/").slice(1);
  if (parts.some((part) => !part)) return null;
  let organization: string;
  let projectPart: string;
  let repoPart: string;
  let collectionUri: string;

  if (host === "dev.azure.com") {
    if (parts.length !== 4 || parts[2] !== "_git") return null;
    const orgPart = decodeSegment(parts[0] ?? "");
    if (!orgPart) return null;
    organization = orgPart.toLowerCase();
    projectPart = parts[1] ?? "";
    repoPart = parts[3] ?? "";
    collectionUri = `https://dev.azure.com/${orgPart}/`;
  } else if (host.endsWith(".visualstudio.com")) {
    const hasDefaultCollection =
      parts.length === 4 &&
      parts[0]?.toLowerCase() === "defaultcollection" &&
      parts[2] === "_git";
    const directProject =
      parts.length === 3 && parts[1] === "_git";
    if (!hasDefaultCollection && !directProject) return null;
    organization = host.slice(0, -".visualstudio.com".length);
    if (organization.length === 0 || organization.includes(".")) return null;
    projectPart = parts[hasDefaultCollection ? 1 : 0] ?? "";
    repoPart = parts[hasDefaultCollection ? 3 : 2] ?? "";
    collectionUri = hasDefaultCollection
      ? `https://${organization}.visualstudio.com/DefaultCollection/`
      : `https://${organization}.visualstudio.com/`;
  } else {
    return null;
  }

  const project = decodeSegment(projectPart);
  const repository = decodeSegment(repoPart);
  if (!project || !repository) return null;
  return { collectionUri, organization, project, repository };
}

export function adoOrganizationFromCollectionUri(raw: string | undefined): string | null {
  if (!raw) return null;
  let url: URL;
  try {
    url = new URL(raw);
  } catch {
    return null;
  }
  if (url.protocol !== "https:" || url.port || url.username || url.password || url.search || url.hash) return null;
  const host = url.hostname.toLowerCase();
  if (host === "dev.azure.com") {
    const parts = url.pathname.split("/").filter((part) => part.length > 0);
    if (parts.length !== 1) return null;
    const org = parts[0];
    return org ? decodeSegment(org)?.toLowerCase() ?? null : null;
  }
  if (host.endsWith(".visualstudio.com")) {
    const org = host.slice(0, -".visualstudio.com".length);
    const parts = url.pathname.split("/").filter(Boolean);
    return org.length > 0 && !org.includes(".")
      && (parts.length === 0 || (parts.length === 1 && parts[0]?.toLowerCase() === "defaultcollection"))
      ? org : null;
  }
  return null;
}

export interface TriggeringPullRequest {
  collection_uri: string;
  project: string;
  repository_name: string;
  repository_id: string;
  id: string;
}

export function positivePrId(value: unknown): string | null {
  if (typeof value === "number") {
    return Number.isSafeInteger(value) && value > 0 ? String(value) : null;
  }
  if (typeof value !== "string" || !/^\d+$/.test(value)) return null;
  const id = BigInt(value);
  return id > 0n && id <= 18446744073709551615n ? id.toString() : null;
}

export function parseTriggeringPrIdentity(value: unknown): TriggeringPullRequest | undefined {
  if (!value || typeof value !== "object" || Array.isArray(value)) return undefined;
  const raw = value as Record<string, unknown>;
  if (typeof raw.collection_uri !== "string" || !adoOrganizationFromCollectionUri(raw.collection_uri)
    || typeof raw.project !== "string" || !raw.project.trim()
    || typeof raw.repository_name !== "string" || !raw.repository_name.trim()
    || /[\/\\\u0000-\u001f\u007f]/.test(raw.project)
    || /[\/\\\u0000-\u001f\u007f]/.test(raw.repository_name)
    || /\$\(|\$\[|\$\{\{|##vso\[|##\[|\{\{/.test(raw.project)
    || /\$\(|\$\[|\$\{\{|##vso\[|##\[|\{\{/.test(raw.repository_name)
    || typeof raw.repository_id !== "string"
    || !/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(raw.repository_id)
    || !positivePrId(raw.id)) return undefined;
  return {
    collection_uri: raw.collection_uri, project: raw.project,
    repository_name: raw.repository_name, repository_id: raw.repository_id,
    id: positivePrId(raw.id)!,
  };
}

/** Capture the target Build.Repository metadata, never the fork's SourceRepositoryURI. */
export function nativeTriggeringPrIdentity(env: NodeJS.ProcessEnv): TriggeringPullRequest | undefined {
  const read = (projected: string, native: string): string | undefined =>
    env.ADO_AW_TRIGGERING_PR_CAPTURED !== undefined ? env[projected] : env[native];
  if (read("ADO_AW_TRIGGER_BUILD_REASON", "BUILD_REASON") !== "PullRequest"
    || read("ADO_AW_TRIGGER_REPOSITORY_PROVIDER", "BUILD_REPOSITORY_PROVIDER") !== "TfsGit") return undefined;
  const remote = parseAdoRepoUrl(read("ADO_AW_TRIGGER_REPOSITORY_URI", "BUILD_REPOSITORY_URI") ?? "");
  const collection = read("ADO_AW_TRIGGER_COLLECTION_URI", "SYSTEM_COLLECTIONURI")
    ?? (env.ADO_AW_TRIGGERING_PR_CAPTURED === undefined ? env.SYSTEM_TEAMFOUNDATIONCOLLECTIONURI : undefined);
  if (!remote || !collection || adoOrganizationFromCollectionUri(collection) !== remote.organization) return undefined;
  return parseTriggeringPrIdentity({
    collection_uri: collection, project: remote.project, repository_name: remote.repository,
    repository_id: read("ADO_AW_TRIGGER_REPOSITORY_ID", "BUILD_REPOSITORY_ID"),
    id: read("ADO_AW_TRIGGER_PR_ID", "SYSTEM_PULLREQUEST_PULLREQUESTID"),
  });
}

export function readTriggeringPrIdentity(env: NodeJS.ProcessEnv): TriggeringPullRequest | undefined {
  if (env.ADO_AW_TRIGGERING_PR_IDENTITY !== undefined) {
    try { return parseTriggeringPrIdentity(JSON.parse(env.ADO_AW_TRIGGERING_PR_IDENTITY)); }
    catch { return undefined; }
  }
  return nativeTriggeringPrIdentity(env);
}

export function isCurrentAdoOrganization(
  identity: AdoRepoIdentity,
  env: NodeJS.ProcessEnv,
): boolean {
  const current = adoOrganizationFromCollectionUri(
    env.SYSTEM_COLLECTIONURI ?? env.SYSTEM_TEAMFOUNDATIONCOLLECTIONURI,
  );
  return current !== null && current === identity.organization;
}
