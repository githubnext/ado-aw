/**
 * Bypass logic: when ADO_BUILD_REASON does not match the spec's expected
 * build reason (e.g. spec is for PullRequest but build was Manual), the
 * gate auto-passes.
 */
import type { GateSpec } from "../shared/types.gen.js";
import { setOutput, addBuildTag, complete, logInfo } from "../shared/vso-logger.js";

export async function runBypass(spec: GateSpec): Promise<boolean> {
  const buildReason = process.env.ADO_BUILD_REASON ?? "";
  if (buildReason !== spec.context.build_reason) {
    // Routed through logInfo so the (compiler-controlled but theoretically
    // template-influenceable) bypass_label cannot smuggle a `##vso[` prefix
    // into the line. Mirrors the Python log line for parity.
    logInfo(`Not a ${spec.context.bypass_label} build -- gate passes automatically`);
    setOutput("SHOULD_RUN", "true");
    addBuildTag(`${spec.context.tag_prefix}:passed`);
    complete("Succeeded");
    return true;
  }
  return false;
}
