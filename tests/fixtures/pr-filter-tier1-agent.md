---
name: "PR Filter Tier 1 Agent"
description: "Agent with Tier 1 PR filters (pipeline variables only, no evaluator needed)"
on:
  pr:
    branches:
      include: [main]
    filters:
      title: "*[agent]*"
      author:
        include: ["dev@corp.com"]
        exclude: ["bot@noreply.com"]
      source-branch: "feature/*"
      target-branch: "main"
      commit-message: "*[skip-agent]*"
      build-reason:
        include: [PullRequest, Manual]
---

## Tier 1 Filter Agent

Run agent only when PR title contains [agent], authored by dev@corp.com,
from a feature branch targeting main.
