import type { BoundaryTimelineRecord } from "./ado-rest.js";

export function verifyPrBoundary(
  mode: "automatic" | "rejected" | "approved",
  buildId: number,
  before: string,
  after: string | undefined,
  records: readonly BoundaryTimelineRecord[],
): void {
  const job = (id: string) => records.find((record) =>
    record.type === "Job" && (record.identifier === id || record.identifier?.endsWith(`.${id}`)));
  for (const id of ["Setup", "Agent", "Detection", "SafeOutputs"]) {
    if (job(id)?.result !== "succeeded") throw new Error(`Boundary prerequisite ${id} did not succeed`);
  }
  if (mode === "rejected") {
    if (job("ManualReview")?.result !== "failed") throw new Error("Expected manual rejection was not observed");
    if (job("SafeOutputs_Reviewed")?.result !== "skipped") throw new Error("Reviewed executor was not skipped");
    if (after !== before) throw new Error("Reviewed PR changed despite gate rejection");
  } else {
    if (mode === "approved" && (job("ManualReview")?.result !== "succeeded"
      || job("SafeOutputs_Reviewed")?.result !== "succeeded")) {
      throw new Error("Approved gate and reviewed execution did not both succeed");
    }
    if (after !== `ado-aw-pr-boundary-${buildId}`) throw new Error("Expected PR mutation was not persisted");
  }
}
