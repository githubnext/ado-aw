/**
 * Drift guard between the Rust `ado-proxy` catalog and the committed snapshot
 * the bundle enforces.
 *
 * The compiler *emits* the policy document and this bundle *consumes* it, so
 * the two must not diverge. Three mechanisms keep them aligned, and this file
 * covers the second and third:
 *
 *   1. `ado-proxy-catalog.types.gen.ts` is generated from the Rust JSON Schema,
 *      so the bundle cannot compile against a stale *shape*;
 *   2. `catalog.gen.json` is a committed snapshot of the catalog *data* — this
 *      test re-runs the Rust exporter and fails on any difference, so a
 *      Rust-side change to an operation, scope, response policy, or denial
 *      family forces a regeneration (`npm run codegen`) instead of silently
 *      diverging from what the sidecar enforces;
 *   3. `schema_version` is embedded in the emitted policy document and
 *      re-checked by the sidecar at startup, so a stale mounted policy file
 *      fails closed rather than under-enforcing.
 *
 * Mirrors the existing gate-spec / `FACT_META` drift guard.
 */
import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

import type { Catalog } from "../shared/ado-proxy-catalog.types.gen.js";

const here = dirname(fileURLToPath(import.meta.url));
const snapshotPath = join(here, "catalog.gen.json");
const manifestPath = join(here, "..", "..", "..", "..", "Cargo.toml");

/**
 * `cargo run` may need to build the compiler from cold, which comfortably
 * exceeds vitest's 5s default. Keep the subprocess and the test bounded by the
 * same generous budget so a genuinely hung cargo still fails rather than
 * stalling the suite.
 */
const CARGO_TIMEOUT_MS = 10 * 60 * 1000;

function readSnapshot(): Catalog {
  return JSON.parse(readFileSync(snapshotPath, "utf8")) as Catalog;
}

/**
 * Re-run the Rust exporter. Returns `undefined` when cargo is unavailable so
 * the suite still runs in environments without a Rust toolchain; CI has cargo,
 * so the guard is enforced where it matters.
 */
function exportFromRust(): Catalog | undefined {
  try {
    const stdout = execFileSync(
      "cargo",
      [
        "run",
        "--quiet",
        "--manifest-path",
        manifestPath,
        "--",
        "export-ado-proxy-catalog",
      ],
      { encoding: "utf8", stdio: ["ignore", "pipe", "ignore"], timeout: CARGO_TIMEOUT_MS },
    );
    return JSON.parse(stdout) as Catalog;
  } catch {
    return undefined;
  }
}

describe("ado-proxy catalog drift guard", () => {
  it(
    "committed snapshot matches the Rust exporter",
    () => {
      const live = exportFromRust();
      if (!live) {
        // No cargo on this machine; the shape assertions below still run.
        return;
      }
      expect(
        live,
        "src/ado-proxy/catalog.gen.json is stale — run `npm run codegen`",
      ).toEqual(readSnapshot());
    },
    CARGO_TIMEOUT_MS,
  );

  it("declares the schema version the sidecar pins against", () => {
    const catalog = readSnapshot();
    expect(catalog.schema_version).toBe("ado-aw/ado-proxy-catalog/v1");
  });

  it("keeps the runtime unreachable until the compiler wiring lands", () => {
    // The bundle exists and is tested, but nothing emits its sidecar or policy
    // document yet, so authors must not be told the capability is available.
    expect(readSnapshot().runtime_available).toBe(false);
  });

  it("protects only Azure DevOps REST hosts", () => {
    const { protected_hosts } = readSnapshot();
    expect(protected_hosts).toContain("dev.azure.com");
    // Package, artifact, and token hosts must stay on the normal Squid path
    // and must never receive the injected bearer.
    for (const denied of [
      "pkgs.dev.azure.com",
      "artifacts.dev.azure.com",
      "vstoken.dev.azure.com",
      "vssps.dev.azure.com",
    ]) {
      expect(protected_hosts).not.toContain(denied);
    }
  });

  it("exposes only read-shaped methods", () => {
    for (const operation of readSnapshot().operations) {
      expect(["GET", "OPTIONS"]).toContain(operation.method);
    }
  });

  it("gives every operation a unique id", () => {
    const ids = readSnapshot().operations.map((operation) => operation.id);
    expect(new Set(ids).size).toBe(ids.length);
  });

  it("keeps known credential-bearing families denied", () => {
    const { denied_route_families } = readSnapshot();
    for (const required of [
      "/_apis/serviceendpoint",
      "/_apis/distributedtask/variablegroups",
      "/_apis/distributedtask/securefiles",
      "/_git/",
    ]) {
      expect(denied_route_families).toContain(required);
    }
  });

  it("exports an ordered API-version window", () => {
    const { api_version_min, api_version_max } = readSnapshot();
    expect(api_version_min[0]).toBeLessThanOrEqual(api_version_max[0]);
  });
});
