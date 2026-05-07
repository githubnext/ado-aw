---
name: prompt-context-agent
description: Test agent for the prompt-context parameters feature
parameters:
  - name: focusArea
    displayName: Focus area for this run
    type: string
    default: "no specific focus"
    prompt-context: true
  - name: relatedWorkItem
    displayName: Related work item URL
    type: string
    default: "(none)"
    prompt-context: true
  - name: verbose
    displayName: Verbose output
    type: boolean
    default: false
---

## Prompt Context Test

Investigate the repository and produce a report. Use any additional context
from the run parameters when prioritising the work.
