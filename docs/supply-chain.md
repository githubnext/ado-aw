# Internal Supply-Chain Mirror (`supply-chain:`)

By default a compiled agentic pipeline fetches four kinds of artifact from
GitHub / GHCR at run time:

| # | Artifact | Default source |
|---|----------|----------------|
| 1 | `ado-aw` compiler (`ado-aw-linux-x64`) | `github.com/githubnext/ado-aw` releases |
| 2 | AWF firewall (`awf-linux-x64`) | `github.com/github/gh-aw-firewall` releases |
| 3 | `ado-script.zip` bundle | `github.com/githubnext/ado-aw` releases |
| 4 | AWF + MCPG container images | `ghcr.io/github/...` |

The optional `supply-chain:` front-matter section reroutes these fetches to an
**internal Azure DevOps Artifacts feed** or one exact **Azure DevOps pipeline
artifact** (for the binaries #1–#3), and/or an **internal container registry**
(for the images #4). This is intended for supply-chain-hardened environments
where the build agent pool cannot reach GitHub / GHCR.

When `supply-chain:` is omitted, the generated pipeline is byte-for-byte
identical to before — there is no behavioural change for existing agents.

## Configuration

```yaml
supply-chain:
  feed:                          # mirrors binaries #1, #2, #3
    name: my-project/my-feed     # feed name or "project/feed"
    service-connection: feed-conn  # optional (see Authentication)
  # Alternative to feed (the two are mutually exclusive):
  # pipeline-artifact:
  #   project: AgentPlayground
  #   definition-id: 2560
  #   run-id: 630001
  #   artifact: ado-aw-candidate
  registry:                      # mirrors images #4
    name: myacr.azurecr.io/mirror  # registry host or base path
    service-connection: acr-conn   # required when registry is set
  service-connection: shared-conn  # optional shared fallback for both targets
```

| Field | Type | Required | Purpose |
|-------|------|----------|---------|
| `feed` | scalar **or** `{ name, service-connection }` | optional | Enables the binary mirror (#1–#3). A bare string is shorthand for `{ name: <string> }`. |
| `pipeline-artifact` | `{ project, definition-id, run-id, artifact }` | optional | Uses one exact producer run as the complete binary source (#1–#3). Mutually exclusive with `feed`. |
| `registry` | scalar **or** `{ name, service-connection }` | optional | Enables the image mirror (#4), independently of either binary source. |
| `service-connection` | string | optional | Shared fallback connection for `feed` and `registry`; it does not apply to `pipeline-artifact`. |

`feed` and `pipeline-artifact` are mutually exclusive. `registry` is
independent and may accompany either binary source.

### Scalar shorthand

A bare scalar is sugar for an object with no per-target connection:

```yaml
supply-chain:
  feed: my-feed                  # same as { name: my-feed }
  registry: myacr.azurecr.io
  service-connection: shared-conn
```

## Authentication

Authentication is **asymmetric** because the two targets authenticate
differently:

### Feed (binaries)

The feed mirror uses `NuGetAuthenticate@1` + `DownloadPackage@1`. The effective
service connection resolves as:

1. the feed's own `service-connection`, else
2. the top-level `service-connection`, else
3. `$(System.AccessToken)` (the build service identity).

For a **same-organization** feed, no service connection is required: grant the
pipeline's build identity (e.g. `<Project> Build Service`) the **Feed Reader**
role and `NuGetAuthenticate@1` authenticates automatically via
`$(System.AccessToken)`. Set a `service-connection` only for cross-org or
external feeds.

### Pipeline artifact (binaries)

The pipeline-artifact source is immutable configuration: the generated
`DownloadPipelineArtifact@2` tasks always use `source: specific`,
`runVersion: specific`, the configured project, definition ID (`pipeline`),
run ID (`runId`), and artifact name. The compiler never selects latest runs,
tags, or triggering pipelines.

`project` must be an Azure DevOps project name or canonical GUID;
`definition-id` and `run-id` must be positive integers; and `artifact` follows
Azure DevOps artifact-name rules. The current build identity must have
permission to read the configured project, pipeline run, and artifact.
The producer must be in the same Azure DevOps organization; cross-organization
pipeline-artifact downloads are not supported by this source.
`supply-chain.service-connection` is not used for this source, and no
`NuGetAuthenticate@1` step is emitted.

With an exact run ID, the artifact is consumable as soon as its publish task
has completed even if later jobs keep the producer run active. Consumers
queued before the named artifact exists fail; producers should confirm artifact
visibility before queueing them.

### Registry (images)

The image mirror authenticates with `az acr login` (`AzureCLI@2`) using the
resolved service connection, then `docker pull`s the rewritten image
references. **`$(System.AccessToken)` cannot authenticate to a container
registry**, so a service connection **must** resolve when `registry` is set —
either `registry.service-connection` or the top-level `service-connection`.
Compilation fails otherwise:

```
supply-chain.registry requires a service connection: set
`registry.service-connection` or a top-level `supply-chain.service-connection`.
A container registry (ACR) cannot be accessed with $(System.AccessToken).
```

The registry connection is an ARM / Azure service connection (the same kind
used by `permissions:`), passed to `AzureCLI@2` as `azureSubscription`.

> **Private Link note:** the ACR name passed to `az acr login --name` is derived
> from the host portion of `registry.name`, assuming the standard
> `<name>.azurecr.io` login server. If your ACR is reached over Azure Private
> Link with a custom domain (e.g. `myacr.internal.contoso.com`), set
> `registry.name` to the canonical `*.azurecr.io` login server so the derived
> registry name is correct.

## What the binary source and registry must contain

Versions stay **pinned by the generating compiler** — the internal mirror must
host the exact pinned versions. A mirror that is a few days behind is fine: use
the matching `ado-aw` compiler version.

### Feed packages (NuGet)

The feed must host these NuGet packages (same base names, **bare semver**
versions — no leading `v`):

| Package id | Version | Must contain |
|------------|---------|--------------|
| `ado-aw` | compiler version (e.g. `0.37.0`) | `ado-aw-linux-x64`, `checksums.txt` |
| `awf` | the AWF version (e.g. `0.27.3`) | `awf-linux-x64`, `checksums.txt` |
| `ado-script` | compiler version | `ado-script.zip`, `checksums.txt` |

A NuGet package is a renamed zip; the compiler unzips it and relocates the
payload, so the package simply needs to carry the same files (and matching
`checksums.txt`) that the GitHub release ships. **Checksum verification with
`sha256sum -c checksums.txt` is preserved**, so the mirror must ship the
matching `checksums.txt`.

### Pipeline artifact

The single configured artifact is a complete source and must contain exactly
one file with each of these names (directory nesting is allowed):

- `ado-aw-linux-x64`
- `awf-linux-x64`
- `ado-script.zip`
- `checksums.txt`
- `provenance.json`

`checksums.txt` must have exactly one standard sha256sum entry for each payload,
with the exact filename. Each consumer locates its payload plus the shared
manifest and provenance document, verifies the exact checksum entry, then
preserves the normal chmod/move/unzip/PATH behavior.

`provenance.json` must be a JSON object using this Phase 1 contract:

```json
{
  "schema": "ado-aw/candidate-artifact/1",
  "producer_definition_id": 2560,
  "producer_build_id": 630001,
  "repository": "githubnext/ado-aw",
  "source_ref": "refs/heads/main",
  "source_version": "0123456789abcdef0123456789abcdef01234567",
  "reason": "IndividualCI",
  "compiler_version": "0.45.1",
  "awf_version": "0.27.32"
}
```

The required identity fields are exactly `schema`, `producer_definition_id`,
and `producer_build_id`; both producer IDs are JSON numbers. Compilation embeds
the configured IDs, and every consumer fails closed if the schema or either
producer ID is missing, non-numeric, or mismatched. `repository`, `source_ref`,
`source_version`, `reason`, `compiler_version`, and `awf_version` are optional
diagnostic fields: when present, the generated step includes them in the
validated, non-secret provenance output. Their absence does not invalidate an
otherwise valid artifact.

### Registry images

`registry.name` is a registry **host or base path** — teams generally cannot
publish under GHCR's `github/...` namespace, so the original GHCR prefix is
**not** preserved. Only the **artifact name** (the final image-name segment)
is kept, placed directly under the configured base path at the **same tag**:

| GHCR source | Internal reference (base path `<registry>`) |
|-------------|---------------------------------------------|
| `ghcr.io/github/gh-aw-firewall/squid:<awf-version>` | `<registry>/squid:<awf-version>` |
| `ghcr.io/github/gh-aw-firewall/agent:<awf-version>` | `<registry>/agent:<awf-version>` |
| `ghcr.io/github/gh-aw-firewall/api-proxy:<awf-version>` | `<registry>/api-proxy:<awf-version>` |
| `ghcr.io/github/gh-aw-mcpg:v<mcpg-version>` | `<registry>/gh-aw-mcpg:v<mcpg-version>` |

`<registry>` may be a bare host (`myacr.azurecr.io`) or a host with an
arbitrary namespace path (`myacr.azurecr.io/oss-mirror`,
`contoso.azurecr.io/team/oss/mirror`). The contract is only that the artifact
names (`squid`, `agent`, `api-proxy`, `gh-aw-mcpg`) and tags remain unchanged
under that path. `az acr login` derives the ACR registry name from the host
portion of the base path.

> **The `agent` image is dual-purpose.** It backs both the AWF sandbox that
> runs the Copilot CLI *and* the containerized SafeOutputs MCP server that
> MCPG spawns as a hardened stdio sibling (`--network none`, non-root,
> read-only rootfs — see [`docs/mcpg.md`](mcpg.md)). The compiler resolves
> the same rewritten `<registry>/agent:<awf-version>` reference for both, so
> no separate mirror entry is needed for SafeOutputs.

AWF 0.27.32+ always runs with its api-proxy sidecar enabled, so `api-proxy`
must be pre-pulled and mirrored alongside `squid`, `agent`, and MCPG — it is
not an optional/BYOK-only image. When `registry` is configured, the compiler
passes both `--image-tag <awf-version>` and `--image-registry <registry>`
directly to the AWF invocation so `--skip-pull` resolves every pre-pulled image
(including `api-proxy`) under the mirror name instead of GHCR.

## Examples

Mirror everything, two different connections:

```yaml
supply-chain:
  feed:
    name: my-project/my-internal-feed
    service-connection: feed-conn
  registry:
    name: myacr.azurecr.io
    service-connection: acr-conn
```

Binaries only, same-org feed (uses `$(System.AccessToken)`):

```yaml
supply-chain:
  feed: my-internal-feed
```

All binaries from one exact candidate pipeline run:

```yaml
supply-chain:
  pipeline-artifact:
    project: AgentPlayground
    definition-id: 2560
    run-id: 630001
    artifact: ado-aw-candidate
```

Candidate binaries plus mirrored images:

```yaml
supply-chain:
  pipeline-artifact:
    project: AgentPlayground
    definition-id: 2560
    run-id: 630001
    artifact: ado-aw-candidate
  registry:
    name: myacr.azurecr.io
    service-connection: acr-conn
```

Images only:

```yaml
supply-chain:
  registry:
    name: myacr.azurecr.io
    service-connection: acr-conn
```

## Network isolation note

The mirror fetches (`NuGetAuthenticate@1`, `DownloadPackage@1`,
`DownloadPipelineArtifact@2`, `docker pull`, `az acr login`) run as ordinary
ADO steps on the build agent — **outside** the AWF network-isolation sandbox,
which wraps only the copilot agent command.
Consequently:

- The feed/registry hosts are **not** added to the agent's AWF
  `--allow-domains` allowlist (the network-isolated agent never contacts them).
- True isolation of the build agent from GitHub / GHCR is enforced by the agent
  pool's own network policy; the `supply-chain:` rerouting is what lets such a
  locked-down pool succeed.

See also: [`docs/network.md`](network.md),
[`docs/front-matter.md`](front-matter.md).
