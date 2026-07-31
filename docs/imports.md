# Reusable imports

`imports:` composes reusable markdown workflow components at compile time. An
import may contribute prompt text and a controlled subset of front matter. The
compiler resolves every remote branch or tag to a full commit SHA, caches the
manifest, expands nested imports, and emits no runtime repository checkout.

This is separate from [`{{#runtime-import}}`](runtime-imports.md), which reads
prompt files on a pipeline runner.

## Syntax

### Local

Local paths are relative to the file declaring the import:

```yaml
imports:
  - ./components/review-guidance.md
```

Nested local imports are likewise relative to the importing component. They
must remain inside the consumer repository.

### Canonical remote object

The canonical form separates the manifest path/ref from the repository:

```yaml
imports:
  # Azure Repos, current organization and current project
  - uses: components/review.md@main
    repository: shared-agents

  # Azure Repos, current organization and explicit project
  - uses: components/review.md@v2
    repository: Platform/shared-agents

  # Azure Repos in another organization
  - uses: components/review.md@refs/tags/v2
    repository: Platform/shared-agents
    source: https://dev.azure.com/contoso

  # GitHub.com
  - uses: components/review.md@release/v2
    repository: githubnext/shared-agents
    source: github.com

  # GitHub Enterprise Server
  - uses: components/review.md@0123456789abcdef0123456789abcdef01234567
    repository: engineering/shared-agents
    source: ghe.contoso.com
```

`uses:` is `path@ref`. `ref` may be a branch, tag, fully qualified
`refs/heads/...` / `refs/tags/...` name, or full 40-character commit SHA.

`repository:` accepts:

- `repository` — the current Azure DevOps project (Azure Repos only)
- `project/repository` — the current Azure DevOps organization
- `owner/repository` — GitHub.com or GHES

`source:` is an optional scalar:

- omitted — Azure Repos in the consumer's current organization
- an HTTPS Azure DevOps collection URL — cross-organization Azure Repos
- `github.com` — GitHub.com
- another validated host — GitHub Enterprise Server

`source:` is compile-time routing only. It is not a service connection and does
not create an Azure DevOps `resources.repositories` entry.

### Combined shorthand

For compatibility with gh-aw-style component references, the combined form is
also accepted:

```yaml
imports:
  - owner-or-project/repository/components/review.md@main
```

With omitted `source`, this means same-organization Azure Repos. To select
GitHub or cross-organization Azure Repos, use the canonical object and set
`source:`.

### Sections and optional imports

Append `#Heading` to import a matching `#` or `##` markdown section. Append `?`
to make a missing import optional:

```yaml
imports:
  - uses: components/review.md@main#Instructions?
    repository: Platform/shared-agents
```

Optional imports skip only a typed not-found result. Authentication failures,
invalid refs, malformed manifests, cache-integrity failures, and other errors
remain fatal.

## Ref resolution and committed cache

Remote refs are resolved to a full commit SHA before the manifest is fetched:

- Azure Repos uses the Git Refs and Git Items APIs.
- GitHub.com and GHES use `gh api` with the compiler host's existing auth.

The immutable manifest cache is stored below:

```text
.ado-aw/imports/cache/<source>/<project-or-owner>/<repo>/<sha>/<path>
```

The exact directory segments include stable hashes so source identities cannot
collide after filesystem sanitization. Each manifest has a `.sha256` sidecar.
`.ado-aw/imports/.gitattributes` marks the cache generated and `merge=ours`.

Requested-ref metadata is persisted in:

```text
.ado-aw/imports/refs.json
```

It records source, repository, requested ref, and resolved SHA. Commit the
cache and metadata together. When a branch/tag cannot be refreshed (for
example, during an offline `ado-aw check`), the compiler may reuse the recorded
SHA only when its immutable manifest is present in the committed cache. A
typed not-found response is not replaced by stale metadata.

## Nested imports and limits

Imports are expanded breadth-first in declaration order:

1. all imports declared by the consumer,
2. their nested imports,
3. the next depth, and so on.

Nested relative imports inherit their origin:

- local component → relative to that component's directory
- remote component → relative to that path in the same source/repository and
  the parent's resolved SHA

Limits are fail-closed:

- at most 20 unique resolved files
- at most 256 resolution lookups, counting duplicates resolved before
  deduplication
- maximum nesting depth 5
- maximum 256 KiB per manifest
- cycles report the import path

The same resolved file (including section) with the same `with:` values is
deduplicated. Resolving the same file with different `with:` values is an
error.

## `import-schema:` and `with:`

Components can declare typed, non-secret inputs:

```markdown
---
import-schema:
  channel:
    type: string
    required: true
  severity:
    type: choice
    options: [info, warning, critical]
    default: info
---
Notify `{{ inputs.channel }}` at `{{ inputs.severity }}` severity.
```

Consumer:

```yaml
imports:
  - uses: components/notify.md@v2
    repository: Platform/shared-agents
    with:
      channel: service-alerts
      severity: warning
```

`{{ inputs.x }}` is retained as the compile-time syntax. Do not use the Azure
DevOps `${{ ... }}` delimiter. Existing rich schema types remain supported:
`string`, `number`, `boolean`, `choice`, `array`, and one-level `object`
properties, including defaults and required values. Unknown inputs, missing
required inputs, wrong types, unresolved placeholders, and malformed
placeholders are compile-time errors.

Input values are not secrets. Bind secrets through the normal consumer-owned
pipeline configuration.

## Merge contract

Only these imported fields are applied:

| Field | Merge behavior |
|---|---|
| `imports`, `import-schema` | Consumed by import expansion/substitution |
| Markdown body | Imports in breadth-first order, then consumer body |
| `tools` | Recursive merge; command/allow arrays union and deduplicate; earlier imports provide defaults and consumer scalar settings win |
| `mcp-servers` | First import wins across imports; an imported server overrides a same-named consumer server |
| `network.allowed` | Ordered union across imports and consumer; all other network controls remain consumer-owned |
| `permissions-required` | Boolean OR of abstract `read` / `write` requirements |
| `safe-outputs.jobs` | Custom job names are unique across consumer and imports; duplicates fail |
| Built-in `safe-outputs` keys | Duplicates across imports fail; consumer configuration replaces imported built-in configuration |
| `runtimes` | Consumer fields override imported fields; earlier imports fill remaining fields |
| `env` | Duplicate keys across imports fail; consumer overrides |
| `repos` | Consumer entries first, then imported entries, deduplicated by alias/name. An import can therefore add a `resources.repositories` entry, including one that references an `endpoint:` service connection — review the compiled `.lock.yml` diff |
| `steps` | Imported steps prepend in import order; consumer steps follow |
| `post-steps` | Consumer post-steps first; imported post-steps follow in import order |

An imported custom job's executor is immutable, and consumers do not redeclare
it. Configure ado-aw policy such as approval or staged mode at the top-level
tool key:

```yaml
safe-outputs:
  send-notification:
    require-approval: true
```

Imported remote custom jobs receive compiler-owned compile-time provenance:

- `component-source`
- `component-ref` (the requested branch, tag, or SHA)
- `component-sha`
- `manifest-digest`

The removed runtime-checkout fields `component-repo-type` and
`component-endpoint` are not generated.

The following fields are consumer-owned and ignored with a warning when an
import declares them:

`name`, `description`, `target`, `engine`, `workspace`, `pool`, `on`,
concrete `permissions`, `variable-groups`, `parameters`, `setup`, `teardown`,
`execution-context`, `supply-chain`, `ado-aw-debug`, and `inlined-imports`.
Other unsupported imported fields are also warned and ignored.

## `permissions-required`

A component can declare abstract ADO capability requirements without naming a
consumer's service connections:

```yaml
permissions-required:
  read: true
  write: true
```

Requirements are unioned across all imports and the consumer. The consumer must satisfy `read: true` with a concrete Agent read connection:

```yaml
permissions:
  read: ado-read-connection
```

Imported components cannot set concrete `permissions:` values. Compilation
fails when required Agent read capability is missing. `write: true` is
satisfied by Stage 3's ordinary default `$(System.AccessToken)` capability;
`permissions.write` remains an optional consumer choice for cross-org scope or
named-identity attribution. The reusable validation helper is also exposed on
the typed requirements value for parent integration paths that
validate front matter outside import orchestration.
