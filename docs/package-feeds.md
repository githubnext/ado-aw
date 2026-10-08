# Package feeds (`permissions.packages`)

`permissions.packages` lets the agent restore packages from Azure Artifacts
feeds with `npm`, `pip`/`uv`, `dotnet`, and `cargo` **without ever holding a
feed credential**. The trusted `ado-proxy` container holds the credential and
attaches it only to read-only requests for the feeds the workflow grants.

## Why not the Azure Pipelines authenticate tasks?

`NuGetAuthenticate`, `npmAuthenticate`, `PipAuthenticate`, and
`CargoAuthenticate` publish their credential in a form the client tools can read:

- a **non-secret** job variable (`VSS_NUGET_ACCESSTOKEN`, `PIP_EXTRA_INDEX_URL`
  with an embedded token, `CARGO_REGISTRIES_<NAME>_TOKEN`); or
- a file (`_authToken` lines in `.npmrc`).

Azure Pipelines maps non-secret variables into every later step, and AWF starts
the agent with `--env-all`, so any of them would hand the job's token to the
agent. The compiler therefore never runs these tasks in a job that starts AWF,
and rejects them in `steps:` and `safe-outputs.threat-detection.steps` (see
[`runtimes.md`](runtimes.md#package-feed-credentials)).

For Azure Artifacts the tasks grant no extra access anyway. They use the same
build identity, or the same workload-identity exchange, that
`permissions.packages` uses.

## Front matter

```yaml
permissions:
  packages:
    # Optional. Omit to use the job's build identity.
    service-connection: artifacts-reader-wif
    connection-type: azureRM          # azureRM (default) | azureDevOps
    identity-role: reader             # reader | collaborator (default)
    feeds:
      - name: internal                # handle for runtimes.<x>.feed; defaults to `feed`
        organization: contoso         # defaults to the current organization
        project: Engineering          # omit for an organization-scoped feed
        feed: internal-packages       # feed name or GUID
        view: Release                 # optional; only this view is readable
        protocols: [npm, pypi, nuget, cargo]
        upstream: deny                # deny (default) | allow

runtimes:
  node:
    feed: internal                    # make the feed npm's registry
  python:
    feed: internal
    public-registry: block            # remove pypi.org from the allowlist
  dotnet:
    feed: internal
```

| Field | Description |
|---|---|
| `service-connection` | Workload-identity-federated service connection used to mint the feed credential. Omit to use the build identity (`$(System.AccessToken)`). |
| `connection-type` | `AzureCLI@3` connection type for `service-connection`: `azureRM` (default) or `azureDevOps`. Only valid with `service-connection`. |
| `identity-role` | Highest feed role the identity holds. `collaborator` (default) matches the build identity's default feed role. Declare `reader` when the identity holds only **Feed Reader**. |
| `feeds[].name` | Local handle referenced by `runtimes.<x>.feed`. Defaults to `feed`. Must be unique, case-insensitively. |
| `feeds[].organization` | Organization hosting the feed. Defaults to the current organization; required when the compiler cannot infer the organization from the git remote and a runtime selects the feed. |
| `feeds[].project` | Project for a project-scoped feed. Omit for an organization-scoped feed. |
| `feeds[].feed` | Feed name or GUID. |
| `feeds[].view` | Feed view such as `Release`. When set, requests must target `feed@view`. Requests for the bare feed or another view are refused. |
| `feeds[].protocols` | Required, non-empty subset of `npm`, `pypi`, `nuget`, `cargo`. |
| `feeds[].upstream` | Whether agent reads may save packages from the feed's upstream sources. See [Upstream ingestion](#upstream-ingestion). |

Compilation fails when:

- `feeds` or any `protocols` list is empty, or lists a protocol twice;
- two feeds share a handle, or the same feed, organization, project, and view
  is granted twice;
- `connection-type` is set without `service-connection`;
- a feed has `upstream: deny` with no `view` while `identity-role` is
  `collaborator`;
- `runtimes.<x>.feed` names a feed that is not granted, or a feed that does not
  grant that runtime's protocol;
- `runtimes.<x>.feed` is combined with `feed-url` or `config`.

`permissions-required.packages: true` makes the `permissions.packages` block
mandatory, for imported components that need feed access.

## How a request flows

1. The agent runs `npm install`, `pip install`, `uv sync`, `dotnet restore`, or
   `cargo fetch`.
2. Each tool name resolves to a generated wrapper in `/tmp/ado-aw-pkg-bin`,
   which AWF prepends to `PATH`. The wrapper sets `HTTPS_PROXY` and per-tool CA
   trust **for that process only**, then `exec`s the real binary.
3. The `ado-proxy` container terminates TLS for `pkgs.dev.azure.com` with a
   leaf signed by its private interception CA. Every other `CONNECT`
   destination is tunnelled unchanged to AWF's Squid, so the AWF allowlist
   still governs it.
4. The proxy checks the request before attaching anything:
   - the method is `GET` or `HEAD`;
   - the path matches a catalogued protocol route
     (`npm/registry/`, `pypi/simple/`, `pypi/download/`, `nuget/v3/`,
     `cargo/`);
   - the organization, project, feed (name or resolved GUID), and view match a
     grant;
   - the protocol is granted for that feed.
5. The proxy strips any client credential and attaches the feed credential:
   `Bearer` for npm and Cargo, `Basic` (`ado-aw:<token>`) for PyPI and NuGet.
6. The request leaves through Squid. Azure Artifacts authenticates the token and
   applies the identity's feed role.
7. Package downloads usually answer with a redirect to blob storage. The proxy
   never follows redirects. It relays a `3xx` `Location` only when the target
   is `*.vsblob.visualstudio.com` or `*.blob.core.windows.net`. The client then
   fetches that pre-signed URL through Squid without a credential. Any other
   redirect is refused with `redirect-denied`.

A tool that bypasses its wrapper, for example `python -m pip`, reaches
`pkgs.dev.azure.com` through Squid with no credential and is refused by Azure
Artifacts. Routing selects the credentialed path; it is never what keeps the
credential from the agent.

### Wrapper environment

| Tool | Variables set for that process |
|---|---|
| `npm`, `npx` | `HTTPS_PROXY`, `NODE_EXTRA_CA_CERTS=<proxy CA>`, `npm_config_audit=false` |
| `pip`, `pip3` | `HTTPS_PROXY`, `PIP_CERT=<system roots + proxy CA>` |
| `uv`, `dotnet` | `HTTPS_PROXY`, `SSL_CERT_FILE=<system roots + proxy CA>` |
| `cargo` | `HTTPS_PROXY`, `CARGO_HTTP_PROXY`, `CARGO_HTTP_CAINFO=<system roots + proxy CA>`, and a credential provider that prints a non-secret placeholder token |

The CA bundle contains public certificates only. The proxy CA's private key
never leaves the proxy container. The CA is never installed system-wide.

`npm audit` is a `POST`, which the proxy refuses, so the npm wrapper disables
it.

## Credential custody

### Build identity (`service-connection` omitted)

`$(System.AccessToken)` belongs to:

- `<Project> Build Service (<org>)` when **Limit job authorization scope to
  current project** is on;
- otherwise, `Project Collection Build Service (<org>)`.

The token is secret and is not exported to step environments by default. Only
the proxy start step maps it, as a secret env value, into the material document
it streams to the proxy over stdin. That step runs and exits before AWF starts.
The token only works in the current organization. A feed in another
organization fails at startup; use a service connection for it.

### Workload identity (`service-connection` set)

An `AzureCLI@3` step exchanges the pipeline's OIDC token for an Azure
DevOps-audience Entra token. It runs
`az account get-access-token --resource 499b84ac-1321-427f-aa17-267ca6975798`
and stores the result in the secret variable `SC_PACKAGES_TOKEN`. Only the
proxy start step maps it, and AWF excludes it from `--env-all`. Entra tokens
last about 60–90 minutes and are not yet renewed, so the agent's
`timeout-minutes` is capped as for `permissions.read`.

The identity must be in the same Entra tenant as every target organization.
Cross-tenant feeds are not supported.

### What the credential could do

Neither credential is scoped to feeds: each can do whatever its identity can
do. The boundary is therefore **custody** (the token exists only in the start
step's secret environment and in proxy memory) plus **policy** (the proxy uses
it only for `GET`/`HEAD` requests to granted feeds and protocols). A
reader-only identity also limits what the token could do if that boundary
failed.

## Startup resolution and preflight

Before AWF starts, the proxy start step runs
`node ado-proxy.js resolve-feeds` on the host, with the credential on stdin. For
each grant it:

1. resolves the feed (and project) name to its GUID through
   `feeds.dev.azure.com`, so requests that use the GUID form match the same
   grant;
2. confirms that the identity can read the feed.

Any failure stops the job before the agent starts. The error names the feed,
the organization and project, the identity (when Azure DevOps reports it), and
the role to grant. A `404` is reported with the same guidance because Azure
Artifacts answers `404` for a feed the identity cannot see.

## Granting feed access

The authenticate tasks have exactly these requirements too; neither path
bypasses feed permissions.

| Identity | Explicit grant needed? |
|---|---|
| Build identity, project-scoped feed in the same project | Usually no. New feeds grant the project's Build Service **Feed and Upstream Reader (Collaborator)**. |
| Build identity, organization-scoped feed | Yes when **Limit job authorization scope to current project** is on. Defaults grant only the Project Collection Build Service. |
| Build identity, another project's feed | Yes. Grant the job identity a feed role and make sure it can see that project. |
| Build identity, feed whose default permissions were changed | Yes. |
| Workload identity | Always. |

To grant a feed role:

1. Open **Artifacts**, select the feed, then **Feed settings → Permissions**.
2. Select **Add users/groups** and add the identity:
   - the build identity named in the preflight error, for example
     `Engineering Build Service (contoso)`; or
   - the service principal behind the service connection. First add it to the
     organization (**Organization settings → Users**) with an access level that
     includes Azure Artifacts.
3. Choose the role. **Feed Reader** is recommended and is declared with
   `identity-role: reader`. **Feed and Upstream Reader (Collaborator)** is
   needed only for `upstream: allow`.

## Upstream ingestion

Azure Artifacts saves a package from an upstream source (npmjs, nuget.org,
PyPI, crates.io) the first time an identity with the **Collaborator** role or
higher requests it. A plain read can therefore write to the feed. `upstream`
records which behavior the author intends, and the compiler requires a
configuration that achieves it:

| Configuration | Effect |
|---|---|
| `upstream: allow` | Agent reads may save new upstream packages into the feed. Explicit opt-in. |
| `upstream: deny` with `view:` | Only packages already promoted to the view are readable. Views never ingest. |
| `upstream: deny` with `identity-role: reader` | The identity cannot save upstream packages, so only cached packages restore. |
| `upstream: deny`, no `view`, `identity-role: collaborator` | Compile error. |

The proxy cannot see whether a read triggers ingestion, so it does not enforce
`upstream` itself. `identity-role` is a declaration the compiler trusts.
Grant the role you declare.

## Selecting the feed as the package source

Routing works for any canonical `pkgs.dev.azure.com` URL, so a feed already
configured in the repository (`nuget.config`, `.npmrc`, `pyproject.toml`,
`.cargo/config.toml`) works unchanged, provided it is granted.

`runtimes.<x>.feed` makes a granted feed the tool's default source, using
credential-free configuration:

| Runtime | Source selection |
|---|---|
| `python` | `PIP_INDEX_URL` and `UV_DEFAULT_INDEX` set to the feed's `pypi/simple/` URL. |
| `node` | `NPM_CONFIG_REGISTRY` set to the feed's `npm/registry/` URL. |
| `dotnet` | A `nuget.config` with `<clear/>` and the feed's `nuget/v3/index.json`, written before AWF starts only when the workspace has none. |

There is no Cargo runtime yet. Select a Cargo feed in the repository's
`.cargo/config.toml` (#1823 tracks Rust toolchain support).

## Blocking public registries

`runtimes.<x>.public-registry: block` removes that ecosystem's public registry
API hosts from the AWF allowlist, so packages can only come from internal
sources:

| Runtime | Hosts removed |
|---|---|
| `python` | `pypi.org`, `pypi.python.org`, `files.pythonhosted.org`, `*.pythonhosted.org` |
| `node` | `registry.npmjs.org`, `registry.npmjs.com`, `registry.yarnpkg.com`, `repo.yarnpkg.com` |
| `dotnet` | `api.nuget.org`, `nuget.org`, `azuresearch-usnc.nuget.org`, `azuresearch-ussc.nuget.org` |

Public hosts are never redirected to a feed at the network layer; source
selection is how packages reach the feed. Core wildcards such as
`*.blob.core.windows.net` stay allowed because the feed's own downloads need
them.

The compiler warns when `block` leaves the runtime with no source, or with a
feed that serves only cached packages (`upstream: deny` and no `view`).

## Denials and audit

Denied requests answer `403` with a stable reason, and every decision is written
to the `ado-proxy` decision log with `family: packages` and the protocol:

| Reason | Meaning |
|---|---|
| `method-not-read` | Anything other than `GET`/`HEAD`, for example a publish or delete. |
| `unknown-route` | A path outside the catalogued protocol routes. |
| `feed-not-granted`, `view-not-granted`, `protocol-not-granted` | The request targets a feed, view, or protocol the workflow does not grant. |
| `path-traversal`, `encoded-separator`, `double-encoding`, `malformed-target` | A path the proxy refuses to interpret. |
| `redirect-denied` | Azure Artifacts redirected somewhere other than allowlisted blob storage. |
| `upstream-unauthorized` | Azure Artifacts rejected the credential (`401`/`203`). Returned to the client as `502`. |
| `credential-unavailable` | The proxy has no package credential. |

`ado-aw audit` rolls the decisions up by family and protocol, and reports a
finding for `upstream-unauthorized`, which usually means the identity's feed
role is missing.

## Limitations

- Azure DevOps Services only: no Azure DevOps Server or sovereign clouds.
- The legacy `{org}.pkgs.visualstudio.com` host is not intercepted. Requests
  to it carry no credential and fail. Use the canonical `pkgs.dev.azure.com`
  URL.
- External feeds behind service connections (the authenticate tasks'
  `externalFeedCredentials` / endpoint inputs), Maven, and Universal Packages
  are not supported.
- Clients invoked other than by name (`python -m pip`, absolute paths) bypass
  the wrapper and fail closed.
- Workload-identity tokens are not renewed. Runs are bounded by the
  `permissions.read` timeout cap.
- The Detection job has no package proxy.
- The authentication scheme per protocol and the redirect hosts follow the
  official clients and community reports. They need validation against a live
  feed; see `tests/smoke/REGISTERED.md`.

## Migrating from `feed-url`

Codemod `0009_package_feed_permissions` rewrites Azure Artifacts
`runtimes.<x>.feed-url` values into `permissions.packages` grants plus
`runtimes.<x>.feed` the first time an older workflow is compiled. See
[`codemods.md`](codemods.md#package-feed-permissions-0009_package_feed_permissions).
