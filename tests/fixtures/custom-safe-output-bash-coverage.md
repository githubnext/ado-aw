---
name: Custom Safe Output Bash Coverage
description: Exercises compiler-generated bash for scripts-style and jobs-style custom safe outputs.
safe-outputs:
  scripts:
    send-notification:
      description: Send a notification through a pinned reusable component.
      run: node send-notification.js
      component-source: octo/tools/components/send-notification.md
      component-sha: 0123456789012345678901234567890123456789
      manifest-digest: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
      inputs:
        message:
          type: string
          required: true
  jobs:
    create-service-ticket:
      description: Create a service ticket through authored Azure DevOps steps.
      inputs:
        title:
          type: string
          required: true
      steps:
        - bash: |
            set -euo pipefail
            test -f "$ADO_AW_SAFE_OUTPUT_PROPOSALS"
            test -n "$ADO_AW_SAFE_OUTPUT_RESULTS"
            printf '%s\n' '{"schema_version":1,"proposal_id":"create-service-ticket-0","status":"success","message":"created"}' \
              >> "$ADO_AW_SAFE_OUTPUT_RESULTS"
          displayName: Create service ticket
---

Exercise both custom safe-output execution modes.
