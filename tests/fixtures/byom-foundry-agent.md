---
name: "BYOK Foundry Agent"
description: "Exercises Copilot BYOK provider config via engine.provider"
engine:
  id: copilot
  provider:
    base-url: https://my-foundry.cognitiveservices.azure.com/openai/v1
    type: azure
    token:
      service-connection: my-arm-connection
---

## BYOK Foundry Agent

This agent routes Copilot requests to a private Azure Copilot Foundry instance
via the dedicated `engine.provider` block. The compiler mints the bearer token
in-job (Agent + Detection) via `az account get-access-token` and wires it into
`COPILOT_PROVIDER_BEARER_TOKEN` as a same-job secret.

