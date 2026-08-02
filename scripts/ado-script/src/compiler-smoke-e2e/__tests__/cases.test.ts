import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { expandBuildTag, parseManifest } from "../cases.js";

const REPO_ROOT = join(dirname(fileURLToPath(import.meta.url)), "..", "..", "..", "..", "..");
const REAL_MANIFEST_PATH = join(REPO_ROOT, "tests", "smoke", "cases.json");

/** A minimal valid manifest; individual tests override one field at a time. */
function manifest(overrides: Record<string, unknown> = {}): string {
  return JSON.stringify({
    schema: "ado-aw/smoke-cases/1",
    yamlPath: ".smoke/pipeline.yml",
    lanes: {
      agentic: { definitionIdEnv: "SMOKE_LANE_AGENTIC_DEFINITION_ID" },
      debug: { definitionIdEnv: "SMOKE_LANE_DEBUG_DEFINITION_ID" },
    },
    cases: [
      {
        id: "canary",
        lane: "agentic",
        kind: "compiled",
        modes: ["candidate", "released"],
        source: "tests/safe-outputs/canary.md",
      },
    ],
    ...overrides,
  });
}

function cases(entries: unknown[]): string {
  return manifest({ cases: entries });
}

describe("parseManifest", () => {
  it("parses a minimal valid manifest", () => {
    const parsed = parseManifest(manifest());
    expect(parsed.yamlPath).toBe(".smoke/pipeline.yml");
    expect(parsed.cases).toHaveLength(1);
    expect(parsed.cases[0]).toMatchObject({ id: "canary", lane: "agentic", kind: "compiled" });
  });

  it("rejects an unsupported schema", () => {
    expect(() => parseManifest(manifest({ schema: "ado-aw/smoke-cases/2" }))).toThrow(
      /unsupported schema/,
    );
  });

  it("rejects malformed JSON", () => {
    expect(() => parseManifest("{not json")).toThrow(/not valid JSON/);
  });

  describe("case id validation (becomes a git ref segment)", () => {
    // The id is interpolated into `refs/heads/<prefix>/<buildId>/<caseId>`, so
    // anything that could smuggle extra path components, ref options, or shell
    // metacharacters must be rejected before git ever sees it.
    for (const id of [
      "../evil",
      "a/b",
      "UPPER",
      "trailing-",
      "-leading",
      "has space",
      "semi;colon",
      "dot.dot",
      "under_score",
      "",
      "a".repeat(50),
    ]) {
      it(`rejects ${JSON.stringify(id)}`, () => {
        expect(() =>
          parseManifest(
            cases([{ id, lane: "agentic", kind: "compiled", modes: ["candidate"], source: "a.md" }]),
          ),
        ).toThrow();
      });
    }

    it("accepts lowercase alphanumerics with interior hyphens", () => {
      const parsed = parseManifest(
        cases([
          {
            id: "custom-safe-output-2",
            lane: "agentic",
            kind: "compiled",
            modes: ["candidate", "released"],
            source: "a.md",
          },
        ]),
      );
      expect(parsed.cases[0]?.id).toBe("custom-safe-output-2");
    });

    it("rejects duplicate ids", () => {
      const entry = {
        id: "canary",
        lane: "agentic",
        kind: "compiled",
        modes: ["candidate"],
        source: "a.md",
      };
      expect(() => parseManifest(cases([entry, { ...entry }]))).toThrow(/duplicate case id/);
    });
  });

  describe("source path validation", () => {
    for (const source of [
      "../outside.md",
      "tests/../../etc/passwd.md",
      "/absolute.md",
      "C:/windows.md",
      "back\\slash.md",
      "tests//double.md",
      "./relative.md",
    ]) {
      it(`rejects ${JSON.stringify(source)}`, () => {
        expect(() =>
          parseManifest(
            cases([
              { id: "x", lane: "agentic", kind: "compiled", modes: ["candidate"], source },
            ]),
          ),
        ).toThrow();
      });
    }
  });

  describe("kind / extension agreement", () => {
    it("rejects a compiled case whose source is not markdown", () => {
      expect(() =>
        parseManifest(
          cases([
            {
              id: "x",
              lane: "agentic",
              kind: "compiled",
              modes: ["candidate"],
              source: "tests/smoke/raw.yml",
            },
          ]),
        ),
      ).toThrow(/must end in \.md/);
    });

    it("rejects a raw case whose source is markdown", () => {
      expect(() =>
        parseManifest(
          cases([
            {
              id: "x",
              lane: "agentic",
              kind: "raw",
              modes: ["candidate"],
              source: "tests/smoke/a.md",
            },
          ]),
        ),
      ).toThrow(/must end in \.yml or \.yaml/);
    });

    it("accepts a raw case with a .yml source", () => {
      const parsed = parseManifest(
        cases([
          {
            id: "awf",
            lane: "agentic",
            kind: "raw",
            modes: ["candidate", "released"],
            source: "tests/smoke/awf/pipeline.yml",
          },
        ]),
      );
      expect(parsed.cases[0]?.kind).toBe("raw");
    });

    it("rejects an unknown kind", () => {
      expect(() =>
        parseManifest(
          cases([
            { id: "x", lane: "agentic", kind: "magic", modes: ["candidate"], source: "a.md" },
          ]),
        ),
      ).toThrow(/kind 'magic'/);
    });
  });

  describe("lane and mode validation", () => {
    it("rejects a case referencing an unknown lane", () => {
      expect(() =>
        parseManifest(
          cases([
            { id: "x", lane: "nope", kind: "compiled", modes: ["candidate"], source: "a.md" },
          ]),
        ),
      ).toThrow(/unknown lane 'nope'/);
    });

    it("rejects an unknown mode", () => {
      expect(() =>
        parseManifest(
          cases([
            { id: "x", lane: "agentic", kind: "compiled", modes: ["nightly"], source: "a.md" },
          ]),
        ),
      ).toThrow(/mode 'nightly'/);
    });

    it("rejects an empty modes array", () => {
      expect(() =>
        parseManifest(
          cases([{ id: "x", lane: "agentic", kind: "compiled", modes: [], source: "a.md" }]),
        ),
      ).toThrow(/modes must not be empty/);
    });

    it("rejects duplicate modes", () => {
      expect(() =>
        parseManifest(
          cases([
            {
              id: "x",
              lane: "agentic",
              kind: "compiled",
              modes: ["candidate", "candidate"],
              source: "a.md",
            },
          ]),
        ),
      ).toThrow(/duplicates/);
    });

    it("rejects a manifest where a mode has no cases at all", () => {
      // Otherwise a typo could silently reduce an orchestrator to a no-op that
      // still reports success.
      expect(() =>
        parseManifest(
          cases([
            { id: "x", lane: "agentic", kind: "compiled", modes: ["candidate"], source: "a.md" },
          ]),
        ),
      ).toThrow(/no case participates in mode 'released'/);
    });

    it("rejects two lanes sharing one definitionIdEnv", () => {
      expect(() =>
        parseManifest(
          manifest({
            lanes: {
              agentic: { definitionIdEnv: "SHARED_ID" },
              debug: { definitionIdEnv: "SHARED_ID" },
            },
          }),
        ),
      ).toThrow(/share definitionIdEnv/);
    });

    it("rejects a definitionIdEnv that is not an env var name", () => {
      expect(() =>
        parseManifest(manifest({ lanes: { agentic: { definitionIdEnv: "lower-case" } } })),
      ).toThrow(/must match/);
    });
  });

  describe("assertions validation", () => {
    it("rejects an unsupported build tag placeholder", () => {
      expect(() =>
        parseManifest(
          cases([
            {
              id: "x",
              lane: "agentic",
              kind: "compiled",
              modes: ["candidate", "released"],
              source: "a.md",
              assertions: { requiredBuildTags: ["tag-{buildid}"] },
            },
          ]),
        ),
      ).toThrow(/unsupported placeholder/);
    });

    it("accepts the {buildId} placeholder", () => {
      const parsed = parseManifest(
        cases([
          {
            id: "x",
            lane: "agentic",
            kind: "compiled",
            modes: ["candidate", "released"],
            source: "a.md",
            assertions: { requiredBuildTags: ["ado-aw-custom-job-{buildId}"] },
          },
        ]),
      );
      expect(parsed.cases[0]?.assertions?.requiredBuildTags).toEqual([
        "ado-aw-custom-job-{buildId}",
      ]);
    });

    it("rejects an empty assertions object", () => {
      expect(() =>
        parseManifest(
          cases([
            {
              id: "x",
              lane: "agentic",
              kind: "compiled",
              modes: ["candidate", "released"],
              source: "a.md",
              assertions: {},
            },
          ]),
        ),
      ).toThrow(/must declare agentCommand and\/or requiredBuildTags/);
    });

    it("rejects an agentCommand with no snippets", () => {
      expect(() =>
        parseManifest(
          cases([
            {
              id: "x",
              lane: "agentic",
              kind: "compiled",
              modes: ["candidate", "released"],
              source: "a.md",
              assertions: { agentCommand: { required: [], forbidden: [] } },
            },
          ]),
        ),
      ).toThrow(/at least one snippet/);
    });
  });
});

describe("expandBuildTag", () => {
  it("substitutes every occurrence of {buildId}", () => {
    expect(expandBuildTag("ado-aw-custom-job-{buildId}", 42)).toBe("ado-aw-custom-job-42");
    expect(expandBuildTag("{buildId}-{buildId}", 7)).toBe("7-7");
  });

  it("leaves a tag with no placeholder unchanged", () => {
    expect(expandBuildTag("static-tag", 42)).toBe("static-tag");
  });
});

describe("the real shipped tests/smoke/cases.json", () => {
  const parsed = parseManifest(readFileSync(REAL_MANIFEST_PATH, "utf8"));

  it("parses under the same strict rules applied to synthetic manifests", () => {
    expect(parsed.cases.length).toBeGreaterThan(0);
  });

  it("stages every case to one fixed path so lanes can be shared", () => {
    expect(parsed.yamlPath).toBe(".smoke/pipeline.yml");
  });

  it("keeps the debug-token case in its own lane", () => {
    // smoke-failure-reporter needs ADO_AW_DEBUG_GITHUB_TOKEN; isolating it
    // stops that credential being readable by every other case.
    const reporter = parsed.cases.find((entry) => entry.id === "smoke-failure-reporter");
    expect(reporter?.lane).toBe("debug");
    const others = parsed.cases.filter((entry) => entry.id !== "smoke-failure-reporter");
    expect(others.every((entry) => entry.lane !== "debug")).toBe(true);
  });

  it("declares an infra lane ready for the AWF / ado-proxy smokes", () => {
    expect(parsed.lanes.map((lane) => lane.id)).toContain("infra");
  });

  it("runs the candidate-only custom safe-output case in candidate mode only", () => {
    const custom = parsed.cases.find((entry) => entry.id === "custom-safe-output");
    expect(custom?.modes).toEqual(["candidate"]);
  });

  it("covers the janitor in released mode so AgentPlayground keeps being pruned", () => {
    const janitor = parsed.cases.find((entry) => entry.id === "janitor");
    expect(janitor?.modes).toEqual(["released"]);
  });
});
