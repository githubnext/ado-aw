---
name: Custom Safe Output Acceptance
description: A workflow with a custom jobs-style safe-output tool for the acceptance matrix.
safe-outputs:
  jobs:
    send-notification:
      description: Send a structured notification to the configured destination.
      output: Notification proposal accepted.
      max: 1
      inputs:
        title:
          description: Notification title.
          type: string
          required: true
        severity:
          description: Operational severity.
          type: choice
          options: [info, warning, critical]
          required: true
      env:
        NOTIFICATION_TOKEN: $(SHARED_NOTIFICATION_TOKEN)
      steps:
        - bash: |
            set -euo pipefail
            jq -e '.items[] | select(.type == "send-notification")' \
              "$ADO_AW_AGENT_OUTPUT" > /dev/null
          displayName: Validate notification proposals
  send-notification:
    require-approval: true
---

Analyze the run and call `send-notification` only when a person should act.
