---
name: "ado-aw smoke janitor"
description: "Weekly cleanup of ado-aw-smoke-* artifacts in AgentPlayground"
on:
  schedule: weekly on monday around 02:00
target: standalone
pool:
  name: AZS-1ES-L-Playground-ubuntu-22.04
engine:
  id: copilot
  model: claude-sonnet-4.6
  timeout-minutes: 30
permissions:
  read: agent-playground-read
  write: agent-playground-write
safe-outputs:
  noop: {}
setup:
  - bash: |
      set -euo pipefail
      # TODO(smoke): wire real cleanup here using `az` / ADO REST.
      # This step should:
      #   * delete work items whose title matches `ado-aw-smoke-*`
      #     and whose System.CreatedDate is older than 30 days,
      #   * delete refs/heads/ado-aw-smoke-* and refs/tags/ado-aw-smoke-*
      #     older than 30 days from the AgentPlayground repo,
      #   * delete wiki pages whose path starts with /ado-aw-smoke-
      #     older than 30 days,
      #   * abandon any draft PRs whose title starts with
      #     "ado-aw-smoke:" older than 30 days.
      # Until the real cleanup is wired, the smoke prefix is enforced at
      # creation time so junk is bounded.
      echo "ado-aw smoke janitor placeholder build $(Build.BuildId)"
    displayName: "Cleanup: prune ado-aw-smoke-* artifacts older than 30 days"
---

## Weekly ado-aw smoke janitor

You are the weekly janitor. Setup has done the actual cleanup. Call
exactly one safe-output tool: `noop`. Use these literal values (no
improvisation):

- context: "ado-aw smoke janitor build $(Build.BuildId) completed cleanup pass"

Do not call any other tool. After the safe output is emitted, stop.
