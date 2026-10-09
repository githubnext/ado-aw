---
name: "ado-aw candidate smoke: credential-isolated package feeds (WIF)"
description: "Proves npm, pip, and dotnet restore from an Azure Artifacts feed through ado-proxy with a workload-identity token that never reaches the sandbox"
target: standalone
pool:
  name: AZS-1ES-L-Playground-ubuntu-22.04
engine:
  id: copilot
  timeout-minutes: 20
tools:
  bash:
    - printenv
    - head
    - ls
  edit: false
runtimes:
  node:
    feed: smoke
  python:
    feed: smoke
  dotnet:
    feed: smoke
permissions:
  packages:
    # Workload-identity federation: AzureCLI@3 mints an Entra token for this
    # connection's service principal, which holds Feed and Upstream Reader
    # (Collaborator) on the feed. The token goes only to ado-proxy.
    service-connection: agent-playground-read
    feeds:
      - name: smoke
        organization: msazuresphere
        project: AgentPlayground
        feed: AgentPlaygroundTestFeed
        protocols: [npm, pypi, nuget]
        upstream: allow
safe-outputs:
  add-build-tag:
    tag-prefix: "ado-aw-packages-wif-"
    max: 1
---

## Candidate package-feed smoke (workload identity)

You are a deterministic smoke test for credential-isolated Azure Artifacts
package restores. The feed credential is an Entra token held by `ado-proxy`;
you do not have it and must not need it. Run every command exactly as written,
from the current working directory.

Run these checks **in order**. If any check does not behave exactly as
described, stop without emitting a safe output. The parent smoke orchestrator
fails because the proof tag is absent.

1. Prove no package credential reached the sandbox. This command must print
   nothing and exit non-zero:

   ```bash
   printenv SC_PACKAGES_TOKEN SYSTEM_ACCESSTOKEN VSS_NUGET_ACCESSTOKEN PIP_EXTRA_INDEX_URL
   ```

2. Prove npm downloads a package tarball from the internal feed through the
   proxy. This must succeed and the directory must then contain
   `is-number-7.0.0.tgz`:

   ```bash
   npm pack is-number@7.0.0 --prefer-online --pack-destination /tmp/ado-aw-smoke-npm
   ls /tmp/ado-aw-smoke-npm
   ```

3. Prove pip downloads a wheel from the internal feed through the proxy. This
   must succeed and the directory must then contain a `six-1.16.0` wheel:

   ```bash
   pip download --no-deps --no-cache-dir --dest /tmp/ado-aw-smoke-pip six==1.16.0
   ls /tmp/ado-aw-smoke-pip
   ```

4. Prove NuGet restores a package from the internal feed through the proxy.
   Both commands must succeed, and the second must report that the package
   reference was added:

   ```bash
   dotnet new classlib --no-restore --output .ado-aw-smoke-nuget
   dotnet add .ado-aw-smoke-nuget package Newtonsoft.Json --version 13.0.3
   ```

5. Only after every check above behaves as described, invoke the
   `add-build-tag` safe-output tool with:

   - `build_id`: `$(Build.BuildId)`
   - `tag`: `$(Build.BuildId)`

Do not invoke any other safe-output tool. Stop after emitting the tag.
