---
name: "Conclusion Write SC Test"
description: "Tests that Conclusion job mints SC_WRITE_TOKEN locally"
permissions:
  write: my-write-service-connection
safe-outputs:
  create-work-item:
    work-item-type: Task
    max: 1
---

Create one test work item using the configured safe output.
