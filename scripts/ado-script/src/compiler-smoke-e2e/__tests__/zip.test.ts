import { describe, expect, it } from "vitest";

import { listZipEntries, readZipEntryByBasename } from "../zip.js";
import { makeZip } from "./zip-fixture.js";

describe("readZipEntryByBasename", () => {
  it("reads stored and deflated entries beneath the artifact root", () => {
    const zip = makeZip([
      { name: "safe_outputs/", data: "" },
      { name: "safe_outputs/logs/agent-output.txt", data: "log" },
      { name: "safe_outputs/safe-outputs-executed.ndjson", data: '{"name":"noop"}\n', deflate: true },
    ]);
    expect(listZipEntries(zip)).toContain("safe_outputs/safe-outputs-executed.ndjson");
    expect(readZipEntryByBasename(zip, "safe-outputs-executed.ndjson")?.toString()).toBe(
      '{"name":"noop"}\n',
    );
    expect(readZipEntryByBasename(zip, "agent-output.txt")?.toString()).toBe("log");
  });

  it("uses central-directory sizes when the local header defers them", () => {
    const zip = makeZip([{ name: "a/x.ndjson", data: "payload", deflate: true, dataDescriptor: true }]);
    expect(readZipEntryByBasename(zip, "x.ndjson")?.toString()).toBe("payload");
  });

  it("returns undefined for a missing entry and rejects ambiguous matches", () => {
    const zip = makeZip([
      { name: "a/x.txt", data: "1" },
      { name: "b/x.txt", data: "2" },
    ]);
    expect(readZipEntryByBasename(zip, "y.txt")).toBeUndefined();
    expect(() => readZipEntryByBasename(zip, "x.txt")).toThrow(/2 entries named 'x.txt'/);
  });

  it("rejects a buffer that is not a zip archive", () => {
    expect(() => readZipEntryByBasename(Buffer.from("<html>sign in</html>"), "x")).toThrow(
      /not a zip archive/,
    );
  });
});
