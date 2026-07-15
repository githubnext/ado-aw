# Reusable Imports

_Part of the [ado-aw documentation](../AGENTS.md)._

`imports:` lets one agent file reuse another markdown component, including
cross-repository components pinned to an immutable commit SHA. The imported file
is the same markdown + YAML-front-matter format as a normal workflow: its front
matter is merged into the consumer, and its markdown body is prepended to the
consumer's prompt.

This is separate from [`{{#runtime-import}}`](runtime-imports.md). Runtime
imports expand prompt snippets on the pipeline runner; `imports:` is resolved by
`ado-aw compile`, validates optional `import-schema:` inputs, and can contribute
front-matter configuration such as tools, runtimes, MCP servers, and custom safe
outputs.

## Syntax

`imports:` is a flat list. Each entry is either a bare spec string or an object
with `uses`, optional `with`, and optional `endpoint`:

```yaml
imports:
  # Same-org Azure Repos (the primary, default source — no endpoint):
  - myproject/shared-agents/components/notify.md@0123456789abcdef0123456789abcdef01234567
  # Local import:
  - ./components/local-guidance.md
  # GitHub.com, via an ADO service connection (bare-string endpoint shorthand):
  - uses: octo/shared-agents/components/deploy.md@89abcdef0123456789abcdef0123456789abcdef
    endpoint: github-shared-components
    with:
      environment: prod
      region: westus3
```

### Import specs

| Form | Meaning |
|------|---------|
| `path/to/component.md` | Local import, resolved relative to the importing `.md` file. |
| `owner/repo/path/to/component.md@<sha>` | Cross-repository import. `<sha>` must be a full 40-character commit SHA; branches and tags are rejected. |
| `...#Section` | Import only a `# Section` or `## Section` from the markdown body. |
| `...?` | Optional import. If the target is missing, it is skipped. |

The optional marker is trailing, so a sectioned optional import looks like
`owner/repo/component.md@0123...cdef#Usage?`.

For a cross-repository spec `owner/repo/path@<sha>`, `owner` maps to the Azure
DevOps **project** (or GitHub owner) and `repo` to the repository name.

### `endpoint:` — source type and service connection

`endpoint:` selects the **source type** of a cross-repository import and names
the Azure DevOps service connection the generated runtime repository resource
authenticates with. It drives **both** the compile-time manifest fetch **and**
the runtime checkout, so the two can never disagree.

- **Absent** → **same-organization Azure Repos** (the primary, default case for
  this ADO-native compiler). Fetched at compile time via the ADO Git Items API
  and checked out at runtime with `System.AccessToken` (`type: git`, no
  endpoint).
- **Bare string** (`endpoint: my-connection`) → shorthand for a **GitHub.com**
  service connection.
- **Object form** with an explicit `type`:

  | `type` | Extra fields | Source | Runtime `type` |
  |--------|--------------|--------|----------------|
  | `github` (default) | — | GitHub.com | `github` |
  | `ghe` | `host:` (API host, e.g. `api.acme.ghe.com`) | GitHub Enterprise | `githubenterprise` |
  | `azure-repos` | `org:` (target collection URL, e.g. `https://dev.azure.com/otherorg`) | **Cross-organization** Azure Repos | `git` |

```yaml
imports:
  # Cross-org Azure Repos:
  - uses: otherproject/otherrepo/component.md@0123456789abcdef0123456789abcdef01234567
    endpoint:
      name: other-org-repos-connection
      type: azure-repos
      org: https://dev.azure.com/otherorg
  # GitHub Enterprise:
  - uses: octo/components/deploy.md@89abcdef0123456789abcdef0123456789abcdef
    endpoint:
      name: ghe-connection
      type: ghe
      host: api.acme.ghe.com
```

See [Repository resource endpoints](network.md#repository-resource-endpoints).

## Cross-repository resolution and cache

Cross-repository imports are immutable: the spec must include a full commit SHA.
At compile time, ado-aw fetches the imported **markdown manifest** and stores a
SHA-keyed copy under:

```text
.ado-aw/imports/<owner>/<repo>/<sha>/<flat_path>.md
```

The cache is intended to be committed. ado-aw also creates
`.ado-aw/imports/.gitattributes` marking cached imports as generated and using
`merge=ours`, mirroring gh-aw's committed import-cache model.

Only the markdown manifest is cached. Script files and other executor source are
not vendored into `.ado-aw/imports/`; script-bearing custom safe-output
components are checked out in their dedicated executor job and verified at the
pinned SHA before their code runs.

### Compile-time manifest fetch

The manifest fetcher is selected from the import's `endpoint` type so that the
compile-time fetch always matches the runtime checkout source:

- **Azure Repos** (endpoint-less same-org, or `type: azure-repos` cross-org) —
  fetched via the ADO Git Items API. Credentials are resolved
  **non-interactively** in this precedence: `SYSTEM_ACCESSTOKEN` →
  `AZURE_DEVOPS_EXT_PAT` → `az account get-access-token`. The consumer
  organization is taken from `AZURE_DEVOPS_ORG_URL` / `SYSTEM_COLLECTIONURI` or
  the repo's Azure Repos git remote; cross-org imports use the endpoint's
  `org:`.
- **GitHub / GitHub Enterprise** (`type: github` / `type: ghe`) — fetched via
  `gh api` using the compiler host's GitHub auth (`GH_HOST` targets the GHE
  instance).

Routing is **fail-closed**: an endpoint-less (Azure-Repos-intended) import never
silently falls back to GitHub, and a GitHub-typed import is never served by the
Azure Repos fetcher.

Current MVP notes:

- Nested/transitive import resolution is not expanded yet; the current resolver
  processes the workflow's top-level `imports:` list.
- A workflow may declare at most 20 imports, and each resolved manifest is
  capped at 256 KiB.

## `import-schema:` and `with:`

A reusable component can declare non-secret inputs with `import-schema:`.
Consumers pass values through `with:`. Values are validated at compile time,
defaults are applied, and placeholders of the form
`{{ ado.aw.import-inputs.<key> }}` are substituted throughout the imported front
matter and body before merge.

> **Delimiter.** Import inputs use the compile-time `{{ ... }}` delimiter (the
> same family as `{{ workspace }}`), **not** the Azure DevOps template-expression
> delimiter `${{ ... }}`. The substituted output is embedded directly into the
> pipeline YAML and agent prompt, where ADO template-processes any `${{ ... }}`
> it finds — so reusing that delimiter would be a footgun. A `{{` immediately
> preceded by `$` is treated as an ADO `${{ ... }}` expression and left
> untouched. Any `{{ ado.aw.import-inputs.<key> }}` still present after
> substitution (an input the consumer did not supply and the schema did not
> default) is a **compile-time error**.

Supported schema types are `string`, `number`, `boolean`, `choice`, `array`, and
`object`. `choice` uses an `options:` list. `array` uses an `items:` schema.
`object` uses `properties:`; object properties are currently one level deep.
Unknown `with:` keys, missing required inputs, and values of the wrong type are
compile-time errors.

```markdown
---
import-schema:
  channel:
    type: string
    required: true
    description: Notification channel name.
  severity:
    type: choice
    options: [info, warning, critical]
    default: info
  labels:
    type: array
    items:
      type: string
  delivery:
    type: object
    properties:
      retries:
        type: number
        default: 2
safe-outputs:
  scripts:
    notify-team:
      description: Send a team notification.
      max: 3
      run: node tools/notify.js
      inputs:
        title:
          type: string
          required: true
          max-length: 120
        body:
          type: string
          required: true
      env:
        NOTIFY_TOKEN: TEAM_NOTIFY_TOKEN
---
When notifying the team, use channel `{{ ado.aw.import-inputs.channel }}` and
severity `{{ ado.aw.import-inputs.severity }}`.
```

Consumer:

```yaml
imports:
  - uses: octo/shared-agents/components/notify.md@0123456789abcdef0123456789abcdef01234567
    endpoint: github-shared-components
    with:
      channel: service-alerts
      severity: warning
      labels: [agentic, automated]
      delivery:
        retries: 3
safe-outputs:
  notify-team:
    require-approval: true
```

`with:` values are not secrets. Pass secrets through custom safe-output `env:`
bindings, which name Azure DevOps variables and are scoped to the privileged
custom executor job.

## Merge semantics

Imports are merged in declaration order, then the consumer workflow is overlaid
on top. Precedence is:

```text
consumer workflow > later import > earlier import
```

- Scalar/singleton fields use the highest-precedence explicit value.
- Mapping collections (`tools`, `mcp-servers`, `safe-outputs`, `runtimes`,
  `env`) merge additively by key. Duplicate keys from two different imports are
  hard errors.
- Sequence fields (`parameters`, `repos`, `variable-groups`) concatenate in
  import order, then consumer entries.
- The consumer may configure an imported safe-output tool, for example by adding
  `require-approval`, but may not replace executor-defining fields such as
  `steps`, `env`, `inputs`, `run`, or `entrypoint`.
- Imported markdown bodies are concatenated in declaration order, followed by
  the consumer body. Imported bodies are **inlined into the agent prompt at
  compile time** (their `{{ ado.aw.import-inputs.* }}` placeholders are already
  substituted); in the default `inlined-imports: false` mode the consumer's own
  body is delivered ahead-of-time as a `{{#runtime-import}}` marker so it can
  still be edited without recompiling, while imported bodies — which can only be
  substituted at compile time — precede it inline.
- `import-schema:` and `imports:` are consumed by the merge and do not appear in
  the merged workflow.

## Example: shared custom safe-output job

Shared component manifest:

```markdown
---
import-schema:
  service:
    type: string
    required: true
safe-outputs:
  jobs:
    create-service-ticket:
      description: Create an incident ticket in the service desk.
      max: 2
      inputs:
        title:
          type: string
          required: true
          max-length: 160
        priority:
          type: choice
          options: [low, normal, high]
          required: true
      env:
        SERVICE_DESK_TOKEN: SERVICE_DESK_TOKEN
        SERVICE_NAME: SERVICE_NAME
      steps:
        - bash: |
            set -euo pipefail
            : > "$ADO_AW_SAFE_OUTPUT_RESULTS"
            while IFS= read -r proposal; do
              proposal_id=$(echo "$proposal" | jq -r '.proposal_id')
              title=$(echo "$proposal" | jq -r '.title')
              # Call your service-desk client here, honoring staged mode.
              jq -cn \
                --arg proposal_id "$proposal_id" \
                --arg title "$title" \
                '{schema_version:1, proposal_id:$proposal_id, status:"success", message:("created ticket for " + $title)}' \
                >> "$ADO_AW_SAFE_OUTPUT_RESULTS"
            done < "$ADO_AW_SAFE_OUTPUT_PROPOSALS"
          displayName: Create service ticket
---
Use `create-service-ticket` only when a durable service-desk record is needed
for `{{ ado.aw.import-inputs.service }}`.
```

Consumer workflow:

```yaml
imports:
  - uses: contoso/ado-aw-components/service-ticket.md@89abcdef0123456789abcdef0123456789abcdef
    endpoint: github-components
    with:
      service: payments-api
safe-outputs:
  create-service-ticket:
    require-approval:
      approvers: ["[Contoso]\\SRE Leads"]
      instructions: Confirm the ticket title and priority before approving.
```

The imported tool appears to the agent as a typed SafeOutputs MCP tool. Agent
proposals still flow through Detection and optional manual review before the
isolated `Custom_create_service_ticket` executor job receives the secret env
bindings and performs the side effect.
