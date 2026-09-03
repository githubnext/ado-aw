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
5. uses the refreshed assertion for a new `az login` and Azure access-token
   request.

The test logs expiry timestamps and assertion hashes only. It never prints or
publishes token values.
