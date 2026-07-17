export interface AdoRepoIdentity {
  collectionUri: string;
  organization: string;
  project: string;
  repository: string;
}

function decodeSegment(value: string): string | null {
  try {
    const decoded = decodeURIComponent(value);
    return decoded.length > 0 ? decoded : null;
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
  if (url.protocol !== "https:") return null;

  const host = url.hostname.toLowerCase();
  const parts = url.pathname.split("/").filter((part) => part.length > 0);
  let organization: string;
  let projectPart: string;
  let repoPart: string;
  let collectionUri: string;

  if (host === "dev.azure.com") {
    if (parts.length !== 4 || parts[2]?.toLowerCase() !== "_git") return null;
    const orgPart = decodeSegment(parts[0] ?? "");
    if (!orgPart) return null;
    organization = orgPart.toLowerCase();
    projectPart = parts[1] ?? "";
    repoPart = parts[3] ?? "";
    collectionUri = `https://dev.azure.com/${orgPart}/`;
  } else if (host.endsWith(".visualstudio.com")) {
    if (parts.length !== 3 || parts[1]?.toLowerCase() !== "_git") return null;
    organization = host.slice(0, -".visualstudio.com".length);
    if (organization.length === 0) return null;
    projectPart = parts[0] ?? "";
    repoPart = parts[2] ?? "";
    collectionUri = `https://${organization}.visualstudio.com/`;
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
  const host = url.hostname.toLowerCase();
  if (host === "dev.azure.com") {
    const org = url.pathname.split("/").find((part) => part.length > 0);
    return org ? decodeSegment(org)?.toLowerCase() ?? null : null;
  }
  if (host.endsWith(".visualstudio.com")) {
    const org = host.slice(0, -".visualstudio.com".length);
    return org.length > 0 ? org : null;
  }
  return null;
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
