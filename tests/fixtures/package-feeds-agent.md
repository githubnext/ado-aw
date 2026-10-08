---
name: "Package Feeds Agent"
description: "Restores from internal Azure Artifacts feeds through the credential-isolated package proxy"
permissions:
  packages:
    feeds:
      - name: internal
        organization: contoso
        project: Engineering
        feed: internal-packages
        view: Release
        protocols: [npm, pypi, nuget, cargo]
runtimes:
  python:
    feed: internal
    public-registry: block
  node:
    feed: internal
  dotnet:
    feed: internal
safe-outputs:
  noop: {}
---

## Package Feeds Agent

Restore dependencies and run the tests.
