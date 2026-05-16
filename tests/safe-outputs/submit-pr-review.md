---
name: "Daily safe-output smoke: submit-pr-review"
description: "Exercises the submit-pr-review safe output once a day"
on:
  schedule: daily around 03:00
target: standalone
engine:
  id: copilot
  model: gpt-5-mini
  timeout-minutes: 15
permissions:
  read: agent-playground-read
  write: agent-playground-write
safe-outputs:
  submit-pr-review:
    allowed-events:
      - comment
    max: 1
---

## Daily smoke for submit-pr-review

You are a smoke test. The variable group `ado-aw-daily-smoke` provides
the perma PR at `$(permaPullRequestId)`. Call exactly one safe-output
tool: `submit-pr-review`. Use these literal values (no improvisation):

- pull_request_id: $(permaPullRequestId)
- event: "comment"
- body: "ado-aw-smoke-$(Build.BuildId)-submit-pr-review exercising the submit-pr-review safe output for build $(Build.BuildId)."
- repository: "self"

Do not call any other tool. After the safe output is emitted, stop.
