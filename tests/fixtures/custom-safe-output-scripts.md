---
name: Custom Safe Output Acceptance
description: A workflow with a custom scripts-style safe-output tool for the acceptance matrix.
safe-outputs:
  scripts:
    send-notification:
      description: Send a structured notification to the configured destination.
      run: node send-notification.js
      max: 3
      inputs:
        title:
          type: string
          required: true
          max-length: 120
        severity:
          type: choice
          options: [info, warning, critical]
          required: true
  send-notification:
    require-approval: true
---

Analyze the run and call `send-notification` only when a person should act.
