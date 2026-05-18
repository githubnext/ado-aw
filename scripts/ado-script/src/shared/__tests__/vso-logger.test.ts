import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { setOutput, addBuildTag, logWarning, logError, complete, logInfo, _resetCompletedForTesting } from "../vso-logger.js";

describe("vso-logger", () => {
  let writes: string[];

  beforeEach(() => {
    writes = [];
    _resetCompletedForTesting();
    vi.spyOn(process.stdout, "write").mockImplementation((chunk: any) => {
      writes.push(typeof chunk === "string" ? chunk : chunk.toString());
      return true;
    });
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("setOutput emits a setvariable command with isOutput=true", () => {
    setOutput("SHOULD_RUN", "true");
    expect(writes).toEqual([
      "##vso[task.setvariable variable=SHOULD_RUN;isOutput=true]true\n",
    ]);
  });

  it("setOutput escapes ; in name and passes ] through in value", () => {
    setOutput("a;b", "v]w");
    expect(writes[0]).toContain("variable=a%3Bb");
    // Value is in the message body (after closing ]), so ] is NOT escaped
    expect(writes[0]).toContain("]v]w\n");
  });

  it("addBuildTag emits a build tag command with message escaping", () => {
    addBuildTag("gate%tag\r\npassed");
    expect(writes[0]).toBe("##vso[build.addbuildtag]gate%25tag%0D%0Apassed\n");
  });

  it("logWarning escapes newlines in message", () => {
    logWarning("line1\nline2");
    expect(writes[0]).toBe("##vso[task.logissue type=warning;]line1%0Aline2\n");
  });

  it("logError escapes carriage returns", () => {
    logError("line1\rline2");
    expect(writes[0]).toBe("##vso[task.logissue type=error;]line1%0Dline2\n");
  });

  it("complete defaults message to 'done'", () => {
    complete("Succeeded");
    expect(writes[0]).toBe("##vso[task.complete result=Succeeded;]done\n");
  });

  it("complete passes through a custom message", () => {
    complete("Failed", "boom");
    expect(writes[0]).toBe("##vso[task.complete result=Failed;]boom\n");
  });

  it("complete escapes % in message", () => {
    complete("SucceededWithIssues", "100% done");
    expect(writes[0]).toBe(
      "##vso[task.complete result=SucceededWithIssues;]100%25 done\n",
    );
  });

  it("escapeProperty encodes = and space (latent-injection defense)", () => {
    setOutput("name with space", "v");
    expect(writes[0]).toContain("variable=name%20with%20space");

    writes.length = 0;
    setOutput("a=b", "v");
    expect(writes[0]).toContain("variable=a%3Db");
  });

  it("complete() is idempotent — second call is a no-op", () => {
    complete("Succeeded", "first");
    complete("Failed", "second");
    expect(writes).toHaveLength(1);
    expect(writes[0]).toContain("result=Succeeded");
    expect(writes[0]).toContain("first");
  });

  it("logInfo writes an escaped non-vso line and neutralises a leading '#'", () => {
    logInfo("hello world");
    expect(writes).toEqual(["hello world\n"]);

    // Leading `#` is encoded so an adversarial message cannot smuggle a
    // `##vso[` command (ADO interprets that prefix only at line-start).
    writes.length = 0;
    logInfo("##vso[task.complete result=Failed;] line");
    expect(writes[0]!.startsWith("#")).toBe(false);
    expect(writes[0]).toContain("%23#vso[task.complete result=Failed;] line");

    // Embedded newlines are encoded so they can't break out either.
    writes.length = 0;
    logInfo("first\nsecond");
    expect(writes[0]).toBe("first%0Asecond\n");
  });
});
