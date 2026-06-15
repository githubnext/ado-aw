/**
 * ADO authentication helper.
 *
 * Builds and caches an `azure-devops-node-api` `WebApi` from the
 * `SYSTEM_ACCESSTOKEN` and `ADO_COLLECTION_URI` pipeline env vars via
 * `getPersonalAccessTokenHandler`.
 *
 * Import shape: the SDK is imported statically. An earlier revision
 * deferred it via `await import(...)` to save ~50–100 ms of
 * module-evaluation time on bypass paths, but ncc compiles dynamic
 * `import()` into a separate webpack chunk file (`<id>.index.js`)
 * that lives alongside the main bundle in `.ado-build/<name>/`. The
 * release pipeline ships only the flat `<name>.js` files
 * (see `scripts/ado-script/package.json`'s `build:*` targets, plus
 * `src/compile/extensions/ado_script.rs`'s per-file download list),
 * so at runtime the chunk was missing and `getWebApi()` failed with
 * `Cannot find module '/tmp/ado-aw-scripts/ado-script/<id>.index.js'`.
 * A static import keeps everything in a single self-contained bundle.
 *
 * Env-var contract (set by the compiler in
 * `src/compile/filter_ir.rs::compile_gate_step_external` /
 * `collect_ado_exports`):
 *   - `SYSTEM_ACCESSTOKEN` ← `$(System.AccessToken)`
 *   - `ADO_COLLECTION_URI` ← `$(System.CollectionUri)`
 */
import * as azdev from "azure-devops-node-api";
import type { WebApi } from "azure-devops-node-api";
import { logError } from "./vso-logger.js";

let cached: WebApi | undefined;

/** For tests only: clear the cached WebApi. */
export function _resetCacheForTesting(): void {
  cached = undefined;
}

export async function getWebApi(): Promise<WebApi> {
  if (cached) return cached;

  const orgUrl = process.env.ADO_COLLECTION_URI;
  const token = process.env.SYSTEM_ACCESSTOKEN;
  if (!orgUrl) {
    const msg = "ADO_COLLECTION_URI env var is missing";
    logError(msg);
    throw new Error(msg);
  }
  if (!token) {
    const msg = "SYSTEM_ACCESSTOKEN env var is missing";
    logError(msg);
    throw new Error(msg);
  }

  const handler = azdev.getPersonalAccessTokenHandler(token);
  cached = new azdev.WebApi(orgUrl, handler);
  return cached;
}
