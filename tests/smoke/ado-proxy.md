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

4. Prove write methods are refused before route execution. This command must
   fail and its error must contain `ado-proxy: POST is not a read method`:

   ```bash
   az rest \
     --method post \
     --url "$(System.CollectionUri)_apis/projects/$(System.TeamProjectId)?api-version=7.1" \
     --body '{}'
   ```

5. Prove a secret-bearing route family is refused. This command must fail and
   its error must contain
   `ado-proxy: route family /_apis/serviceendpoint is always denied`:

   ```bash
   az rest \
     --method get \
     --url "$(System.CollectionUri)$(System.TeamProject)/_apis/serviceendpoint/endpoints?api-version=7.1"
   ```

6. Prove a real sibling project is refused. `msazuresphere/4x4` exists, but it
   is not in this workflow's scope. This command must fail and its error must
   contain `ado-proxy:` and `out-of-scope`:

   ```bash
   az rest \
     --method get \
     --url "$(System.CollectionUri)_apis/projects/4x4?api-version=7.1"
   ```

7. Prove an ungranted capability is refused. The front matter grants only
   `core` and `repos`; this command must fail and its error must contain
   `ado-proxy:` and `capability-disabled`:

   ```bash
   az rest \
     --method get \
     --url "$(System.CollectionUri)$(System.TeamProject)/_apis/pipelines?api-version=7.1"
   ```

8. Invoke the Azure DevOps MCP tool `core_list_projects`. Confirm its response
   includes `$(System.TeamProject)`. Use the native MCP tool interface, not
   `curl`, raw HTTP, or shell.

9. Only after all allowed reads succeed and all four denials return the
   expected policy reasons, invoke the `add-build-tag` safe-output tool with:

   - `build_id`: `$(Build.BuildId)`
   - `tag`: `$(Build.BuildId)`

Do not invoke any other safe-output tool. Stop after emitting the tag.
