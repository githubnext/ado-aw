import { describe, expect, it } from "vitest";

import { Teardown } from "../scenarios/common.js";

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
