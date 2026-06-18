---
name: "Supply Chain Agent"
description: "Exercises the internal supply-chain feed + registry mirror"
supply-chain:
  feed:
    name: my-project/my-internal-feed
    service-connection: feed-conn
  registry:
    name: myacr.azurecr.io/oss-mirror
    service-connection: acr-conn
---

## Supply Chain Agent

This agent mirrors its compiler, AWF binary, ado-script bundle, and container
images from an internal Azure DevOps Artifacts feed and container registry.
