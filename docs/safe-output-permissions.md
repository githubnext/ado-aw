# Safe-output permissions & the default build identity

_Part of the [ado-aw documentation](../AGENTS.md)._

This page is the reference for diagnosing 401/403 failures from the
Stage 3 SafeOutputs executor — the most common runtime failure class
for ado-aw pipelines once compilation succeeds.

It covers:

- Which Azure DevOps identity Stage 3 actually runs as
- How the **"Limit job authorization scope to current project"**
  toggle changes that identity
- How to read the exact ADO error (`TF401027`) and decode the
  permission bitmask
- A REST recipe for inspecting the relevant ACEs from the command line
- The three fix paths, in order of how on-convention they are

For the broader Stage 3 catalogue (PR / work item / wiki errors), see
[`docs/safe-outputs.md`](safe-outputs.md). For the service-connection
model, see the `permissions:` section of
[`docs/network.md`](network.md).

---

## TL;DR

When a Stage 3 safe output fails with HTTP 403 and the body contains:

```text
TF401027: You need the Git 'PullRequestContribute' permission to
perform this action. Details: identity 'Build\<guid>', scope 'repository'.
```

…the `$(System.AccessToken)` your pipeline is using does not have
that permission on the target repository. The build identity is **not
the user who triggered the run** — it is one of two service accounts
ADO mints on your behalf, and `<guid>` tells you which one. Concrete
fix paths are below in [Fix options](#fix-options).

---

## What identity Stage 3 runs as

By default the Stage 3 executor uses `$(System.AccessToken)` — the
short-lived OAuth token Azure DevOps mints for every pipeline run.
Which identity that token represents depends on a single setting:
**"Limit job authorization scope to current project for non-release
pipelines."**

| Toggle | Identity behind `$(System.AccessToken)` | Display name | Descriptor shape |
|---|---|---|---|
| **OFF** (default) | Collection-scoped build service | `Project Collection Build Service (<org>)` | `Microsoft.TeamFoundation.ServiceIdentity;<host>:Build:<random-guid>` |
| **ON** | Project-scoped build service | `<ProjectName> Build Service (<org>)` | `Microsoft.TeamFoundation.ServiceIdentity;<host>:Build:<projectId>` |

The `<guid>` printed inside `Build\<guid>` in the error message is
exactly what lets you tell them apart: if it matches your project's
ID, it's the project-scoped identity; otherwise it's the
collection-scoped one.

The toggle lives in three places (most-specific wins):

- **Per-pipeline** — Pipeline → Edit → "…" → Triggers → "Limit job
  authorization scope to current project".
- **Project-level** — Project Settings → Pipelines → Settings.
- **Organization-level** — Organization Settings → Pipelines →
  Settings.

> The collection-scoped identity (toggle OFF) can reach resources in
> other projects in the same organization but is more privileged and
> therefore more often subject to explicit Deny ACEs. The
> project-scoped identity (toggle ON) is restricted to its own
> project but is usually already a member of `[Project]\Contributors`,
> which carries `PullRequestContribute` by default.

If `permissions.write:` is set in the agent's front matter, Stage 3
uses the **ARM service connection's identity** instead, and none of
the above applies — see [Option 1](#option-1-wire-a-write-service-connection-recommended).

---

## Decoding the failure

### The error format

```text
TF401027: You need the Git '<PermissionName>' permission to perform
this action. Details: identity 'Build\<guid>', scope '<scope>'.
```

| Field | Meaning |
|---|---|
| `<PermissionName>` | The exact permission bit ADO denied. Map to a bit value using the table below. |
| `Build\<guid>` | The build-service identity Stage 3 ran as. Match against your project ID to identify which one. |
| `<scope>` | `repository` (per-repo ACE), `project` (all repos), or `branch` (`refs/heads/<name>`). |

### Git Repositories permission bits

These are the bits that appear under the "Git Repositories" security
namespace in ADO (namespace ID
`2e9eb7ed-3c0a-47d4-87c1-0ffdd275fd87`). Bitwise OR the values to
decode an `allow` / `deny` mask:

| Bit | Name | Display name |
|---:|---|---|
| 1 | `Administer` | Administer |
| 2 | `GenericRead` | Read |
| 4 | `GenericContribute` | Contribute |
| 8 | `ForcePush` | Force push (rewrite history, delete branches and tags) |
| 16 | `CreateBranch` | Create branch |
| 32 | `CreateTag` | Create tag |
| 64 | `ManageNote` | Manage notes |
| 128 | `PolicyExempt` | Bypass policies when pushing |
| 256 | `CreateRepository` | Create repository |
| 512 | `DeleteRepository` | Delete or disable repository |
| 1024 | `RenameRepository` | Rename repository |
| 2048 | `EditPolicies` | Edit policies |
| 4096 | `RemoveOthersLocks` | Remove others' locks |
| 8192 | `ManagePermissions` | Manage permissions |
| **16384** | **`PullRequestContribute`** | **Contribute to pull requests** |
| 32768 | `PullRequestBypassPolicy` | Bypass policies when completing pull requests |
| 65536 | `ViewAdvSecAlerts` | Advanced Security: view alerts |
| 131072 | `DismissAdvSecAlerts` | Advanced Security: manage and dismiss alerts |
| 262144 | `ManageAdvSecScanning` | Advanced Security: manage settings |
| 524288 | `ManageEnterpriseLiveMigrations` | Enterprise Live Migration: manage migrations |

In ADO, **Deny always wins**: any bit present in `effectiveDeny`
overrides the same bit in `effectiveAllow`, even if the allow comes
from group membership.

### Which Stage 3 tool needs which permission

| Safe-output tool | Permission required (bit) |
|---|---|
| `add-pr-comment`, `submit-pr-review`, `reply-to-pr-comment`, `resolve-pr-thread`, `update-pr` | `PullRequestContribute` (16384) |
| `create-pull-request` | `PullRequestContribute` (16384) + `CreateBranch` (16) + `GenericContribute` (4) on the target repo |
| `create-branch` | `CreateBranch` (16) + `GenericContribute` (4) |
| `create-git-tag` | `CreateTag` (32) + `GenericContribute` (4) |
| `create-work-item`, `update-work-item`, `comment-on-work-item`, `link-work-items`, `upload-workitem-attachment` | Work Items namespace (`5a27515b-ccd7-42c9-84f1-54c998f03866`) — not Git Repositories |
| `create-wiki-page`, `update-wiki-page` | Project-level Wiki permissions — not Git Repositories |
| `queue-build` | Build namespace (`33344d9c-fc72-4d6f-aba5-fa317101a7e9`) — `QueueBuilds` (32) on the target definition |
| `add-build-tag`, `upload-build-attachment`, `upload-pipeline-artifact` | Current build only — never fail on perms |

---

## REST recipe: inspect the ACEs

You usually do not need to wait for another failed run to confirm
which identity has what. The following requires only an
`az`-authenticated session and a Bearer token for ADO (resource
`499b84ac-1321-427f-aa17-267ca6975798`).

### 1. Resolve the build identity from the error message

```text
identity 'Build\2670d706-90db-4242-acd8-5c1db9662bcb'
```

```bash
TOKEN=$(az account get-access-token \
  --resource 499b84ac-1321-427f-aa17-267ca6975798 \
  --query accessToken -o tsv)

# Replace <host-guid> with the descriptor scope (org services host id; you
# can copy it from a known descriptor you've already retrieved for this org).
curl -s -H "Authorization: Bearer $TOKEN" \
  "https://vssps.dev.azure.com/<org>/_apis/identities?descriptors=Microsoft.TeamFoundation.ServiceIdentity;<host-guid>:Build:2670d706-90db-4242-acd8-5c1db9662bcb&api-version=7.1" \
  | jq '.value[] | {customDisplayName, id, descriptor}'
```

`customDisplayName` will be either `Project Collection Build Service
(<org>)` or `<ProjectName> Build Service (<org>)`.

### 2. Pull the per-repo ACE for that identity

```bash
NS=2e9eb7ed-3c0a-47d4-87c1-0ffdd275fd87           # Git Repositories
PROJ=<projectId>
REPO=<repoId>
DESC='Microsoft.TeamFoundation.ServiceIdentity;<host-guid>:Build:<build-guid>'

curl -s -H "Authorization: Bearer $TOKEN" \
  "https://dev.azure.com/<org>/_apis/accesscontrollists/${NS}?token=repoV2/${PROJ}/${REPO}&descriptors=${DESC}&includeExtendedInfo=true&recurse=false&api-version=7.1" \
  | jq '.value[].acesDictionary'
```

You will get back something like:

```json
{
  "Microsoft.TeamFoundation.ServiceIdentity;…:Build:…": {
    "allow": 0,
    "deny": 16404,
    "extendedInfo": {
      "inheritedAllow": 196608,
      "effectiveAllow": 196608,
      "effectiveDeny":  16404
    }
  }
}
```

Decode `effectiveDeny` against the bit table above:
`16404 = 16384 + 16 + 4 = PullRequestContribute | CreateBranch |
GenericContribute`. That is an **explicit Deny on this repo** — no
group-level Allow can win against it.

### 3. (Optional) Check the project-scoped identity

If the failing identity is the collection-scoped one, also pull the
ACE for the project-scoped identity. If `effectiveDeny == 0` and
`effectiveAllow` includes `PullRequestContribute` (16384) there, the
fastest fix is [Option 2](#option-2-flip-the-pipeline-to-the-project-scoped-build-service)
— flip the auth-scope toggle and the next run will just work.

```bash
PROJ_DESC="Microsoft.TeamFoundation.ServiceIdentity;<host-guid>:Build:${PROJ}"
curl -s -H "Authorization: Bearer $TOKEN" \
  "https://dev.azure.com/<org>/_apis/accesscontrollists/${NS}?token=repoV2/${PROJ}/${REPO}&descriptors=${PROJ_DESC}&includeExtendedInfo=true&recurse=false&api-version=7.1" \
  | jq '.value[].acesDictionary'
```

---

## Fix options

In order of how on-convention they are for the ado-aw three-stage
trust model. Pick exactly one — they are alternatives, not
complementary.

### Option 1: Wire a write service connection (recommended)

Add an ARM service connection whose backing identity has the
permission you need on the target repository, and reference it from
the agent front matter:

```yaml
permissions:
  read:  ado-aw-read              # optional, used by Stage 1
  write: ado-aw-write             # used by Stage 3
```

Stage 3 will mint its token via that connection instead of using
`$(System.AccessToken)`, so the build-service ACEs become irrelevant.

This is the most explicit option: the identity used for writes is
named in the front matter, audit logs attribute every action to that
named principal, and the least-privilege grant lives entirely on the
service connection's identity. It also works unchanged for
cross-organization writes.

See [`docs/network.md`](network.md) (Permissions section) and the
"Service Connections" page on the documentation site for the full
setup steps.

### Option 2: Flip the pipeline to the project-scoped build service

If you do not want a dedicated write service connection and the
**project-scoped** Build Service already has `PullRequestContribute`
on the target repo (verify with [Step 3](#3-optional-check-the-project-scoped-identity)
above), the lowest-effort fix is to switch
`$(System.AccessToken)` from the collection-scoped to the
project-scoped identity:

- **Per-pipeline (preferred)** — Pipeline → Edit → "…" → Triggers →
  enable "Limit job authorization scope to current project".
- **Project-level** — Project Settings → Pipelines → Settings →
  enable for all new pipelines in the project.
- **Organization-level** — Organization Settings → Pipelines →
  Settings → enable for all new pipelines org-wide.

> **Cross-project caveat.** With this toggle ON, the token cannot
> reach resources outside the project — `resources.repositories`
> pointing at sibling-project repos, `DownloadPipelineArtifact@2`
> with a `project:` parameter naming another project, secure files
> homed in another project, and template `extends:` from cross-project
> repos all stop working. Anything outside the organization
> entirely (other ADO orgs, GitHub, external registries) is not
> affected — those use their own credentials.

The per-pipeline toggle is the lowest-blast-radius choice: it does
not affect any other pipeline in the project.

### Option 3: Lift the explicit Deny on the collection-scoped identity

Only if you need this pipeline to keep using
`$(System.AccessToken)` *and* you cannot enable
[Option 2](#option-2-flip-the-pipeline-to-the-project-scoped-build-service):

1. Project Settings → Repositories → the affected repo → Security.
2. Select `Project Collection Build Service (<org>)`.
3. Reset the denied permissions (e.g. `Contribute to pull requests`,
   `Contribute`, `Create branch`) from `Deny` to `Not set` or
   `Allow`.

This is rarely the right answer in repos that have a deliberate
Deny in place — the Deny is usually there to keep every pipeline in
the collection from being able to write to one sensitive repo. By
lifting it you re-enable that capability for *every* pipeline in
the entire organization that targets this repo. Use Option 1 or
Option 2 unless you have a specific reason to broaden the grant.

---

## Common 401/403 signatures

| HTTP status | Body fragment | Most likely cause |
|---|---|---|
| 401 Unauthorized | `TF400813: The user '...' is not authorized to access this resource` | Token is malformed or missing — usually a misconfigured service-connection step; check that the AzureCLI@2 mint succeeded. |
| 403 Forbidden | `TF401027: You need the Git 'PullRequestContribute' permission` | This page — Stage 3 identity lacks PR-contribute on the target repo. |
| 403 Forbidden | `TF401027: You need the Git 'GenericContribute' permission` | Same diagnosis; need `Contribute` on the repo (typically because of `create-pull-request` or `create-branch`). |
| 403 Forbidden | `VS800075: The project ... does not exist, or you do not have permission to access it.` | Cross-project request blocked because "Limit job authorization scope to current project" is ON. Use Option 1 with a write service connection that has cross-project rights, or move the resource into the calling project. |
| 403 Forbidden | `TF401019: The Git repository ... is disabled` | Repo disabled by an admin — not a permissions issue; re-enable in Project Settings → Repositories. |
| 404 Not Found | (no body) on a PR or work-item URL | The identity lacks `Read` on the resource — ADO returns 404 instead of 403 for non-readable resources to avoid leaking existence. Grant `Read` on the repo / area path. |

---

## See also

- [`docs/safe-outputs.md`](safe-outputs.md) — full Stage 3 tool reference
- [`docs/network.md`](network.md) — `permissions:` and the service-connection model
- [`docs/audit.md`](audit.md) — `ado-aw audit` extracts every Stage 3 execution outcome under `safe_output_execution`
- Microsoft Learn: [Job authorization scope](https://learn.microsoft.com/azure/devops/pipelines/process/access-tokens)
- Microsoft Learn: [Default permissions and access for Azure DevOps](https://learn.microsoft.com/azure/devops/organizations/security/permissions)
