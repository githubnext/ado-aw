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

The target path is policy-first:

1. AWF points Agent HTTP(S) proxy variables at a hardened managed sidecar.
2. The sidecar MITMs only compiler-owned Azure DevOps REST hosts.
3. Non-Azure-DevOps traffic is tunneled unchanged to Squid.
4. Approved Azure DevOps requests are also sent upstream through Squid.
5. Squid source ACLs deny protected Azure DevOps destinations when the Agent
   tries to address Squid directly, but allow them from the policy sidecar.
6. Clearing proxy variables or opening a direct internet socket has no route.

The sidecar is safe even when directly reachable from another `awf-net` peer:
it applies the same policy to every caller, is not a generic relay, and can
reach the internet only through Squid.

## Authentication and TLS

The Agent receives at most a fixed non-secret sentinel needed for client-side
preflight. The proxy removes all client authorization and injects the current
ADO bearer only after the request matches an allowed operation and resource
scope.

The proxy creates an ephemeral interception CA. Its private key exists only on
sidecar tmpfs. AWF installs only the public certificate into supported client
trust stores.

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

- **Non-protected destination.** For `CONNECT`, the proxy opens a tunnel
  through Squid and byte-tunnels in both directions. It does not terminate
  TLS, parse the payload, or touch the client's own credentials, so package
  feeds and every other allowed host behave exactly as they do without the
  sidecar. Absolute-form plain HTTP is relayed to Squid unchanged, because the
  agent's `HTTP_PROXY` points here and refusing cleartext would silently break
  `http://` package sources.
- **Protected destination.** The proxy terminates TLS with an ephemeral leaf
  (ALPN pinned to `http/1.1`), normalizes the request, evaluates it against
  the versioned catalog, drops every client credential and forwarding header,
  and — only after a complete allow decision, and only for a protected
  upstream — attaches the current bearer and sends the request through Squid.
  Plain HTTP to a protected host, and `CONNECT` to a protected host on any
  port other than 443, are denied outright.

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

## Production gates

Default-on rollout requires evidence that:

- stock `az`, curl, Python clients, and the ADO MCP can perform allowed scoped
  reads with no real client credential;
- write, cross-scope, sensitive, unknown, alternate-host, direct-Squid, and
  direct-socket requests do not reach the upstream operation;
- WIF renewal works after the original assertion expires;
- canary credentials are absent from Agent and Detection surfaces and
  artifacts;
- package restore and non-ADO network behavior remain intact;
- all compile targets emit the same boundary;
- a released, pinned AWF image implements the managed proxy/CA path and required
  internal mirrors contain that image.
