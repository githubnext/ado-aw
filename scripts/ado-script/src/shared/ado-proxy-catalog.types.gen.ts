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
