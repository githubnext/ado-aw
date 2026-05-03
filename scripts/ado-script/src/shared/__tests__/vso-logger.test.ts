import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { setOutput, addBuildTag, logWarning, logError, complete } from "../vso-logger.js";

describe("vso-logger", () => {
  let writes: string[];

  beforeEach(() => {
    writes = [];
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

  it("setOutput escapes ; in name and ] in value", () => {
    setOutput("a;b", "v]w");
    expect(writes[0]).toContain("variable=a%3Bb");
    expect(writes[0]).toContain("]v%5Dw\n");
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
});
