---
name: "Daily smoke: az CLI access"
description: "Exercises that az is mounted and reachable inside the AWF container"
on:
  schedule: daily around 03:00
target: standalone
pool:
  name: AZS-1ES-L-Playground-ubuntu-22.04
engine:
  id: copilot
  model: claude-sonnet-4.6
  timeout-minutes: 15
permissions:
  read: agent-playground-read
safe-outputs:
  noop: {}
---

## Daily smoke for Azure CLI (az)

You are a smoke test. Verify the host-mounted Azure CLI is reachable
inside the AWF container, then emit exactly one safe-output.

Steps (run each in turn using your bash tool):

1. Confirm the binary exists and prints its version:

   ```
   az --version | head -3
   ```

2. Confirm ADO subcommand auth works using `AZURE_DEVOPS_EXT_PAT`
   (populated automatically when `permissions.read` is set). List up to
   3 projects from the current organization:

   ```
   az devops project list \
     --organization "$(System.CollectionUri)" \
     --query 'value[0:3].name' \
     -o tsv
   ```

   Capture the combined stdout/stderr (truncated to 400 characters if
   longer) for the safe-output context below.

3. Call exactly one safe-output tool, `noop`, with:

   - context: a brief one-line proof-of-life containing the az version
     string and the captured project-list output, prefixed with
     `ado-aw-smoke-$(Build.BuildId)-azure-cli:`.

Do not call any other tool. After the safe output is emitted, stop.
