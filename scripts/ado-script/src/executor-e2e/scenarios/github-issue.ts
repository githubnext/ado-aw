/**
 * GitHub issue safe-output scenarios: `create-github-issue`,
 * `set-github-issue-type`, and the same-run `temporary_id` handoff between
 * them.
 *
 * These are the only scenarios that assert against **GitHub** rather than ADO,
 * so `ctx.rest` (an `AdoRest`) is unused here; they drive the shared harness
 * GitHub client in `../github-client.js` instead.
 *
 * ## Close, don't delete
 *
 * Every other scenario in this suite tears down completely. These cannot:
 * GitHub has no REST endpoint to delete an issue. `cleanup()` therefore
 * **closes** each issue as `not_planned`, and every title embeds the greppable
 * `ado-aw-det-<buildId>-<tool>` marker so anything a cleanup misses is findable
 * with a single search. This is documented in `tests/executor-e2e/README.md`.
 *
 * ## Why so many skips
 *
 * Native issue types are an **organisation-level** construct — there is no
 * user-account equivalent of `GET /orgs/{org}/issue-types` — so a user-owned
 * scratch repository can never expose a named type. Scenarios that need one
 * raise `SkipError`, never a failure. The handoff scenario stays runnable by
 * falling back to the documented clear operation (`issue_type: ""`).
 *
 * Test-harness module; not shipped in `ado-script.zip`.
 */
import {
  cleanVar,
  closeIssue,
  createGitHubIssue,
  diagnoseGitHubAuthFailure,
  findOpenIssueByTitle,
  getIssue,
  listOrgIssueTypes,
  patchIssue,
  splitRepo,
} from "../github-client.js";
import type { GitHubClientOptions } from "../github-client.js";
import type { ExecutedRecord, PriorEntry, Scenario, ScenarioContext } from "../scenario.js";
import { SkipError } from "../scenario.js";
import { detBody, numResult, strResult, Teardown } from "./common.js";

/** Static label the operator config always applies. */
const STATIC_LABEL = "executor-e2e";
/** Agent-supplied label that the `allowed-labels` pattern below admits. */
const ALLOWED_AGENT_LABEL = "executor-e2e-agent";
/** Agent-supplied label deliberately outside the allowlist. */
const DENIED_AGENT_LABEL = "definitely-not-allowed";
/** Wildcard pattern that admits `ALLOWED_AGENT_LABEL` but not `DENIED_AGENT_LABEL`. */
const ALLOWED_LABEL_PATTERN = "executor-e2e-*";
/** Prefix the operator config prepends to every agent-supplied title. */
const TITLE_PREFIX = "[executor-e2e] ";
/** Temporary ID exercised by the handoff scenario. */
const TEMPORARY_ID = "#aw_e2e1";
/** Traceability marker the executor appends to every issue body. */
const FOOTER_MARKER = "<!-- ado-aw -->";

/** Everything the scenarios need after preconditions are resolved. */
export interface GithubIssueEnv {
  /** `owner/repo` slug that scratch issues are filed into. */
  repo: string;
  token: string;
  gh: GitHubClientOptions;
}

/**
 * Resolve the GitHub token and target repo, or `SkipError`.
 *
 * There is deliberately **no default repo**: an unset/misconfigured variable
 * must skip rather than spray scratch issues onto the canonical repository.
 */
export function resolveGithubIssueEnv(
  tool: string,
  env: NodeJS.ProcessEnv = process.env,
): GithubIssueEnv {
  const token = cleanVar(env.EXECUTOR_E2E_GITHUB_TOKEN);
  if (!token) {
    throw new SkipError(
      `${tool}: EXECUTOR_E2E_GITHUB_TOKEN is not set; supply a PAT with Issues:write to enable this scenario`,
    );
  }
  const repo = cleanVar(env.EXECUTOR_E2E_SCENARIO_ISSUE_REPO) ?? cleanVar(env.EXECUTOR_E2E_ISSUE_REPO);
  if (!repo) {
    throw new SkipError(
      `${tool}: no scratch issue repo configured (set EXECUTOR_E2E_SCENARIO_ISSUE_REPO or EXECUTOR_E2E_ISSUE_REPO)`,
    );
  }
  // Reject a malformed slug up front rather than letting it 404 mid-scenario.
  splitRepo(repo);
  return { repo, token, gh: { token, repo } };
}

/**
 * Confirm the token can actually mutate issues on the target repo before the
 * scenario creates anything.
 *
 * A token that authenticates but lacks Issues:write is a **missing
 * precondition**, not a product failure, so this skips (with the harness's
 * standard auth diagnosis attached) rather than going red.
 */
async function requireIssueWrite(env: GithubIssueEnv, tool: string): Promise<void> {
  const probe = await patchIssue(env.gh, 0, {}).catch(() => undefined);
  // Issue #0 never exists; 404 means the token reached the repo, which is all
  // we can check without mutating real state. 401/403 means it cannot write.
  if (probe && (probe.status === 401 || probe.status === 403)) {
    const diagnosis: string[] = [];
    await diagnoseGitHubAuthFailure(env.gh, probe.status, (m) => diagnosis.push(m));
    throw new SkipError(
      `${tool}: EXECUTOR_E2E_GITHUB_TOKEN cannot write issues on '${env.repo}' (HTTP ${probe.status}). ${diagnosis.join(" ")}`,
    );
  }
}

/** Deterministic, greppable issue title for a scenario. */
function issueTitle(ctx: ScenarioContext, id: string): string {
  return `${ctx.prefix(id)} scratch issue`;
}

/** The env every scenario passes to `ado-aw execute`. */
function executeEnv(env: GithubIssueEnv): Record<string, string> {
  // Stage 3 reads the credential from ADO_AW_GITHUB_TOKEN only
  // (`ExecutionContext::github_token`); the harness's own
  // EXECUTOR_E2E_GITHUB_TOKEN is not consulted by the binary.
  return { ADO_AW_GITHUB_TOKEN: env.token };
}

/**
 * Close an issue, tolerating the case where it was never created.
 *
 * `cleanup()` must not depend solely on state written by `assert()`: when the
 * executor filed an issue but the record came back non-`succeeded` (e.g.
 * `failure_with_data` from a temporary-ID registration error) the runner never
 * calls `assert()`, so the number is unknown. Falling back to an exact-title
 * search on the `ado-aw-det-*` marker closes the issue anyway.
 */
async function closeByNumberOrTitle(
  env: GithubIssueEnv,
  issueNumber: number | undefined,
  title: string,
): Promise<void> {
  const number = issueNumber ?? (await findOpenIssueByTitle(env.gh, title));
  if (number === undefined) return;
  await closeIssue(env.gh, number);
}

/** Pull one record out of a run by its kebab-case tool name. */
export function recordForTool(
  records: ExecutedRecord[],
  tool: string,
): ExecutedRecord {
  const name = tool.replaceAll("-", "_");
  const record = records.find((r) => r.name === name);
  if (!record) throw new Error(`no executed record for '${tool}'`);
  return record;
}

// ---------------------------------------------------------------------------
// create-github-issue
// ---------------------------------------------------------------------------

interface CreateState extends GithubIssueEnv {
  title: string;
  issueNumber?: number;
}

export const createGithubIssue: Scenario<CreateState> = {
  id: "create-github-issue",
  tool: "create-github-issue",
  config: (_ctx, state) => ({
    "target-repo": state.repo,
    "title-prefix": TITLE_PREFIX,
    labels: [STATIC_LABEL],
    "allowed-labels": [ALLOWED_LABEL_PATTERN],
  }),
  setup: async (ctx) => {
    const env = resolveGithubIssueEnv("create-github-issue");
    await requireIssueWrite(env, "create-github-issue");
    return { ...env, title: issueTitle(ctx, "create-github-issue") };
  },
  ndjson: async (ctx, state) => ({
    title: state.title,
    body: detBody(ctx, "create-github-issue"),
    labels: [ALLOWED_AGENT_LABEL],
  }),
  env: async (_ctx, state) => executeEnv(state),
  assert: async (ctx, state, record) => {
    // Cleanup-critical: remember the number before any fallible assertion.
    state.issueNumber = numResult(record, "number");
    if (strResult(record, "target_repo") !== state.repo) {
      throw new Error(
        `executor filed into '${strResult(record, "target_repo")}', expected '${state.repo}'`,
      );
    }

    const issue = await getIssue(state.gh, state.issueNumber);
    if (!issue) throw new Error(`issue #${state.issueNumber} was not created`);

    const expectedTitle = `${TITLE_PREFIX}${state.title}`;
    if (issue.title !== expectedTitle) {
      throw new Error(`issue title is '${issue.title}', expected '${expectedTitle}'`);
    }
    if (!issue.body?.includes(detBody(ctx, "create-github-issue"))) {
      throw new Error("issue body does not carry the agent-supplied text");
    }
    if (!issue.body.includes(FOOTER_MARKER)) {
      throw new Error(`issue body is missing the '${FOOTER_MARKER}' traceability footer`);
    }
    const labels = issue.labels.map((l) => l.toLowerCase());
    for (const expected of [STATIC_LABEL, ALLOWED_AGENT_LABEL]) {
      if (!labels.includes(expected)) {
        throw new Error(`issue labels ${JSON.stringify(issue.labels)} are missing '${expected}'`);
      }
    }
  },
  cleanup: async (_ctx, state) =>
    closeByNumberOrTitle(state, state.issueNumber, `${TITLE_PREFIX}${state.title}`),
};

// ---------------------------------------------------------------------------
// create-github-issue — allowed-labels rejection
// ---------------------------------------------------------------------------

/**
 * `allowed-labels` is **default-deny**: an empty/absent list rejects every
 * agent-supplied label, and `["*"]` is the explicit opt-out. (Note the
 * asymmetry with `set-github-issue-type.allowed`, which is default-allow.)
 * Here the allowlist is non-empty but does not match, so the executor must
 * reject the proposal and file nothing.
 */
export const createGithubIssueLabelDenied: Scenario<CreateState> = {
  id: "create-github-issue-label-denied",
  tool: "create-github-issue",
  config: (_ctx, state) => ({
    "target-repo": state.repo,
    "title-prefix": TITLE_PREFIX,
    "allowed-labels": [ALLOWED_LABEL_PATTERN],
  }),
  setup: async (ctx) => {
    const env = resolveGithubIssueEnv("create-github-issue-label-denied");
    await requireIssueWrite(env, "create-github-issue-label-denied");
    return { ...env, title: issueTitle(ctx, "create-github-issue-label-denied") };
  },
  ndjson: async (ctx, state) => ({
    title: state.title,
    body: detBody(ctx, "create-github-issue-label-denied"),
    labels: [DENIED_AGENT_LABEL],
  }),
  env: async (_ctx, state) => executeEnv(state),
  expectedFailure: {
    // Deliberately matches ONLY the "allowlist is configured but does not
    // match" message. The other rejection message — "no `allowed-labels`
    // configured" — is what the executor emits when it never read the config at
    // all, so accepting both would let this scenario pass even when
    // `allowed-labels` was silently discarded. See the config-drop note in
    // tests/executor-e2e/README.md.
    error: /labels not in allowed-labels/i,
  },
  // Never reached: the runner short-circuits on a matching expectedFailure.
  assert: async () => {
    throw new Error("create-github-issue should have rejected the disallowed label");
  },
  // Belt and braces: if the executor ever regressed and filed the issue anyway,
  // the title-marker search finds and closes it.
  cleanup: async (_ctx, state) =>
    closeByNumberOrTitle(state, undefined, `${TITLE_PREFIX}${state.title}`),
};

// ---------------------------------------------------------------------------
// set-github-issue-type
// ---------------------------------------------------------------------------

interface SetTypeState extends GithubIssueEnv {
  title: string;
  issueNumber: number;
  issueType: string;
}

/**
 * Discover a named org issue type, or `SkipError`.
 *
 * `E2E_GITHUB_ISSUE_TYPE` forces a specific name for environments where the
 * token cannot read org metadata but the type is known to exist.
 */
async function requireNamedIssueType(
  env: GithubIssueEnv,
  tool: string,
  processEnv: NodeJS.ProcessEnv = process.env,
): Promise<string> {
  const explicit = cleanVar(processEnv.E2E_GITHUB_ISSUE_TYPE);
  if (explicit) return explicit;
  const { owner } = splitRepo(env.repo);
  const types = await listOrgIssueTypes(env.gh, owner);
  if (types.length === 0) {
    throw new SkipError(
      `${tool}: no native issue types are defined for '${owner}'. Issue types are an ` +
        `organisation-level construct with no user-account equivalent, so a user-owned ` +
        `scratch repo can never expose one. Set E2E_GITHUB_ISSUE_TYPE to force a name.`,
    );
  }
  return types[0]!;
}

/**
 * Create the scratch issue this scenario will mutate.
 *
 * Called only after every `SkipError` check has passed, so `setup()` never
 * leaves an orphan behind: a throw before this point creates nothing, and a
 * throw after it is torn down inline (the runner does not run `cleanup()` when
 * `setup()` fails).
 */
async function seedIssue(
  ctx: ScenarioContext,
  env: GithubIssueEnv,
  id: string,
): Promise<{ title: string; issueNumber: number }> {
  const title = issueTitle(ctx, id);
  const url = await createGitHubIssue(env.gh, title, detBody(ctx, id), [STATIC_LABEL]);
  const match = url.match(/\/(\d+)$/);
  if (!match) {
    // The issue exists but we could not parse its number from the URL — close
    // it by title so setup's failure does not leak an open issue.
    await closeByNumberOrTitle(env, undefined, title).catch(() => {});
    throw new Error(`could not parse an issue number out of '${url}'`);
  }
  return { title, issueNumber: Number(match[1]) };
}

export const setGithubIssueType: Scenario<SetTypeState> = {
  id: "set-github-issue-type",
  tool: "set-github-issue-type",
  config: (_ctx, state) => ({ "target-repo": state.repo }),
  setup: async (ctx) => {
    const env = resolveGithubIssueEnv("set-github-issue-type");
    await requireIssueWrite(env, "set-github-issue-type");
    const issueType = await requireNamedIssueType(env, "set-github-issue-type");
    const seeded = await seedIssue(ctx, env, "set-github-issue-type");
    return { ...env, ...seeded, issueType };
  },
  ndjson: async (_ctx, state) => ({
    issue_number: state.issueNumber,
    issue_type: state.issueType,
  }),
  env: async (_ctx, state) => executeEnv(state),
  assert: async (_ctx, state, record) => {
    if (numResult(record, "number") !== state.issueNumber) {
      throw new Error(
        `executor targeted issue #${numResult(record, "number")}, expected #${state.issueNumber}`,
      );
    }
    const issue = await getIssue(state.gh, state.issueNumber);
    if (issue?.type?.toLowerCase() !== state.issueType.toLowerCase()) {
      throw new Error(
        `issue #${state.issueNumber} has type '${issue?.type ?? "(none)"}', expected '${state.issueType}'`,
      );
    }
  },
  cleanup: async (_ctx, state) => closeByNumberOrTitle(state, state.issueNumber, state.title),
};

// ---------------------------------------------------------------------------
// set-github-issue-type — documented clear operation
// ---------------------------------------------------------------------------

/**
 * Probe whether the repo accepts a type clear (`{"type": ""}`).
 *
 * Where no issue types exist the clear is a legitimate no-op, but GitHub is
 * free to reject the field outright — so probe once and `SkipError` instead of
 * reporting a product failure.
 */
async function requireClearSupport(
  env: GithubIssueEnv,
  issueNumber: number,
  tool: string,
): Promise<void> {
  const res = await patchIssue(env.gh, issueNumber, { type: "" });
  if (!res.ok) {
    throw new SkipError(
      `${tool}: GitHub rejected a native issue-type clear on '${env.repo}' (HTTP ${res.status}); ` +
        `this repository does not support the issue-type field`,
    );
  }
}

export const setGithubIssueTypeClear: Scenario<SetTypeState> = {
  id: "set-github-issue-type-clear",
  tool: "set-github-issue-type",
  config: (_ctx, state) => ({ "target-repo": state.repo }),
  setup: async (ctx) => {
    const env = resolveGithubIssueEnv("set-github-issue-type-clear");
    await requireIssueWrite(env, "set-github-issue-type-clear");
    const seeded = await seedIssue(ctx, env, "set-github-issue-type-clear");
    // Everything below can throw, and the runner will NOT call cleanup() for a
    // setup failure — so tear the seeded issue down inline before rethrowing.
    try {
      // Give the issue a type first where one exists, so the clear is
      // observable rather than a no-op.
      let issueType = "";
      try {
        issueType = await requireNamedIssueType(env, "set-github-issue-type-clear");
      } catch (err) {
        if (!(err instanceof SkipError)) throw err;
      }
      if (issueType) {
        const applied = await patchIssue(env.gh, seeded.issueNumber, { type: issueType });
        if (!applied.ok) issueType = "";
      }
      await requireClearSupport(env, seeded.issueNumber, "set-github-issue-type-clear");
      if (issueType) {
        await patchIssue(env.gh, seeded.issueNumber, { type: issueType });
      }
      return { ...env, ...seeded, issueType };
    } catch (err) {
      await closeByNumberOrTitle(env, seeded.issueNumber, seeded.title).catch(() => {});
      throw err;
    }
  },
  ndjson: async (_ctx, state) => ({ issue_number: state.issueNumber, issue_type: "" }),
  env: async (_ctx, state) => executeEnv(state),
  assert: async (_ctx, state, record) => {
    if (numResult(record, "number") !== state.issueNumber) {
      throw new Error(
        `executor targeted issue #${numResult(record, "number")}, expected #${state.issueNumber}`,
      );
    }
    if (strResult(record, "issue_type") !== "") {
      throw new Error(
        `executor reported issue_type '${strResult(record, "issue_type")}', expected the clear sentinel ''`,
      );
    }
    const issue = await getIssue(state.gh, state.issueNumber);
    if (issue?.type) {
      throw new Error(`issue #${state.issueNumber} still has type '${issue.type}' after the clear`);
    }
  },
  cleanup: async (_ctx, state) => closeByNumberOrTitle(state, state.issueNumber, state.title),
};

// ---------------------------------------------------------------------------
// same-run temporary_id handoff
// ---------------------------------------------------------------------------

interface HandoffState extends GithubIssueEnv {
  title: string;
  /** Named type when one is discoverable; "" exercises the clear path. */
  issueType: string;
  /** Populated by assert() from the prior create record. */
  issueNumber?: number;
}

/**
 * The highest-value scenario: `create-github-issue` mints an issue under
 * `temporary_id`, and `set-github-issue-type` — in the **same**
 * `ado-aw execute` process — resolves that id to the real issue number.
 *
 * The registry backing this handoff (`ExecutionContext.resolved_github_issues`)
 * is an in-process `Arc<Mutex<HashMap<…>>>`, so a wrong REST shape or a scoping
 * failure would be invisible to any test that runs the two tools separately.
 * The proof is that the *type* record reports the number the *create* record
 * actually produced.
 */
export const createGithubIssueTemporaryIdHandoff: Scenario<HandoffState> = {
  id: "create-github-issue-temporary-id-handoff",
  // Primary tool is the CONSUMER of the temporary id; create-github-issue is
  // staged ahead of it as a prior entry.
  tool: "set-github-issue-type",
  config: (_ctx, state) => ({ "target-repo": state.repo }),
  setup: async (ctx) => {
    const id = "create-github-issue-temporary-id-handoff";
    const env = resolveGithubIssueEnv(id);
    await requireIssueWrite(env, id);
    // Prefer a named type; fall back to the documented clear operation so the
    // handoff still runs on a repo with no issue types.
    let issueType = "";
    try {
      issueType = await requireNamedIssueType(env, id);
    } catch (err) {
      if (!(err instanceof SkipError)) throw err;
    }
    return { ...env, title: issueTitle(ctx, id), issueType };
  },
  priorEntries: async (ctx, state): Promise<PriorEntry[]> => [
    {
      tool: "create-github-issue",
      config: {
        "target-repo": state.repo,
        labels: [STATIC_LABEL],
        "require-temporary-id": true,
      },
      entry: {
        title: state.title,
        body: detBody(ctx, "create-github-issue-temporary-id-handoff"),
        temporary_id: TEMPORARY_ID,
      },
    },
  ],
  ndjson: async (_ctx, state) => ({
    issue_number: TEMPORARY_ID,
    issue_type: state.issueType,
  }),
  env: async (_ctx, state) => executeEnv(state),
  assert: async (_ctx, state, record, records) => {
    // Cleanup-critical: capture the real number from the create record before
    // any fallible assertion, so a later throw still tears the issue down.
    const created = recordForTool(records, "create-github-issue");
    const createdNumber = numResult(created, "number");
    state.issueNumber = createdNumber;

    if (strResult(created, "temporary_id") !== TEMPORARY_ID) {
      throw new Error(
        `create-github-issue reported temporary_id '${strResult(created, "temporary_id")}', expected '${TEMPORARY_ID}'`,
      );
    }

    // THE handoff assertion: the consumer must have resolved the temporary id
    // to the issue the producer actually filed, in the same repository.
    const resolvedNumber = numResult(record, "number");
    if (resolvedNumber !== createdNumber) {
      throw new Error(
        `temporary_id '${TEMPORARY_ID}' resolved to issue #${resolvedNumber}, but ` +
          `create-github-issue filed #${createdNumber}`,
      );
    }
    const resolvedRepo = strResult(record, "target_repo");
    if (resolvedRepo !== strResult(created, "target_repo")) {
      throw new Error(
        `temporary_id '${TEMPORARY_ID}' resolved to repository '${resolvedRepo}', but ` +
          `create-github-issue filed into '${strResult(created, "target_repo")}'`,
      );
    }

    // Corroborate against GitHub itself so a fabricated result payload cannot
    // satisfy the assertion above.
    const issue = await getIssue(state.gh, createdNumber);
    if (!issue) throw new Error(`issue #${createdNumber} does not exist on '${state.repo}'`);
    if (issue.title !== state.title) {
      throw new Error(
        `issue #${createdNumber} is titled '${issue.title}', expected '${state.title}'`,
      );
    }
    if (state.issueType && issue.type?.toLowerCase() !== state.issueType.toLowerCase()) {
      throw new Error(
        `issue #${createdNumber} has type '${issue.type ?? "(none)"}', expected '${state.issueType}'`,
      );
    }
  },
  cleanup: async (_ctx, state) =>
    new Teardown()
      .add("close handoff issue", () => closeByNumberOrTitle(state, state.issueNumber, state.title))
      .run(),
};

export const githubIssueScenarios: Scenario<unknown>[] = [
  createGithubIssue,
  createGithubIssueLabelDenied,
  setGithubIssueType,
  setGithubIssueTypeClear,
  createGithubIssueTemporaryIdHandoff,
];
