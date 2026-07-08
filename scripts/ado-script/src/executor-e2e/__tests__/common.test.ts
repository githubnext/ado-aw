import { describe, expect, it } from "vitest";

import { stagedSafeOutputFile, Teardown } from "../scenarios/common.js";

describe("Teardown", () => {
  it("runs every step even when an earlier one throws", async () => {
    const ran: string[] = [];
    const teardown = new Teardown()
      .add("first", async () => {
        ran.push("first");
        throw new Error("boom");
      })
      .add("second", async () => {
        ran.push("second");
      })
      .add("third", async () => {
        ran.push("third");
      });

    await expect(teardown.run()).rejects.toThrow(/teardown had 1 failure/);
    // The throwing step must NOT prevent later steps from running.
    expect(ran).toEqual(["first", "second", "third"]);
  });

  it("resolves cleanly when all steps succeed", async () => {
    const ran: string[] = [];
    await new Teardown()
      .add("a", async () => {
        ran.push("a");
      })
      .add("b", async () => {
        ran.push("b");
      })
      .run();
    expect(ran).toEqual(["a", "b"]);
  });

  it("aggregates all failures with their labels", async () => {
    const teardown = new Teardown()
      .add("delete branch", async () => {
        throw new Error("409 conflict");
      })
      .add("abandon pr", async () => {
        throw new Error("network");
      });

    await expect(teardown.run()).rejects.toThrow(
      /teardown had 2 failure\(s\).*delete branch: 409 conflict.*abandon pr: network/s,
    );
  });

  it("runs in registration order", async () => {
    const order: number[] = [];
    await new Teardown()
      .add("1", async () => {
        order.push(1);
      })
      .add("2", async () => {
        order.push(2);
      })
      .add("3", async () => {
        order.push(3);
      })
      .run();
    expect(order).toEqual([1, 2, 3]);
  });
});

describe("stagedSafeOutputFile", () => {
  it("emits the staged result fields required by staged-file executors", () => {
    const staged = stagedSafeOutputFile(
      "upload-build-attachment",
      "ado-aw-det-123",
      "build-att.txt",
      "hello\n",
    );

    expect(staged.result).toEqual({
      file_path: "build-att.txt",
      staged_file: "upload-build-attachment-ado-aw-det-123-e2e.txt",
      file_size: 6,
      staged_sha256: "5891b5b522d5df086d0ff0b110fbd9d21bb4fc7163af34d08286a2e846f6be03",
    });
    expect(staged.files).toEqual({
      "build-att.txt": "hello\n",
      "upload-build-attachment-ado-aw-det-123-e2e.txt": "hello\n",
    });
  });

  it("records byte length for UTF-8 content and handles extensionless files", () => {
    const staged = stagedSafeOutputFile(
      "upload-pipeline-artifact",
      "ado-aw-det-art-123",
      "artifact",
      "£\n",
    );

    expect(staged.result.file_size).toBe(3);
    expect(staged.result.staged_file).toBe("upload-pipeline-artifact-ado-aw-det-art-123-e2e");
    expect(staged.files).toEqual({
      artifact: "£\n",
      "upload-pipeline-artifact-ado-aw-det-art-123-e2e": "£\n",
    });
  });
});
