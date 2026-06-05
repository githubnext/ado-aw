# Network Isolation, Allowed Domains & Permissions

_Part of the [ado-aw documentation](../AGENTS.md)._

## Network Isolation (AWF)

Network isolation is provided by AWF (Agentic Workflow Firewall), which provides L7 (HTTP/HTTPS) egress control using Squid proxy and Docker containers. AWF restricts network access to a whitelist of approved domains.

The `ado-aw` compiler binary is distributed via [GitHub Releases](https://github.com/githubnext/ado-aw/releases) with SHA256 checksum verification. The AWF binary is distributed via [GitHub Releases](https://github.com/github/gh-aw-firewall/releases) with SHA256 checksum verification. Docker is sourced via the `DockerInstaller@0` ADO task.

## Default Allowed Domains

The following domains are always allowed. Most are defined in `CORE_ALLOWED_HOSTS` in `allowed_hosts.rs`; `host.docker.internal` is the exception — it is added by the standalone compiler in `generate_allowed_domains` (`src/compile/common.rs`) because standalone pipelines always use MCPG, which needs host access from the AWF container:

| Host Pattern | Purpose |
|-------------|---------|
| `dev.azure.com`, `*.dev.azure.com` | Azure DevOps |
| `vstoken.dev.azure.com` | Azure DevOps tokens |
| `vssps.dev.azure.com` | Azure DevOps identity |
| `*.visualstudio.com` | Visual Studio services |
| `*.vsassets.io` | Visual Studio assets |
| `*.vsblob.visualstudio.com` | Visual Studio blob storage |
| `*.vssps.visualstudio.com` | Visual Studio identity |
| `pkgs.dev.azure.com`, `*.pkgs.dev.azure.com` | Azure DevOps Artifacts/NuGet |
| `aex.dev.azure.com`, `aexus.dev.azure.com` | Azure DevOps CDN |
| `vsrm.dev.azure.com`, `*.vsrm.dev.azure.com` | Visual Studio Release Management |
| `github.com` | GitHub main site |
| `api.github.com` | GitHub API |
| `*.githubusercontent.com` | GitHub raw content |
| `*.github.com` | GitHub services |
| `*.copilot.github.com` | GitHub Copilot |
| `*.githubcopilot.com` | GitHub Copilot |
| `copilot-proxy.githubusercontent.com` | GitHub Copilot proxy |
| `login.microsoftonline.com` | Microsoft identity (OAuth) |
| `login.live.com` | Microsoft account authentication |
| `login.windows.net` | Azure AD authentication |
| `*.msauth.net`, `*.msftauth.net` | Microsoft authentication assets |
| `*.msauthimages.net` | Microsoft authentication images |
| `graph.microsoft.com` | Microsoft Graph API |
| `management.azure.com` | Azure Resource Manager |
| `*.blob.core.windows.net` | Azure Blob storage |
| `*.table.core.windows.net` | Azure Table storage |
| `*.queue.core.windows.net` | Azure Queue storage |
| `*.applicationinsights.azure.com` | Application Insights telemetry |
| `*.in.applicationinsights.azure.com` | Application Insights ingestion |
| `dc.services.visualstudio.com` | Visual Studio telemetry |
| `rt.services.visualstudio.com` | Visual Studio runtime telemetry |
| `config.edge.skype.com` | Configuration |
| `host.docker.internal` | MCP Gateway (MCPG) on host — added by the standalone compiler, not part of `CORE_ALLOWED_HOSTS` |
| `aka.ms` | Microsoft link shortener (used by `az` subcommand metadata) — contributed by the always-on Azure CLI extension |

## Always-on Azure CLI (`az`)

Every compiled pipeline adds the Azure auth and management hosts listed
above (`login.microsoftonline.com`, `login.windows.net`,
`management.azure.com`, `graph.microsoft.com`, `aka.ms`) to the AWF
allowlist and emits a small *Detect Azure CLI on host* prepare step
that runs early in the Agent job. This mirrors gh-aw's "assume `gh` is
on the runner" model: agents can call `az` from their bash tool
without opting in — *when the runner has it*.

### Runtime detection and graceful degradation

Because `azure-cli` is not universally pre-installed on every ADO
runner image (notably some 1ES self-hosted pools), the compiler does
**not** declare static AWF bind-mounts for `/opt/az` and `/usr/bin/az`.
Static mounts would cause `docker run` to fail with "bind source path
does not exist" on runners without `az`, breaking the pipeline before
the agent ever started.

Instead, the prepare step does the detection itself at pipeline time:

* If both `/usr/bin/az` (the launcher shim) and `/opt/az` (the Python
  venv that `az` actually runs in) exist on the host, the step sets
  the ADO pipeline variable
  `AW_AZ_MOUNTS=--mount /opt/az:/opt/az:ro --mount /usr/bin/az:/usr/bin/az:ro`
  via `##vso[task.setvariable]`.
* If either is missing, the step emits a
  `##vso[task.logissue type=warning]` explaining `az` won't be
  available inside the agent sandbox and leaves `AW_AZ_MOUNTS` unset
  (which expands to the empty string).

The AWF invocation in the compiled YAML then includes a literal
`$(AW_AZ_MOUNTS) \` line on its own in the `--mount` chain.
At step start, ADO interpolates that pipeline variable into the bash
script: when az is present the two `--mount` args appear; when it's
absent the line collapses to empty whitespace + the `\` continuation,
which is a no-op.

### Operator implications

- **Microsoft-hosted `ubuntu-latest`**: `az` is detected, mounted, and
  available inside the agent sandbox. Nothing to do.
- **1ES self-hosted runners *with* azure-cli baked in**: same as above.
- **1ES self-hosted runners *without* azure-cli**: the pipeline runs
  successfully, but agents that invoke `az` get the standard
  `command not found` inside the sandbox. The warning emitted by the
  prepare step is visible in the ADO log as a yellow-flagged issue on
  the build summary; treat it as a signal to either ignore (if no
  agent on that runner needs `az`) or to install `azure-cli` on the
  runner image.

See [`docs/tools.md`](tools.md#built-in-clis) for the agent-facing
contract (auth scope, available subcommands).

## Adding Additional Hosts

Agents can specify additional allowed hosts in their front matter using either ecosystem identifiers or raw domain patterns:

```yaml
network:
  allowed:
    - python                     # Ecosystem identifier — expands to Python/PyPI domains
    - rust                       # Ecosystem identifier — expands to Rust/crates.io domains
    - "*.mycompany.com"          # Raw domain pattern
    - "api.external-service.com" # Raw domain
```

### Ecosystem Identifiers

Ecosystem identifiers are shorthand names that expand to curated domain lists for common language ecosystems and services. The domain lists are sourced from [gh-aw](https://github.com/github/gh-aw) and kept up to date via an automated workflow.

Available ecosystem identifiers include:

| Identifier | Includes |
|------------|----------|
| `defaults` | Certificate infrastructure, Ubuntu mirrors, common package registries |
| `github` | GitHub domains (`github.com`, `*.githubusercontent.com`, etc.) |
| `local` | Loopback addresses (`localhost`, `127.0.0.1`, `::1`) |
| `containers` | Docker Hub, GHCR, Quay, Kubernetes |
| `linux-distros` | Debian, Alpine, Fedora, CentOS, Arch Linux package repositories |
| `dev-tools` | CI/CD and developer tool services (Codecov, Shields.io, Snyk, etc.) |
| `python` | PyPI, pip, Conda, Anaconda |
| `rust` | crates.io, rustup, static.rust-lang.org |
| `node` | npm, Yarn, pnpm, Bun, Deno, Node.js |
| `go` | proxy.golang.org, pkg.go.dev, Go module proxy |
| `java` | Maven Central, Gradle, JDK downloads |
| `dotnet` | NuGet, .NET SDK |
| `ruby` | RubyGems, Bundler |
| `swift` | Swift.org, CocoaPods |
| `terraform` | HashiCorp releases, Terraform registry |

Additional ecosystems: `bazel`, `chrome`, `clojure`, `dart`, `deno`, `elixir`, `fonts`, `github-actions`, `haskell`, `julia`, `kotlin`, `latex`, `lean`, `lua`, `node-cdns`, `ocaml`, `perl`, `php`, `playwright`, `powershell`, `python-native`, `r`, `scala`, `zig`.

The full domain lists are defined in `src/data/ecosystem_domains.json`.

All hosts (core + MCP-specific + ecosystem expansions + user-specified) are combined into a comma-separated domain list passed to AWF's `--allow-domains` flag.

### Blocking Hosts

The `network.blocked` field removes hosts from the combined allowlist. Both ecosystem identifiers and raw domain strings are supported. Blocking an ecosystem identifier removes all of its domains. Blocking a raw domain uses exact-string matching — blocking `"github.com"` does **not** also remove `"*.github.com"`.

```yaml
network:
  allowed:
    - python
    - node
  blocked:
    - python                 # Remove all Python ecosystem domains
    - "github.com"           # Remove exact domain
    - "*.github.com"         # Remove wildcard variant too
```

## Permissions (ADO Access Tokens)

ADO does not support fine-grained permissions — there are two access levels:
blanket read and blanket write. The executor (Stage 3) always has a
write-capable token; what changes is its *source* and *attribution*:

| Source                              | When                                          | Identity                                        |
| ----------------------------------- | --------------------------------------------- | ----------------------------------------------- |
| `$(System.AccessToken)` *(default)* | No `permissions.write` configured             | `Project Collection Build Service (org)`        |
| `$(SC_WRITE_TOKEN)` *(opt-in)*      | `permissions.write: <arm-service-connection>` | The federated identity behind the ARM SC        |

The agent (Stage 1) never receives the executor's token. Stage separation —
not token type — is the trust boundary.

**`System.AccessToken` exceptions.** Two other steps also map
`System.AccessToken`:

1. **Setup-job trigger filter gate** — self-cancels the build when filters
   don't match (`PATCH _apis/build/builds/{id}`) and fetches PR metadata for
   Tier 2 filters (labels, draft status, changed files). Runs before the
   agent, outside the AWF sandbox.
2. **Stage 3 executor** — when no ARM write SC is configured (the default),
   the executor's `SYSTEM_ACCESSTOKEN` env var is sourced from
   `$(System.AccessToken)`.

Both require the pipeline setting "Allow scripts to access the OAuth token"
to be enabled (the ADO default).

`System.AccessToken` is scoped by the pipeline's
**"Limit job authorization scope to current project"** toggle. With this on
(strongly recommended), writes are limited to the pipeline's host project.
Operators can scope further per-pipeline by editing the build definition's
*Run-time settings*.

```yaml
permissions:
  read: my-read-arm-connection    # Stage 1 agent — read-only ADO access
  # write: my-write-arm-connection  # Optional — see below
```

### When to set `permissions.write`

The default (`$(System.AccessToken)`) is sufficient for the vast majority of
agents. Set `permissions.write` only when you need:

1. **Cross-org or cross-project writes** — `System.AccessToken` is scoped to
   the host project. Targeting work items or repos in a different ADO
   project / organization requires an ARM SC with broader scope.
2. **Named-identity attribution** — `System.AccessToken` writes are
   attributed to the `Project Collection Build Service` identity. An ARM SC
   attributes writes to its underlying federated identity (e.g.
   `safe-output-bot@contoso.com`), useful when audit logs or work-item
   notifications need a specific actor.

### Security Model

- **`permissions.read`**: Mints a read-only ADO-scoped token given to the
  agent inside the AWF sandbox (Stage 1). The agent can query ADO APIs but
  cannot write.
- **`permissions.write` (optional)**: Mints a write-capable ADO-scoped token
  used **only** by the executor in Stage 3 (`SafeOutputs` job). Overrides
  the default `$(System.AccessToken)` for write operations. Never exposed
  to the agent.
- **Both omitted**: The agent has no ADO API access. The executor still has
  a write-capable token via `$(System.AccessToken)`, scoped by the
  pipeline's job-authorization settings.

### Examples

```yaml
# Default: agent can read ADO, executor writes via $(System.AccessToken).
permissions:
  read: my-read-sc

# Cross-org / named-identity attribution — executor writes via ARM SC.
permissions:
  read: my-read-sc
  write: my-write-sc

# Agent has no ADO read access; executor still writes via $(System.AccessToken).
# (Empty front matter — no `permissions:` key at all.)
```
