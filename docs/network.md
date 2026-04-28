# Network Isolation, Allowed Domains & Permissions

_Part of the [ado-aw documentation](../AGENTS.md)._

## Network Isolation (AWF)

Network isolation is provided by AWF (Agentic Workflow Firewall), which provides L7 (HTTP/HTTPS) egress control using Squid proxy and Docker containers. AWF restricts network access to a whitelist of approved domains.

The `ado-aw` compiler binary is distributed via [GitHub Releases](https://github.com/githubnext/ado-aw/releases) with SHA256 checksum verification. The AWF binary is distributed via [GitHub Releases](https://github.com/github/gh-aw-firewall/releases) with SHA256 checksum verification. Docker is sourced via the `DockerInstaller@0` ADO task.

## Default Allowed Domains

The following domains are always allowed (defined in `allowed_hosts.rs`):

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
| `host.docker.internal` | MCP Gateway (MCPG) on host |

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

Additional ecosystems: `bazel`, `chrome`, `clojure`, `dart`, `deno`, `elixir`, `fonts`, `github-actions`, `haskell`, `julia`, `kotlin`, `lua`, `node-cdns`, `ocaml`, `perl`, `php`, `playwright`, `powershell`, `r`, `scala`, `zig`.

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

ADO does not support fine-grained permissions — there are two access levels: blanket read and blanket write. Tokens are minted from ARM service connections; `System.AccessToken` is never used for agent or executor operations.

```yaml
permissions:
  read: my-read-arm-connection    # Stage 1 agent — read-only ADO access
  write: my-write-arm-connection  # Stage 3 executor — write access for safe-outputs
```

### Security Model

- **`permissions.read`**: Mints a read-only ADO-scoped token given to the agent inside the AWF sandbox (Stage 1). The agent can query ADO APIs but cannot write.
- **`permissions.write`**: Mints a write-capable ADO-scoped token used **only** by the executor in Stage 3 (`Execution` job). This token is never exposed to the agent.
- **Both omitted**: No ADO tokens are passed anywhere. The agent has no ADO API access.

### Compile-Time Validation

If write-requiring safe-outputs (`create-pull-request`, `create-work-item`) are configured but `permissions.write` is missing, compilation fails with a clear error message.

### Examples

```yaml
# Agent can read ADO, safe-outputs can write
permissions:
  read: my-read-sc
  write: my-write-sc

# Agent can read ADO, no write safe-outputs needed
permissions:
  read: my-read-sc

# Agent has no ADO access, but safe-outputs can create PRs/work items
permissions:
  write: my-write-sc
```
