---
name: Runtime Coverage 1ES Agent
description: 1ES variant of runtime-coverage-agent.md so the bash-lint test exercises code-generated runtime/tool bash bodies on the 1ES target as well as standalone
target: 1es
on:
  schedule: daily
runtimes:
  lean: true
  node:
    version: "22.x"
    feed-url: "https://pkgs.dev.azure.com/example/example/_packaging/example/npm/registry/"
  dotnet:
    version: "8.0.x"
    feed-url: "https://pkgs.dev.azure.com/example/example/_packaging/example/nuget/v3/index.json"
tools:
  cache-memory: true
---

## Runtime Coverage 1ES Agent

1ES variant of `runtime-coverage-agent.md`. Same runtimes and tools, compiled to
the 1ES target so the bash-lint integration test analyses code-generated bash
bodies on both targets. Today the runtime/tool extension generators emit
target-agnostic bash, but this fixture guards against future divergence.
