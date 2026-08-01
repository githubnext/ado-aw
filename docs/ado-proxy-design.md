# Credential-Isolated Azure DevOps Proxy (`ado-proxy`)

_Security contract and implementation design. The runtime described here is
implemented behind a hidden, pipeline-internal CLI surface, but it is not yet
wired into generated pipelines: `ado-aw catalog --kind ado-proxy`
still reports `runtime_available: false` until the compiler, credential, and
AWF wiring land._

## Why this is required

`permissions.read` names an Azure Resource Manager service connection, but its
Azure subscription or resource-group scope does not make the underlying
identity read-only in Azure DevOps. An AAD token for the Azure DevOps audience
inherits that identity's Azure DevOps permissions. The compiler therefore
cannot safely treat the service-connection name or ARM scope as an
authorization boundary.

The current implementation keeps `SC_READ_TOKEN` out of the Agent process and
passes it only to the trusted first-party Azure DevOps MCP backend. Direct
`az devops`, curl, and SDK calls are not authenticated. Issues
[#1652](https://github.com/githubnext/ado-aw/issues/1652) and
[#1717](https://github.com/githubnext/ado-aw/issues/1717) track the missing
credential-isolated direct HTTP path and the earlier documentation mismatch.

## Scope

The first production provider supports Azure DevOps Services reads for the
current organization, project, and repository. Broader scopes require explicit
configuration. The following remain outside this provider:

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
- workload-identity assertions and any `System.AccessToken` used to renew them;
- every Azure DevOps REST bearer minted from that identity;
- private token files and proxy CA private keys.

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
| Azure CLI (`az`) | RPC broker: an agent-side wrapper forwards argv to a sidecar that runs the real `az`, pointed at the policy engine with `--organization https://<engine>/<org>` | the `az` process only |
| Azure DevOps MCP | container attached only to an internal network where `dev.azure.com` is a DNS alias for the policy engine | the MCP container only |
| Hand-rolled `curl` / SDK calls from the Agent | none — Squid denies the protected hosts | none; fails closed |

**Why `az` uses the broker.** `az` accepts an arbitrary base URL, so it can be
*told* to talk to the policy engine rather than deceived about a public
hostname. Verified: pointed at `https://localhost:<port>/<org>`, `az devops
project list` issued `OPTIONS /<org>/_apis` followed by `GET
/<org>/_apis/projects` to that endpoint, with TLS verified against a
certificate trusted only by that process. No public hostname is impersonated
and no CA is installed anywhere. This mirrors AWF's existing `cli-proxy`
sidecar, which relocates `gh` into a sidecar holding `GH_TOKEN` and points it
at a guard via `GH_HOST`.

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

### Upstream leg

The engine verifies the real Azure DevOps certificate normally;
`rejectUnauthorized` is never disabled. Interception is trusted at both ends
rather than bypassed at either. This is load-bearing and observable: during
testing the engine correctly refused a self-signed upstream with `unable to
verify the first certificate`.

### Credential renewal

Production must support WIF renewal beyond the original assertion lifetime.
The expected trusted path requests a fresh assertion from
`$(System.OidcRequestUri)` using a host-task-only `$(System.AccessToken)`,
re-authenticates outside AWF, and atomically updates a proxy-only token file.
If this cannot be demonstrated without exposing identity material, rollout
stops rather than falling back to an Agent credential.

This is an Azure Pipelines-supported pattern rather than a custom refresh
protocol. `AzureCLI@2` implements the same behavior behind its experimental
`keepAzSessionActive` input: for WIF connections it requests a new OIDC token
and repeats `az login --federated-token` on an interval. The proxy integration
must use `addSpnToEnvironment: false`; client ID, tenant ID, and service
connection ID are non-secret task metadata, while the raw OIDC assertion and
`System.AccessToken` remain trusted-task-only.

## Authorization contract

The operation catalog matches normalized host, method, route template, API
version, current organization/project/repository scope, and bounded
operation-specific request fields. It explicitly models read-like POSTs;
method alone never determines safety.

The in-tree catalog can be inspected before runtime enablement with
`ado-aw catalog --kind ado-proxy --json`. Its
`runtime_available` field remains `false` until the credential and AWF wiring
are enabled.

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
`ado-script` bundles and already covered by the `supply-chain:` mirror. AWF
runs it as the managed sidecar's entrypoint from the pinned AWF agent image,
the same entrypoint-override pattern SafeOutputs already uses.

It is not a Rust subcommand. A Rust implementation would need a TLS stack plus
certificate minting (`rustls` + `rcgen` → `ring`), which would make a native C
toolchain a hard build requirement for the whole compiler; ado-aw is otherwise
pure-Rust and must stay buildable without one. Node's built-in `tls`, `http`,
and `net` modules cover the same ground with no new runtime dependency, and
match how AWF implements its own credential-isolating sidecars.

Configuration follows the generic `AWF_POLICY_PROXY_*` contract AWF publishes
for any policy-proxy sidecar. No credential is ever passed through argv or the
environment: the bearer is read from a private, rotating token file, and the
policy document is a mounted read-only JSON file carrying the
`catalog_version` the bundle re-checks at startup, so a stale policy fails
closed.

Request handling has exactly two paths:

- **Direct TLS on the protected path.** Both the broker (`az`) and the
  DNS-aliased MCP connect straight to the engine on 443. It terminates TLS with
  a leaf selected by SNI (ALPN pinned to `http/1.1`), normalizes the request,
  evaluates it against the versioned catalog, drops every client credential and
  forwarding header, and — only after a complete allow decision, and only for a
  protected upstream — attaches the current bearer and forwards through Squid.
- **`CONNECT` for proxy-style clients.** Retained for clients configured with
  `HTTPS_PROXY`. Protected destinations are intercepted as above; non-protected
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
| `az` reaches a hardcoded SPS host | `OPTIONS`/`GET` to `app.vssps.visualstudio.com` not redirected by `--organization` — see open questions |
| Upstream verification is real | The engine refused a self-signed upstream with `unable to verify the first certificate` |
| Denials surface usefully to clients | `az` printed the engine's `WrappedException` message verbatim |

The harness is `scripts/az-probe.mjs`. It stands up a fake Squid and a fake
Azure DevOps, runs the real `az` through the real bundle with a canary bearer,
and asserts both that allowed reads carry the injected credential and that
denials never reach the upstream. It should become the basis of a conformance
test rather than remaining a one-off.

## Open questions

These gate implementation and are unresolved at the time of writing:

1. **Is the SPS call avoidable?** `az` contacted `app.vssps.visualstudio.com`
   despite a custom base URL. This is likely an artifact of the probe serving an
   incomplete resource-location document, since that document — which the engine
   controls in the broker model — is what tells `az` where each area lives. If
   it is *not* avoidable, the sidecar reaches real SPS directly. That is
   probably acceptable, as SPS returns service topology rather than project
   data, but it must be a decision rather than an accident.
2. **Does Docker's embedded DNS reliably win for a public FQDN alias?** The MCP
   path depends entirely on this and it has not been tested on a real runner.
3. **How does the engine obtain egress?** AWF's `DOCKER-USER` rules block the
   default bridge — the reason the MCP runs `--network host` today — so it needs
   `awf-net`. Whether that attachment can be made from the pipeline, or requires
   AWF to own the sidecar's lifecycle, determines how much of the AWF change is
   avoidable.
4. **Does the MCP still start without npm registry access?** `npx -y
   @azure-devops/mcp` resolves at spawn time. Pre-baking the image removes this
   dependency and is preferable on supply-chain grounds regardless.

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
