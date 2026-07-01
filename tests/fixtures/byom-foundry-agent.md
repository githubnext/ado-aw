---
name: "BYOM Foundry Agent"
description: "Exercises Copilot BYOM/BYOK provider env config via engine.env"
engine:
  id: copilot
  env:
    COPILOT_PROVIDER_TYPE: azure
    COPILOT_PROVIDER_BASE_URL: https://my-foundry.cognitiveservices.azure.com/openai/v1
    COPILOT_PROVIDER_BEARER_TOKEN: $(Setup.FOUNDRY_TOKEN)
setup:
  - bash: echo "##vso[task.setvariable variable=FOUNDRY_TOKEN;isOutput=true]token-value"
    displayName: Acquire Foundry bearer token
---

## BYOM Foundry Agent

This agent routes Copilot requests to a private Azure Copilot Foundry instance
via Bring Your Own Model (BYOM) provider environment variables.
