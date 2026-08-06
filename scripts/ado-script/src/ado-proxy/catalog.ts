/**
 * The `ado-proxy` operation catalog, as the bundle sees it.
 *
 * The catalog is **authored in Rust** (`src/ado_proxy/catalog.rs`) and exported
 * here by `npm run codegen` as `catalog.gen.json`, with its TypeScript shape
 * generated into `../shared/ado-proxy-catalog.types.gen.ts`. This module only
 * loads that snapshot and exposes helpers over it — it never restates policy,
 * so the compiler and the sidecar cannot disagree about what is allowed.
 *
 * A drift test (`catalog-drift.test.ts`) re-runs the Rust exporter and fails if
 * the snapshot is stale.
 */
import catalogJson from "./catalog.gen.json" with { type: "json" };

import type {
  Capability,
  Catalog,
  Operation,
} from "../shared/ado-proxy-catalog.types.gen.js";

/** The committed catalog snapshot. */
export const CATALOG: Catalog = catalogJson as Catalog;

/**
 * Catalog version this bundle enforces.
 *
 * Embedded by the compiler into the emitted policy document and re-checked at
 * startup, so a stale policy file fails closed.
 */
export const CATALOG_SCHEMA_VERSION: string = CATALOG.schema_version;

/** Hosts whose traffic is TLS-terminated and policy-checked. */
export const PROTECTED_HOSTS: readonly string[] = CATALOG.protected_hosts;

/** Route families that are always denied, regardless of capability. */
export const DENIED_ROUTE_FAMILIES: readonly string[] =
  CATALOG.denied_route_families;

/** Every catalogued operation. */
export const OPERATIONS: readonly Operation[] = CATALOG.operations;

/** Operations reachable with the given capability set. */
export function operationsFor(
  capabilities: readonly Capability[],
): readonly Operation[] {
  const enabled = new Set(capabilities);
  return OPERATIONS.filter((operation) => enabled.has(operation.capability));
}

/**
 * Canonicalize a host for protection checks.
 *
 * Handles the forms a CONNECT target or `Host` header can legitimately take
 * for the *same* origin:
 *
 *   - surrounding whitespace and mixed case;
 *   - a `host:port` suffix (CONNECT targets always carry one);
 *   - a trailing DNS root dot (`dev.azure.com.` is an absolute FQDN for the
 *     same host);
 *   - bracketed IPv6 literals, which are never protected but must not be
 *     mangled into something that accidentally matches.
 */
export function canonicalizeHost(host: string): string {
  let value = host.trim().toLowerCase();

  if (value.startsWith("[")) {
    // IPv6 literal: keep the bracketed form, drop only a trailing :port.
    const closing = value.indexOf("]");
    if (closing !== -1) value = value.slice(0, closing + 1);
    return value;
  }

  const colon = value.lastIndexOf(":");
  if (colon !== -1 && /^\d+$/.test(value.slice(colon + 1))) {
    value = value.slice(0, colon);
  }

  // Strip the DNS root dot. Repeated so `host..` cannot survive as a variant
  // that fails the equality check while still resolving.
  while (value.endsWith(".")) value = value.slice(0, -1);

  return value;
}

/**
 * True when `host` is one this proxy must terminate and police.
 *
 * Compared as an exact match against the canonicalized protected set — never
 * by suffix, so a look-alike such as `dev.azure.com.evil.test` is not
 * protected (and therefore never receives the injected bearer).
 */
export function isProtectedHost(host: string): boolean {
  const normalized = canonicalizeHost(host);
  if (normalized === "") return false;
  return PROTECTED_HOSTS.some(
    (protectedHost) => canonicalizeHost(protectedHost) === normalized,
  );
}

/** Inclusive `[major, minor]` bounds of the accepted REST API version. */
export const API_VERSION_MIN: readonly [number, number] = CATALOG.api_version_min;
export const API_VERSION_MAX: readonly [number, number] = CATALOG.api_version_max;
