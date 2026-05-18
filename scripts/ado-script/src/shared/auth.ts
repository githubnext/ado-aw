/**
 * ADO authentication helper.
 *
 * Builds and caches an `azure-devops-node-api` `WebApi` from the
 * `SYSTEM_ACCESSTOKEN` and `ADO_COLLECTION_URI` pipeline env vars via
 * `getPersonalAccessTokenHandler`.
 *
 * The `azure-devops-node-api` package is heavy (~1 MB; includes
 * `typed-rest-client` and `tunnel` transitive deps). Loading it eagerly
 * adds ~50–100 ms of startup latency that is wasted whenever the gate
 * is invoked for a code path that never touches the ADO REST API
 * (e.g. a manual build that hits the bypass branch in `bypass.ts`, or
 * a pipeline whose facts are all pipeline variables). The dynamic
 * `import()` below is statically analysable by ncc, so the SDK is
 * still bundled into `dist/gate/index.js` — only its module-evaluation
 * cost is deferred until the first `getWebApi()` call.
 *
 * Env-var contract (set by the compiler in
 * `src/compile/filter_ir.rs::compile_gate_step_external` /
 * `collect_ado_exports`):
 *   - `SYSTEM_ACCESSTOKEN` ← `$(System.AccessToken)`
 *   - `ADO_COLLECTION_URI` ← `$(System.CollectionUri)`
 */
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

  const azdev = await import("azure-devops-node-api");
  const handler = azdev.getPersonalAccessTokenHandler(token);
  cached = new azdev.WebApi(orgUrl, handler);
  return cached;
}
