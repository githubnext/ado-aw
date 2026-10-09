// AUTO-GENERATED from Rust via cargo run -- export-ado-proxy-catalog-schema. Do not edit; run npm run codegen.

export type Capability = "discovery" | "core" | "repos" | "pipelines" | "boards";
export type HostPolicy = "current-organization" | "sps-fallback";
export type HttpMethod = "GET" | "OPTIONS";
export type ResponsePolicy =
  | "json"
  | "filter-projects"
  | "filter-resource-areas"
  | "validate-project"
  | "validate-project-and-repository";
export type ScopePolicy =
  | "current-organization"
  | "allowed-resource-area"
  | "current-project-path"
  | "current-repository-path"
  | "filter-projects-to-current"
  | "filter-resource-areas"
  | "response-current-project"
  | "response-current-repository";
/**
 * How the package credential is presented upstream.
 *
 * Mirrors what the protocol's official Azure Pipelines authenticate task
 * makes its client send, so Azure Artifacts sees the same request shape.
 */
export type PackageAuthScheme = "bearer" | "basic";
export type PackageMethod = "GET" | "HEAD";
/**
 * A package protocol the package family understands.
 */
export type PackageProtocolId = "npm" | "pypi" | "nuget" | "cargo";

export interface Catalog {
  /**
   * Inclusive `[major, minor]` upper bound of the accepted REST API version.
   *
   * @minItems 2
   * @maxItems 2
   */
  api_version_max: [number, number];
  /**
   * Inclusive `[major, minor]` lower bound of the accepted REST API version.
   *
   * @minItems 2
   * @maxItems 2
   */
  api_version_min: [number, number];
  denied_route_families: string[];
  operations: Operation[];
  /**
   * Hosts of the package family. Intercepted only when the policy carries
   * a `packages` section.
   */
  package_hosts: string[];
  /**
   * Per-protocol package request contracts.
   */
  package_protocols: PackageProtocolRoute[];
  /**
   * Host suffixes a package redirect may target.
   */
  package_redirect_host_suffixes: string[];
  /**
   * Largest package response streamed through the proxy.
   */
  package_response_limit: number;
  protected_hosts: string[];
  runtime_available: boolean;
  schema_version: string;
  [k: string]: unknown;
}
export interface Operation {
  allowed_query: string[];
  api_version: string;
  capability: Capability;
  denied_query: string[];
  host: HostPolicy;
  id: string;
  max_response_bytes: number;
  method: HttpMethod;
  response: ResponsePolicy;
  route: string;
  scope: ScopePolicy;
  [k: string]: unknown;
}
/**
 * One package protocol's request contract.
 */
export interface PackageProtocolRoute {
  /**
   * Whether `%2F` may appear inside a path segment. npm encodes the `/` of
   * a scoped package name (`@scope%2Fname`) this way.
   */
  allow_encoded_slash: boolean;
  auth_scheme: PackageAuthScheme;
  /**
   * Methods allowed for this protocol. Only reads.
   */
  methods: PackageMethod[];
  /**
   * Path prefixes (case-insensitive) allowed after
   * `/_packaging/{feed}[@{view}]/`. Anything else under the feed is denied.
   */
  path_prefixes: string[];
  protocol: PackageProtocolId;
  [k: string]: unknown;
}
