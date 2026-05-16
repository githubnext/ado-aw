---
name: "Daily safe-output smoke: comment-on-work-item"
description: "Exercises the comment-on-work-item safe output once a day"
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
  comment-on-work-item:
    target: "*"
    max: 1
    include-stats: false
---

## Daily smoke for comment-on-work-item

You are a smoke test. The variable group `ado-aw-daily-smoke` provides
a perma work item at `$(permaWorkItemId)`. Call exactly one safe-output
tool: `comment-on-work-item`. Use these literal values (no
improvisation):

- work_item_id: $(permaWorkItemId)
- body: "ado-aw-smoke-$(Build.BuildId)-comment-on-work-item exercising the comment-on-work-item safe output for build $(Build.BuildId)."

Do not call any other tool. After the safe output is emitted, stop.
