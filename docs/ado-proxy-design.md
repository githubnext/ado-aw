# Credential-Isolated Azure DevOps Proxy (`ado-proxy`)

_Security contract and implementation design. The runtime is wired into
generated pipelines when `tools.azure-devops` is enabled, and the catalog
reports `runtime_available: true`._

## Why this is required

`permissions.read` names an Azure Resource Manager service connection, but its
Azure subscription or resource-group scope does not make the underlying
identity read-only in Azure DevOps. An AAD token for the Azure DevOps audience
inherits that identity's Azure DevOps permissions. The compiler therefore
cannot safely treat the service-connection name or ARM scope as an
authorization boundary.

The implementation keeps `SC_READ_TOKEN` out of the Agent, MCPG, Azure DevOps
MCP container, and Azure CLI. Only `ado-proxy` holds it. The MCP is redirected
at the proxy with `--add-host`; a generated `az` wrapper sets `HTTPS_PROXY`,
process-scoped CA trust, and a non-secret sentinel PAT.

## Scope

The provider supports Azure DevOps Services reads for:

- the current organization/project/repository (by name or GUID);
- Azure Repos `type: git` resources declared under `repos:`, repository-only;
- additional organizations/projects/repositories declared under
  `permissions.read.allow`, resolved organization-relatively.

Capabilities are global across those scopes and may be narrowed under
`permissions.read.capabilities`; discovery is always enabled. Cross-org works
only where the one service-connection identity has access in the same AAD
tenant. Cross-tenant reads need another credential and are unsupported.

The following remain outside this provider:

- Azure Resource Manager, Microsoft Graph, and Azure data-plane APIs;
- Stage 1 mutations;
- credential-, token-, key-, SAS-, service-connection-, variable-secret-, and
  secure-file-returning operations;
- Azure DevOps Server/on-premises and sovereign/custom clouds;
- git smart HTTP, artifacts and signed redirects, Analytics/OData, and broad
  batch APIs until separately modeled.

Writes remain SafeOutputs or future privileged executors.

## Trust boundaries

Trusted components are the Azure Pipelines host task, AWF, Squid, the managed
policy sidecar, MCPG, and the pinned container/runtime supply chain. The Agent,
its prompt, repository content, tool arguments, generated scripts, and all
client-provided HTTP headers and bodies are untrusted.

The protected credential set is:

- the identity behind `permissions.read`;
- workload-identity assertions and the host-task `System.AccessToken` used to
  mint the one-shot ADO token;
- every Azure DevOps REST bearer minted from that identity;
- proxy CA and leaf private keys.

Those values must never appear in Agent or Detection environment, argv,
`/proc`, files, mounts, prompts, MCP configuration or payloads, logs, or
published artifacts. Existing package-feed credentials created by
`PipAuthenticate`, `npmAuthenticate`, or `NuGetAuthenticate` are a separate,
explicitly out-of-scope path.

## Network contract

ado-aw uses AWF `--network-isolation`. `awf-net` is an internal Docker network;
the Agent has no direct internet route and there is no legacy iptables/DNAT
fallback.

**Enforcement comes from topology, not from client cooperation.** Squid denies
the protected Azure DevOps hosts to the Agent, so the only route to them is
through the policy engine. A client that is misconfigured, ignores proxy
environment variables, or declines to trust the interception certificate does
not reach Azure DevOps unpoliced — it fails.

That gives a useful property: certificate trust is an **availability** control,
not a security one. It decides whether a given client *succeeds* or *fails
closed*. It never decides whether policy applies. This is what allows trust to
be distributed narrowly, per client, instead of container-wide.

### Ingress is per client; policy is shared

Different clients reach the policy engine by different means, because they
differ in what they can be told. All of them terminate at the same catalog, so
there is exactly one place where "what may be read" is decided.

| Client | Ingress | Certificate trust scope |
|---|---|---|
| Azure CLI (`az`) | an agent-side wrapper sets `HTTPS_PROXY` at the engine's `CONNECT` port and execs stock `az`, which keeps the canonical `dev.azure.com` URL | the `az` process only |
| Azure DevOps MCP | container on a Docker `--internal` network, where `dev.azure.com` is redirected at the policy engine via `--add-host`. Internal is load-bearing: a normal bridge has outbound NAT and would leave a direct route past the engine | the MCP container only |
| Hand-rolled `curl` / SDK calls from the Agent | none — Squid denies the protected hosts | none; fails closed |

**Why `az` needs no argument rewriting.** Earlier drafts pointed `az` at the
engine with `--organization https://<engine>/<org>`. That works — `az` accepts
an arbitrary base URL — but it is both harder and weaker than redirecting the
transport. Harder, because the organization can also arrive via `--org`,
`AZURE_DEVOPS_ORG`, or a stored `az devops configure --defaults` value, so the
wrapper would have to enumerate every form and stay correct as the CLI evolves;
a missed form silently escapes the policy. Weaker, because a non-canonical
hostname has no interception leaf and matches no catalogued route — both are
keyed to `dev.azure.com`.

Setting `HTTPS_PROXY` instead puts the redirect *below* the CLI's own
configuration, so every form resolves to the same canonical host and that host
is what gets intercepted. Verified with real `az` 2.86 against the live engine:
`az devops project list --organization https://dev.azure.com/contoso` completed
the `CONNECT`, verified the intercepted certificate from `REQUESTS_CA_BUNDLE`
alone, and the request arriving upstream carried the injected bearer rather
than the sentinel the CLI held. No public hostname is impersonated to anything
but this one process, and no CA is installed system-wide.

**Why the MCP uses a DNS alias.** It cannot be told. `@azure-devops/mcp`
derives its base URL as `"https://dev.azure.com/" + orgName` with no override,
so redirection has to happen at name resolution. A Docker network alias also
covers the server's raw `fetch()` call sites, which ignore proxy environment
variables regardless of configuration, and therefore removes any dependency on
`NODE_USE_ENV_PROXY` (unavailable on the pinned Node 20 image).

The alias must live on a network that Squid is **not** attached to. Squid
resolves `dev.azure.com` for the policy engine's own egress; if it saw the
alias, that egress would loop back to the engine.

### Attack surface this leaves

- The Agent may still address the policy engine directly on the internal
  network. That is safe by construction: the engine applies the same policy to
  every caller, is not a generic relay, and can reach the internet only through
  Squid.
- `az rest` and `az devops invoke` can express arbitrary REST calls. Command
  -level filtering alone does not contain them; they are contained because they
  travel the same base URL and are therefore subject to the catalog.
- Requests the client sends to a host outside the protected set are not
  policed by the catalog and are not intended to be. Squid's domain policy is
  the control there, exactly as for any other traffic.

## Authentication and TLS

The Agent never holds an Azure DevOps credential. The policy engine removes all
client-supplied authorization and injects the current bearer only after a
request matches an allowed operation and resource scope.

### Certificate strategy, per client

The engine mints certificates at startup; private keys exist only on its own
tmpfs. What differs per client is *which name* is certified and *who trusts it*.

**`az` — a certificate for an endpoint we own.** The broker points `az` at the
engine's own hostname, so the certificate is issued for that name rather than
for `dev.azure.com`. Trust is supplied to the single `az` process via
`REQUESTS_CA_BUNDLE`. Nothing impersonates a public hostname, and no trust
anchor is installed in any trust store.

**MCP — an interception certificate for `dev.azure.com`.** Because the base URL
is hardcoded, the engine must present a certificate for the real name, served
by SNI. Trust is installed **only in the MCP container**, whose image, network,
and environment we fully control.

This split matters. A CA trusted container-wide is trusted for *every* host by
*every* process for the whole run; scoping it to one container bounds that to a
single, purpose-built process. The engine also mints leaves only for protected
hosts, so even within that container it cannot impersonate anything else.

### Why not install the CA container-wide

The earlier design installed one CA into the Agent's trust stores. It was
rejected on evidence:

- There is no single trust store. `update-ca-certificates` covers curl, git,
  Go, and .NET, but Python's `requests` uses its own bundled
  `certifi/cacert.pem` and Node ignores the OS store entirely. Measured: with
  the OS store updated and nothing else, `az` still fails
  `CERTIFICATE_VERIFY_FAILED`.
- The remedies are themselves sharp. `REQUESTS_CA_BUNDLE`/`SSL_CERT_FILE`
  *replace* rather than extend the bundle, so they must carry the public roots
  too or every non-Azure-DevOps HTTPS request breaks.
  `NODE_EXTRA_CA_CERTS` takes a single path, so it must be concatenated with
  any ssl-bump CA rather than overwritten.
- Each additional runtime — Go, Java, .NET — needs its own handling, so the
  mechanism does not converge.

Per-client scoping avoids all of this and yields a smaller blast radius.

### Where the CA is minted

The constraint is narrow: **the CA private key must never be readable by the
agent.** It does not follow that the key must be minted inside the engine's
container — an earlier draft claimed that, and it was wrong.

Two facts make a simpler arrangement safe. The engine starts *before* the AWF
invocation, so during CA setup no agent exists to read anything. And the
material can be passed on **stdin**, so it never touches a filesystem at all —
not the runner's, not the container's.

Because the protected host set is compiler-known, the **leaves are generated
alongside the CA**, and the whole lot arrives as one PEM stream from a host
pipeline step:

```sh
# host step: mints CA + one leaf per protected host, straight to stdout
generate_ca_material \
  | docker run -i --name ado-proxy … node:20-slim ado-proxy.js
```

Generation runs directly on the runner with `openssl`, not in a helper
container. Every compiled pipeline **already** depends on host `openssl` —
`prepare_mcpg_config_step` mints the MCPG API key with `openssl rand` on every
run — so this adds no new dependency, no second image to pull, and nothing
further for `supply-chain:` to mirror. A helper container would have been pure
overhead.

Verified end to end: CA and all three leaves (`dev.azure.com`,
`app.vssps.visualstudio.com`, and the engine's own broker hostname) reach the
container, and a client verifies the served identity **as `dev.azure.com`
against the piped CA** (`authorized: true`). Piping configuration into a
container this way is the pattern MCPG already uses (`echo "$MCPG_CONFIG" |
docker run -i …`).

`openssl` being absent is a hard failure, not a degradation: the step must exit
non-zero rather than continue without an interception identity.



Why this matters for the agent: AWF's chroot makes the agent's root the host's
`/host` bind mount, so the agent's `/tmp` **is** the runner's `/tmp` — which is
how AWF installs its own `gh` wrapper (`cp … /host/tmp/awf-lib/gh` appears
inside the chroot as `/tmp/awf-lib/gh`). A key written to a host path would
therefore be agent-readable. Keeping it on stdin sidesteps that entirely, rather
than relying on deleting it in time.

Only the **public** certificate is written to a host path, so it can be mounted
into the MCP container for `NODE_EXTRA_CA_CERTS`.

This also frees the base image. `ca.ts` shells out to `openssl` because Node can
parse X.509 but cannot issue it, and adding a certificate library would
reintroduce the native dependency this runtime exists to avoid. Measured:

| Image | `openssl` |
|---|---|
| `node:20-slim`, `node:20-bookworm-slim` | **absent** |
| `node:20` | present (3.0.19) |

With both CA and leaves generated on the host ahead of the engine, the engine
needs no `openssl` at all and runs on `node:20-slim`. A restart is fail-closed:
the engine holds the material in memory only, so a dead container ends the run
rather than silently serving a new CA the MCP does not trust.

`ca.ts` therefore changes from *minting* to *parsing* — it keeps the same
`CaMaterials` shape so nothing downstream moves.



### Upstream leg

The engine verifies the real Azure DevOps certificate normally;
`rejectUnauthorized` is never disabled. Interception is trusted at both ends
rather than bypassed at either. This is load-bearing and observable: during
testing the engine correctly refused a self-signed upstream with `unable to
verify the first certificate`.

### Credential delivery

`generate_acquire_ado_token` emits an `AzureCLI@2`
step that mints an ADO-audience token from the ARM service connection and stores
it as the secret pipeline variable `SC_READ_TOKEN`.

**Delivery never uses a runner path.** AWF mounts the runner's `/tmp` into the
agent at both `/tmp` and `/host/tmp`; a bearer written there is agent-readable
and destroys the boundary. Instead, the host step builds one versioned JSON
document containing base64 certificate material and the bearer and pipes it to
`docker run -i`. The engine reads it once from stdin and holds private material
in memory. The CA signing key and leaf keys are shredded immediately after
handover; only the public interception certificate is published.

The MCP and `az` wrapper receive a non-secret sentinel. The proxy strips all
client credential headers and attaches its bearer only after a complete allow
decision. The token is not exposed in container `Env`, argv, the process table,
or an agent-readable mount.

The token is not rotated. Proxied workflows are therefore bounded at compile
time to 50 minutes so the run cannot silently outlive its credential.

### Credential renewal

Renewal is deferred. Extending the 50-minute limit requires a trusted refresh
path that never exposes a WIF assertion, `System.AccessToken`, or refreshed ADO
bearer to the agent. Rollout must not fall back to an agent credential.

## Authorization contract

The operation catalog matches normalized host, method, route template, API
version, organization-relative project/repository scope, and bounded
operation-specific request fields. Every catalogued operation is `GET` or
`OPTIONS`; all other methods are rejected before route matching.

The in-tree catalog can be inspected with
`ado-aw catalog --kind ado-proxy --json`. Its `runtime_available` field is
`true`; the credential, topology, client and scope wiring are enabled.

Unknown hosts, methods, routes, API versions, redirects, or body shapes fail
closed. Client authorization is never preferred over the proxy credential.
Required Azure DevOps discovery, `X-TFS-*`, session, continuation, and API
version headers are preserved. Denial responses must be proven not to trigger
unsafe retries or interactive sign-in behavior.

Known credential-bearing endpoints are denied, but ordinary repository files,
work-item text, PR text, and build logs may still contain user-authored
secrets. The proxy limits API authority; it is not a general content
classification or exfiltration-prevention system.

## Runtime implementation

The proxy ships as **`ado-proxy`**, a TypeScript bundle in
`scripts/ado-script/`, packaged in `ado-script.zip` alongside the other
`ado-script` bundles and already covered by the `supply-chain:` mirror. A host
step bind-mounts the bundle into the existing `node:20-slim` image and starts
it before AWF. AWF's repeatable `--topology-attach` then dual-homes that
container onto `awf-net`; no new image is built, published, pinned, or mirrored.

It is not a Rust subcommand. A Rust implementation would need a TLS stack plus
certificate minting (`rustls` + `rcgen` → `ring`), which would make a native C
toolchain a hard build requirement for the whole compiler; ado-aw is otherwise
pure-Rust and must stay buildable without one. Node's built-in `tls`, `http`,
and `net` modules cover the same ground with no new runtime dependency, and
match how AWF implements its own credential-isolating sidecars.

Configuration is supplied by compiler-owned flags plus a mounted, read-only
policy JSON document. No credential is passed through argv, environment, or a
runner file: the bearer arrives in the versioned stdin material document. The
policy carries the `catalog_version` the bundle re-checks at startup, so a
stale compiler/bundle pair fails closed.

Request handling has exactly two paths:

- **Direct TLS for the DNS-redirected MCP.** The MCP connects to
  `dev.azure.com:443`, which `--add-host` redirects to the engine. It terminates
  TLS with a leaf selected by SNI (ALPN pinned to `http/1.1`), normalizes the request,
  evaluates it against the versioned catalog, drops every client credential and
  forwarding header, and — only after a complete allow decision, and only for a
  protected upstream — attaches the current bearer and forwards through Squid.
- **`CONNECT` for `az`.** The generated wrapper sets `HTTPS_PROXY` to the
  engine's port 11080. Protected destinations are intercepted as above; non-protected
  destinations are byte-tunnelled to Squid untouched, so package feeds behave
  exactly as they do without the sidecar. Plain HTTP to a protected host, and
  `CONNECT` to a protected host on any port other than 443, are denied.

> **Superseded.** Earlier drafts made `CONNECT` the *only* ingress by pointing
> the Agent's `HTTPS_PROXY` at the engine, which put it on the path for all
> traffic. Under the per-client model the engine sees only Azure DevOps
> traffic; everything else keeps its existing route to Squid and is provably
> unaffected. The byte-tunnel path is therefore a compatibility affordance
> rather than the primary design, and can be removed if no client needs it.

Request normalization is deliberately strict rather than lenient: a target
that would need rewriting to become safe is refused instead, so the bytes the
policy inspects are the bytes the upstream receives. Encoded path separators,
double encoding, traversal segments, control characters, and an `api-version`
that disagrees between the query string and the `Accept` header are all
denials.

Fail-closed behavior is structural rather than advisory:

- the only egress is the configured Squid URL, so a Squid outage is a `502`
  and never a direct socket;
- the policy document must declare this bundle's catalog schema version,
  carry no unrecognized key, and list every cataloged protected host — a host
  missing from the policy would take the byte-tunnel path instead of being
  policed, which is the one bypass the proxy exists to prevent;
- policy denials return a stable `403` with an Azure DevOps
  `WrappedException`-shaped body (`message`, `typeKey`), so `az` and every
  msrest-based SDK surface an actionable sentence, and with no `Location`,
  `WWW-Authenticate`, `Set-Cookie`, or `Retry-After` header, so no client
  retries a semantic request or falls into an interactive sign-in;
- credential and upstream failures return `502` with a different `typeKey`,
  deliberately avoiding `401`/`429`/`503` because msrest retries those; an
  Azure DevOps `203` sign-in page or `401` challenge is never relayed;
- response headers are allow-listed, so upstream `Set-Cookie`,
  `WWW-Authenticate`, and redirect `Location` headers cannot reach the agent;
- response bodies are bounded by the operation's declared limit, and — for
  organization-addressed reads — must prove they belong to the current project
  and repository before any byte reaches the agent.

Custody rules the implementation enforces:

- the CA and its per-host leaves are minted at startup with the `openssl`
  binary already present in the AWF agent image (Node can parse but not issue
  X.509, and adding a certificate library would reintroduce the native
  dependency this runtime exists to avoid). Every private key is written only
  under the container tmpfs directory AWF mounts for this purpose; only the
  public CA PEM is copied out, into the file AWF pre-creates and bind-mounts
  read-only into the agent;
- the bearer is read from its private file, cached on the file's mtime and
  size, so a rotation is observed on the next request and a removed or emptied
  file immediately becomes an infrastructure failure rather than a stale
  credential. It is applied to a copy of the sanitized header set *after* the
  allow decision, so no code path can emit it for a denied request;
- the JSONL decision log is schema-versioned and carries only the timestamp,
  request id, protected host, method, normalized operation id, decision,
  machine-readable reason and short detail, upstream status class, latency,
  response byte count, and the names of any credential headers the client
  supplied and the proxy stripped. Raw paths, query values, headers, bodies,
  and credentials have nowhere to go in the record type.

## Evidence

Findings from driving the real Azure CLI against the implemented engine. These
are what moved the design from container-wide interception to per-client
ingress; they are recorded so the reasoning can be re-checked rather than
re-derived.

| Claim | Evidence |
|---|---|
| `az` honours a non-`dev.azure.com` base URL | Pointed at `https://localhost:<port>/<org>`, it issued `OPTIONS /<org>/_apis` then `GET /<org>/_apis/projects` to that endpoint |
| Per-process trust is sufficient for `az` | The above verified TLS using `REQUESTS_CA_BUNDLE` alone, with no trust store modified |
| OS trust store alone is **not** sufficient | `az` fails `CERTIFICATE_VERIFY_FAILED`; Python `requests` uses its own `certifi/cacert.pem` |
| The MCP cannot be redirected by configuration | `src/index.ts`: `const orgUrl = "https://dev.azure.com/" + orgName`, no env override |
| The MCP would partially bypass a proxy-env-var approach | 8 raw `fetch()` call sites; undici ignores `HTTP(S)_PROXY` without `NODE_USE_ENV_PROXY`, which needs Node ≥24.5 against a pinned `node:20-slim` |
| **`--add-host` redirects a container to the proxy, TLS verified** | A `node:20-slim` container given `--add-host dev.azure.com:<ip>` and `NODE_EXTRA_CA_CERTS` reached the stand-in proxy over **both** `node:https` *and* global `fetch`, with `rejectUnauthorized` left on. Server observed `Host: dev.azure.com`, so the client genuinely believed it was talking to Azure DevOps |
| **The redirect is narrow** | In the same run an unrelated host failed `ENOTFOUND` — only the named host is affected |
| **SPS is avoidable, and `az` completes entirely against the policy endpoint** | Three scenarios (`scripts/sps-probe.mjs`): a *minimal* discovery document fails (`location` area not registered); *faithful* document + a sparse area list falls back to `app.vssps.visualstudio.com`; *faithful* document + a **complete** area list — real area GUIDs, every `locationUrl` pointing back at the endpoint — completed with **exit 0** and never contacted SPS |
| **Stock `az` works end to end through the real bundle with no real credential** | With the rewrite implemented, `az devops project list` and `az repos show` both returned **exit 0** and correct JSON. The fake upstream deliberately advertised `vsrm.dev.azure.com`; `az` stayed on the policed origin throughout, and SPS was never contacted. Every request was matched to a catalogued operation (`discovery.host-options`, `discovery.resource-areas`, `core.project-validation-probe`, `repos.repository-get`); the sentinel PAT never reached the upstream and the injected bearer did |
| **The MCP runs from a host-installed mount, with no network at all** | `@azure-devops/mcp@2.8.1` installed on the host and mounted read-only at `/app/node_modules` completed an MCP `initialize` handshake inside `node:20-slim` with `--network none`, returning its full tool capabilities. No pre-baked image is needed, so nothing new enters the supply chain |
| **The MCP's startup tenant lookup is non-fatal** | In that run `org-tenants.js` failed its `fetchTenantFromApi` call (`TypeError: fetch failed`) and the server logged the error and carried on serving. It targets `vssps.dev.azure.com`, which is *not* in the protected set, so under interception it will fail the same way rather than blocking startup |
| Upstream verification is real | The engine refused a self-signed upstream with `unable to verify the first certificate` |
| Denials surface usefully to clients | `az` printed the engine's `WrappedException` message verbatim |
| **The emitted start step provisions the engine end to end** | The compiler-generated bash was run with a stubbed `docker`: it substituted the scope from `System.CollectionUri` / `System.TeamProject` (`org=contoso project=Widgets`), minted the CA and a leaf per catalogued protected host (`CN=dev.azure.com`, `SAN=DNS:dev.azure.com`), and assembled a valid `ado-aw/ado-proxy-material/v1` document whose token round-tripped |
| **No private key survives the step** | After the run, the work directory held only certificates, CSRs and the policy — every `.key`, including the CA signing key, had been shredded |
| **The real container starts from exactly that document** | Piping the captured material into `node:20-slim` with the generated policy mounted brought the engine up on both ingresses (`0.0.0.0:11080` proxy, `0.0.0.0:443` direct TLS) and it published its interception CA to the shared host directory |
| **The engine polices live traffic through `--add-host`** | A separate container redirected at the engine's IP, trusting only the published CA with verification on, got: allowed discovery → `502 upstream-failed` (policy allowed; egress attempted **only** via the configured Squid, absent locally); a `repos` route outside the granted capabilities → `403 unknown-route`; `/_apis/distributedtask/variablegroups` → `403` always-denied route family. No denial reached an upstream |
| `--public-ca-file` is an **output**, not a trust store | It is where the engine *writes* its interception CA for clients. Pointing it at `/etc/ssl/certs/ca-certificates.crt` failed `EROFS`. Upstream verification instead uses Node's bundled roots — `node:20-slim` ships **no** OS trust store but carries 144 roots — so nothing needs mounting for it |
| **A shared bridge is not a boundary; `--internal` is** | A container on a normal user-defined bridge reached `https://example.com` (status 200) through Docker's outbound NAT. On an `--internal` bridge the same request failed, while the container still routed to its peers. A dual-homed container kept full egress via its *second* network. This is why the MCP network is created `--internal`: otherwise the MCP keeps a direct route to every Azure DevOps host the redirect does not override, and the engine polices one hostname rather than the boundary. It corrects an earlier claim in this document that AWF's `DOCKER-USER` scoping alone left the MCP with "no unpoliced route out" |
| **The full chain works end to end, and the engine injects the credential** | With a fake Squid and a fake Azure DevOps behind it, a client on the internal network got `200` and real JSON. Every request reaching the upstream carried an `Authorization` header, it was the **injected canary**, and the sentinel the client held **never** appeared upstream |
| **Denied requests never reach the upstream** | Against the same live chain, `distributedtask/variablegroups`, `serviceendpoint`, a `POST` write, and an unknown route all returned `403` with distinct reasons, and the upstream request count was **unchanged** across all four |
| **The MCP cannot see the credential** | Scanning the MCP container's environment, every mount, `/tmp`, and the process table for the canary found **0 occurrences**; `ADO_MCP_AUTH_TOKEN` held the sentinel |
| **The MCP starts with no registry access** | On the internal network `npm view` failed `EAI_AGAIN`, yet the MCP completed an `initialize` handshake from the mounted package |
| **Stock `az` needs no argument rewriting at all** | With `HTTPS_PROXY` pointed at the engine's `CONNECT` port, `REQUESTS_CA_BUNDLE` at the published CA, and a sentinel PAT, real `az` 2.86 ran `az devops project list --organization https://dev.azure.com/contoso` straight through the engine. The request arriving upstream (`OPTIONS /contoso/_apis`) carried the **injected** bearer; the sentinel never appeared there. Because the redirect happens below the CLI's own configuration, `--organization`, `--org`, `AZURE_DEVOPS_ORG` and stored defaults all work without the wrapper interpreting any of them |
| **A `pathlen` CA without `keyCertSign` breaks strict verifiers** | The first `az` run failed `CERTIFICATE_VERIFY_FAILED … Path length given without key usage keyCertSign`. Every Node client had accepted the same CA — only Python's `requests`, which verifies strictly, rejected it. The CA now declares `keyUsage=critical,keyCertSign,cRLSign`, after which `az` completed TLS and reached policy |

Three harnesses produce this evidence and should become conformance tests:

- `scripts/az-probe.mjs` stands up a fake Squid and a fake Azure DevOps, runs
  the real `az` through the real bundle with a canary bearer, and asserts both
  that allowed reads carry the injected credential and that denials never reach
  the upstream.
- `scripts/add-host-probe.mjs` proves the container-level redirection the MCP
  path depends on, including the undici path and the negative control.
- `scripts/sps-probe.mjs` proves which discovery-document shape keeps `az` on
  the policy endpoint.

Both container probes were run on Docker Desktop 29.6.2 (`linux/arm64`).

### Consequence for the resource-area response

`az` resolves service locations from `/_apis/resourceAreas`, so that response
determines whether it stays on the policy endpoint:

- omit the `location` area → `az` fails outright (`API resource location
  e81700f7-… is not registered`);
- advertise it but return an incomplete area list → `az` falls back to
  deployment-level SPS;
- return the real area GUIDs with every `locationUrl` pointing at the policy
  endpoint → `az` completes without ever contacting SPS.

The engine must therefore **rewrite** `locationUrl` to itself rather than
merely filtering the list. A filter that drops entries not matching a protected
host would empty the list and reintroduce the SPS fallback — the opposite of
the intent. Implemented in `response.ts` as the `filter-resource-areas` policy:
each URL's scheme and host are replaced with the origin the client is already
using, the path is preserved, and only entries that cannot be rewritten at all
are dropped.

## Open questions

These gate implementation and are unresolved at the time of writing:

1. **How does the engine obtain egress?** AWF's `DOCKER-USER` rules block the
   default bridge — the reason the MCP runs `--network host` today. The jump
   rule is scoped `-i <awf bridge>`, so a container of ours should be
   unaffected, but this needs confirming on a real runner.
2. **Does the MCP still start without npm registry access?** `npx -y
   @azure-devops/mcp` resolves at spawn time. Pre-baking the image removes this
   dependency and is preferable on supply-chain grounds regardless.

Resolved since the first draft:

- ~~Does Docker's embedded DNS reliably win for a public FQDN alias?~~ Moot:
  `--add-host` is used instead, and is proven above. It needs no DNS at all,
  which is why it is preferred — AWF itself falls back to `/etc/hosts` because
  embedded DNS is unreachable under gVisor and on ARC/DinD.
- ~~Is the SPS call avoidable?~~ **Yes**, provided the engine returns a
  complete resource-area list pointing at itself (see above). SPS therefore
  need not be reached at all on the `az` path. The catalog retains
  `discovery.sps-host-options` and `discovery.sps-resource-area` as a
  defence-in-depth affordance for clients that still fall back; both return
  service topology only.

## Production gates

Default-on rollout requires evidence that:

- stock `az` and the ADO MCP can perform allowed scoped reads with no real
  client credential;
- write, cross-scope, sensitive, unknown, alternate-host, direct-Squid, and
  direct-socket requests do not reach the upstream operation;
- WIF renewal works after the original assertion expires;
- canary credentials are absent from Agent and Detection surfaces and
  artifacts;
- package restore and non-ADO network behavior remain intact;
- all compile targets emit the same boundary;
- a released, pinned AWF image implements the required sidecar and network
  wiring, and internal mirrors contain that image.
