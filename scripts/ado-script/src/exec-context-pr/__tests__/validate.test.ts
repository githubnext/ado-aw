import { describe, expect, it } from "vitest";

import { isIdentifierError, validateIdentifiers } from "../validate.js";

function env(overrides: Record<string, string> = {}): NodeJS.ProcessEnv {
  return {
    SYSTEM_PULLREQUEST_PULLREQUESTID: "4242",
    SYSTEM_PULLREQUEST_TARGETBRANCH: "refs/heads/main",
    SYSTEM_TEAMPROJECT: "MyProject",
    BUILD_REPOSITORY_NAME: "my-repo",
    ...overrides,
  };
}

describe("validateIdentifiers", () => {
  it("accepts a well-formed set of identifiers and strips refs/heads/", () => {
    const result = validateIdentifiers(env());
    expect(isIdentifierError(result)).toBe(false);
    if (!isIdentifierError(result)) {
      expect(result.prId).toBe("4242");
      expect(result.project).toBe("MyProject");
      expect(result.repo).toBe("my-repo");
      expect(result.targetBranch).toBe("refs/heads/main");
      expect(result.targetShort).toBe("main");
    }
  });

  it("accepts project names with spaces (ADO allows them)", () => {
    const result = validateIdentifiers(env({ SYSTEM_TEAMPROJECT: "My Project" }));
    expect(isIdentifierError(result)).toBe(false);
  });

  it("rejects non-numeric PR id", () => {
    const result = validateIdentifiers(env({ SYSTEM_PULLREQUEST_PULLREQUESTID: "not-a-number" }));
    expect(isIdentifierError(result)).toBe(true);
    if (isIdentifierError(result)) {
      expect(result.reason).toContain("PR_ID='not-a-number'");
    }
  });

  it("rejects empty PR id (missing env var)", () => {
    const result = validateIdentifiers(env({ SYSTEM_PULLREQUEST_PULLREQUESTID: "" }));
    expect(isIdentifierError(result)).toBe(true);
  });

  it("rejects project name with disallowed characters", () => {
    const result = validateIdentifiers(env({ SYSTEM_TEAMPROJECT: "bad$name" }));
    expect(isIdentifierError(result)).toBe(true);
    if (isIdentifierError(result)) {
      expect(result.reason).toContain("PROJECT='bad$name'");
    }
  });

  it("rejects repo name with spaces", () => {
    const result = validateIdentifiers(env({ BUILD_REPOSITORY_NAME: "bad repo" }));
    expect(isIdentifierError(result)).toBe(true);
    if (isIdentifierError(result)) {
      expect(result.reason).toContain("REPO='bad repo'");
    }
  });

  it("rejects empty target branch with a dedicated message", () => {
    const result = validateIdentifiers(env({ SYSTEM_PULLREQUEST_TARGETBRANCH: "" }));
    expect(isIdentifierError(result)).toBe(true);
    if (isIdentifierError(result)) {
      expect(result.reason).toContain("TargetBranch is empty");
    }
  });

  it("rejects target branch with disallowed characters", () => {
    const result = validateIdentifiers(env({ SYSTEM_PULLREQUEST_TARGETBRANCH: "refs/heads/main; rm -rf /" }));
    expect(isIdentifierError(result)).toBe(true);
    if (isIdentifierError(result)) {
      expect(result.reason).toContain("PR_TARGET_BRANCH=");
    }
  });

  it("accepts branch names with slashes, dots, dashes, underscores", () => {
    const result = validateIdentifiers(env({ SYSTEM_PULLREQUEST_TARGETBRANCH: "refs/heads/release/v1.2.3-beta_rc" }));
    expect(isIdentifierError(result)).toBe(false);
    if (!isIdentifierError(result)) {
      expect(result.targetShort).toBe("release/v1.2.3-beta_rc");
    }
  });

  it("handles non-refs/heads/-prefixed branch as a bare name", () => {
    const result = validateIdentifiers(env({ SYSTEM_PULLREQUEST_TARGETBRANCH: "main" }));
    expect(isIdentifierError(result)).toBe(false);
    if (!isIdentifierError(result)) {
      expect(result.targetShort).toBe("main");
    }
  });
});
