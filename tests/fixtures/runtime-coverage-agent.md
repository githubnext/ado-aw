---
name: Runtime Coverage Agent
description: Lint-only fixture exercising Lean, Node, .NET runtimes and the cache-memory tool
on:
  schedule: daily
runtimes:
  lean: true
  python: true
  node:
    version: "22.x"
    feed-url: "https://pkgs.dev.azure.com/example/example/_packaging/example/npm/registry/"
  dotnet:
    version: "8.0.x"
    feed-url: "https://pkgs.dev.azure.com/example/example/_packaging/example/nuget/v3/index.json"
tools:
  cache-memory: true
---

## Runtime Coverage Agent

This agent enables every runtime that produces a code-generated bash step,
plus the `cache-memory` tool. Its sole job is to compile cleanly so the
bash-step lint can analyse those generated bodies. Python is included to
exercise the `Append Python prompt` step from `src/runtimes/python/extension.rs`.
