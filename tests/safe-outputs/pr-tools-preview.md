---
name: "PR safe-output preview contract"
description: "Exercise focused PR tool discovery and proposal schemas without mutating repositories"
target: standalone
pool:
  name: AZS-1ES-L-Playground-ubuntu-22.04
engine:
  id: copilot
  timeout-minutes: 10
permissions:
  write: agent-playground-write
safe-outputs:
  staged: true
  update-pull-request:
    target: "*"
    include-stats: false
  abandon-pull-request:
    target: "*"
    include-stats: false
  add-pull-request-labels: {}
  add-pull-request-reviewers:
    allowed-reviewers: ["preview@example.test"]
    max-reviewers: 1
  set-pull-request-auto-complete: {}
  submit-pull-request-review:
    allowed-events: [reset]
---

## Preview-only PR tool contract

All declared safe outputs are staged previews. Do not inspect or modify a real
pull request. Emit exactly one proposal for each of the six tools below using
the synthetic numeric pull_request_id `1` and repository `self`.

1. `update-pull-request`: body "Preview-only content update.", operation "replace".
2. `add-pull-request-labels`: labels ["preview"].
3. `add-pull-request-reviewers`: reviewers ["preview@example.test"].
4. `submit-pull-request-review`: event "reset".
5. `set-pull-request-auto-complete`: no additional fields.
6. `abandon-pull-request`: body "Preview-only abandonment."

Stop after the six proposals. No catch-all PR update tool should be needed.
