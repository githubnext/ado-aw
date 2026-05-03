/**
 * Bypass logic: when ADO_BUILD_REASON does not match the spec's expected
 * build reason (e.g. spec is for PullRequest but build was Manual), the
 * gate auto-passes.
 */
import type { GateSpec } from "../shared/types.gen.js";
import { setOutput, addBuildTag, complete } from "../shared/vso-logger.js";

export async function runBypass(spec: GateSpec): Promise<boolean> {
  const buildReason = process.env.ADO_BUILD_REASON ?? "";
  if (buildReason !== spec.context.build_reason) {
    // Mirror Python log line for parity in pipeline logs
    process.stdout.write(
      `Not a ${spec.context.bypass_label} build -- gate passes automatically\n`,
    );
    setOutput("SHOULD_RUN", "true");
    addBuildTag(`${spec.context.tag_prefix}:passed`);
    complete("Succeeded");
    return true;
  }
  return false;
}
