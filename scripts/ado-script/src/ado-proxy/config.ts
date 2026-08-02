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

import type { Capability } from "../shared/ado-proxy-catalog.types.gen.js";
import { CATALOG_SCHEMA_VERSION, PROTECTED_HOSTS } from "./catalog.js";

/** Resolved, validated proxy configuration. */
export interface ProxyConfig {
  /** Address the agent's `HTTP(S)_PROXY` points at. */
  readonly listenAddress: string;
  /** Port the agent's `HTTP(S)_PROXY` points at. */
  readonly listenPort: number;
  /** Squid URL. The proxy's only route out; there is no direct-internet path. */
  readonly upstreamProxy: string;
  /** Private file the trusted host task rotates the ADO bearer into. */
  readonly tokenFile: string;
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
  /** Enabled capability groups; an operation outside these is denied. */
  readonly capabilities: readonly Capability[];
  /** Hosts whose traffic is TLS-terminated and policy-checked. */
  readonly protected_hosts: readonly string[];
  /** Resource-area ids the SPS fallback discovery route may resolve. */
  readonly allowed_resource_areas: readonly string[];
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
];

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
  };
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
    tokenFile: requireOption(argv, "token-file", "ADO_PROXY_TOKEN_FILE"),
    publicCaFile: requireOption(
      argv,
      "public-ca-file",
      "AWF_POLICY_PROXY_PUBLIC_CA_PATH",
    ),
    logDir: readOption(argv, "log-dir", "AWF_POLICY_PROXY_LOG_DIR"),
    policy: parsePolicy(policyRaw),
  };
}
