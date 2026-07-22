---
name: "Pipeline Artifact Agent"
description: "Exercises the pinned candidate pipeline-artifact supply-chain source"
supply-chain:
  pipeline-artifact:
    project: AgentPlayground
    definition-id: 2560
    run-id: 630001
    artifact: ado-aw-candidate
safe-outputs:
  create-pull-request:
    target-branch: main
---

## Pipeline Artifact Agent

Use the compiler, AWF binary, and ado-script bundle from one pinned,
provenance-checked Azure DevOps pipeline artifact.
