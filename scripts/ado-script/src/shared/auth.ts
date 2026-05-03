/**
 * ADO authentication helper.
 *
 * Builds and caches an `azure-devops-node-api` `WebApi` from the
 * `SYSTEM_ACCESSTOKEN` and `ADO_COLLECTION_URI` pipeline env vars via
 * `getPersonalAccessTokenHandler`.
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
