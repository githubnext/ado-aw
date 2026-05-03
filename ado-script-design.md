# Design exploration: an `actions/github-script` analog for ADO

> **Mode**: thought experiment. No scope committed. Goal is to map the design
> space, surface trade-offs, and identify the highest-leverage entry point if
> we ever pull the trigger.

## 1. The concern that motivates this

`scripts/gate-eval.py` is already 388 lines. It conflates several
responsibilities:

1. **Spec deserialization** (base64 → JSON → dict)
2. **Fact acquisition** (env vars, REST API for PR metadata, REST API for
   iteration changes, datetime arithmetic) — including auth, URL building,
   retry/timeout semantics
3. **Predicate evaluation** (10 predicate types, recursive, with overnight
   time-window arithmetic and `_strip_ref_prefix` quirks)
4. **Failure-policy state machine** (`fail_closed` / `fail_open` /
   `skip_dependents`, with transitive propagation through fact dependencies)
5. **ADO logging-command emission** (`##vso[...]`)
6. **Self-cancel** (PATCH to builds API)

Every new filter type forces a coordinated change across:
`filter_ir.rs` (Rust IR) → JSON schema → `gate-eval.py` evaluator →
fixtures → docs. The Python file has no static typing, no test harness in CI
(only Rust-side spec-serialization tests), and grows with the IR.

There are at least two more places where a similar Python/bash blob is on the
roadmap or already exists:

- The Stage-3 safe-output executor — currently a typed Rust binary
  (`src/execute.rs` + `src/safeoutputs/*.rs`). Strong story today, but every
  ADO interaction is hand-rolled HTTP via `reqwest`.
- The agent shim & the prepare/setup steps — currently bash interleaved with
  ADO macro expansion.

The user's instinct: rather than letting `gate-eval.py` grow into a
monstrosity (and rather than reinventing it for each new use case), give
ado-aw a single, well-tested primitive — the way `actions/github-script`
gives gh-aw its "drop in JS, get a pre-authed Octokit + context" lever.

## 2. What `actions/github-script` actually is

For grounding, the github-script contract:

```yaml
- uses: actions/github-script@v7
  with:
    github-token: ${{ secrets.GITHUB_TOKEN }}
    script: |
      const { data } = await github.rest.issues.createComment({
        owner: context.repo.owner, repo: context.repo.repo,
        issue_number: context.issue.number, body: 'hi',
      });
      core.setOutput('comment-id', data.id);
```

Mechanics worth copying:

| Property | Detail |
|---|---|
| Language | Node.js (single ecosystem, ncc-bundled, no `npm install` at runtime) |
| Auth | Pre-injected `github` Octokit, token from input |
| Context | Pre-injected `context` (event payload + repo/issue/PR shortcuts) |
| Helpers | `core` (output/secrets/log), `glob`, `io`, `exec`, `fetch` |
| Wrapper | `(async () => { <script> })()` — top-level `await` works |
| Return | Stringified into a step output |
| TS-aware | `@octokit/rest` types via JSDoc; some IDEs surface them |
| Distribution | Action repo bundles all deps; runner downloads the action tarball once |

Mechanics that **don't** translate cleanly:

- GH Actions runners have Node pre-installed; ADO Microsoft-hosted agents do
  too, but **AWF-isolated 1ES sandboxes do not** by default. Anything we ship
  must either be self-contained or be installed in `prepare` before the
  network is locked down.
- github-script has *no* notion of fail-open / skip-dependents / multi-stage
  trust boundaries. The gate logic isn't just "call an API"; it has a
  bespoke policy DSL.

## 3. Two distinct primitives are being conflated

It pays to separate these up front, because they pull in opposite directions:

### (A) **Internal** primitive — for the compiler to target

> "Stop emitting hand-rolled Python; emit calls to a single bundled binary
> with a typed, declarative spec and a small evaluator surface."

- Audience: ado-aw maintainers
- Surface: minimal, declarative, deterministic
- Driver: maintainability of the *compiler output*
- Examples: gate evaluator, future "wait for X" pollers, agent-stats parser

### (B) **User-facing** primitive — for agent authors

> "Give pipeline authors an `ado-script:` block in their agent file that runs
> arbitrary JS with `ado` (azure-devops-node-api), `context`, `core`."

- Audience: humans writing `.md` agent files
- Surface: rich, ergonomic, escape-hatch-friendly
- Driver: power & extensibility
- Examples: custom triggers, custom safe-output post-processing, ad-hoc
  reporting

These are **separate features** even if they share a runtime. Our concern
about gate-eval.py becoming a monstrosity is squarely about (A). The
github-script analogy points at (B). Picking one direction without naming the
other is how scope creep happens.

## 4. Design space — variant matrix

Three orthogonal axes:

### Axis 1 — Language

| Option | Pros | Cons |
|---|---|---|
| **Node.js** (mirrors github-script directly) | `azure-devops-node-api` is the most mature SDK; ncc-bundling produces a single file; same mental model as gh-aw | New runtime dependency; AWF sandbox needs Node pre-staged in chroot; bigger binary (~30 MB bundled) |
| **Python** (continues current trajectory) | Already in the chroot for gate-eval; stdlib-only is feasible (current approach); easy to embed | No first-class ADO SDK that's stdlib-only; users have to hand-roll `urllib`; weak typing means the same maintenance pain we have today |
| **Embed in the Rust binary** (`ado-aw script ...` subcommand) | Strongest typing and testability; reuses `reqwest`/`anyhow` already in the binary; no new runtime to ship; same audit surface as the rest of ado-aw | Useless as a (B) user-facing primitive (no inline scripting); for (A), basically just "move gate-eval.py into Rust", which is a viable answer in itself |
| **Deno / Bun** | Single-binary, sandboxed-by-default, TypeScript-first | Not in standard ADO/1ES images; one more thing to vet for OneBranch |

### Axis 2 — Distribution

| Option | Pros | Cons |
|---|---|---|
| **Bundled into ado-aw release artifacts** (current `gate-eval.py` model) | Versioned with the compiler; deterministic URL; CI publishes alongside binary | Each script is a separate download (already 2 artifacts; will be N) |
| **Single ncc-bundled JS** (one `ado-script.js` per release) | One artifact regardless of how many internal use sites; deps frozen at build time | If we add a (B) user surface, users can't `npm install` extras |
| **Inline heredoc** (today's Tier-1 inline gate) | No download, no extra failure mode | Caps at "tiny scripts"; ADO macro expansion + heredoc quoting is already painful |
| **Subcommand of the ado-aw binary** | Zero extra artifacts; one auth/sanitization story | Forces (A)-only — no inline user scripts |

### Axis 3 — User surface

| Option | Description |
|---|---|
| **None** — internal-only (A) | Compiler emits `ado-aw script eval-gate --spec=…` (or `node ado-script.js gate ...`). User never sees it. Pure refactor of gate-eval.py. |
| **Front-matter `scripts:` block** | Authors declare named scripts that run in stage 1 prepare or stage 3. Tightly typed inputs/outputs. Limited blast radius. |
| **Free-form `ado-script:` step** | Mirrors github-script 1:1. Maximum power, maximum risk. Needs sanitization, prompt-injection review, allow-list of called APIs. |
| **Safe-output kind** (`safe-outputs.run-script`) | The agent itself proposes a script to run; Stage 2 detection reviews it; Stage 3 executes. Symmetrical with existing safe outputs but inverts trust (agent-authored code). |

## 5. What changes for `gate-eval.py` under each scope

### Scope A1 — "Move gate-eval.py into the Rust binary"

- Add `ado-aw eval-gate --spec-base64=…` subcommand
- Reuse `reqwest`, `serde_json`, the existing `Fact`/`Predicate` types as
  *runtime* types (not just IR)
- Bash shim drops to: `export GATE_SPEC=…; ado-aw eval-gate`
- **Trade-off**: ado-aw binary is now also a runtime dependency in the chroot
  (it already is — `prepare` downloads it). The big win is testability:
  predicate evaluation gets unit-tested in Rust, the policy state machine
  becomes a typed `enum`, and we lose the JSON-schema-dance.
- **Risk**: every agent pipeline now invokes the ado-aw binary at runtime;
  any panic surfaces as a build failure. (Mitigated by `Result` discipline.)

### Scope A2 — "Bundle a Node ado-script and emit `node ado-script.js gate ...`"

- New `scripts/ado-script/` workspace with `azure-devops-node-api`
- ncc-bundle to a single `ado-script.js`
- Compiler emits `node /tmp/ado-aw-scripts/ado-script.js gate <base64-spec>`
- **Trade-off**: best ergonomics for *future user-facing* (B) work; worst
  fit for the immediate problem (we already have the spec types in Rust;
  re-deserializing in JS just moves the pain).
- **Risk**: Node version skew across hosted vs 1ES vs OneBranch images.

### Scope B — "User-facing ado-script:"

Independent question. Even if we pick A1 (Rust subcommand), we might still
later add a `.md` front-matter `scripts:` block that runs Node. They're not
mutually exclusive.

## 6. Recommendation framework (no commitment yet)

If the immediate pain is gate-eval.py specifically, **Scope A1** has by far
the best cost/benefit ratio:

- Eliminates the JSON-spec round-trip (Rust IR → JSON → Python dict → eval).
  The `FilterCheck` enum *is* the runtime representation.
- Eliminates the dual codebase and the schema-drift class of bugs.
- Removes the `scripts/gate-eval.py` and `scripts/gate-spec.schema.json`
  release artifacts.
- Keeps the door open for a future (B) primitive without prejudging it.

If the longer-term vision is "agent authors should be able to drop in custom
ADO logic", **Scope B with bundled Node** is the right shape, but it should
be approached as a deliberate user-facing feature with its own RFC — not as
a back-door from the gate-eval refactor.

The framing the user is reacting against — "embedded Python that grows
forever" — is solved by either A1 *or* A2. The github-script-shaped solution
(Node + SDK + inline scripts) only pays off if we commit to (B).

## 7. Open questions to resolve before any implementation

1. **Is the chroot OK with a second invocation of the ado-aw binary at
   runtime?** Today it's only invoked in `prepare` (download) and as the MCP
   server. Promoting it to "the gate evaluator and everything else" changes
   its operational profile.
2. **Can the existing Rust `Fact`/`Predicate`/`Policy` types be the runtime
   types directly, or do they leak compiler concerns (spans, diagnostics)
   that would have to be split?**
3. **What's the ADO REST client story?** `azure-devops-node-api` for Node,
   nothing canonical for Python, hand-rolled `reqwest` for Rust. If we go
   A1, we should consolidate the ad-hoc HTTP in `safeoutputs/*.rs` against
   the same client.
4. **Self-cancel & `##vso` emission.** These are tiny but pervasive. Worth
   a single `AdoLogger` + `AdoBuildClient` abstraction in whichever language
   we land on.
5. **Failure-policy semantics.** `skip_dependents` + transitive `fail_open`
   propagation is *not* in any off-the-shelf SDK. It's our DSL. Whichever
   language we pick, this lives in our code.
6. **Stage-3 trust boundary.** A user-facing (B) `ado-script` would need to
   live in Stage 3 (not Stage 1) to have write access. That's the same
   pattern as safe outputs — agent proposes, executor decides.

## 8. Suggested next step (if and when scope is committed)

Spike A1 on a single throwaway branch:

- Add `ado-aw eval-gate` subcommand
- Move ~80% of `gate-eval.py` logic into `src/gate/eval.rs` (predicate
  eval + policy state machine) — re-using existing `Fact`/`Predicate`
  types where possible
- Keep the bash shim and the JSON spec format unchanged for the spike;
  only the *evaluator* moves
- Compare LoC, test count, and binary-size delta against today

That spike answers questions 1, 2, and 3 concretely without committing to
any user-facing surface.

---

*This is a design note, not an implementation plan. No todos created. If you
want to move forward on any of these scopes, ask for a fresh planning pass.*
