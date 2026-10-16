/**
 * Runtime configuration for the `ado-proxy` sidecar.
 *
 * Everything here is non-secret. The bearer never appears in argv, the
 * environment, or this module: it lives in a private file the trusted host task
 * rotates, read on demand by `token.ts`.
 *
 * Sources, in precedence order: explicit CLI flags, then the generic
 * `AWF_POLICY_PROXY_*` environment contract AWF publishes for any policy-proxy
 * sidecar, then defaults. Invalid or missing required values are fatal — a
 * half-configured proxy would silently downgrade to an open tunnel.
 */
import { readFileSync } from "node:fs";

import type {
  Capability,
  PackageProtocolId,
} from "../shared/ado-proxy-catalog.types.gen.js";
import {
  CATALOG_SCHEMA_VERSION,
  PACKAGE_HOSTS,
  PACKAGE_PROTOCOLS,
  PROTECTED_HOSTS,
  canonicalizeHost,
} from "./catalog.js";
import { projectScopeDefaults } from "./scope.js";

/** Resolved, validated proxy configuration. */
export interface ProxyConfig {
  /** Address the agent's `HTTP(S)_PROXY` points at. */
  readonly listenAddress: string;
  /** Port the agent's `HTTP(S)_PROXY` points at. */
  readonly listenPort: number;
  /** Squid URL. The proxy's only route out; there is no direct-internet path. */
  readonly upstreamProxy: string;
  /** Pre-created file the public interception certificate is written into. */
  readonly publicCaFile: string;
  /** Directory for the sanitized JSONL decision log, when configured. */
  readonly logDir?: string;
  /**
   * Port for direct TLS, where clients connect believing they are talking to
   * Azure DevOps itself.
   *
   * Defaults to 443, since a client redirected by `--add-host` or pointed at
   * the engine's hostname uses the ordinary HTTPS port. Configurable only so
   * tests can run unprivileged.
   */
  readonly tlsPort: number;
  /** The scope and capability policy this proxy enforces. */
  readonly policy: ProxyPolicy;
}

/** The compiler-emitted policy document. */
/** One project's grant inside an organization scope. */
export interface PolicyProjectScope {
  /** Project name. */
  readonly project: string;
  /** Project id (GUID), when the author supplied one. */
  readonly project_id?: string;
  /**
   * Whether project-addressed reads are granted.
   *
   * True when the author named the project in `permissions.read.allow`. False
   * for a scope derived from a `repos:` declaration, which grants only the
   * repositories it names — declaring a repository is not a request for the
   * work items and pipelines beside it.
   */
  readonly project_scoped?: boolean;
  /** Repository names and/or ids granted within this project. */
  readonly repositories?: readonly string[];
}

/** An organization and the projects granted within it. */
export interface PolicyOrganizationScope {
  readonly organization: string;
  readonly projects: readonly PolicyProjectScope[];
}

/**
 * One Azure Artifacts feed the agent may read through the package family.
 *
 * Names come from the front matter. The `*_id` GUIDs are filled in on the
 * trusted host by `resolve-feeds` before the proxy starts, so a client that
 * addresses the feed (or its project or view) by id matches the same grant as
 * one that uses the name.
 */
export interface PackageFeedGrant {
  readonly organization: string;
  /**
   * Project of a project-scoped feed.
   *
   * Absent for an organization-scoped feed, in which case only the
   * organization-scoped URL shape (`/{org}/_packaging/…`) matches.
   */
  readonly project?: string;
  readonly feed: string;
  /**
   * View the agent is pinned to (`feed@view`).
   *
   * When set, a request without a view, or naming another view, is denied: the
   * author chose to expose only, say, `@Release`, and the bare feed would
   * include unpromoted packages.
   */
  readonly view?: string;
  /** Package protocols the agent may use against this feed. */
  readonly protocols: readonly PackageProtocolId[];
  readonly project_id?: string;
  readonly feed_id?: string;
  readonly view_id?: string;
}

/** The `packages` policy section: which feeds, on which hosts. */
export interface PackagePolicy {
  /** Package hosts to intercept. Always covers the catalog's package hosts. */
  readonly hosts: readonly string[];
  /** Feed grants; never empty. */
  readonly feeds: readonly PackageFeedGrant[];
}

export interface ProxyPolicy {
  /**
   * Catalog version this document was generated against.
   *
   * Re-checked against the version compiled into this bundle at startup, so a
   * stale mounted policy file fails closed instead of under-enforcing.
   */
  readonly catalog_version: string;
  /** Azure DevOps organization the agent is scoped to. */
  readonly organization: string;
  /** Project name the agent is scoped to. */
  readonly project: string;
  /** Project id (GUID), when the compiler could resolve one. */
  readonly project_id?: string;
  /** Repository name the agent is scoped to. */
  readonly repository?: string;
  /** Repository id (GUID), when the compiler could resolve one. */
  readonly repository_id?: string;
  /**
   * Scopes beyond the current organization and project.
   *
   * Empty or absent means the agent may read only the scope its own pipeline
   * runs in. Entries come from `permissions.read.allow` (which grants the
   * project) and from `repos:` declarations (which grant only the repository).
   */
  readonly additional_scopes?: readonly PolicyOrganizationScope[];
  /** Enabled capability groups; an operation outside these is denied. */
  readonly capabilities: readonly Capability[];
  /** Hosts whose traffic is TLS-terminated and policy-checked. */
  readonly protected_hosts: readonly string[];
  /** Resource-area ids the SPS fallback discovery route may resolve. */
  readonly allowed_resource_areas: readonly string[];
  /**
   * Azure Artifacts feed grants.
   *
   * Absent means the package family is off: package hosts are byte-tunnelled
   * to Squid exactly as before, and no package credential is required.
   */
  readonly packages?: PackagePolicy;
}

export class ConfigError extends Error {}

function fail(message: string): never {
  throw new ConfigError(message);
}

/** Read a flag from argv (`--name value` or `--name=value`), else the env. */
function readOption(
  argv: readonly string[],
  flag: string,
  envName: string,
): string | undefined {
  const prefixed = `--${flag}=`;
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === undefined) continue;
    if (arg === `--${flag}`) return argv[index + 1];
    if (arg.startsWith(prefixed)) return arg.slice(prefixed.length);
  }
  const fromEnv = process.env[envName];
  return fromEnv === undefined || fromEnv === "" ? undefined : fromEnv;
}

function requireOption(
  argv: readonly string[],
  flag: string,
  envName: string,
): string {
  const value = readOption(argv, flag, envName);
  if (value === undefined || value.trim() === "") {
    fail(`missing required option --${flag} (or ${envName})`);
  }
  return value;
}

function parsePort(raw: string, label: string): number {
  const port = Number(raw);
  if (!Number.isInteger(port) || port < 1 || port > 65535) {
    fail(`${label} must be an integer port in 1-65535, got ${JSON.stringify(raw)}`);
  }
  return port;
}

/** Guard against a policy document that is not a JSON object. */
function asRecord(value: unknown, label: string): Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    fail(`${label} must be a JSON object`);
  }
  return value as Record<string, unknown>;
}

function requireString(
  source: Record<string, unknown>,
  key: string,
): string {
  const value = source[key];
  if (typeof value !== "string" || value.trim() === "") {
    fail(`policy.${key} must be a non-empty string`);
  }
  return value;
}

function optionalString(
  source: Record<string, unknown>,
  key: string,
): string | undefined {
  const value = source[key];
  if (value === undefined || value === null) return undefined;
  if (typeof value !== "string" || value.trim() === "") {
    fail(`policy.${key} must be a non-empty string when present`);
  }
  return value;
}

function requireStringArray(
  source: Record<string, unknown>,
  key: string,
): string[] {
  const value = source[key];
  if (!Array.isArray(value)) fail(`policy.${key} must be an array`);
  return value.map((entry, index) => {
    if (typeof entry !== "string" || entry.trim() === "") {
      fail(`policy.${key}[${index}] must be a non-empty string`);
    }
    return entry;
  });
}

const KNOWN_CAPABILITIES: readonly Capability[] = [
  "discovery",
  "core",
  "repos",
  "pipelines",
  "boards",
];

/**
 * Every key the policy document may carry.
 *
 * An unrecognized key means the compiler emitted a constraint this bundle does
 * not implement. Ignoring it would silently under-enforce, so it is fatal.
 */
const KNOWN_POLICY_KEYS: readonly string[] = [
  "catalog_version",
  "organization",
  "project",
  "project_id",
  "repository",
  "repository_id",
  "capabilities",
  "protected_hosts",
  "allowed_resource_areas",
  "additional_scopes",
  "packages",
];

/** Keys a single `additional_scopes` entry may carry. */
const KNOWN_SCOPE_KEYS: readonly string[] = ["organization", "projects"];

/** Keys a single project entry may carry. */
const KNOWN_PROJECT_KEYS: readonly string[] = [
  "project",
  "project_id",
  "project_scoped",
  "repositories",
];

/**
 * Parse `additional_scopes`, failing closed on anything unrecognized.
 *
 * Strict for the same reason as the top-level document: a key this bundle does
 * not implement means the compiler intended a constraint that would otherwise
 * be silently dropped. An entry naming no projects is refused outright — in
 * the front matter that would be a request to grant an entire organization,
 * and a widening produced by *omitting* a key is exactly the accident this
 * proxy exists to prevent.
 */
function parseAdditionalScopes(document: Record<string, unknown>): PolicyOrganizationScope[] {
  const raw = document.additional_scopes;
  if (raw === undefined) return [];
  if (!Array.isArray(raw)) fail("policy.additional_scopes must be an array");

  return raw.map((entry, index) => {
    const scope = asRecord(entry, `policy.additional_scopes[${index}]`);
    for (const key of Object.keys(scope)) {
      if (!KNOWN_SCOPE_KEYS.includes(key)) {
        fail(`policy.additional_scopes[${index}] has unknown key ${JSON.stringify(key)}`);
      }
    }

    const organization = requireString(scope, "organization");
    const projects = scope.projects;
    if (!Array.isArray(projects) || projects.length === 0) {
      fail(
        `policy.additional_scopes[${index}] (${organization}) lists no projects; ` +
          "an empty list would grant the whole organization",
      );
    }

    return {
      organization,
      projects: projects.map((projectEntry, projectIndex) => {
        const label = `policy.additional_scopes[${index}].projects[${projectIndex}]`;
        const project = asRecord(projectEntry, label);
        for (const key of Object.keys(project)) {
          if (!KNOWN_PROJECT_KEYS.includes(key)) {
            fail(`${label} has unknown key ${JSON.stringify(key)}`);
          }
        }
        const repositories = project.repositories;
        if (repositories !== undefined && !Array.isArray(repositories)) {
          fail(`${label}.repositories must be an array`);
        }
        if (project.project_scoped !== undefined && typeof project.project_scoped !== "boolean") {
          fail(`${label}.project_scoped must be a boolean`);
        }
        return projectScopeDefaults({
          project: requireString(project, "project"),
          project_id: optionalString(project, "project_id"),
          project_scoped: project.project_scoped as boolean | undefined,
          repositories: (repositories ?? []) as readonly string[],
        });
      }),
    };
  });
}

/** Keys the `packages` section may carry. */
const KNOWN_PACKAGES_KEYS: readonly string[] = ["hosts", "feeds"];

/** Keys a single feed grant may carry. */
const KNOWN_FEED_KEYS: readonly string[] = [
  "organization",
  "project",
  "feed",
  "view",
  "protocols",
  "project_id",
  "feed_id",
  "view_id",
];

/** Canonical 8-4-4-4-12 GUID, the only id form the resolver ever writes. */
const GUID = /^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$/;

/** True when `value` is a canonical GUID. */
export function isCanonicalGuid(value: string): boolean {
  return GUID.test(value);
}

function rejectUnknownKeys(
  source: Record<string, unknown>,
  known: readonly string[],
  label: string,
): void {
  for (const key of Object.keys(source)) {
    if (!known.includes(key)) {
      fail(`${label} has unknown key ${JSON.stringify(key)}`);
    }
  }
}

function labelledString(
  source: Record<string, unknown>,
  key: string,
  label: string,
  required: boolean,
): string | undefined {
  const value = source[key];
  if (value === undefined && !required) return undefined;
  if (typeof value !== "string" || value.trim() === "") {
    fail(`${label}.${key} must be a non-empty string${required ? "" : " when present"}`);
  }
  return value;
}

function labelledGuid(
  source: Record<string, unknown>,
  key: string,
  label: string,
): string | undefined {
  const value = labelledString(source, key, label, false);
  if (value !== undefined && !isCanonicalGuid(value)) {
    fail(`${label}.${key} must be a canonical GUID (8-4-4-4-12 hex)`);
  }
  return value;
}

/**
 * Parse the optional `packages` section, failing closed on anything unknown.
 *
 * The section turns on interception of the package hosts, so every field is
 * load-bearing:
 *
 *   - `hosts` must cover the catalogued package hosts — a host missing here
 *     would be byte-tunnelled with no policy at all — and may not add hosts the
 *     catalog does not know, because the package authorizer has no rules for
 *     them;
 *   - `feeds` must name at least one feed, each with at least one known,
 *     non-duplicated protocol. An empty list would intercept the host only to
 *     deny everything, which is almost certainly a compiler bug worth surfacing;
 *   - `*_id` values must be canonical GUIDs, since they are compared as feed,
 *     project, and view identifiers and anything looser could alias a name.
 */
function parsePackages(document: Record<string, unknown>): PackagePolicy | undefined {
  const raw = document.packages;
  if (raw === undefined) return undefined;
  const section = asRecord(raw, "policy.packages");
  rejectUnknownKeys(section, KNOWN_PACKAGES_KEYS, "policy.packages");

  const hostsRaw = section.hosts;
  if (!Array.isArray(hostsRaw)) fail("policy.packages.hosts must be an array");
  const hosts = hostsRaw.map((entry, index) => {
    if (typeof entry !== "string" || entry.trim() === "") {
      fail(`policy.packages.hosts[${index}] must be a non-empty string`);
    }
    return entry;
  });
  const catalogued = PACKAGE_HOSTS.map(canonicalizeHost);
  for (const host of hosts) {
    if (!catalogued.includes(canonicalizeHost(host))) {
      fail(
        `policy.packages.hosts names ${host}, which is not a catalogued package host; ` +
          "the proxy has no rules for it",
      );
    }
  }
  for (const host of PACKAGE_HOSTS) {
    if (!hosts.some((entry) => canonicalizeHost(entry) === canonicalizeHost(host))) {
      fail(
        `policy.packages.hosts omits the catalogued package host ${host}; ` +
          "it would bypass policy enforcement.",
      );
    }
  }

  const feedsRaw = section.feeds;
  if (!Array.isArray(feedsRaw) || feedsRaw.length === 0) {
    fail("policy.packages.feeds must be a non-empty array");
  }
  const knownProtocols = PACKAGE_PROTOCOLS.map((route) => route.protocol);

  const feeds = feedsRaw.map((entry, index): PackageFeedGrant => {
    const label = `policy.packages.feeds[${index}]`;
    const grant = asRecord(entry, label);
    rejectUnknownKeys(grant, KNOWN_FEED_KEYS, label);

    const protocolsRaw = grant.protocols;
    if (!Array.isArray(protocolsRaw) || protocolsRaw.length === 0) {
      fail(`${label}.protocols must be a non-empty array`);
    }
    const protocols: PackageProtocolId[] = [];
    for (const protocol of protocolsRaw) {
      if (typeof protocol !== "string" || !knownProtocols.includes(protocol as PackageProtocolId)) {
        fail(`${label}.protocols contains an unknown protocol: ${String(protocol)}`);
      }
      if (protocols.includes(protocol as PackageProtocolId)) {
        fail(`${label}.protocols lists ${protocol} more than once`);
      }
      protocols.push(protocol as PackageProtocolId);
    }

    const project = labelledString(grant, "project", label, false);
    const projectId = labelledGuid(grant, "project_id", label);
    if (projectId !== undefined && project === undefined) {
      // A project id with no project would silently turn an
      // organization-scoped grant into a project-scoped one keyed only by id.
      fail(`${label}.project_id is set but project is not`);
    }
    const view = labelledString(grant, "view", label, false);
    const viewId = labelledGuid(grant, "view_id", label);
    if (viewId !== undefined && view === undefined) {
      fail(`${label}.view_id is set but view is not`);
    }

    return {
      organization: labelledString(grant, "organization", label, true) as string,
      feed: labelledString(grant, "feed", label, true) as string,
      protocols,
      ...(project === undefined ? {} : { project }),
      ...(view === undefined ? {} : { view }),
      ...(projectId === undefined ? {} : { project_id: projectId }),
      ...optionalField("feed_id", labelledGuid(grant, "feed_id", label)),
      ...(viewId === undefined ? {} : { view_id: viewId }),
    };
  });

  return { hosts, feeds };
}

function optionalField<K extends string>(
  key: K,
  value: string | undefined,
): Partial<Record<K, string>> {
  return value === undefined ? {} : ({ [key]: value } as Record<K, string>);
}

/**
 * Parse and validate the compiler-emitted policy document.
 *
 * Fails closed on: a non-object document, a missing or mismatched
 * `catalog_version`, an unknown key, an unknown capability, a protected-host
 * set that does not cover the catalog, or a missing required scope. Any of
 * those would otherwise let the proxy enforce a different policy than the
 * compiler intended.
 */
export function parsePolicy(raw: string): ProxyPolicy {
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch (error) {
    fail(`policy file is not valid JSON: ${(error as Error).message}`);
  }
  const document = asRecord(parsed, "policy");

  for (const key of Object.keys(document)) {
    if (!KNOWN_POLICY_KEYS.includes(key)) {
      fail(
        `policy contains unknown key ${JSON.stringify(key)}. Refusing to start: ` +
          "an unrecognized constraint would be silently ignored.",
      );
    }
  }

  const catalogVersion = requireString(document, "catalog_version");
  if (catalogVersion !== CATALOG_SCHEMA_VERSION) {
    fail(
      `policy catalog_version ${JSON.stringify(catalogVersion)} does not match ` +
        `this bundle's ${JSON.stringify(CATALOG_SCHEMA_VERSION)}. Refusing to ` +
        "start: a stale policy document would under-enforce.",
    );
  }

  const capabilities = requireStringArray(document, "capabilities");
  // An empty list is legitimate: a packages-only workflow enables no REST
  // capability, and the REST authorizer then denies every operation.
  for (const capability of capabilities) {
    if (!KNOWN_CAPABILITIES.includes(capability as Capability)) {
      fail(`policy.capabilities contains an unknown capability: ${capability}`);
    }
  }

  const protectedHosts = requireStringArray(document, "protected_hosts");
  if (protectedHosts.length === 0) {
    fail("policy.protected_hosts must not be empty");
  }
  for (const catalogued of PROTECTED_HOSTS) {
    // A catalogued host missing here would be byte-tunnelled to Squid instead
    // of policed, which is the one failure mode this proxy cannot tolerate.
    if (!protectedHosts.some((host) => host.toLowerCase() === catalogued.toLowerCase())) {
      fail(
        `policy.protected_hosts omits the catalogued host ${catalogued}; ` +
          "it would bypass policy enforcement.",
      );
    }
  }

  return {
    catalog_version: catalogVersion,
    organization: requireString(document, "organization"),
    project: requireString(document, "project"),
    project_id: optionalString(document, "project_id"),
    repository: optionalString(document, "repository"),
    repository_id: optionalString(document, "repository_id"),
    capabilities: capabilities as Capability[],
    protected_hosts: protectedHosts,
    allowed_resource_areas: Array.isArray(document.allowed_resource_areas)
      ? requireStringArray(document, "allowed_resource_areas")
      : [],
    additional_scopes: parseAdditionalScopes(document),
    ...optionalPackages(parsePackages(document)),
  };
}

function optionalPackages(
  packages: PackagePolicy | undefined,
): { packages?: PackagePolicy } {
  return packages === undefined ? {} : { packages };
}

/** Resolve the full runtime configuration from argv and the environment. */
export function loadConfig(argv: readonly string[]): ProxyConfig {
  const policyFile = requireOption(argv, "policy-file", "ADO_PROXY_POLICY_FILE");
  let policyRaw: string;
  try {
    policyRaw = readFileSync(policyFile, "utf8");
  } catch (error) {
    fail(`cannot read policy file ${policyFile}: ${(error as Error).message}`);
  }

  const listenPortRaw =
    readOption(argv, "listen-port", "AWF_POLICY_PROXY_LISTEN_PORT") ?? "11080";
  const tlsPortRaw = readOption(argv, "tls-port", "ADO_PROXY_TLS_PORT") ?? "443";

  return {
    listenAddress:
      readOption(argv, "listen-address", "AWF_POLICY_PROXY_LISTEN_ADDRESS") ??
      "0.0.0.0",
    listenPort: parsePort(listenPortRaw, "--listen-port"),
    tlsPort: parsePort(tlsPortRaw, "--tls-port"),
    upstreamProxy: requireOption(
      argv,
      "upstream-proxy",
      "AWF_POLICY_PROXY_UPSTREAM_PROXY",
    ),
    publicCaFile: requireOption(
      argv,
      "public-ca-file",
      "AWF_POLICY_PROXY_PUBLIC_CA_PATH",
    ),
    logDir: readOption(argv, "log-dir", "AWF_POLICY_PROXY_LOG_DIR"),
    policy: parsePolicy(policyRaw),
  };
}
