---
name: "ado-aw candidate smoke: credential-isolated ADO reads"
description: "Proves the real runner topology, wrapped az path, ADO MCP path, and proxy bearer injection"
target: standalone
pool:
  name: AZS-1ES-L-Playground-ubuntu-22.04
engine:
  id: copilot
  model: claude-sonnet-4.6
  timeout-minutes: 15
tools:
  bash:
    - az
    - head
  edit: false
  azure-devops:
    org: msazuresphere
    toolsets: [core]
    allowed: [core_list_projects]
permissions:
  read:
    service-connection: agent-playground-read
    capabilities: [core, repos]
safe-outputs:
  add-build-tag:
    tag-prefix: "ado-aw-proxy-"
    max: 1
---

## Candidate ado-proxy runner smoke

You are a deterministic smoke test for credential-isolated Azure DevOps reads.
The real Azure DevOps bearer is held by `ado-proxy`; neither you, `az`, nor the
Azure DevOps MCP has it.

Run these checks **in order**. If any check fails, stop without emitting a safe
output. The parent smoke orchestrator will fail because the proof tag is absent.

1. Prove the generated `az` wrapper can read the current project through the
   proxy:

   ```bash
   az devops project show \
     --organization "$(System.CollectionUri)" \
     --project "$(System.TeamProject)" \
     --output json | head -40
   ```

2. Prove a GUID-addressed current-project read works through `az rest`:

   ```bash
   az rest \
     --method get \
     --url "$(System.CollectionUri)_apis/projects/$(System.TeamProjectId)?api-version=7.1" \
     --output json | head -40
   ```

3. Prove the current repository is readable by repository GUID:

   ```bash
   az rest \
     --method get \
     --url "$(System.CollectionUri)$(System.TeamProject)/_apis/git/repositories/$(Build.Repository.ID)/refs?api-version=7.1&filter=heads" \
     --output json | head -40
   ```

4. Invoke the Azure DevOps MCP tool `core_list_projects`. Confirm its response
   includes `$(System.TeamProject)`. Use the native MCP tool interface, not
   `curl`, raw HTTP, or shell.

5. Only after all four reads succeed, invoke the `add-build-tag` safe-output
   tool with:

   - `build_id`: `$(Build.BuildId)`
   - `tag`: `$(Build.BuildId)`

Do not invoke any other safe-output tool. Stop after emitting the tag.
