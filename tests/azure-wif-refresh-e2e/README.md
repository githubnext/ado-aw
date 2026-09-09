# Azure WIF refresh E2E

This manual Azure Pipelines test proves the runtime boundary that local tests
cannot model: an ARM workload-identity service connection can obtain a fresh
Azure DevOps ID token after the assertion exposed by AzureCLI@3 has expired.

Queue `azure-pipelines.yml` and set the `serviceConnection` parameter to an
authorized ARM workload-identity service connection. The test:

1. builds the candidate `azure-wif-refresh.js` bundle;
2. starts it with the job's `System.AccessToken`, `System.OidcRequestUri`, and
   AzureCLI@3 service-connection metadata;
3. waits until the original assertion has expired;
4. verifies that the projected token changed and has a later expiry; and
5. exchanges the refreshed assertion directly with Entra for an Azure access
   token, without relying on an Azure CLI token cache.

The test logs expiry timestamps and assertion hashes only. It never prints or
publishes token values.

## Credential-free regressions

The existing `ado-script` GitHub Actions job also runs
`scripts/ado-script/test/azure-wif-isolation.test.ts`. No Azure service
connection or pipeline registration is needed:

- A Linux Docker test runs the compiler's directory/FIFO setup and the bundled
  refresher with a fake clock/provider. A persistent, different-UID consumer
  uses the compiler-generated read-only mount and observes atomic rotation.
  The test also checks denied writes, inaccessible private sibling files,
  and denied access for an unrelated UID through the original directory tree.
- A Linux AWF test captures the compiled agent invocation, replaces the AI
  command with a file/environment probe, and runs the compiler-pinned AWF
  binary (downloaded with checksum verification). It checks normal and
  `/host` paths plus a workspace symlink, and verifies that internal identity
  variables are excluded. It omits MCPG network attachment because this probe
  has no MCPG service.

Both use synthetic values only. They do not prove Azure token issuance or
Entra exchange; the manual credentialed test above covers that boundary.

After building the compiler and refresher bundle, run the Docker regression:

```bash
cargo build
cd scripts/ado-script
npm run build:azure-wif-refresh
ADO_AW_TEST_DOCKER=1 npx vitest run -c vitest.config.smoke.ts test/azure-wif-isolation.test.ts
```

On Linux, also set `ADO_AW_TEST_AWF=1` to run the real AWF probe. It requires
Docker, access to GitHub release assets/GHCR, and no concurrent AWF session
(AWF owns fixed container names). Private fixture data is placed outside
`/tmp` and the workspace; AWF intentionally exposes those locations.
