/**
 * Concise final results table: fixture / definition / build / url / result / duration.
 *
 * Test-harness module; not shipped in `ado-script.zip`.
 */
import type { FixtureBuildResult } from "./runner.js";

function pad(value: string, width: number): string {
  return value.length >= width ? value : value + " ".repeat(width - value.length);
}

/** Render the final per-fixture outcome table, in the caller's declaration order. */
export function renderResultsTable(results: readonly FixtureBuildResult[]): string {
  const headers = ["fixture", "definition", "build", "url", "result", "duration"];
  const rows = results.map((r) => [
    r.name,
    String(r.definitionId),
    r.buildId !== undefined ? String(r.buildId) : "-",
    r.url ?? "-",
    r.result ? `${r.status} (${r.result})` : r.status,
    `${(r.durationMs / 1000).toFixed(1)}s`,
  ]);
  const widths = headers.map((h, i) => Math.max(h.length, ...rows.map((row) => row[i]?.length ?? 0)));

  const lines = [
    headers.map((h, i) => pad(h, widths[i] ?? h.length)).join("  "),
    widths.map((w) => "-".repeat(w)).join("  "),
    ...rows.map((row) => row.map((cell, i) => pad(cell, widths[i] ?? cell.length)).join("  ")),
  ];
  for (const r of results) {
    if (r.message) lines.push(`  [${r.name}] ${r.message}`);
  }
  return lines.join("\n");
}
