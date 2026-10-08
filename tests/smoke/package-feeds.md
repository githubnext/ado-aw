---
name: "ado-aw candidate smoke: credential-isolated package feeds"
description: "Proves npm and pip restore from an Azure Artifacts feed through ado-proxy with no feed credential in the sandbox"
target: standalone
pool:
  name: AZS-1ES-L-Playground-ubuntu-22.04
engine:
  id: copilot
  timeout-minutes: 15
tools:
  bash:
    - printenv
    - head
    - ls
  edit: false
runtimes:
  node:
    feed: smoke
    public-registry: block
  python:
    feed: smoke
    public-registry: block
permissions:
  packages:
    feeds:
      - name: smoke
        organization: msazuresphere
        project: AgentPlayground
        feed: ado-aw-smoke
        protocols: [npm, pypi]
        upstream: allow
safe-outputs:
  add-build-tag:
    tag-prefix: "ado-aw-packages-"
    max: 1
---

## Candidate package-feed smoke

You are a deterministic smoke test for credential-isolated Azure Artifacts
package restores. The feed credential is held by `ado-proxy`; you do not have
it and must not need it.

Run these checks **in order**. If any check does not behave exactly as
described, stop without emitting a safe output. The parent smoke orchestrator
fails because the proof tag is absent.

1. Prove no package credential reached the sandbox. This command must print
   nothing and exit non-zero:

   ```bash
   printenv SYSTEM_ACCESSTOKEN SC_PACKAGES_TOKEN VSS_NUGET_ACCESSTOKEN PIP_EXTRA_INDEX_URL
   ```

2. Prove npm metadata is read from the internal feed through the proxy. This
   must succeed and print a version number:

   ```bash
   npm view is-number version
   ```

3. Prove a PyPI package downloads from the internal feed through the proxy,
   including the redirect to blob storage. This must succeed and the directory
   must then contain a `six-1.16.0` wheel:

   ```bash
   pip download --no-deps --dest /tmp/ado-aw-smoke-pip six==1.16.0
   ls /tmp/ado-aw-smoke-pip
   ```

4. Prove an ungranted feed is refused. This command must fail with an HTTP
   `403`:

   ```bash
   npm view is-number version \
     --registry https://pkgs.dev.azure.com/msazuresphere/AgentPlayground/_packaging/ado-aw-smoke-not-granted/npm/registry/
   ```

5. Prove the public npm registry is blocked. This command must fail without
   returning a version:

   ```bash
   npm view is-number version --registry https://registry.npmjs.org/
   ```

6. Only after every check above behaves as described, invoke the
   `add-build-tag` safe-output tool with:

   - `build_id`: `$(Build.BuildId)`
   - `tag`: `$(Build.BuildId)`

Do not invoke any other safe-output tool. Stop after emitting the tag.
